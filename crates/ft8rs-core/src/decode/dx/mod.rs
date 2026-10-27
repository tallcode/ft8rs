//! Target-only orchestration using isolated, unchanged upstream sessions.
use crate::decode::cooperative::{OutputLedger, SignalSource};
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance, StreamDecodeSession,
    StreamDecodedMessage, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;
use std::sync::mpsc;
use std::time::Duration;

mod context;
mod filter;
use context::TargetContextStore;
use filter::DxTarget;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HisgridSource {
    #[default]
    None,
    User,
    Harvested,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DxSnapshot {
    pub target: String,
    pub foci: Vec<f64>,
    pub tx_parity: Option<u8>,
    pub hisgrid: Option<String>,
    pub hisgrid_source: HisgridSource,
    pub dt: Option<f64>,
}

#[derive(Clone)]
struct RecoveryTask {
    config: StreamDecodeConfig,
    wsjtx: bool,
    cold: bool,
}
enum Event {
    Row(usize, Box<StreamDecodedMessage>),
    Done(usize, Result<Vec<StreamDecodedWithProvenance>, String>),
}

pub struct DxStreamDecodeSession {
    base_config: StreamDecodeConfig,
    context: TargetContextStore,
    hash_seed_calls: Vec<String>,
    listen: JtdxStreamDecodeSession,
    cold_misses: u8,
}
impl DxStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        let target = DxTarget::new(config.hiscall.as_deref().unwrap_or_default());
        let hash_seed_calls = dx_hash_seed_calls(&config);
        let mut listen = JtdxStreamDecodeSession::new(dx_listen_config(&config));
        listen.import_hash_calls(&hash_seed_calls);
        let context = TargetContextStore::new(
            target.clone(),
            config.mycall.as_deref(),
            config.nfqso,
            config.hisgrid.as_deref(),
            config.lhound,
            config.nfa,
            config.nfb,
        );
        Self {
            base_config: config,
            context,
            hash_seed_calls,
            listen,
            cold_misses: 0,
        }
    }
    pub fn reconfigured(
        &self,
        new_config: StreamDecodeConfig,
        reset_target: bool,
        reset_operator: bool,
    ) -> Self {
        let same_target = !reset_target
            && normalize_call(self.base_config.hiscall.as_deref())
                == normalize_call(new_config.hiscall.as_deref());
        let mut next = Self::new(new_config);
        if same_target {
            let mut context = self.context.clone();
            if reset_operator {
                context.drop_operator_intel();
            }
            context.apply_user_config(
                next.base_config.mycall.as_deref(),
                next.base_config.hisgrid.as_deref(),
                next.base_config.nfqso,
                next.base_config.lhound,
                next.base_config.nfa,
                next.base_config.nfb,
            );
            next.context = context;
            if !reset_operator {
                next.cold_misses = self.cold_misses;
            }
        }
        next
    }
    pub fn context_snapshot(&self) -> DxSnapshot {
        let (foci, tx_parity, hisgrid, hisgrid_source, dt) = self.context.snapshot_parts();
        DxSnapshot {
            target: self.base_config.hiscall.clone().unwrap_or_default(),
            foci,
            tx_parity,
            hisgrid,
            hisgrid_source,
            dt,
        }
    }
    pub fn decode_slot_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
    ) -> Vec<StreamDecodedMessage> {
        self.decode_slot_streaming_at(timestamp, samples, |_| Ok(()))
            .expect("in-memory DX callback cannot fail")
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
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        self.context.begin_slot(timestamp);
        let snapshot = self.context.clone();
        let started = std::time::Instant::now();
        let known_foci = snapshot.selected_foci();
        let foci = if self.base_config.mycall.is_some() && snapshot.should_run_focused(timestamp) {
            known_foci.clone()
        } else {
            Vec::new()
        };
        let mut tasks = Vec::new();
        for focus in foci.iter().copied() {
            tasks.push(RecoveryTask {
                config: dx_focus_config(&self.base_config, focus),
                wsjtx: false,
                cold: false,
            });
            if let Some(grid) = snapshot.hisgrid() {
                tasks.push(RecoveryTask {
                    config: dx_a8_config(&self.base_config, focus, grid),
                    wsjtx: true,
                    cold: false,
                });
            }
        }
        // Two standard attempts followed by one quiet slot. Both parities are
        // covered over time; no blind full-band deep search is introduced.
        if known_foci.is_empty() && self.base_config.mycall.is_some() {
            if self.cold_misses >= 2 {
                self.cold_misses = 0;
            } else {
                let mut config = self.base_config.clone_for_profile_wsjt_x();
                config.nagain = false;
                tasks.push(RecoveryTask {
                    config,
                    wsjtx: true,
                    cold: true,
                });
            }
        }
        let cold_started = tasks.iter().any(|t| t.cold);
        let mut ledger = OutputLedger::with_frequency_tolerance(3.0);
        let mut finished_rows = vec![Vec::new(); tasks.len() + 1];
        let mut first_error = None;
        let realtime = self.base_config.cooperative_realtime;
        let hash_calls = &self.hash_seed_calls;
        let listen = &mut self.listen;
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            let listen_tx = tx.clone();
            scope.spawn(move || {
                let result = guard_worker(|| {
                    listen.decode_slot_streaming_with_provenance_at(timestamp, samples, |row| {
                        listen_tx
                            .send(Event::Row(0, Box::new(row.clone())))
                            .map_err(|_| "DX event receiver closed".to_string())
                    })
                });
                let _ = listen_tx.send(Event::Done(0, result));
            });
            let workers = tasks.len().min(4);
            for worker in 0..workers {
                let assigned: Vec<_> = tasks
                    .iter()
                    .enumerate()
                    .filter(|(id, _)| id % workers == worker)
                    .map(|(id, task)| (id + 1, task.clone()))
                    .collect();
                let task_tx = tx.clone();
                scope.spawn(move || {
                    for (id, task) in assigned {
                        let result = guard_worker(|| {
                            if dx_focused_budget_expired(&task.config, started.elapsed()) {
                                return Ok(Vec::new());
                            }
                            let emit = |row: &StreamDecodedMessage| {
                                task_tx
                                    .send(Event::Row(id, Box::new(row.clone())))
                                    .map_err(|_| "DX event receiver closed".to_string())
                            };
                            if task.wsjtx {
                                let mut session = StreamDecodeSession::new(task.config);
                                session.import_hash_calls(hash_calls);
                                session.decode_slot_streaming_with_provenance_at(
                                    timestamp, samples, emit,
                                )
                            } else {
                                let mut session = JtdxStreamDecodeSession::new(task.config);
                                session.import_hash_calls(hash_calls);
                                session.decode_slot_streaming_with_provenance_at(
                                    timestamp, samples, emit,
                                )
                            }
                        });
                        let _ = task_tx.send(Event::Done(id, result));
                    }
                });
            }
            let mut completed = 0;
            while completed < tasks.len() + 1 {
                match rx.recv().expect("DX coordinator retains an event sender") {
                    Event::Row(id, row) => {
                        if id == 0 || realtime {
                            emit_target(
                                &snapshot,
                                &mut ledger,
                                &row,
                                if id == 0 {
                                    StreamDecodeProvenance::Unknown
                                } else {
                                    StreamDecodeProvenance::ContextRecovery
                                },
                                &mut first_error,
                                &mut on_decode,
                            );
                        }
                    }
                    Event::Done(id, result) => {
                        completed += 1;
                        match result {
                            Ok(rows) => finished_rows[id] = rows,
                            Err(error) => {
                                first_error.get_or_insert(error);
                            }
                        }
                    }
                }
            }
        });
        // Commit after every task: the whole slot used one frozen snapshot.
        if !realtime {
            for group in finished_rows.iter().skip(1) {
                for row in group {
                    emit_target(
                        &snapshot,
                        &mut ledger,
                        &row.decode,
                        StreamDecodeProvenance::ContextRecovery,
                        &mut first_error,
                        &mut on_decode,
                    );
                }
            }
        }
        self.context.harvest_tagged(timestamp, &finished_rows[0]);
        for (index, group) in finished_rows.iter().skip(1).enumerate() {
            let rows: Vec<_> = group
                .iter()
                .filter(|row| snapshot.should_emit_target_row(&row.decode))
                .map(|row| row.decode.clone())
                .collect();
            if tasks[index].cold {
                if rows.is_empty() {
                    self.cold_misses = self.cold_misses.saturating_add(1);
                } else {
                    self.cold_misses = 0;
                }
            }
            self.context.harvest_focused(timestamp, &rows);
        }
        if !cold_started && !foci.is_empty() {
            self.cold_misses = 0;
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(ledger.into_sorted())
        }
    }
}
fn emit_target<F>(
    context: &TargetContextStore,
    ledger: &mut OutputLedger,
    row: &StreamDecodedMessage,
    provenance: StreamDecodeProvenance,
    error: &mut Option<String>,
    on_decode: &mut F,
) where
    F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
{
    if context.should_emit_target_row(row)
        && ledger.admit(
            if provenance == StreamDecodeProvenance::Unknown {
                SignalSource::Jtdx
            } else {
                SignalSource::Recovery
            },
            row,
            provenance,
        )
        && error.is_none()
    {
        if let Err(failure) = on_decode(&StreamDecodedWithProvenance {
            decode: row.clone(),
            provenance,
        }) {
            *error = Some(failure);
        }
    }
}
fn guard_worker<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map_err(|_| "DX worker panicked".to_string())?
}

