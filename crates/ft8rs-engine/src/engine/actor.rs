//! Decode session and stage handling inside the supervised worker process.
#[cfg(test)]
use std::sync::mpsc::{self, SyncSender};
use std::sync::mpsc::{Receiver, Sender};
#[cfg(test)]
use std::thread;
use std::time::Instant;

use ft8rs::stream::profile::{
    A7Snapshot, JtdxRestore, ProfileSlotState, ProfileStreamDecodeSession, SlotRecovery,
};
use ft8rs::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedWithProvenance,
    StreamSlotDecodeState,
};
use ft8rs::SlotTimestamp;

#[cfg(test)]
use super::DECODE_QUEUE_CAP;
use crate::protocol::{DecodeRecord, DecodeStage, DxContextSnapshot, EngineEvent, HisgridSource};
use crate::soundcard::SlotStage;
#[cfg(test)]
use crate::worker::pause_fixture_after_confirmation;

pub(crate) enum DecodeCmd {
    Stage {
        stage: SlotStage,
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
        submitted: Instant,
        recovery_deadline: Option<Instant>,
        restore_a7: Option<A7Snapshot>,
        restore_jtdx: [Option<JtdxRestore>; 2],
        supervised: bool,
    },
    /// Discard the supervisor's cached WSJT-X and JTDX checkpoints.
    InvalidateWorkerHistory,
    Barrier(Sender<()>),
    Reconfigure {
        config: StreamDecodeConfig,
        reset_dx_target: bool,
        reset_dx_operator: bool,
    },
    Stop,
}

/// wsjtx uses the staged `StreamDecodeSession` (early decode); other profiles use
/// the unified `ProfileStreamDecodeSession` (final decode only) — mirroring the
/// shipped soundcard worker exactly, so decode behavior is unchanged.
#[expect(
    clippy::large_enum_variant,
    reason = "These long-lived sessions retain their existing inline ownership and construction"
)]
enum DecodeSession {
    Wsjtx(StreamDecodeSession),
    Profile(ProfileStreamDecodeSession),
}

fn build_session(config: &StreamDecodeConfig) -> DecodeSession {
    if config.profile == DecodeProfile::Wsjtx {
        DecodeSession::Wsjtx(StreamDecodeSession::new(config.clone()))
    } else {
        DecodeSession::Profile(ProfileStreamDecodeSession::new(config.clone()))
    }
}

/// Rebuild the decode session for a new config, carrying forward DX intel where
/// valid (dx → dx). wsjtx uses the staged session; everything else the unified
/// one. The DX carry-over honors the reconfig plan's reset flags.
fn reconfigure_session(
    old: DecodeSession,
    config: StreamDecodeConfig,
    reset_dx_target: bool,
    reset_dx_operator: bool,
) -> DecodeSession {
    if config.profile == DecodeProfile::Wsjtx {
        return DecodeSession::Wsjtx(StreamDecodeSession::new(config));
    }
    match old {
        DecodeSession::Profile(profile) => {
            DecodeSession::Profile(profile.reconfigure(config, reset_dx_target, reset_dx_operator))
        }
        DecodeSession::Wsjtx(_) => DecodeSession::Profile(ProfileStreamDecodeSession::new(config)),
    }
}

fn export_hash(session: &DecodeSession) -> Vec<String> {
    match session {
        DecodeSession::Wsjtx(s) => s.export_regular_hash_calls(),
        DecodeSession::Profile(p) => p.export_hash_calls(),
    }
}

fn import_hash(session: &mut DecodeSession, calls: &[String]) {
    match session {
        DecodeSession::Wsjtx(s) => s.import_hash_calls(calls),
        DecodeSession::Profile(p) => p.import_hash_calls(calls),
    }
}

