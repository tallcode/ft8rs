//! Common execution boundary for isolated WSJT-X+ and JTDX observations.
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::decode::lib_wsjtx_plus::session::StreamDecodeSession as WsjtxPlusSession;
use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeProvenance, StreamDecodedMessage, StreamDecodedWithProvenance,
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
impl NativeObservation<'_> {
    pub(crate) fn decode<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut emit: F,
    ) -> Result<Vec<StreamDecodedWithProvenance>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        match self {
            Self::WsjtxPlus(session) => {
                // DX publishes compatibility rows from the completed result.
                let mut rows = Vec::new();
                session.decode_slot_streaming_tagged_at(timestamp, samples, |row| {
                    if row.provenance != StreamDecodeProvenance::ContextRecovery {
                        emit(&row.decode)?;
                    }
                    rows.push(row.clone());
                    Ok(())
                })?;
                Ok(rows)
            }
            Self::Jtdx(session) => {
                session.decode_slot_streaming_with_provenance_at(timestamp, samples, emit)
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