fn dx_listen_config(config: &StreamDecodeConfig) -> StreamDecodeConfig {
    let mut listen = config.clone_for_profile_jtdx();
    listen.swl = true;
    listen.nagain = false;
    listen.filter = false;
    listen.mycall = None;
    listen.hiscall = None;
    listen.hisgrid = None;
    listen.lhound = false;
    listen
}

fn dx_focus_config(config: &StreamDecodeConfig, focus: f64) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_jtdx();
    let focus = focus.clamp(config.nfa, config.nfb);
    focused.profile = DecodeProfile::Jtdx;
    focused.swl = true;
    focused.nagain = true;
    focused.filter = false;
    focused.nfqso = focus;
    focused.nfa = (focus - 25.0).max(config.nfa);
    focused.nfb = (focus + 25.0).min(config.nfb);
    focused
}

fn dx_a8_config(config: &StreamDecodeConfig, focus: f64, hisgrid: &str) -> StreamDecodeConfig {
    let mut focused = config.clone_for_profile_wsjt_x();
    let focus = focus.clamp(config.nfa, config.nfb);
    focused.profile = DecodeProfile::Wsjtx;
    focused.nfqso = focus;
    focused.nfa = (focus - 25.0).max(config.nfa);
    focused.nfb = (focus + 25.0).min(config.nfb);
    focused.hisgrid = Some(hisgrid.to_string());
    focused.lft8apon = true;
    focused
}