fn map_dx_snapshot(snapshot: ft8rs::decode::dx::DxSnapshot) -> DxContextSnapshot {
    use ft8rs::decode::dx::HisgridSource as Core;
    let hisgrid_source = match snapshot.hisgrid_source {
        Core::None => None,
        Core::User => Some(HisgridSource::User),
        Core::Harvested => Some(HisgridSource::Harvested),
    };
    DxContextSnapshot {
        target: snapshot.target,
        foci: snapshot.foci,
        tx_parity: snapshot.tx_parity,
        hisgrid: snapshot.hisgrid,
        hisgrid_source,
        dt: snapshot.dt,
    }
}

fn send_record(
    evt_tx: &Sender<EngineEvent>,
    timestamp: &SlotTimestamp,
    row: &StreamDecodedWithProvenance,
    stage: DecodeStage,
) {
    let _ = evt_tx.send(EngineEvent::Decode(DecodeRecord {
        timestamp: timestamp.clone(),
        row: row.decode.clone(),
        provenance: row.provenance,
        stage,
    }));
}

#[cfg(test)]
pub(super) fn spawn_decode_actor(
    config: StreamDecodeConfig,
) -> (SyncSender<DecodeCmd>, Receiver<EngineEvent>) {
    // Test-only in-process actor queue. Tests feed with blocking sends;
    // production uses the supervised worker and responsive `send_stage` loop.
    let (cmd_tx, cmd_rx) = mpsc::sync_channel::<DecodeCmd>(DECODE_QUEUE_CAP);
    let (evt_tx, evt_rx) = mpsc::channel::<EngineEvent>();
    thread::spawn(move || decode_actor(config, cmd_rx, evt_tx));
    (cmd_tx, evt_rx)
}

