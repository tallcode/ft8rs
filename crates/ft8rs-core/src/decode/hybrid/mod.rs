//! Independent upstream sessions plus bounded, isolated cooperative recovery.
use std::collections::HashMap;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::decode::cooperative::recovery::{self, CancellationStats};
use crate::decode::cooperative::{
    normalize_message as normalized_message, same_signal, OutputLedger, SignalSource,
};
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
    pub recovered_rows: usize,
    pub recovery_tasks: usize,
    pub accepted_seeds: usize,
    pub rejected_seeds: usize,
    pub elapsed: Duration,
}

enum WorkerEvent {
    Row(SignalSource, StreamDecodedMessage),
    Done(
        SignalSource,
        Result<Vec<StreamDecodedWithProvenance>, String>,
    ),
    Recovered(SignalSource, StreamDecodedMessage),
    RecoveryDone(Result<CancellationStats, String>),
}

pub struct HybridStreamDecodeSession {
    wsjtx: StreamDecodeSession,
    jtdx: JtdxStreamDecodeSession,
    config: StreamDecodeConfig,
    early: Vec<StreamDecodedWithProvenance>,
    last_evidence_store: SharedEvidenceStore,
    active_call_context: ActiveCallContext,
    diagnostics: HybridSlotDiagnostics,
}

impl HybridStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        Self {
            wsjtx: StreamDecodeSession::new(config.clone_for_profile_wsjt_x()),
            jtdx: JtdxStreamDecodeSession::new(config.clone_for_profile_jtdx()),
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
        let mut pending_j = Vec::new();
        let mut recovery_rows: [Vec<StreamDecodedMessage>; 2] = [Vec::new(), Vec::new()];
        let mut scheduled = [false; 2];
        let mut diagnostics = HybridSlotDiagnostics::default();
        let config = self.config.clone();
        let realtime = config.cooperative_realtime;
        let mut first_error = None;
        let wsjtx = &mut self.wsjtx;
        let jtdx = &mut self.jtdx;
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
                        None => wsjtx
                            .decode_slot_streaming_with_provenance_at(timestamp, samples, &mut emit)
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
                    jtdx.decode_slot_streaming_with_provenance_at(timestamp, samples, |row| {
                        j_tx.send(WorkerEvent::Row(SignalSource::Jtdx, row.clone()))
                            .map_err(|_| "hybrid event receiver closed".to_string())
                    })
                });
                let _ = j_tx.send(WorkerEvent::Done(SignalSource::Jtdx, result));
            });
            let (mut active, mut finished) = (2, 0);
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
                    let seeds: Vec<_> = source
                        .iter()
                        .filter(|r| {
                            (direction == SignalSource::Wsjtx
                                || r.provenance == StreamDecodeProvenance::Regular)
                                && recovery::eligible_shape(&r.decode, config.nfa, config.nfb)
                                && !peer.iter().any(|p| same_signal(p, &r.decode))
                        })
                        .take(recovery::MAX_SEEDS)
                        .map(|r| r.decode.clone())
                        .collect();
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
                                    JtdxStreamDecodeSession::new(
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
        }
        let w_rows = w_rows.unwrap_or_default();
        let j_rows = j_rows.unwrap_or_default();
        let baseline = merge_decodes(&w_seen, &j_seen);
        diagnostics.baseline_rows = baseline.len();
        diagnostics.recovered_rows = ledger
            .tagged()
            .iter()
            .filter(|r| !baseline.iter().any(|b| same_signal(b, &r.decode)))
            .count();
        diagnostics.elapsed = clock.elapsed();
        self.diagnostics = diagnostics;
        self.last_evidence_store = build_passive_evidence_store_from_tagged(&w_rows, &j_rows);
        for (index, group) in recovery_rows.iter().enumerate() {
            for row in group {
                self.last_evidence_store.admit(shared_decode_candidate(
                    DecoderId(if index == 0 { "sic-jtdx" } else { "sic-wsjtx" }),
                    Provenance::CrossCancellation,
                    row,
                ));
            }
        }
        self.active_call_context
            .update_from_evidence(&self.last_evidence_store);
        self.early.clear();
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(ledger.into_sorted())
        }
    }
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
    for row in wsjtx {
        store.admit(shared_decode_candidate(
            DecoderId::WSJTX,
            provenance_from_stream(row.provenance),
            &row.decode,
        ));
    }
    for row in jtdx {
        store.admit(shared_decode_candidate(
            DecoderId::JTDX,
            provenance_from_stream(row.provenance),
            &row.decode,
        ));
    }
    store
}

fn provenance_from_stream(provenance: StreamDecodeProvenance) -> Provenance {
    match provenance {
        StreamDecodeProvenance::Unknown => Provenance::Unknown,
        StreamDecodeProvenance::CooperativeRecovery => Provenance::CrossCancellation,
        StreamDecodeProvenance::ContextRecovery => Provenance::ImportedMemory,
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
    let mut by_msg: HashMap<String, Vec<usize>> = HashMap::new();

    for decode in wsjtx {
        let key = normalized_message(&decode.msg);
        by_msg.entry(key).or_default().push(rows.len());
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Wsjtx,
        });
    }

    for decode in jtdx {
        let key = normalized_message(&decode.msg);
        if let Some(idx) = by_msg.get(&key).and_then(|indices| {
            indices
                .iter()
                .copied()
                .find(|&idx| is_same_signal(&rows[idx].decode, decode))
        }) {
            if rows[idx].source == DecodeSource::Wsjtx {
                rows[idx].source = DecodeSource::Both;
            }
            continue;
        }
        by_msg.entry(key).or_default().push(rows.len());
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
