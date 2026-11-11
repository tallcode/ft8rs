//! Independent decoder sessions plus bounded, isolated cooperative recovery.
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::decode::cooperative::observation::jtdx_branch_configs;
use crate::decode::cooperative::recovery::{self, CancellationStats};
use crate::decode::cooperative::{same_signal, OutputLedger, SignalSource};
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::decode::lib_wsjtx_plus::session::{
    StreamDecodeSession as WsjtxPlusSession, StreamSlotDecodeState as WsjtxPlusSlotState,
};
use crate::decode::lib_wsjtx_plus::subtractft8::subtract_ft8;
use crate::stream::profile::{A7Snapshot, JtdxBranch, JtdxCheckpoint, JtdxRestore, SlotRecovery};
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance, StreamDecodedMessage,
    StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;

mod context;
mod evidence;
mod report;
mod shared;
pub use crate::decode::experiment::ExperimentDiagnostics;
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
    pub agc_rows: usize,
    pub baseline_finish: [Option<Duration>; 3],
    pub workers_finish: Duration,
    pub experiment_start: Option<Duration>,
    pub experiment_finish: Option<Duration>,
    pub experiment_publish_start: Option<Duration>,
    pub recovery_budget_skips: usize,
    pub recovered_rows: usize,
    pub recovery_tasks: usize,
    pub accepted_seeds: usize,
    pub rejected_seeds: usize,
    pub elapsed: Duration,
    pub experiment: ExperimentDiagnostics,
}

enum WorkerEvent {
    Row(SignalSource, StreamDecodedMessage, StreamDecodeProvenance),
    WsjtxCompatibility(StreamDecodedMessage),
    Supplemental(StreamDecodedMessage, StreamDecodeProvenance),
    A7Checkpoint(A7Snapshot),
    JtdxCheckpoint(JtdxBranch, JtdxCheckpoint),
    RetryDone(Result<(), String>),
    Experiment(ExperimentEvent),
    Done(
        SignalSource,
        Result<Vec<StreamDecodedWithProvenance>, String>,
    ),
    AgcRow(StreamDecodedMessage),
    AgcDone(Result<Vec<StreamDecodedWithProvenance>, String>),
    Recovered(SignalSource, StreamDecodedMessage),
    RecoveryDone(Result<CancellationStats, String>),
}

// A separate FIFO cannot add DSP traffic to the native callback event queue.
// Results remain buffered until the coordinator opens the publication gate.
enum ExperimentEvent {
    Row(StreamDecodedMessage),
    Done(Result<ExperimentDiagnostics, String>, Duration, Duration),
}

fn record_experiment(
    diagnostics: &mut HybridSlotDiagnostics,
    result: Result<ExperimentDiagnostics, String>,
    start: Duration,
    finish: Duration,
) {
    diagnostics.experiment_start = Some(start);
    diagnostics.experiment_finish = Some(finish);
    match result {
        Ok(report) => diagnostics.experiment = report,
        Err(failure) => diagnostics.experiment.error = Some(failure),
    }
}

fn spawn_experiment<'scope, 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    clock: Instant,
    live: Option<mpsc::Sender<WorkerEvent>>,
    work: impl FnOnce(&mut dyn FnMut(&StreamDecodedMessage)) -> Result<ExperimentDiagnostics, String>
        + Send
        + 'scope,
) -> mpsc::Receiver<ExperimentEvent> {
    let (tx, rx) = mpsc::channel();
    scope.spawn(move || {
        let send = |event| match &live {
            Some(live) => {
                let _ = live.send(WorkerEvent::Experiment(event));
            }
            None => {
                let _ = tx.send(event);
            }
        };
        let start = clock.elapsed();
        let result = guard_worker(|| work(&mut |row| send(ExperimentEvent::Row(row.clone()))));
        send(ExperimentEvent::Done(result, start, clock.elapsed()));
    });
    rx
}

