//! Independent upstream sessions plus bounded, isolated cooperative recovery.
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::decode::cooperative::observation::{jtdx_branch_configs, NativeObservation};
use crate::decode::cooperative::recovery::{self, CancellationStats};
use crate::decode::cooperative::{same_signal, OutputLedger, SignalSource};
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeProvenance, StreamDecodeSession, StreamDecodedMessage,
    StreamDecodedWithProvenance, StreamSlotDecodeState,
};
use crate::stream::time::SlotTimestamp;

mod context;
mod evidence;
mod report;
mod shared;
mod specialist;
pub use context::{ActiveCallContext, QsoContextHint, QsoContextOpportunityReport};
pub use evidence::{
    DecodeConfidence, DecodeEvidence, DecoderId, Provenance, SharedDecode, SharedDecodeCandidate,
    SharedEvidenceStore,
};
pub use report::{
    divergence_report_from_evidence, HybridDivergenceReport, MessageClass, SnrBucket,
};
use shared::SharedHashCallBook;
pub use shared::{hash_call_opportunity_report, HashCallOpportunityReport};
pub use specialist::SpecialistDiagnostics;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeSource {
    Wsjtx,
    Jtdx,
    Both,
}
#[derive(Clone, Debug)]
pub struct HybridDecodedMessage {
    pub decode: StreamDecodedMessage,
    pub source: DecodeSource,
}

#[derive(Clone, Debug, Default)]
pub struct HybridSlotDiagnostics {
    pub baseline_rows: usize,
    pub agc_rows: usize,
    pub baseline_finish: [Option<Duration>; 3],
    pub recovery_budget_skips: usize,
    pub recovered_rows: usize,
    pub recovery_tasks: usize,
    pub accepted_seeds: usize,
    pub rejected_seeds: usize,
    pub elapsed: Duration,
    pub specialist: SpecialistDiagnostics,
}

enum WorkerEvent {
    Row(SignalSource, StreamDecodedMessage),
    Done(
        SignalSource,
        Result<Vec<StreamDecodedWithProvenance>, String>,
    ),
    AgcRow(StreamDecodedMessage),
    AgcDone(Result<Vec<StreamDecodedWithProvenance>, String>),
    Recovered(SignalSource, StreamDecodedMessage),
    RecoveryDone(Result<CancellationStats, String>),
}

pub struct HybridStreamDecodeSession {
    wsjtx: StreamDecodeSession,
    jtdx: JtdxStreamDecodeSession,
    jtdx_agc: JtdxStreamDecodeSession,
    config: StreamDecodeConfig,
    early: Vec<StreamDecodedWithProvenance>,
    last_evidence_store: SharedEvidenceStore,
    active_call_context: ActiveCallContext,
    diagnostics: HybridSlotDiagnostics,
}

impl HybridStreamDecodeSession {
    pub fn new(mut config: StreamDecodeConfig) -> Self {
        // Hybrid always retains an unmodified no-AGC baseline. The second JTDX
        // session differs only in AGC; SWL and explicit partitions reach both.
        config.nagcc = false;
        let [no_agc, agc] = jtdx_branch_configs(&config);
        Self {
            wsjtx: StreamDecodeSession::new(config.clone_for_profile_wsjt_x()),
            jtdx: crate::stream::session::seeded_jtdx_session(no_agc),
            jtdx_agc: crate::stream::session::seeded_jtdx_session(agc),
            config,
            early: Vec::new(),
            last_evidence_store: SharedEvidenceStore::new(),
            active_call_context: ActiveCallContext::new(4),
            diagnostics: HybridSlotDiagnostics::default(),
        }
    }
    pub fn last_evidence_store(&self) -> &SharedEvidenceStore {
        &self.last_evidence_store
    }
    pub fn qso_context_hints(&self) -> &[QsoContextHint] {
        self.active_call_context.hints()
    }
    pub fn last_diagnostics(&self) -> &HybridSlotDiagnostics {
        &self.diagnostics
    }

