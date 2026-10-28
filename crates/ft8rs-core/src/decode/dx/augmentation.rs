//! Terminal target recovery policy. None of these observations update legacy DX state.
use super::context::TargetContextStore;
use super::filter::DxTarget;
use crate::decode::cooperative::message::is_call;
use crate::decode::cooperative::{normalize_word, recovery, same_signal, ParsedMessage};
use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeProvenance, StreamDecodedMessage, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;
use std::time::Duration;

const MAX_RECOVERY_FOCI: usize = 2;
const SEED_NEIGHBORHOOD_HZ: f64 = 100.0;
pub(super) const START_BUDGET: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct RecoveryPlan {
    pub foci: Vec<f64>,
    pub seeds: Vec<StreamDecodedMessage>,
    pub resolved_foci: usize,
}

pub(super) fn plan(
    snapshot: &TargetContextStore,
    target: &DxTarget,
    timestamp: &SlotTimestamp,
    config: &StreamDecodeConfig,
    observations: [&[StreamDecodedWithProvenance]; 3],
    emitted: &[StreamDecodedWithProvenance],
) -> RecoveryPlan {
    let mut foci = if snapshot.should_run_focused(timestamp) {
        snapshot.selected_foci()
    } else {
        Vec::new()
    };
    // A target recipient never owns the sender's frequency. Additional native
    // observations are slot-local opportunities, not new strong legacy state.
    for row in observations.into_iter().flatten() {
        let decode = &row.decode;
        if row.provenance == StreamDecodeProvenance::Regular
            && snapshot.should_emit_target_row(decode)
            && target_sender(decode, target)
            && !foci.iter().any(|f| (f - decode.freq).abs() <= 25.0)
        {
            foci.push(decode.freq);
        }
    }
    let expected_dt = snapshot.snapshot_parts().4;
    let mut result = RecoveryPlan::default();
    for focus in foci {
        if !focus.is_finite() || focus < config.nfa || focus > config.nfb {
            continue;
        }
        if emitted.iter().any(|row| {
            snapshot.should_emit_target_row(&row.decode)
                && target_sender(&row.decode, target)
                && (row.decode.freq - focus).abs() <= 3.0
                && expected_dt.is_none_or(|dt| (row.decode.dt - dt).abs() <= 0.3)
        }) {
            result.resolved_foci += 1;
        } else if result.foci.len() < MAX_RECOVERY_FOCI {
            result.foci.push(focus);
        }
    }
    let mut candidates: Vec<_> = observations
        .into_iter()
        .flatten()
        .filter(|row| row.provenance == StreamDecodeProvenance::Regular)
        .map(|row| &row.decode)
        .filter(|row| {
            !target.matches_message(&row.msg)
                && result
                    .foci
                    .iter()
                    .any(|f| (row.freq - f).abs() <= SEED_NEIGHBORHOOD_HZ)
                && recovery::eligible_shape(row, config.nfa, config.nfb)
        })
        .collect();
    candidates.sort_by(|a, b| {
        b.snr
            .total_cmp(&a.snr)
            .then_with(|| a.freq.total_cmp(&b.freq))
    });
    for row in candidates {
        if !result.seeds.iter().any(|old| same_signal(old, row)) {
            result.seeds.push(row.clone());
        }
        if result.seeds.len() == recovery::MAX_SEEDS {
            break;
        }
    }
    result
}

fn target_sender(row: &StreamDecodedMessage, target: &DxTarget) -> bool {
    ParsedMessage::parse(&row.msg)
        .sender
        .as_deref()
        .is_some_and(|call| target.matches_word(call))
}

pub(super) fn wsjtx_observation_config(config: &StreamDecodeConfig) -> StreamDecodeConfig {
    let mut observed = config.clone_for_profile_wsjt_x();
    observed.mycall = None;
    observed.hiscall = None;
    observed.hisgrid = None;
    observed.nQSOProgress = 0;
    observed.nagain = false;
    observed.lhound = false;
    observed
}

pub(super) fn recovery_config(
    config: &StreamDecodeConfig,
    focus: f64,
    grid: Option<&str>,
) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_wsjt_x();
    let focus = focus.clamp(config.nfa, config.nfb);
    focused.nfqso = focus;
    focused.nfa = (focus - 25.0).max(config.nfa);
    focused.nfb = (focus + 25.0).min(config.nfb);
    focused.nagain = false;
    let pair = config
        .mycall
        .as_deref()
        .zip(config.hiscall.as_deref())
        .map(|(my, his)| (normalize_word(my.trim()), normalize_word(his.trim())))
        .filter(|(my, his)| is_call(my) && is_call(his) && my != his);
    if let Some((my, his)) = pair {
        focused.mycall = Some(my);
        focused.hiscall = Some(his);
        focused.hisgrid = grid.map(str::to_string);
    } else {
        // A partial call pair can produce incomplete target-assisted output.
        // Ordinary native search and hash lookup remain available in this
        // private session; no missing identity is guessed or backfilled.
        focused.mycall = None;
        focused.hiscall = None;
        focused.hisgrid = None;
        focused.nQSOProgress = 0;
    }
    focused
}

