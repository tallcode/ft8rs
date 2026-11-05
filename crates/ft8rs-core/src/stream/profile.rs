use crate::decode::dx::{DxSnapshot, DxStreamDecodeSession};
use crate::decode::hybrid::HybridStreamDecodeSession;
use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::decode::lib_wsjtx_plus::session::{
    StreamDecodeSession as WsjtxPlusSession, StreamSlotDecodeState as WsjtxPlusSlotState,
};
use crate::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage,
    StreamDecodedWithProvenance, StreamSlotDecodeState,
};
use crate::stream::time::SlotTimestamp;

/// Per-slot state for profiles using this wrapper's progressive monitor stages.
pub enum ProfileSlotState {
    WsjtxPlus(WsjtxPlusSlotState),
    Hybrid(StreamSlotDecodeState),
    Stateless,
}

#[allow(clippy::large_enum_variant)]
pub enum ProfileStreamDecodeSession {
    Wsjtx(StreamDecodeSession),
    WsjtxPlus(WsjtxPlusSession),
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
            DecodeProfile::WsjtxPlus => Self::WsjtxPlus(WsjtxPlusSession::new(config)),
            DecodeProfile::Jtdx => Self::Jtdx(crate::stream::session::seeded_jtdx_session(
                config.clone_for_profile_jtdx(),
            )),
            DecodeProfile::Hybrid => Self::Hybrid(HybridStreamDecodeSession::new(config)),
            DecodeProfile::Dx => Self::Dx(DxStreamDecodeSession::new(config)),
        }
    }

    /// Seed the learned hash-call book of the underlying session. Used by the
    /// live engine to migrate hash calls across a session rebuild. Hybrid keeps
    /// baseline books independent and DX seeds only user target calls, so both
    /// are no-ops here.
    pub fn import_hash_calls(&mut self, calls: &[String]) {
        match self {
            Self::Wsjtx(session) => session.import_hash_calls(calls),
            Self::WsjtxPlus(session) => session.import_hash_calls(calls),
            Self::Jtdx(session) => session.import_hash_calls(calls),
            Self::Hybrid(_) | Self::Dx(_) => {}
        }
    }

    /// Export the learned regular hash calls for migration into a rebuilt
    /// session. Hybrid/DX return empty (their hash state is reseeded on rebuild).
    pub fn export_hash_calls(&self) -> Vec<String> {
        match self {
            Self::Wsjtx(session) => session.export_regular_hash_calls(),
            Self::WsjtxPlus(session) => session.export_regular_hash_calls(),
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

    /// Decode a slot, invoking `on_decode` per row (with provenance for the GUI's
    /// `a7`/AP marker) and returning the row count.
    ///
    /// Hybrid and DX stream concurrent worker rows in real-time mode and keep
    /// stable merge ordering in file mode. Cooperative paths preserve unknown
    /// native provenance and label isolated recovery tasks. The standalone
    /// profiles retain their original batch wrapper and provenance semantics.
    pub fn decode_slot_streaming_with_provenance_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        match self {
            Self::Wsjtx(session) => {
                let rows =
                    session
                        .decode_slot_streaming_with_provenance_at(timestamp, samples, |_| Ok(()))?;
                for row in &rows {
                    on_decode(row)?;
                }
                Ok(rows.len())
            }
            Self::Jtdx(session) => {
                let rows =
                    session
                        .decode_slot_streaming_with_provenance_at(timestamp, samples, |_| Ok(()))?;
                for row in &rows {
                    on_decode(row)?;
                }
                Ok(rows.len())
            }
            Self::WsjtxPlus(session) => session
                .decode_slot_streaming_tagged_at(timestamp, samples, on_decode)
                .map(|rows| rows.len()),
            Self::Hybrid(session) => {
                let mut count = 0usize;
                session.decode_slot_streaming_tagged_at(timestamp, samples, |row| {
                    count += 1;
                    on_decode(row)
                })?;
                Ok(count)
            }
            Self::Dx(session) => {
                let mut count = 0usize;
                session.decode_slot_streaming_tagged_at(timestamp, samples, |row| {
                    count += 1;
                    on_decode(row)
                })?;
                Ok(count)
            }
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

    // ── Staged (monitor) decode ─────────────────────────────────────────────
    //
    // WSJT-X+ and Hybrid use progressive stages here. JTDX and DX run at
    // nzhsym=50. The original WSJT-X monitor uses its own staged session.

    /// Begin a staged slot; returns the per-stage state.
    pub fn start_slot(&mut self) -> ProfileSlotState {
        match self {
            Self::WsjtxPlus(session) => ProfileSlotState::WsjtxPlus(session.start_slot_decode()),
            Self::Hybrid(session) => ProfileSlotState::Hybrid(session.start_slot()),
            _ => ProfileSlotState::Stateless,
        }
    }

    /// `nzhsym=41` streams early rows for progressive profiles.
    pub fn decode_slot_nzhsym41_streaming_with_provenance<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: &mut ProfileSlotState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        match (self, state) {
            (Self::WsjtxPlus(session), ProfileSlotState::WsjtxPlus(state)) => {
                session.decode_slot_nzhsym41_tagged(timestamp, state, samples, on_decode)
            }
            (Self::Hybrid(session), ProfileSlotState::Hybrid(state)) => {
                session.decode_slot_nzhsym41_tagged(timestamp, state, samples, &mut on_decode)
            }
            _ => Ok(0),
        }
    }

    /// `nzhsym=47` subtracts the early results for progressive profiles.
    pub fn subtract_slot_nzhsym47(&mut self, state: &mut ProfileSlotState, samples: &[f32]) {
        match (self, state) {
            (Self::WsjtxPlus(session), ProfileSlotState::WsjtxPlus(state)) => {
                session.subtract_slot_nzhsym47(state, samples);
            }
            (Self::Hybrid(session), ProfileSlotState::Hybrid(state)) => {
                session.subtract_slot_nzhsym47(state, samples);
            }
            _ => {}
        }
    }

    /// `nzhsym=50` emits the remaining results and returns the total slot count.
    pub fn decode_slot_nzhsym50_streaming_with_provenance<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: ProfileSlotState,
        _early_count: usize,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        match self {
            Self::WsjtxPlus(session) => {
                let ProfileSlotState::WsjtxPlus(state) = state else {
                    return Err("wsjtx-plus nzhsym=50 without slot state".to_string());
                };
                session
                    .decode_slot_nzhsym50_tagged(state, samples, on_decode)
                    .map(|rows| rows.len())
            }
            Self::Hybrid(session) => {
                let ProfileSlotState::Hybrid(state) = state else {
                    return Err("hybrid nzhsym=50 without hybrid slot state".to_string());
                };
                let merged = session.decode_slot_nzhsym50_tagged(
                    timestamp,
                    state,
                    samples,
                    &mut on_decode,
                )?;
                Ok(merged.len())
            }
            session => {
                session.decode_slot_streaming_with_provenance_at(timestamp, samples, on_decode)
            }
        }
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
            Self::WsjtxPlus(decoder) => {
                decoder.decode_slot_streaming_at(timestamp, samples, on_decode)
            }
            Self::Jtdx(decoder) => decoder.decode_slot_streaming_at(timestamp, samples, on_decode),
            Self::Hybrid(decoder) => {
                decoder.decode_slot_streaming_at(timestamp, samples, on_decode)
            }
            Self::Dx(decoder) => decoder.decode_slot_streaming_at(timestamp, samples, on_decode),
        }
    }
}