    pub fn decode_slot_streaming_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.decode_slot_streaming_tagged_at(timestamp, samples, |row| on_decode(&row.decode))
    }
    pub fn decode_slot_streaming_tagged_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.early.clear();
        self.run_slot(timestamp, samples, None, on_decode)
    }
    pub fn start_slot(&mut self) -> StreamSlotDecodeState {
        // No shared calls are injected into either long-lived baseline book.
        self.early.clear();
        self.wsjtx.start_slot_decode()
    }
    pub fn decode_slot_nzhsym41<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.decode_slot_nzhsym41_tagged(timestamp, state, samples, |row| on_decode(&row.decode))
    }
    pub fn decode_slot_nzhsym41_tagged<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.early =
            self.wsjtx
                .decode_slot_nzhsym41_with_provenance_at(Some(timestamp), state, samples)?;
        for row in &mut self.early {
            if row.provenance == StreamDecodeProvenance::Regular {
                row.provenance = StreamDecodeProvenance::Unknown;
            }
        }
        for row in &self.early {
            on_decode(row)?;
        }
        Ok(self.early.len())
    }
    pub fn subtract_slot_nzhsym47(&mut self, state: &mut StreamSlotDecodeState, samples: &[f32]) {
        self.wsjtx.subtract_slot_nzhsym47(state, samples);
    }
    pub fn decode_slot_nzhsym50<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: StreamSlotDecodeState,
        _early_count: usize,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.run_slot(timestamp, samples, Some(state), |row| {
            on_decode(&row.decode)
        })
    }
    pub fn decode_slot_nzhsym50_tagged<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: StreamSlotDecodeState,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.run_slot(timestamp, samples, Some(state), on_decode)
    }

    fn run_slot<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        state: Option<StreamSlotDecodeState>,
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        let clock = Instant::now();
        let mut ledger = OutputLedger::default();
        for row in &self.early {
            ledger.admit(SignalSource::Wsjtx, &row.decode, row.provenance);
        }
        let mut w_seen: Vec<_> = self.early.iter().map(|r| r.decode.clone()).collect();
        let mut j_seen = Vec::new();
        let mut w_rows: Option<Vec<StreamDecodedWithProvenance>> = None;
        let mut j_rows: Option<Vec<StreamDecodedWithProvenance>> = None;
        let mut agc_rows = Vec::new();
        let mut agc_error = None;
        let mut pending_j = Vec::new();
        let mut recovery_rows: [Vec<StreamDecodedMessage>; 2] = [Vec::new(), Vec::new()];
        let mut scheduled = [false; 2];
        let mut diagnostics = HybridSlotDiagnostics::default();
        let config = self.config.clone();
        let realtime = config.cooperative_realtime;
        let mut first_error = None;
        let wsjtx = &mut self.wsjtx;
        let jtdx = &mut self.jtdx;
        let jtdx_agc = &mut self.jtdx_agc;
        thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            let w_tx = tx.clone();
            scope.spawn(move || {
                let result = guard_worker(|| {
                    let mut emit = |row: &StreamDecodedMessage| {
                        w_tx.send(WorkerEvent::Row(SignalSource::Wsjtx, row.clone()))
                            .map_err(|_| "hybrid event receiver closed".to_string())
                    };
                    match state {
                        Some(state) => wsjtx
                            .decode_slot_nzhsym50_and_finish(state, samples, &mut emit)
                            .map(|rows| {
                                rows.into_iter()
                                    .map(|decode| StreamDecodedWithProvenance {
                                        provenance: wsjtx_provenance(&decode),
                                        decode,
                                    })
                                    .collect()
                            }),
                        None => NativeObservation::Wsjtx(wsjtx)
                            .decode(timestamp, samples, &mut emit)
                            .map(|mut rows| {
                                for row in &mut rows {
                                    if row.provenance == StreamDecodeProvenance::Regular {
                                        row.provenance = StreamDecodeProvenance::Unknown;
                                    }
                                }
                                rows
                            }),
                    }
                });
                let _ = w_tx.send(WorkerEvent::Done(SignalSource::Wsjtx, result));
            });
            let j_tx = tx.clone();
            scope.spawn(move || {
                let result = guard_worker(|| {
                    NativeObservation::Jtdx(jtdx).decode(timestamp, samples, |row| {
                        j_tx.send(WorkerEvent::Row(SignalSource::Jtdx, row.clone()))
                            .map_err(|_| "hybrid event receiver closed".to_string())
                    })
                });
                let _ = j_tx.send(WorkerEvent::Done(SignalSource::Jtdx, result));
            });
            let agc_tx = tx.clone();
            scope.spawn(move || {
                let result = guard_worker(|| {
                    NativeObservation::Jtdx(jtdx_agc).decode(timestamp, samples, |row| {
                        agc_tx
                            .send(WorkerEvent::AgcRow(row.clone()))
                            .map_err(|_| "hybrid event receiver closed".to_string())
                    })
                });
                let _ = agc_tx.send(WorkerEvent::AgcDone(result));
            });
            let (mut active, mut finished) = (3, 0);
            while finished < active {
                let event = rx.recv().expect("coordinator retains an event sender");
                match event {
                    WorkerEvent::Row(source, row) => {
                        if source == SignalSource::Wsjtx {
                            w_seen.push(row.clone());
                        } else {
                            j_seen.push(row.clone());
                        }
                        if !realtime && source == SignalSource::Jtdx && w_rows.is_none() {
                            pending_j.push(row);
                        } else {
                            publish(
                                &mut ledger,
                                source,
                                &row,
                                if source == SignalSource::Wsjtx {
                                    wsjtx_provenance(&row)
                                } else {
                                    StreamDecodeProvenance::Unknown
                                },
                                &mut first_error,
                                &mut on_decode,
                            );
                        }
                    }
                    WorkerEvent::Done(source, result) => {
                        finished += 1;
                        diagnostics.baseline_finish[usize::from(source == SignalSource::Jtdx)] =
                            Some(clock.elapsed());
                        let tagged = match result {
                            Ok(rows) => rows,
                            Err(error) => {
                                first_error.get_or_insert(error);
                                Vec::new()
                            }
                        };
                        if source == SignalSource::Wsjtx {
                            w_rows = Some(tagged);
                            for row in pending_j.drain(..) {
                                publish(
                                    &mut ledger,
                                    SignalSource::Jtdx,
                                    &row,
                                    StreamDecodeProvenance::Unknown,
                                    &mut first_error,
                                    &mut on_decode,
                                );
                            }
                        } else {
                            j_rows = Some(tagged);
                        }
                    }
                    WorkerEvent::AgcRow(row) => {
                        // This branch contributes output, not peer observations
                        // or seeds for the existing two-way cancellation round.
                        if realtime {
                            publish(
                                &mut ledger,
                                SignalSource::Jtdx,
                                &row,
                                StreamDecodeProvenance::Unknown,
                                &mut first_error,
                                &mut on_decode,
                            );
                        }
                    }
                    WorkerEvent::AgcDone(result) => {
                        finished += 1;
                        diagnostics.baseline_finish[2] = Some(clock.elapsed());
                        match result {
                            Ok(rows) => agc_rows = rows,
                            Err(error) => agc_error = Some(error),
                        }
                    }
                    WorkerEvent::Recovered(direction, row) => {
                        let index = usize::from(direction == SignalSource::Jtdx);
                        recovery_rows[index].push(row.clone());
                        if realtime {
                            publish(
                                &mut ledger,
                                SignalSource::Recovery,
                                &row,
                                StreamDecodeProvenance::CooperativeRecovery,
                                &mut first_error,
                                &mut on_decode,
                            );
                        }
                    }
                    WorkerEvent::RecoveryDone(result) => {
                        finished += 1;
                        match result {
                            Ok(stats) => {
                                diagnostics.accepted_seeds += stats.accepted;
                                diagnostics.rejected_seeds += stats.rejected_support;
                            }
                            Err(error) => {
                                first_error.get_or_insert(error);
                            }
                        }
                    }
                }
                for (index, direction) in [SignalSource::Wsjtx, SignalSource::Jtdx]
                    .into_iter()
                    .enumerate()
                {
                    if scheduled[index] || !config.hybrid_sic || first_error.is_some() {
                        continue;
                    }
                    let ready = if realtime {
                        if direction == SignalSource::Wsjtx {
                            w_rows.is_some()
                        } else {
                            j_rows.is_some()
                        }
                    } else {
                        w_rows.is_some() && j_rows.is_some()
                    };
                    if !ready {
                        continue;
                    }
                    scheduled[index] = true;
                    if realtime && clock.elapsed() >= Duration::from_secs(3) {
                        diagnostics.recovery_budget_skips += 1;
                        continue;
                    }
                    let peer = if direction == SignalSource::Wsjtx {
                        &j_seen
                    } else {
                        &w_seen
                    };
                    let source = if direction == SignalSource::Wsjtx {
                        w_rows.as_ref().unwrap()
                    } else {
                        j_rows.as_ref().unwrap()
                    };
                    let seeds = cancellation_seeds(source, peer, direction, &config);
                    if seeds.is_empty() {
                        continue;
                    }
                    active += 1;
                    diagnostics.recovery_tasks += 1;
                    let task_tx = tx.clone();
                    let task_config = config.clone();
                    scope.spawn(move || {
                        let result = guard_worker(|| {
                            let (input, stats) = recovery::prepare(
                                samples,
                                &seeds,
                                task_config.nfa,
                                task_config.nfb,
                            );
                            if stats.accepted > 0 {
                                let emit = |row: &StreamDecodedMessage| {
                                    task_tx
                                        .send(WorkerEvent::Recovered(direction, row.clone()))
                                        .map_err(|_| "hybrid event receiver closed".to_string())
                                };
                                if direction == SignalSource::Wsjtx {
                                    crate::stream::session::seeded_jtdx_session(
                                        task_config.clone_for_profile_jtdx(),
                                    )
                                    .decode_slot_streaming_at(timestamp, &input, emit)?;
                                } else {
                                    StreamDecodeSession::new(
                                        task_config.clone_for_profile_wsjt_x(),
                                    )
                                    .decode_slot_streaming_at(timestamp, &input, emit)?;
                                }
                            }
                            Ok(stats)
                        });
                        let _ = task_tx.send(WorkerEvent::RecoveryDone(result));
                    });
                }
            }
        });
        // Deterministic file output: baseline first, fixed recovery direction.
        if !realtime {
            for group in &recovery_rows {
                for row in group {
                    publish(
                        &mut ledger,
                        SignalSource::Recovery,
                        row,
                        StreamDecodeProvenance::CooperativeRecovery,
                        &mut first_error,
                        &mut on_decode,
                    );
                }
            }
            // Preserve the prior baseline/recovery ordering in file mode. The
            // supplemental branch cannot replace an existing representative.
            for row in &agc_rows {
                publish(
                    &mut ledger,
                    SignalSource::Jtdx,
                    &row.decode,
                    row.provenance,
                    &mut first_error,
                    &mut on_decode,
                );
            }
        }
        let w_rows = w_rows.unwrap_or_default();
        let j_rows = j_rows.unwrap_or_default();
        diagnostics.agc_rows = agc_rows.len();
        let agc_plain: Vec<_> = agc_rows.iter().map(|row| row.decode.clone()).collect();
        let mut baseline = merge_decodes(&w_seen, &j_seen);
        baseline = merge_decodes(&baseline, &agc_plain);
        diagnostics.baseline_rows = baseline.len();
        // Original workers and their callbacks finish first. Supplementary DSP
        // cannot replace their representatives or change persistent native state.
        let mut specialist_rows = Vec::new();
        if config.hybrid_specialist && first_error.is_none() {
            let result = guard_worker(|| {
                Ok(specialist::run(
                    samples,
                    config.nfa,
                    config.nfb,
                    clock + Duration::from_secs(10),
                    &mut |row| {
                        specialist_rows.push(row.clone());
                        publish(
                            &mut ledger,
                            SignalSource::Recovery,
                            row,
                            StreamDecodeProvenance::HybridSpecialist,
                            &mut first_error,
                            &mut on_decode,
                        );
                    },
                ))
            });
            match result {
                Ok(report) => diagnostics.specialist = report,
                Err(error) => diagnostics.specialist.error = Some(error),
            }
        }
        diagnostics.recovered_rows = ledger
            .tagged()
            .iter()
            .filter(|r| !baseline.iter().any(|b| same_signal(b, &r.decode)))
            .count();
        diagnostics.elapsed = clock.elapsed();
        self.diagnostics = diagnostics;
        // Both AGC variants remain one decoder family in passive evidence; a
        // repeated JTDX result cannot manufacture independent confirmation.
        self.last_evidence_store = build_passive_evidence_store_from_tagged(&w_rows, &j_rows);
        admit_tagged_rows(
            &mut self.last_evidence_store,
            DecoderId::JTDX_AGC,
            &agc_rows,
        );
        for (index, group) in recovery_rows.iter().enumerate() {
            for row in group {
                self.last_evidence_store.admit(shared_decode_candidate(
                    DecoderId(if index == 0 { "sic-jtdx" } else { "sic-wsjtx" }),
                    Provenance::CrossCancellation,
                    row,
                ));
            }
        }
        for row in &specialist_rows {
            self.last_evidence_store.admit(shared_decode_candidate(
                DecoderId("hybrid-specialist"),
                Provenance::Specialist,
                row,
            ));
        }
        self.active_call_context
            .update_from_evidence(&self.last_evidence_store);
        self.early.clear();
        if let Some(error) = first_error.or(agc_error) {
            Err(error)
        } else {
            Ok(ledger.into_sorted())
        }
    }
}

