use crate::decode::dx::DxStreamDecodeSession;
use crate::decode::hybrid::HybridStreamDecodeSession;
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage,
};
use crate::stream::time::SlotTimestamp;

#[allow(clippy::large_enum_variant)]
pub enum ProfileStreamDecodeSession {
    Wsjtx(StreamDecodeSession),
    Jtdx(JtdxStreamDecodeSession),
    Hybrid(HybridStreamDecodeSession),
    Dx(DxStreamDecodeSession),
}

impl ProfileStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        match config.profile {
            DecodeProfile::Wsjtx => {
                Self::Wsjtx(StreamDecodeSession::new(config.clone_for_profile_wsjt_x()))
            }
            DecodeProfile::Jtdx => Self::Jtdx(JtdxStreamDecodeSession::new(
                config.clone_for_profile_jtdx(),
            )),
            DecodeProfile::Hybrid => Self::Hybrid(HybridStreamDecodeSession::new(config)),
            DecodeProfile::Dx => Self::Dx(DxStreamDecodeSession::new(config)),
        }
    }

    /// Seed the learned hash-call book of the underlying session. Used by the
    /// live engine to migrate hash calls across a session rebuild (GUI_PLAN.md
    /// §5.3). Hybrid manages its own shared hash book and DX seeds from the
    /// target calls, so both are no-ops here.
    pub fn import_hash_calls(&mut self, calls: &[String]) {
        match self {
            Self::Wsjtx(session) => session.import_hash_calls(calls),
            Self::Jtdx(session) => session.import_hash_calls(calls),
            Self::Hybrid(_) | Self::Dx(_) => {}
        }
    }

    /// Export the learned regular hash calls for migration into a rebuilt
    /// session. Hybrid/DX return empty (their hash state is reseeded on rebuild).
    pub fn export_hash_calls(&self) -> Vec<String> {
        match self {
            Self::Wsjtx(session) => session.export_regular_hash_calls(),
            Self::Jtdx(session) => session.export_regular_hash_calls(),
            Self::Hybrid(_) | Self::Dx(_) => Vec::new(),
        }
    }

    pub fn decode_slot_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
    ) -> Vec<StreamDecodedMessage> {
        self.decode_slot_streaming_at(timestamp, samples, |_| Ok(()))
            .expect("in-memory profile decode callback cannot fail")
    }

    pub fn decode_slot_streaming_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        match self {
            Self::Wsjtx(decoder) => decoder.decode_slot_streaming_at(timestamp, samples, on_decode),
            Self::Jtdx(decoder) => decoder.decode_slot_streaming_at(timestamp, samples, on_decode),
            Self::Hybrid(decoder) => {
                decoder.decode_slot_streaming_at(timestamp, samples, on_decode)
            }
            Self::Dx(decoder) => decoder.decode_slot_streaming_at(timestamp, samples, on_decode),
        }
    }
}
