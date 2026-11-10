//! Slot result delivery and checkpoints retained across worker replacement.
use std::sync::mpsc::Sender;

use ft8rs::stream::profile::{same_signal, A7Snapshot, JtdxCheckpoint, JtdxRestore};
use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::{DecodeProfile, SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};

use crate::protocol::{DecodeRecord, DecodeStage, EngineEvent};

use super::ipc::{event, Response};

pub(super) fn parity(timestamp: &SlotTimestamp) -> usize {
    (timestamp.milliseconds_since_midnight() / 15_000 % 2) as usize
}

#[derive(Default)]
pub(super) struct WorkerHistory {
    pub(super) a7: [Option<(SlotTimestamp, A7Snapshot)>; 2],
    jtdx: [Option<(SlotTimestamp, JtdxRestore)>; 2],
}

impl WorkerHistory {
    pub(super) fn a7_for_slot(&mut self, timestamp: &SlotTimestamp) -> &mut A7Snapshot {
        let bank = &mut self.a7[parity(timestamp)];
        if bank.as_ref().is_none_or(|(slot, _)| slot != timestamp) {
            *bank = Some((timestamp.clone(), A7Snapshot::default()));
        }
        &mut bank.as_mut().unwrap().1
    }

    pub(super) fn jtdx_for_restart(&self, timestamp: &SlotTimestamp) -> [Option<JtdxRestore>; 2] {
        self.jtdx.each_ref().map(|branch| {
            branch
                .as_ref()
                .filter(|(slot, _)| {
                    slot.add_seconds(15) == *timestamp || slot.add_seconds(30) == *timestamp
                })
                .map(|(_, restore)| restore.clone())
        })
    }
}

#[derive(Default)]
pub(super) struct SlotDelivery {
    pub(super) rows: Vec<StreamDecodedMessage>,
    pub(super) complete: bool,
}

impl SlotDelivery {
    pub(super) fn forward(
        &mut self,
        value: Response,
        history: &mut WorkerHistory,
        config: &StreamDecodeConfig,
        events: &Sender<EngineEvent>,
    ) -> Result<(), String> {
        let Some(value) = event(value)? else {
            return Ok(());
        };
        let value = match value {
            EngineEvent::A7Checkpoint {
                timestamp,
                snapshot,
            } => {
                let saved = history.a7_for_slot(&timestamp);
                let mut combined = snapshot;
                combined.merge(std::mem::take(saved));
                *saved = combined;
                return Ok(());
            }
            EngineEvent::JtdxCheckpoint {
                timestamp,
                branch,
                update,
                publish,
            } => {
                let (latest, restore) = history.jtdx[branch.index()]
                    .get_or_insert_with(|| (timestamp.clone(), JtdxRestore::default()));
                *latest = timestamp.clone();
                match update {
                    JtdxCheckpoint::Complete(snapshot) => {
                        restore.completed = Some(snapshot);
                        restore.partial.clear();
                        return Ok(());
                    }
                    JtdxCheckpoint::Row(confirmed) => {
                        let slot = timestamp.format();
                        if restore.partial.last().is_none_or(|(last, _)| last != &slot) {
                            // Preserve a partial bank of each parity across repeated kills.
                            if restore.partial.len() == 2 {
                                restore.partial.remove(0);
                            }
                            restore.partial.push((slot, Vec::new()));
                        }
                        let record = publish.then(|| DecodeRecord {
                            timestamp,
                            row: confirmed.row.decode.clone(),
                            provenance: if config.profile == DecodeProfile::Jtdx {
                                confirmed.row.provenance.for_jtdx_display()
                            } else {
                                StreamDecodeProvenance::Unknown
                            },
                            stage: DecodeStage::Final,
                        });
                        restore.partial.last_mut().unwrap().1.push(*confirmed);
                        let Some(record) = record else {
                            return Ok(());
                        };
                        EngineEvent::Decode(record)
                    }
                }
            }
            other => other,
        };
        let value = match value {
            EngineEvent::Decode(record) => {
                let native_seed = matches!(
                    config.profile,
                    DecodeProfile::Wsjtx | DecodeProfile::WsjtxPlus
                ) && record.provenance.is_wsjtx_history_seed();
                // Hybrid may seed A7 from an original JTDX row whose AP subtype
                // is unknown. Supplemental callbacks never become history.
                let hybrid_seed = config.profile == DecodeProfile::Hybrid
                    && (record.provenance == StreamDecodeProvenance::Unknown
                        || record.provenance.is_wsjtx_history_seed());
                if hybrid_seed || native_seed {
                    let frequency =
                        if native_seed && record.provenance == StreamDecodeProvenance::A8List {
                            config.nfqso
                        } else {
                            record.row.freq
                        };
                    history
                        .a7_for_slot(&record.timestamp)
                        .record(&record.row, frequency);
                }
                if self.rows.iter().any(|row| {
                    (config.profile != DecodeProfile::Dx
                        || (row.freq - record.row.freq).abs() <= 3.0)
                        && same_signal(row, &record.row)
                }) {
                    return Ok(());
                }
                self.rows.push(record.row.clone());
                EngineEvent::Decode(record)
            }
            EngineEvent::SlotComplete { timestamp, .. } => {
                if self.complete {
                    return Ok(());
                }
                self.complete = true;
                EngineEvent::SlotComplete {
                    timestamp,
                    count: self.rows.len(),
                }
            }
            other => other,
        };
        events.send(value).map_err(|_| "monitor stopped".into())
    }
}