fn cancellation_seeds(
    source: &[StreamDecodedWithProvenance],
    peer: &[StreamDecodedMessage],
    direction: SignalSource,
    config: &StreamDecodeConfig,
) -> Vec<StreamDecodedMessage> {
    let eligible: Vec<_> = source
        .iter()
        .filter(|row| {
            (direction == SignalSource::Wsjtx || row.provenance == StreamDecodeProvenance::Regular)
                && recovery::eligible_shape(&row.decode, config.nfa, config.nfb)
        })
        .collect();
    let seeds: Vec<_> = eligible
        .iter()
        .filter(|row| !peer.iter().any(|other| same_signal(other, &row.decode)))
        .take(recovery::MAX_SEEDS)
        .map(|row| row.decode.clone())
        .collect();
    seeds
}

fn wsjtx_provenance(row: &StreamDecodedMessage) -> StreamDecodeProvenance {
    if row.itone.iter().all(|tone| *tone == 0) {
        StreamDecodeProvenance::A7Memory
    } else {
        StreamDecodeProvenance::Unknown
    }
}
fn guard_worker<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map_err(|_| "cooperative worker panicked".to_string())?
}
fn publish<F>(
    ledger: &mut OutputLedger,
    source: SignalSource,
    row: &StreamDecodedMessage,
    provenance: StreamDecodeProvenance,
    error: &mut Option<String>,
    emit: &mut F,
) where
    F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
{
    if ledger.admit(source, row, provenance) && error.is_none() {
        if let Err(failure) = emit(&StreamDecodedWithProvenance {
            decode: row.clone(),
            provenance,
        }) {
            *error = Some(failure);
        }
    }
}