fn publish_experiment<F>(
    rx: mpsc::Receiver<ExperimentEvent>,
    clock: Instant,
    diagnostics: &mut HybridSlotDiagnostics,
    ledger: &mut OutputLedger,
    error: &mut Option<String>,
    emit: &mut F,
) -> Vec<StreamDecodedMessage>
where
    F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
{
    // Native/SIC errors retain the old policy of suppressing the experiment.
    // A callback error during exp still retains its collected passive rows.
    let admit = error.is_none();
    diagnostics.experiment_publish_start = Some(clock.elapsed());
    let mut rows = Vec::new();
    for event in rx {
        match event {
            ExperimentEvent::Row(row) => {
                if admit {
                    publish(
                        ledger,
                        SignalSource::Recovery,
                        &row,
                        StreamDecodeProvenance::Experiment,
                        error,
                        emit,
                    );
                    rows.push(row);
                }
            }
            ExperimentEvent::Done(result, start, finish) => {
                record_experiment(diagnostics, result, start, finish);
                return rows;
            }
        }
    }
    diagnostics.experiment.error = Some("experiment worker closed without completion".into());
    rows
}

fn baseline_snapshot(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
    agc: &[StreamDecodedWithProvenance],
) -> Vec<StreamDecodedMessage> {
    let agc: Vec<_> = agc.iter().map(|row| row.decode.clone()).collect();
    merge_decodes(&merge_decodes(wsjtx, jtdx), &agc)
}

/// In-progress state for the WSJT-X leg of a Hybrid slot.
pub struct HybridSlotDecodeState(WsjtxPlusSlotState);

fn wsjtx_session(config: &StreamDecodeConfig, primary: bool) -> WsjtxPlusSession {
    let mut config = config.clone_for_profile_wsjt_x();
    // The primary leg can retry candidates with or without the DSP experiment.
    if primary && config.wsjtx_plus_candidate_retry {
        config.profile = DecodeProfile::WsjtxPlus;
    }
    config.wsjtx_plus_compatibility =
        primary && (!config.hybrid_experiment || config.wsjtx_plus_compatibility);
    WsjtxPlusSession::new(config)
}

pub struct HybridStreamDecodeSession {
    wsjtx: WsjtxPlusSession,
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
            wsjtx: wsjtx_session(&config, true),
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

    pub(crate) fn restore_a7(&mut self, timestamp: &SlotTimestamp, snapshot: A7Snapshot) {
        self.wsjtx.restore_a7(timestamp, snapshot);
    }

