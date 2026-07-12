//! ft8rs live runtime.
//!
//! Owns the real-time audio path (cpal soundcard capture) and the shared output
//! sinks (UDP decode reports), built on top of the UI-independent `ft8rs` core.
//! Offline WAV decoding stays in the core crate; this crate is for live monitor.

pub mod engine;
pub mod protocol;
pub mod reconfig;
pub mod report;
pub mod soundcard;

pub use engine::EngineHandle;

pub use soundcard::{
    decode_soundcard_streaming, decode_soundcard_streaming_decodes, list_soundcards,
    open_soundcard_stream, InputChannel, SoundcardDecodeOptions, SoundcardDeviceInfo,
    SoundcardFormatInfo,
};