pub(super) fn admissible_recovery(row: &StreamDecodedMessage) -> bool {
    !row.msg
        .split_whitespace()
        .any(|word| normalize_word(word) == "<>")
}

pub(super) fn budget_expired(config: &StreamDecodeConfig, elapsed: Duration) -> bool {
    (config.cooperative_realtime && elapsed >= START_BUDGET)
        || super::dx_focused_budget_expired(config, elapsed)
}

pub(super) struct RecoveryRequest<'a> {
    pub plan: &'a RecoveryPlan,
    pub config: &'a StreamDecodeConfig,
    pub timestamp: &'a SlotTimestamp,
    pub samples: &'a [f32],
    pub grid: Option<&'a str>,
    pub hash_calls: &'a [String],
    pub started: std::time::Instant,
}
#[derive(Default)]
pub(super) struct RecoveryOutcome {
    pub tasks: usize,
    pub budget_skips: usize,
    pub rejected_empty: usize,
    pub stats: recovery::CancellationStats,
    pub errors: Vec<String>,
}
enum RecoveryEvent {
    Row(usize, Box<StreamDecodedMessage>),
    Done(Result<(), String>),
    Skipped,
}

pub(super) fn execute<F>(request: RecoveryRequest<'_>, mut emit: F) -> RecoveryOutcome
where
    F: FnMut(&StreamDecodedMessage),
{
    use crate::decode::cooperative::observation::NativeObservation;
    use crate::stream::session::StreamDecodeSession;
    use std::sync::mpsc;
    let mut outcome = RecoveryOutcome::default();
    if request.plan.foci.is_empty() || request.plan.seeds.is_empty() {
        return outcome;
    }
    if budget_expired(request.config, request.started.elapsed()) {
        outcome.budget_skips = request.plan.foci.len();
        return outcome;
    }
    let (residual, stats) = recovery::prepare(
        request.samples,
        &request.plan.seeds,
        request.config.nfa,
        request.config.nfb,
    );
    outcome.stats = stats;
    if outcome.stats.accepted == 0 {
        return outcome;
    }
    let mut buffered = vec![Vec::new(); request.plan.foci.len()];
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        let workers = request
            .plan
            .foci
            .len()
            .min(std::thread::available_parallelism().map_or(1, usize::from));
        for worker in 0..workers {
            let tx = tx.clone();
            let request = &request;
            let residual = &residual;
            scope.spawn(move || {
                for id in (worker..request.plan.foci.len()).step_by(workers) {
                    if budget_expired(request.config, request.started.elapsed()) {
                        let _ = tx.send(RecoveryEvent::Skipped);
                        continue;
                    }
                    let result = super::guard_worker(|| {
                        let config =
                            recovery_config(request.config, request.plan.foci[id], request.grid);
                        let mut session = StreamDecodeSession::new(config);
                        session.import_hash_calls(request.hash_calls);
                        NativeObservation::Wsjtx(&mut session)
                            .decode(request.timestamp, residual, |row| {
                                tx.send(RecoveryEvent::Row(id, Box::new(row.clone())))
                                    .map_err(|_| "DX recovery event receiver closed".to_string())
                            })
                            .map(|_| ())
                    });
                    let _ = tx.send(RecoveryEvent::Done(result));
                }
            });
        }
        let mut finished = 0;
        while finished < request.plan.foci.len() {
            match rx.recv().expect("DX recovery coordinator retains a sender") {
                RecoveryEvent::Row(id, row) => {
                    if !admissible_recovery(&row) {
                        outcome.rejected_empty += 1;
                    } else if request.config.cooperative_realtime {
                        emit(&row);
                    } else {
                        buffered[id].push(row);
                    }
                }
                RecoveryEvent::Done(result) => {
                    finished += 1;
                    outcome.tasks += 1;
                    if let Err(error) = result {
                        outcome.errors.push(error);
                    }
                }
                RecoveryEvent::Skipped => {
                    finished += 1;
                    outcome.budget_skips += 1;
                }
            }
        }
    });
    for group in buffered {
        for row in group {
            emit(&row);
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::{genft8::get_ft8_tones_from_77bits, packjt77::pack77};

    fn tagged(message: &str, freq: f64) -> StreamDecodedWithProvenance {
        StreamDecodedWithProvenance {
            decode: StreamDecodedMessage {
                freq,
                dt: 0.2,
                snr: -5.0,
                msg: message.into(),
                sync: 3.0,
                itone: get_ft8_tones_from_77bits(&pack77(message)),
            },
            provenance: StreamDecodeProvenance::Regular,
        }
    }
    fn snapshot() -> (TargetContextStore, DxTarget, StreamDecodeConfig) {
        let target = DxTarget::new("BG5ATV");
        let config = StreamDecodeConfig {
            nfqso: 1000.0,
            mycall: Some("K1JT".into()),
            hiscall: Some("BG5ATV".into()),
            ..Default::default()
        };
        let store = TargetContextStore::new(
            target.clone(),
            Some("K1JT"),
            1000.0,
            None,
            false,
            200.0,
            3000.0,
        );
        (store, target, config)
    }
    #[test]
    fn recipient_does_not_resolve_the_target_sender_focus() {
        let (store, target, config) = snapshot();
        let stamp = SlotTimestamp::parse("140600").unwrap();
        let seed = [tagged("CQ K1ABC FN42", 1025.0)];
        let recipient = [tagged("BG5ATV W9XYZ -07", 1000.0)];
        let plan = plan(
            &store,
            &target,
            &stamp,
            &config,
            [&seed, &[], &[]],
            &recipient,
        );
        assert_eq!(plan.foci, [1000.0]);
        assert_eq!(plan.seeds.len(), 1);
        let sender = [tagged("K1JT BG5ATV R-07", 1000.0)];
        let plan = super::plan(&store, &target, &stamp, &config, [&seed, &[], &[]], &sender);
        assert!(plan.foci.is_empty());
        assert!(plan.seeds.is_empty());
        assert_eq!(plan.resolved_foci, 1);
    }
    #[test]
    fn only_regular_non_target_waveforms_can_be_seeds() {
        let (store, target, config) = snapshot();
        let stamp = SlotTimestamp::parse("140600").unwrap();
        let mut assisted = tagged("CQ W9XYZ EN50", 970.0);
        assisted.provenance = StreamDecodeProvenance::CooperativeRecovery;
        let observations = [
            tagged("K1JT BG5ATV -04", 1000.0),
            assisted,
            tagged("CQ K1ABC FN42", 1025.0),
            tagged("CQ G4AAA IO91", 1800.0),
        ];
        let plan = plan(
            &store,
            &target,
            &stamp,
            &config,
            [&observations, &[], &[]],
            &[],
        );
        assert_eq!(plan.seeds.len(), 1);
        assert_eq!(plan.seeds[0].msg, "CQ K1ABC FN42");
    }
    #[test]
    fn missing_invalid_or_self_call_pairs_cannot_enable_target_ap() {
        for my in [None, Some(""), Some("<...>"), Some("bg5atv")] {
            let config = StreamDecodeConfig {
                mycall: my.map(str::to_string),
                hiscall: Some("BG5ATV".into()),
                hisgrid: Some("PM00".into()),
                ..Default::default()
            };
            let recovery = recovery_config(&config, 1000.0, Some("PM00"));
            assert!(recovery.mycall.is_none());
            assert!(recovery.hiscall.is_none());
            assert!(recovery.hisgrid.is_none());
            assert_eq!(config.hiscall.as_deref(), Some("BG5ATV"));
        }
        let (_, _, config) = snapshot();
        let recovery = recovery_config(&config, 1000.0, Some("PM00"));
        assert_eq!(recovery.mycall.as_deref(), Some("K1JT"));
        assert_eq!(recovery.hiscall.as_deref(), Some("BG5ATV"));
        assert_eq!(recovery.hisgrid.as_deref(), Some("PM00"));
        assert!(admissible_recovery(
            &tagged("<...> BG5ATV -07", 1000.0).decode
        ));
        assert!(!admissible_recovery(
            &tagged("<> BG5ATV -07", 1000.0).decode
        ));
    }
    #[test]
    fn realtime_start_budget_does_not_change_file_search_or_legacy_watchdog() {
        let config = StreamDecodeConfig {
            cooperative_realtime: true,
            ..Default::default()
        };
        assert!(!budget_expired(&config, Duration::from_millis(2999)));
        assert!(budget_expired(&config, Duration::from_secs(3)));
        assert!(!super::super::dx_focused_budget_expired(
            &config,
            Duration::from_secs(3)
        ));
        assert!(!budget_expired(
            &StreamDecodeConfig::default(),
            Duration::from_secs(30)
        ));
    }
}
