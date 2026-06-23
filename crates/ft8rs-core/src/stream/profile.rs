use crate::decode::dx::{DxSnapshot, DxStreamDecodeSession};
use crate::decode::hybrid::HybridStreamDecodeSession;
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance, StreamDecodeSession,
    StreamDecodedMessage, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;

fn as_regular(rows: Vec<StreamDecodedMessage>) -> Vec<StreamDecodedWithProvenance> {
    rows.into_iter()
        .map(|decode| StreamDecodedWithProvenance {
            decode,
            provenance: StreamDecodeProvenance::Regular,
        })
        .collect()
}

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

    /// Rebuild for a new config, carrying forward DX intel when staying in the dx
    /// profile (others rebuild fresh). `reset_dx_*` come from the reconfig plan.
    pub fn reconfigure(
        &self,
        new_config: StreamDecodeConfig,
        reset_dx_target: bool,
        reset_dx_operator: bool,
    ) -> Self {
        if let Self::Dx(old) = self {
            if new_config.profile == DecodeProfile::Dx {
                return Self::Dx(old.reconfigured(new_config, reset_dx_target, reset_dx_operator));
            }
        }
        Self::new(new_config)
    }

    /// Read-only DX intel snapshot, present only for the dx profile.
    pub fn dx_context_snapshot(&self) -> Option<DxSnapshot> {
        match self {
            Self::Dx(session) => Some(session.context_snapshot()),
            _ => None,
        }
    }

    /// Decode a slot, returning rows tagged with provenance (for the GUI's
    /// `a7`/AP marker). wsjtx and jtdx carry real provenance; hybrid and dx fall
    /// back to `Regular` (their unified path does not surface per-row provenance).
    pub fn decode_slot_streaming_with_provenance_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
    ) -> Result<Vec<StreamDecodedWithProvenance>, String> {
        match self {
            Self::Wsjtx(session) => {
                session.decode_slot_streaming_with_provenance_at(timestamp, samples, |_| Ok(()))
            }
            Self::Jtdx(session) => {
                session.decode_slot_streaming_with_provenance_at(timestamp, samples, |_| Ok(()))
            }
            Self::Hybrid(session) => Ok(as_regular(
                session.decode_slot_streaming_at(timestamp, samples, |_| Ok(()))?,
            )),
            Self::Dx(session) => Ok(as_regular(
                session.decode_slot_streaming_at(timestamp, samples, |_| Ok(()))?,
            )),
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