pub(crate) fn decode_actor(
    config: StreamDecodeConfig,
    cmd_rx: Receiver<DecodeCmd>,
    evt_tx: Sender<EngineEvent>,
) {
    let mut experiment_enabled = config.hybrid_experiment;
    let mut session = build_session(&config);
    // Classic WSJT-X preserves staged audio and duplicate tracking through N50.
    let mut slot_state: Option<StreamSlotDecodeState> = None;
    // JTDX/DX only run their searches at N50; their sessions persist across slots.
    let mut profile_slot: Option<(ProfileSlotState, usize)> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            DecodeCmd::Stop => break,
            DecodeCmd::InvalidateWorkerHistory => {}
            DecodeCmd::Barrier(done) => {
                let _ = done.send(());
            }
            DecodeCmd::Reconfigure {
                config,
                reset_dx_target,
                reset_dx_operator,
            } => {
                experiment_enabled = config.hybrid_experiment;
                let calls = export_hash(&session);
                session = reconfigure_session(session, config, reset_dx_target, reset_dx_operator);
                import_hash(&mut session, &calls);
                slot_state = None;
                profile_slot = None;
            }
            DecodeCmd::Stage {
                stage,
                timestamp,
                samples_12k,
                submitted: _,
                recovery_deadline,
                restore_a7,
                restore_jtdx,
                supervised,
            } => match (&mut session, stage) {
                (DecodeSession::Wsjtx(s), SlotStage::N41) => {
                    if let Some(snapshot) = restore_a7 {
                        s.restore_a7(&timestamp, snapshot);
                    }
                    let mut state = s.start_slot_decode();
                    match s.decode_slot_nzhsym41_tagged(
                        Some(&timestamp),
                        &mut state,
                        &samples_12k,
                        |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Early);
                            Ok(())
                        },
                    ) {
                        Ok(()) => slot_state = Some(state),
                        Err(err) => {
                            let _ = evt_tx.send(EngineEvent::Error(err));
                        }
                    }
                }
                (DecodeSession::Wsjtx(s), SlotStage::N47) => {
                    if let Some(state) = slot_state.as_mut() {
                        s.subtract_slot_nzhsym47(state, &samples_12k);
                    }
                }
                (DecodeSession::Wsjtx(s), SlotStage::N50) => {
                    if let Some(state) = slot_state.take() {
                        match s.decode_slot_nzhsym50_tagged(state, &samples_12k, |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Final);
                            #[cfg(test)]
                            pause_fixture_after_confirmation(&timestamp, &row.decode.msg);
                            Ok(())
                        }) {
                            Ok(count) => {
                                let _ = evt_tx.send(EngineEvent::SlotComplete { timestamp, count });
                            }
                            Err(err) => {
                                let _ = evt_tx.send(EngineEvent::Error(err));
                            }
                        }
                    }
                }
                (DecodeSession::Profile(p), SlotStage::N41) => {
                    // Hybrid streams early WSJT-X rows here (before the slot
                    // boundary); jtdx/dx are no-ops that just open a stateless slot.
                    if let Some(snapshot) = restore_a7 {
                        p.restore_a7(&timestamp, snapshot);
                    }
                    p.restore_jtdx(restore_jtdx);
                    let mut state = p.start_slot();
                    match p.decode_slot_nzhsym41_streaming_with_provenance(
                        &timestamp,
                        &mut state,
                        &samples_12k,
                        |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Early);
                            Ok(())
                        },
                    ) {
                        Ok(early_count) => profile_slot = Some((state, early_count)),
                        Err(err) => {
                            let _ = evt_tx.send(EngineEvent::Error(err));
                        }
                    }
                }
                (DecodeSession::Profile(p), SlotStage::N47) => {
                    if let Some((state, _)) = profile_slot.as_mut() {
                        p.subtract_slot_nzhsym47(state, &samples_12k);
                    }
                }
                (DecodeSession::Profile(p), SlotStage::N50) => {
                    // Finish the slot: hybrid emits the remaining WSJT-X rows then
                    // JTDX-unique rows; jtdx/dx run their full decode here.
                    if let Some((state, _)) = profile_slot.take() {
                        let mut checkpoint = |snapshot| {
                            let _ = evt_tx.send(EngineEvent::A7Checkpoint {
                                timestamp: timestamp.clone(),
                                snapshot,
                            });
                        };
                        let mut jtdx_checkpoint = |branch, update, publish| {
                            #[cfg(test)]
                            let message = match &update {
                                ft8rs::stream::profile::JtdxCheckpoint::Row(entry) => {
                                    Some(entry.row.decode.msg.clone())
                                }
                                _ => None,
                            };
                            let _ = evt_tx.send(EngineEvent::JtdxCheckpoint {
                                timestamp: timestamp.clone(),
                                branch,
                                update,
                                publish,
                            });
                            #[cfg(test)]
                            if let Some(message) = message {
                                pause_fixture_after_confirmation(&timestamp, &message);
                            }
                        };
                        let result = p.finish_slot_nzhsym50(
                            &timestamp,
                            state,
                            &samples_12k,
                            SlotRecovery {
                                deadline: recovery_deadline,
                                checkpoint: supervised.then_some(&mut checkpoint),
                                jtdx_checkpoint: supervised.then_some(&mut jtdx_checkpoint),
                            },
                            |row| {
                                send_record(&evt_tx, &timestamp, row, DecodeStage::Final);
                                #[cfg(test)]
                                pause_fixture_after_confirmation(&timestamp, &row.decode.msg);
                                Ok(())
                            },
                        );
                        if let ProfileStreamDecodeSession::Hybrid(session) = p {
                            log::info!(target: "ft8rs_engine::engine",
                                "Hybrid slot {}: experiment_enabled={} samples={} diagnostics={:?}",
                                timestamp.format_time(),
                                experiment_enabled,
                                samples_12k.len(),
                                session.last_diagnostics().experiment,
                            );
                        }
                        match result {
                            Ok(count) => {
                                if let Some(snapshot) = p.dx_context_snapshot() {
                                    let _ = evt_tx
                                        .send(EngineEvent::DxContext(map_dx_snapshot(snapshot)));
                                }
                                let _ = evt_tx.send(EngineEvent::SlotComplete { timestamp, count });
                            }
                            Err(err) => {
                                let _ = evt_tx.send(EngineEvent::Error(err));
                            }
                        }
                    }
                }
            },
        }
    }
}