fn dx_hash_seed_calls(config: &StreamDecodeConfig) -> Vec<String> {
    let mut calls = Vec::new();
    push_normalized_call(&mut calls, config.hiscall.as_deref());
    push_normalized_call(&mut calls, config.mycall.as_deref());
    calls
}

fn dx_focused_budget_expired(config: &StreamDecodeConfig, elapsed: Duration) -> bool {
    config
        .dx_monitor_watchdog_ms
        .is_some_and(|budget_ms| elapsed >= Duration::from_millis(budget_ms))
}

fn normalize_call(call: Option<&str>) -> Option<String> {
    let call = call?.trim();
    if call.is_empty() {
        None
    } else {
        Some(call.to_ascii_uppercase())
    }
}

fn push_normalized_call(calls: &mut Vec<String>, call: Option<&str>) {
    let Some(call) = call else {
        return;
    };
    let call = call.trim();
    if call.is_empty() {
        return;
    }
    let call = call.to_ascii_uppercase();
    if !calls.iter().any(|existing| existing == &call) {
        calls.push(call);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::session::DecodeProfile;

    #[test]
    fn dx_listen_config_pins_primary_pass() {
        let mut config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            swl: false,
            nagain: true,
            filter: true,
            lhound: true,
            mycall: Some("F1MLZ".to_string()),
            hiscall: Some("UA3QNA".to_string()),
            hisgrid: Some("KO95".to_string()),
            ..StreamDecodeConfig::default()
        };
        config.nfa = 100.0;
        config.nfb = 3200.0;
        config.nfqso = 1152.0;

        let listen = dx_listen_config(&config);

        assert_eq!(listen.profile, DecodeProfile::Jtdx);
        assert!(listen.swl);
        assert!(!listen.nagain);
        assert!(!listen.filter);
        assert!(!listen.lhound);
        assert_eq!(listen.nfa, 100.0);
        assert_eq!(listen.nfb, 3200.0);
        assert_eq!(listen.nfqso, 1152.0);
        assert!(listen.mycall.is_none());
        assert!(listen.hiscall.is_none());
        assert!(listen.hisgrid.is_none());
    }

    #[test]
    fn dx_hash_seed_calls_keeps_target_calls_only_once() {
        let config = StreamDecodeConfig {
            mycall: Some(" f1mlz ".to_string()),
            hiscall: Some("F1MLZ".to_string()),
            ..StreamDecodeConfig::default()
        };

        assert_eq!(dx_hash_seed_calls(&config), vec!["F1MLZ".to_string()]);
    }

    #[test]
    fn dx_focus_config_enables_focused_deep_only_for_focus() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 200.0,
            nfb: 3000.0,
            mycall: Some("F1MLZ".to_string()),
            hiscall: Some("UA3QNA".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_focus_config(&config, 1152.0);

        assert_eq!(focused.profile, DecodeProfile::Jtdx);
        assert!(focused.swl);
        assert!(focused.nagain);
        assert_eq!(focused.nfqso, 1152.0);
        assert_eq!(focused.nfa, 1127.0);
        assert_eq!(focused.nfb, 1177.0);
        assert_eq!(focused.mycall.as_deref(), Some("F1MLZ"));
        assert_eq!(focused.hiscall.as_deref(), Some("UA3QNA"));
    }

    #[test]
    fn dx_a8_config_builds_focused_wsjtx_worker_config() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 200.0,
            nfb: 3000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_a8_config(&config, 1000.0, "PM00");

        assert_eq!(focused.profile, DecodeProfile::Wsjtx);
        assert_eq!(focused.nfqso, 1000.0);
        assert_eq!(focused.nfa, 975.0);
        assert_eq!(focused.nfb, 1025.0);
        assert_eq!(focused.mycall.as_deref(), Some("K1JT"));
        assert_eq!(focused.hiscall.as_deref(), Some("BG5ATV"));
        assert_eq!(focused.hisgrid.as_deref(), Some("PM00"));
        assert!(focused.lft8apon);
    }

    #[test]
    fn dx_focused_configs_clamp_out_of_band_focus() {
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Dx,
            nfa: 200.0,
            nfb: 3000.0,
            mycall: Some("K1JT".to_string()),
            hiscall: Some("BG5ATV".to_string()),
            ..StreamDecodeConfig::default()
        };

        let focused = dx_focus_config(&config, 3500.0);
        assert_eq!(focused.nfqso, 3000.0);
        assert_eq!(focused.nfa, 2975.0);
        assert_eq!(focused.nfb, 3000.0);

        let a8 = dx_a8_config(&config, 100.0, "PM00");
        assert_eq!(a8.nfqso, 200.0);
        assert_eq!(a8.nfa, 200.0);
        assert_eq!(a8.nfb, 225.0);
    }

    #[test]
    fn dx_monitor_watchdog_is_opt_in_and_budget_based() {
        let mut config = StreamDecodeConfig::default();

        assert!(!dx_focused_budget_expired(
            &config,
            Duration::from_secs(999)
        ));

        config.dx_monitor_watchdog_ms = Some(12_000);
        assert!(!dx_focused_budget_expired(
            &config,
            Duration::from_millis(11_999)
        ));
        assert!(dx_focused_budget_expired(
            &config,
            Duration::from_millis(12_000)
        ));
    }
}