    pub(crate) fn restore_jtdx(
        &mut self,
        primary: Option<JtdxRestore>,
        complement: Option<JtdxRestore>,
    ) {
        if let Some(primary) = primary {
            self.jtdx.restore_history(primary);
        }
        if let Some(complement) = complement {
            self.jtdx_agc.restore_history(complement);
        }
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
        self.run_slot(timestamp, samples, None, SlotRecovery::default(), on_decode)
    }
    pub fn start_slot(&mut self) -> HybridSlotDecodeState {
        // No shared calls are injected into either long-lived baseline book.
        self.early.clear();
        HybridSlotDecodeState(self.wsjtx.start_slot_decode())
    }
    pub fn decode_slot_nzhsym41<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: &mut HybridSlotDecodeState,
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
        state: &mut HybridSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.early.clear();
        self.wsjtx
            .decode_slot_nzhsym41_tagged(timestamp, &mut state.0, samples, |row| {
                let mut tagged = row.clone();
                if tagged.provenance == StreamDecodeProvenance::Regular {
                    tagged.provenance = StreamDecodeProvenance::Unknown;
                }
                on_decode(&tagged)?;
                self.early.push(tagged);
                Ok(())
            })?;
        Ok(self.early.len())
    }
    pub fn subtract_slot_nzhsym47(&mut self, state: &mut HybridSlotDecodeState, samples: &[f32]) {
        self.wsjtx.subtract_slot_nzhsym47(&mut state.0, samples);
    }
    pub fn decode_slot_nzhsym50<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: HybridSlotDecodeState,
        _early_count: usize,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.run_slot(
            timestamp,
            samples,
            Some(state),
            SlotRecovery::default(),
            |row| on_decode(&row.decode),
        )
    }
    pub fn decode_slot_nzhsym50_tagged<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: HybridSlotDecodeState,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.finish_slot_nzhsym50(
            timestamp,
            state,
            samples,
            SlotRecovery::default(),
            on_decode,
        )
    }

    pub(crate) fn finish_slot_nzhsym50<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: HybridSlotDecodeState,
        samples: &[f32],
        recovery: SlotRecovery<'_>,
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.run_slot(timestamp, samples, Some(state), recovery, on_decode)
    }

    fn run_slot<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        state: Option<HybridSlotDecodeState>,
        mut recovery: SlotRecovery<'_>,
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        let clock = Instant::now();
        let mut ledger = OutputLedger::default();
        // Supervised live output is deduplicated by the parent. Keep the local
        // ledger unique for file output and diagnostics.
        ledger.forward_duplicates(
            self.config.cooperative_realtime && recovery.jtdx_checkpoint.is_some(),
        );
        for row in &self.early {
            ledger.admit(SignalSource::Wsjtx, &row.decode, row.provenance);
        }
        let mut w_seen: Vec<_> = self.early.iter().map(|r| r.decode.clone()).collect();
        let mut j_seen = Vec::new();
        let mut w_rows: Option<Vec<StreamDecodedWithProvenance>> = None;
        let mut j_rows: Option<Vec<StreamDecodedWithProvenance>> = None;
        let mut agc_rows = Vec::new();
        let mut compatibility_rows = Vec::new();
        let mut agc_error = None;
        let mut agc_done = false;
        let mut pending_j = Vec::new();
        let mut recovery_rows: [Vec<StreamDecodedMessage>; 2] = [Vec::new(), Vec::new()];
        let mut scheduled = [false; 2];
        let mut diagnostics = HybridSlotDiagnostics::default();
        let config = self.config.clone();
        let realtime = config.cooperative_realtime;
        let deadline = recovery.deadline;
        let within_budget = || deadline.is_none_or(|deadline| Instant::now() < deadline);
        let checkpoint_enabled = recovery.checkpoint.is_some();
        let jtdx_checkpoint_enabled = recovery.jtdx_checkpoint.is_some();
        let mut first_error = None;
        let mut output_error = None;
        let wsjtx = &mut self.wsjtx;
        let jtdx = &mut self.jtdx;
        let jtdx_agc = &mut self.jtdx_agc;
        let mut baseline = None;
        let mut experiment_rows = Vec::new();
        let mut wsjtx_rows = Some(self.early.clone());
        thread::scope(|scope| {
            let mut experiment_rx = None;
            let (tx, rx) = mpsc::channel();
            let (retry_tx, retry_rx) = mpsc::channel();
            let w_tx = tx.clone();
            scope.spawn(move || {
                let staged = state.is_some();
                let mut checkpoint = |snapshot| {
                    let _ = w_tx.send(WorkerEvent::A7Checkpoint(snapshot));
                };
                let result = guard_worker(|| {
                    wsjtx.decode_hybrid_slot_tagged(
                        timestamp,
                        state.map(|state| state.0),
                        samples,
                        SlotRecovery {
                            deadline,
                            checkpoint: checkpoint_enabled.then_some(&mut checkpoint),
                            jtdx_checkpoint: None,
                        },
                        |row| {
                            let event = match row {
                                None => WorkerEvent::Done(
                                    SignalSource::Wsjtx,
                                    Ok(wsjtx_rows.take().expect("WSJT-X baseline completed twice")),
                                ),
                                Some(row)
                                    if realtime
                                        && matches!(
                                            row.provenance,
                                            StreamDecodeProvenance::CandidateRetry
                                                | StreamDecodeProvenance::PlusCompatibility
                                        ) =>
                                {
                                    WorkerEvent::Supplemental(row.decode.clone(), row.provenance)
                                }
                                Some(row)
                                    if row.provenance == StreamDecodeProvenance::CandidateRetry =>
                                {
                                    return retry_tx
                                        .send(Ok(row.decode.clone()))
                                        .map_err(|_| "hybrid retry receiver closed".to_string());
                                }
                                Some(row)
                                    if row.provenance
                                        == StreamDecodeProvenance::PlusCompatibility =>
                                {
                                    WorkerEvent::WsjtxCompatibility(row.decode.clone())
                                }
                                Some(row) => {
                                    let mut tagged = row.clone();
                                    if tagged.provenance == StreamDecodeProvenance::Regular {
                                        tagged.provenance = StreamDecodeProvenance::Unknown;
                                    }
                                    let event = WorkerEvent::Row(
                                        SignalSource::Wsjtx,
                                        tagged.decode.clone(),
                                        tagged.provenance,
                                    );
                                    // Preserve the live evidence policy independently of the
                                    // annotation forwarded to the display.
                                    if staged && tagged.provenance == StreamDecodeProvenance::A8List
                                    {
                                        tagged.provenance = StreamDecodeProvenance::Unknown;
                                    }
                                    wsjtx_rows
                                        .as_mut()
                                        .expect("WSJT-X baseline already completed")
                                        .push(tagged);
                                    event
                                }
                            };
                            w_tx.send(event)
                                .map_err(|_| "hybrid event receiver closed".to_string())
                        },
                    )
                });
                let result = match result {
                    Err(error) if wsjtx_rows.is_some() => {
                        let _ = w_tx.send(WorkerEvent::Done(SignalSource::Wsjtx, Err(error)));
                        Ok(())
                    }
                    result => result,
                };
                if realtime {
                    let _ = w_tx.send(WorkerEvent::RetryDone(result));
                } else if let Err(error) = result {
                    let _ = retry_tx.send(Err(error));
                }
            });
            let j_tx = tx.clone();
            scope.spawn(move || {
                let result = guard_worker(|| {
                    let mut checkpoint = |update| {
                        let _ = j_tx.send(WorkerEvent::JtdxCheckpoint(JtdxBranch::Primary, update));
                    };
                    jtdx.decode_slot_tagged(
                        timestamp,
                        samples,
                        jtdx_checkpoint_enabled.then_some(&mut checkpoint),
                        |tagged| {
                            let row = &tagged.decode;
                            j_tx.send(WorkerEvent::Row(
                                SignalSource::Jtdx,
                                row.clone(),
                                StreamDecodeProvenance::Unknown,
                            ))
                            .map_err(|_| "hybrid event receiver closed".to_string())
                        },
                    )
                });
                let _ = j_tx.send(WorkerEvent::Done(SignalSource::Jtdx, result));
            });
            let agc_tx = tx.clone();
            scope.spawn(move || {
                let result = guard_worker(|| {
                    let mut checkpoint = |update| {
                        let _ = agc_tx
                            .send(WorkerEvent::JtdxCheckpoint(JtdxBranch::Complement, update));
                    };
                    jtdx_agc.decode_slot_tagged(
                        timestamp,
                        samples,
                        jtdx_checkpoint_enabled.then_some(&mut checkpoint),
                        |tagged| {
                            let row = &tagged.decode;
                            agc_tx
                                .send(WorkerEvent::AgcRow(row.clone()))
                                .map_err(|_| "hybrid event receiver closed".to_string())
                        },
                    )
                });
                let _ = agc_tx.send(WorkerEvent::AgcDone(result));
            });
            let (mut active, mut finished) = (3 + usize::from(realtime), 0);
            while finished < active {
                let event = rx.recv().expect("coordinator retains an event sender");
                match event {
                    WorkerEvent::WsjtxCompatibility(row) => compatibility_rows.push(row),
                    WorkerEvent::A7Checkpoint(snapshot) => {
                        if let Some(checkpoint) = recovery.checkpoint.as_mut() {
                            checkpoint(snapshot);
                        }
                    }
                    WorkerEvent::JtdxCheckpoint(branch, update) => {
                        if let Some(checkpoint) = recovery.jtdx_checkpoint.as_mut() {
                            checkpoint(branch, update, realtime);
                        }
                    }
                    WorkerEvent::Supplemental(row, provenance) => publish(
                        &mut ledger,
                        SignalSource::Recovery,
                        &row,
                        provenance,
                        &mut output_error,
                        &mut on_decode,
                    ),
                    WorkerEvent::RetryDone(result) => {
                        finished += 1;
                        if let Err(error) = result {
                            first_error.get_or_insert(error);
                        }
                    }
                    WorkerEvent::Experiment(ExperimentEvent::Row(row)) => {
                        diagnostics
                            .experiment_publish_start
                            .get_or_insert(clock.elapsed());
                        publish(
                            &mut ledger,
                            SignalSource::Recovery,
                            &row,
                            StreamDecodeProvenance::Experiment,
                            &mut output_error,
                            &mut on_decode,
                        );
                        experiment_rows.push(row);
                    }
                    WorkerEvent::Experiment(ExperimentEvent::Done(result, start, finish)) => {
                        finished += 1;
                        record_experiment(&mut diagnostics, result, start, finish);
                    }
                    WorkerEvent::Row(source, row, provenance) => {
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
                                provenance,
                                &mut output_error,
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
                                    &mut output_error,
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
                                &mut output_error,
                                &mut on_decode,
                            );
                        }
                    }
                    WorkerEvent::AgcDone(result) => {
                        agc_done = true;
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
                                &mut output_error,
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
                    if !within_budget() || realtime && clock.elapsed() >= Duration::from_secs(3) {
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
                            let (input, stats) = recovery::prepare_with_subtractor(
                                samples,
                                &seeds,
                                task_config.nfa,
                                task_config.nfb,
                                subtract_ft8,
                                deadline,
                            );
                            if stats.accepted > 0 && within_budget() {
                                let emit = |row: &StreamDecodedMessage| {
                                    task_tx
                                        .send(WorkerEvent::Recovered(direction, row.clone()))
                                        .map_err(|_| "hybrid event receiver closed".to_string())
                                };
                                if direction == SignalSource::Wsjtx {
                                    crate::stream::session::seeded_jtdx_session(
                                        task_config.clone_for_profile_jtdx(),
                                    )
                                    .decode_slot_with_deadline(timestamp, &input, deadline, emit)?;
                                } else {
                                    wsjtx_session(&task_config, false)
                                        .with_search_deadline(deadline)
                                        .decode_slot_streaming_at(timestamp, &input, emit)?;
                                }
                            }
                            Ok(stats)
                        });
                        let _ = task_tx.send(WorkerEvent::RecoveryDone(result));
                    });
                }
                if config.hybrid_experiment
                    && experiment_rx.is_none()
                    && first_error.is_none()
                    && w_rows.is_some()
                    && j_rows.is_some()
                    && agc_done
                    && within_budget()
                {
                    // All three Done events follow their callbacks. Only owned
                    // snapshots cross into exp; no native session or SIC rows do.
                    baseline = Some(baseline_snapshot(&w_seen, &j_seen, &agc_rows));
                    let input = samples.to_vec();
                    let models = baseline.as_ref().unwrap().clone();
                    let (low, high) = (config.nfa, config.nfb);
                    active += usize::from(realtime);
                    experiment_rx = Some(spawn_experiment(
                        scope,
                        clock,
                        realtime.then(|| tx.clone()),
                        move |emit| {
                            Ok(crate::decode::experiment::decode(
                                crate::decode::experiment::ExperimentInput {
                                    samples: &input,
                                    baseline: &models,
                                    low,
                                    high,
                                    deadline: deadline.unwrap_or(clock + Duration::from_secs(10)),
                                },
                                emit,
                            ))
                        },
                    ));
                }
            }
            diagnostics.workers_finish = clock.elapsed();
            // Deterministic file output: baseline first, fixed recovery direction.
            if !realtime {
                for group in &recovery_rows {
                    for row in group {
                        publish(
                            &mut ledger,
                            SignalSource::Recovery,
                            row,
                            StreamDecodeProvenance::CooperativeRecovery,
                            &mut output_error,
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
                        &mut output_error,
                        &mut on_decode,
                    );
                }
            }
            diagnostics.agc_rows = agc_rows.len();
            let baseline =
                baseline.get_or_insert_with(|| baseline_snapshot(&w_seen, &j_seen, &agc_rows));
            diagnostics.baseline_rows = baseline.len();
            // Compatibility output remains separate from baseline evidence and
            // follows existing representatives, even when it finishes first.
            for row in &compatibility_rows {
                publish(
                    &mut ledger,
                    SignalSource::Recovery,
                    row,
                    StreamDecodeProvenance::PlusCompatibility,
                    &mut output_error,
                    &mut on_decode,
                );
            }
            // Waiting for exp cannot delay baseline, SIC, or compatibility callbacks.
            if let Some(rx) = experiment_rx.filter(|_| !realtime) {
                experiment_rows = publish_experiment(
                    rx,
                    clock,
                    &mut diagnostics,
                    &mut ledger,
                    &mut output_error,
                    &mut on_decode,
                );
            }
            // Retry output follows existing representatives and is never
            // offered as native evidence, to SIC, or to the experiment.
            for result in retry_rx {
                match result {
                    Ok(row) => publish(
                        &mut ledger,
                        SignalSource::Recovery,
                        &row,
                        StreamDecodeProvenance::CandidateRetry,
                        &mut output_error,
                        &mut on_decode,
                    ),
                    Err(error) => {
                        first_error.get_or_insert(error);
                    }
                }
            }
            // Pulse recovery is last so it cannot claim a ledger representative
            // or delay a callback from any of the existing searches.
            if diagnostics.experiment_finish.is_some()
                && diagnostics.experiment.error.is_none()
                && first_error.is_none()
                && agc_error.is_none()
                && output_error.is_none()
            {
                let pulse_deadline = deadline.unwrap_or(clock + Duration::from_secs(10));
                let result = guard_worker(|| {
                    Ok(crate::decode::experiment::recover_impulses(
                        samples,
                        config.nfa,
                        config.nfb,
                        pulse_deadline,
                        &diagnostics.experiment,
                        &mut |row| {
                            if ledger
                                .tagged()
                                .iter()
                                .any(|old| same_signal(&old.decode, row))
                            {
                                return;
                            }
                            diagnostics
                                .experiment_publish_start
                                .get_or_insert(clock.elapsed());
                            publish(
                                &mut ledger,
                                SignalSource::Recovery,
                                row,
                                StreamDecodeProvenance::Experiment,
                                &mut output_error,
                                &mut on_decode,
                            );
                            experiment_rows.push(row.clone());
                        },
                    ))
                });
                match result {
                    Ok(Some(report)) => {
                        diagnostics.experiment_finish = Some(clock.elapsed());
                        diagnostics.experiment = report;
                    }
                    Ok(None) => {}
                    Err(error) => diagnostics.experiment.error = Some(error),
                }
                // Drift recovery follows pulse publication and uses only the
                // immutable native snapshot, never SIC, retry or exp outputs.
                if diagnostics.experiment.error.is_none() && output_error.is_none() {
                    let result = guard_worker(|| {
                        Ok(crate::decode::experiment::recover_drift(
                            crate::decode::experiment::ExperimentInput {
                                samples,
                                baseline,
                                low: config.nfa,
                                high: config.nfb,
                                deadline: pulse_deadline,
                            },
                            &diagnostics.experiment,
                            &mut |row| {
                                if ledger
                                    .tagged()
                                    .iter()
                                    .any(|old| same_signal(&old.decode, row))
                                {
                                    return;
                                }
                                diagnostics
                                    .experiment_publish_start
                                    .get_or_insert(clock.elapsed());
                                publish(
                                    &mut ledger,
                                    SignalSource::Recovery,
                                    row,
                                    StreamDecodeProvenance::Experiment,
                                    &mut output_error,
                                    &mut on_decode,
                                );
                                experiment_rows.push(row.clone());
                            },
                        ))
                    });
                    match result {
                        Ok(Some(report)) => {
                            diagnostics.experiment_finish = Some(clock.elapsed());
                            diagnostics.experiment = report;
                        }
                        Ok(None) => {}
                        Err(error) => diagnostics.experiment.error = Some(error),
                    }
                }
            }
        });
        let baseline = baseline.unwrap_or_default();
        let w_rows = w_rows.unwrap_or_default();
        let j_rows = j_rows.unwrap_or_default();
        diagnostics.recovered_rows = ledger
            .tagged()
            .iter()
            .filter(|r| !baseline.iter().any(|b| same_signal(b, &r.decode)))
            .count();
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
        for row in &experiment_rows {
            self.last_evidence_store.admit(shared_decode_candidate(
                DecoderId("hybrid-experiment"),
                Provenance::Experiment,
                row,
            ));
        }
        self.active_call_context
            .update_from_evidence(&self.last_evidence_store);
        // Use the native snapshot even when an earlier recovery callback
        // became the displayed representative of a signal.
        self.wsjtx.add_a7_rows(timestamp, baseline.iter());
        self.early.clear();
        diagnostics.elapsed = clock.elapsed();
        self.diagnostics = diagnostics;
        if let Some(error) = output_error.or(first_error).or(agc_error) {
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
        StreamDecodeProvenance::Unknown | StreamDecodeProvenance::WsjtxAp(_) => Provenance::Unknown,
        StreamDecodeProvenance::CooperativeRecovery => Provenance::CrossCancellation,
        StreamDecodeProvenance::ContextRecovery
        | StreamDecodeProvenance::PlusCompatibility
        | StreamDecodeProvenance::CandidateRetry => Provenance::ImportedMemory,
        StreamDecodeProvenance::Experiment => Provenance::Experiment,
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
    fn buffered_exp_keeps_native_and_sic_priority_without_blocking_callbacks() {
        use std::cell::RefCell;
        let native = decoded(1000.0, 0.5, "CQ K1ABC FN42");
        let recovered = decoded(1500.0, 0.5, "CQ W9XYZ EN50");
        let unique = decoded(2000.0, 0.5, "CQ G4ABC IO91");
        let observed = RefCell::new(Vec::new());
        let mut emit = |row: &StreamDecodedWithProvenance| {
            observed.borrow_mut().push(row.clone());
            Ok(())
        };
        thread::scope(|scope| {
            let (ready_tx, ready_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let rows = [native.clone(), recovered.clone(), unique.clone()];
            let clock = Instant::now();
            let rx = spawn_experiment(scope, clock, None, move |emit| {
                for row in &rows {
                    emit(row);
                }
                ready_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(ExperimentDiagnostics::default())
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            // Exp has produced all rows, but is still running. These callbacks
            // must run before it can complete or acquire a ledger representative.
            let mut ledger = OutputLedger::default();
            let mut error = None;
            publish(
                &mut ledger,
                SignalSource::Wsjtx,
                &native,
                StreamDecodeProvenance::Unknown,
                &mut error,
                &mut emit,
            );
            publish(
                &mut ledger,
                SignalSource::Recovery,
                &recovered,
                StreamDecodeProvenance::CooperativeRecovery,
                &mut error,
                &mut emit,
            );
            assert_eq!(observed.borrow().len(), 2);
            release_tx.send(()).unwrap();
            let mut diagnostics = HybridSlotDiagnostics::default();
            let raw = publish_experiment(
                rx,
                clock,
                &mut diagnostics,
                &mut ledger,
                &mut error,
                &mut emit,
            );
            assert_eq!(raw.len(), 3);
            assert!(error.is_none());
            let rows = observed.borrow();
            assert_eq!(
                rows.iter()
                    .map(|row| row.decode.msg.as_str())
                    .collect::<Vec<_>>(),
                [native.msg, recovered.msg, unique.msg]
            );
            assert_eq!(
                rows.iter().map(|row| row.provenance).collect::<Vec<_>>(),
                [
                    StreamDecodeProvenance::Unknown,
                    StreamDecodeProvenance::CooperativeRecovery,
                    StreamDecodeProvenance::Experiment
                ]
            );
        });
    }

    #[test]
    fn prior_native_error_suppresses_already_computed_exp_rows() {
        thread::scope(|scope| {
            let clock = Instant::now();
            let rx = spawn_experiment(scope, clock, None, |emit| {
                emit(&decoded(2000.0, 0.5, "CQ G4ABC IO91"));
                Ok(ExperimentDiagnostics::default())
            });
            let mut error = Some("native callback failed".to_string());
            let mut ledger = OutputLedger::default();
            let mut diagnostics = HybridSlotDiagnostics::default();
            let rows = publish_experiment(
                rx,
                clock,
                &mut diagnostics,
                &mut ledger,
                &mut error,
                &mut |_| panic!("native error must suppress exp"),
            );
            assert!(rows.is_empty());
            assert!(ledger.tagged().is_empty());
            assert_eq!(error.as_deref(), Some("native callback failed"));
            assert!(diagnostics.experiment_finish.is_some());
        });
    }

    #[test]
    fn exp_callback_failure_keeps_collected_passive_rows_and_original_error() {
        thread::scope(|scope| {
            let clock = Instant::now();
            let rx = spawn_experiment(scope, clock, None, |emit| {
                emit(&decoded(1000.0, 0.5, "CQ K1ABC FN42"));
                emit(&decoded(2000.0, 0.5, "CQ G4ABC IO91"));
                Ok(ExperimentDiagnostics::default())
            });
            let mut error = None;
            let mut ledger = OutputLedger::default();
            let mut diagnostics = HybridSlotDiagnostics::default();
            let mut callbacks = 0;
            let rows = publish_experiment(
                rx,
                clock,
                &mut diagnostics,
                &mut ledger,
                &mut error,
                &mut |_| {
                    callbacks += 1;
                    Err("exp callback failed".into())
                },
            );
            assert_eq!(callbacks, 1);
            assert_eq!(rows.len(), 2);
            assert_eq!(ledger.tagged().len(), 2);
            assert_eq!(error.as_deref(), Some("exp callback failed"));
        });
    }

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