pub fn merge_decodes(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> Vec<StreamDecodedMessage> {
    merge_decodes_with_source(wsjtx, jtdx)
        .into_iter()
        .map(|row| row.decode)
        .collect()
}

pub fn hybrid_hash_call_opportunity_report(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> HashCallOpportunityReport {
    let mut report =
        hash_call_opportunity_report(wsjtx, jtdx, |row| row.msg.as_str(), is_same_signal);
    let mut book = SharedHashCallBook::default();
    book.import_regular_calls(extract_full_call_candidates(wsjtx));
    book.import_regular_calls(extract_full_call_candidates(jtdx));
    report.hash_conflicts = book.conflict_count();
    report
}

/// Test-only convenience that treats every row as `Regular` provenance.
///
/// Real callers must use [`build_passive_evidence_store_from_tagged`] with true
/// provenance: assuming `Regular` would let assisted/AP/deep rows masquerade as
/// `ConfirmedRegular` and become `import_eligible`, defeating the confidence
/// gate. Gated to `cfg(test)` so it can never be reached from a production path.
#[cfg(test)]
fn build_passive_evidence_store(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> SharedEvidenceStore {
    let wsjtx_tagged: Vec<StreamDecodedWithProvenance> = wsjtx
        .iter()
        .cloned()
        .map(|decode| StreamDecodedWithProvenance {
            decode,
            provenance: StreamDecodeProvenance::Regular,
        })
        .collect();
    let jtdx_tagged: Vec<StreamDecodedWithProvenance> = jtdx
        .iter()
        .cloned()
        .map(|decode| StreamDecodedWithProvenance {
            decode,
            provenance: StreamDecodeProvenance::Regular,
        })
        .collect();
    build_passive_evidence_store_from_tagged(&wsjtx_tagged, &jtdx_tagged)
}

pub fn build_passive_evidence_store_from_tagged(
    wsjtx: &[StreamDecodedWithProvenance],
    jtdx: &[StreamDecodedWithProvenance],
) -> SharedEvidenceStore {
    let mut store = SharedEvidenceStore::new();
    admit_tagged_rows(&mut store, DecoderId::WSJTX, wsjtx);
    admit_tagged_rows(&mut store, DecoderId::JTDX, jtdx);
    store
}

fn admit_tagged_rows(
    store: &mut SharedEvidenceStore,
    decoder: DecoderId,
    rows: &[StreamDecodedWithProvenance],
) {
    for row in rows {
        store.admit(shared_decode_candidate(
            decoder,
            provenance_from_stream(row.provenance),
            &row.decode,
        ));
    }
}

fn provenance_from_stream(provenance: StreamDecodeProvenance) -> Provenance {
    match provenance {
        StreamDecodeProvenance::Unknown => Provenance::Unknown,
        StreamDecodeProvenance::CooperativeRecovery => Provenance::CrossCancellation,
        StreamDecodeProvenance::ContextRecovery => Provenance::ImportedMemory,
        StreamDecodeProvenance::HybridSpecialist => Provenance::Specialist,
        StreamDecodeProvenance::Regular => Provenance::Regular,
        StreamDecodeProvenance::ApMask => Provenance::ApMask,
        StreamDecodeProvenance::A7Memory => Provenance::A7Memory,
        StreamDecodeProvenance::A8List => Provenance::A8List,
        StreamDecodeProvenance::JtdxDeep => Provenance::JtdxDeep,
        StreamDecodeProvenance::ImportedMemory => Provenance::ImportedMemory,
    }
}

fn shared_decode_candidate(
    source: DecoderId,
    provenance: Provenance,
    row: &StreamDecodedMessage,
) -> SharedDecodeCandidate {
    SharedDecodeCandidate {
        source,
        provenance,
        message: row.msg.clone(),
        freq_hz: row.freq,
        dt_sec: row.dt,
        snr_db: row.snr.round() as i32,
    }
}

fn merge_decodes_with_source(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> Vec<HybridDecodedMessage> {
    let mut rows: Vec<HybridDecodedMessage> = Vec::new();

    for decode in wsjtx {
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Wsjtx,
        });
    }

    for decode in jtdx {
        if let Some(idx) = rows.iter().position(|row| same_signal(&row.decode, decode)) {
            if rows[idx].source == DecodeSource::Wsjtx {
                rows[idx].source = DecodeSource::Both;
            }
            continue;
        }
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Jtdx,
        });
    }

    rows
}

