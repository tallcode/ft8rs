//! Common execution boundary for isolated, unchanged native observations.
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObservationId {
    Wsjtx,
    Jtdx,
    JtdxAgc,
}

pub(crate) enum NativeObservation<'a> {
    Wsjtx(&'a mut StreamDecodeSession),
    Jtdx(&'a mut JtdxStreamDecodeSession),
}
impl NativeObservation<'_> {
    pub(crate) fn decode<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        emit: F,
    ) -> Result<Vec<StreamDecodedWithProvenance>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        match self {
            Self::Wsjtx(session) => {
                session.decode_slot_streaming_with_provenance_at(timestamp, samples, emit)
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
