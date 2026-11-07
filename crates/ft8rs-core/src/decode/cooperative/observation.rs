//! Common execution boundary for isolated WSJT-X+ and JTDX observations.
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::decode::lib_wsjtx_plus::session::StreamDecodeSession as WsjtxPlusSession;
use crate::stream::profile::{A7Snapshot, JtdxCheckpoint};
use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeProvenance, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationId {
    Wsjtx,
    Jtdx,
    JtdxAgc,
}

pub(crate) enum NativeObservation<'a> {
    WsjtxPlus(&'a mut WsjtxPlusSession),
    Jtdx(&'a mut JtdxStreamDecodeSession),
}
pub(crate) enum ObservationHistory {
    A7(A7Snapshot),
    Jtdx(JtdxCheckpoint),
}

impl NativeObservation<'_> {
    pub(crate) fn decode_with_history<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        realtime: bool,
        mut history: Option<&mut dyn FnMut(ObservationHistory)>,
        mut emit: F,
    ) -> Result<Vec<StreamDecodedWithProvenance>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        match self {
            Self::WsjtxPlus(session) => {
                // File mode preserves its canonical compatibility ordering.
                let mut rows = Vec::new();
                session.decode_slot_streaming_tagged_at(timestamp, samples, |row| {
                    if let Some(history) = history.as_deref_mut() {
                        if matches!(
                            row.provenance,
                            StreamDecodeProvenance::Regular
                                | StreamDecodeProvenance::A7Memory
                                | StreamDecodeProvenance::A8List
                        ) {
                            let mut seeds = A7Snapshot::default();
                            seeds.record(&row.decode, row.decode.freq);
                            history(ObservationHistory::A7(seeds));
                        }
                    }
                    if realtime || row.provenance != StreamDecodeProvenance::PlusCompatibility {
                        emit(row)?;
                    }
                    rows.push(row.clone());
                    Ok(())
                })?;
                Ok(rows)
            }
            Self::Jtdx(session) => {
                let enabled = history.is_some();
                let mut checkpoint = |update| {
                    if let Some(history) = history.as_deref_mut() {
                        history(ObservationHistory::Jtdx(update));
                    }
                };
                session.decode_slot_tagged(
                    timestamp,
                    samples,
                    enabled.then_some(&mut checkpoint),
                    emit,
                )
            }
        }
    }
}

pub(crate) fn jtdx_branch_configs(config: &StreamDecodeConfig) -> [StreamDecodeConfig; 2] {
    let mut no_agc = config.clone_for_profile_jtdx();
    no_agc.nagcc = false;
    let mut agc = no_agc.clone();
    agc.nagcc = true;
    [no_agc, agc]
}