fn is_same_signal(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3
}

fn extract_full_call_candidates(rows: &[StreamDecodedMessage]) -> Vec<String> {
    let mut calls = Vec::new();
    for row in rows {
        for word in row.msg.split_whitespace() {
            let token = word.trim_matches(|c: char| c == ';' || c == ',');
            if is_shared_full_call_candidate(token) {
                calls.push(
                    token
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    calls
}

fn is_shared_full_call_candidate(token: &str) -> bool {
    let token = token.trim();
    if token.len() < 3 || token == "<...>" || token.eq_ignore_ascii_case("CQ") {
        return false;
    }
    if matches!(
        token.to_ascii_uppercase().as_str(),
        "DE" | "QRZ" | "DX" | "RRR" | "RR73" | "73" | "R" | "TU"
    ) {
        return false;
    }
    let bare = token.trim_start_matches('<').trim_end_matches('>');
    bare.chars().any(|c| c.is_ascii_digit())
        && bare.chars().any(|c| c.is_ascii_alphabetic())
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agc_branches_share_swl_and_preserve_explicit_partitions() {
        for swl in [false, true] {
            for partitions in [0, 1, 4] {
                let [no_agc, agc] = jtdx_branch_configs(&StreamDecodeConfig {
                    swl,
                    nagcc: true,
                    jtdx_threads: partitions,
                    ..Default::default()
                });
                assert!(!no_agc.nagcc);
                assert!(agc.nagcc);
                for branch in [no_agc, agc] {
                    assert_eq!(branch.swl, swl);
                    assert_eq!(branch.jtdx_threads, partitions);
                }
            }
        }
    }

    #[test]
    fn two_jtdx_agc_variants_do_not_create_independent_confirmation() {
        let no_agc = tagged(
            1205.0,
            0.6,
            "K1JT BG5ATV RR73",
            StreamDecodeProvenance::Regular,
        );
        let agc = tagged(
            1206.0,
            0.5,
            "K1JT BG5ATV RR73",
            StreamDecodeProvenance::Regular,
        );
        let mut evidence = build_passive_evidence_store_from_tagged(&[], &[no_agc]);
        admit_tagged_rows(&mut evidence, DecoderId::JTDX_AGC, &[agc]);
        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(evidence.rows()[0].sources, [DecoderId::JTDX]);
        assert_eq!(evidence.rows()[0].evidence[1].source, DecoderId::JTDX_AGC);
        assert_eq!(
            evidence.rows()[0].confidence,
            DecodeConfidence::ConfirmedRegular
        );
    }

    #[test]
    fn repeated_jtdx_result_never_becomes_both() {
        let jtdx = [
            decoded(1000.0, 0.2, "K1JT BG5ATV RR73"),
            decoded(1000.5, 0.2, "K1JT BG5ATV RR73"),
        ];
        let merged = merge_decodes_with_source(&[], &jtdx);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, DecodeSource::Jtdx);
    }

    fn decoded(freq: f64, dt: f64, msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq,
            dt,
            snr: 0.0,
            msg: msg.to_string(),
            sync: 0.0,
            itone: [0; 79],
        }
    }

    fn tagged(
        freq: f64,
        dt: f64,
        msg: &str,
        provenance: StreamDecodeProvenance,
    ) -> StreamDecodedWithProvenance {
        StreamDecodedWithProvenance {
            decode: decoded(freq, dt, msg),
            provenance,
        }
    }

    #[test]
    fn merge_decodes_deduplicates_hash_brace_display_variants() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <RK4FF> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes_with_source(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, DecodeSource::Both);
        assert_eq!(merged[0].decode.msg, "EA5/DH0YAH <RK4FF> RR73");
    }

    #[test]
    fn merge_decodes_keeps_unresolved_hash_marker_distinct() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <...> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes_with_source(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn hash_call_opportunity_report_detects_cross_decoder_resolution() {
        let wsjtx = [decoded(588.0, 0.5, "<...> US5VAC -13")];
        let jtdx = [decoded(589.0, 0.6, "R5AF/O US5VAC -13")];

        let report = hybrid_hash_call_opportunity_report(&wsjtx, &jtdx);

        assert_eq!(report.unresolved_hash_rows, 1);
        assert_eq!(report.rows_resolvable_by_other_decoder, 1);
        assert_eq!(report.hash_conflicts, 0);
    }

    #[test]
    fn passive_evidence_store_merges_agreeing_decoder_rows_without_affecting_output_shape() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <RK4FF> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes(&wsjtx, &jtdx);
        let evidence = build_passive_evidence_store(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 1);
        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(
            evidence.rows()[0].confidence,
            DecodeConfidence::ConfirmedMulti
        );
        assert!(evidence.rows()[0].import_eligible);
    }

    #[test]
    fn passive_evidence_store_does_not_import_assisted_agreement() {
        let wsjtx = [tagged(
            1205.0,
            0.6,
            "EA5/DH0YAH RK4FF RR73",
            StreamDecodeProvenance::A7Memory,
        )];
        let jtdx = [tagged(
            1206.0,
            0.8,
            "EA5/DH0YAH RK4FF RR73",
            StreamDecodeProvenance::JtdxDeep,
        )];

        let evidence = build_passive_evidence_store_from_tagged(&wsjtx, &jtdx);

        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(evidence.rows()[0].confidence, DecodeConfidence::Assisted);
        assert!(!evidence.rows()[0].import_eligible);
    }

    #[test]
    fn passive_evidence_store_does_not_import_a8_list_decodes() {
        let wsjtx = [tagged(
            1000.0,
            0.5,
            "K1JT BG5ATV PM00",
            StreamDecodeProvenance::A8List,
        )];
        let jtdx: [StreamDecodedWithProvenance; 0] = [];

        let evidence = build_passive_evidence_store_from_tagged(&wsjtx, &jtdx);

        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(evidence.rows()[0].confidence, DecodeConfidence::Assisted);
        assert!(!evidence.rows()[0].import_eligible);
    }
}
