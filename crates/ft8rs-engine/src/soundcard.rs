//! Live audio capture, slot timing, and blocking soundcard decode APIs.

use std::time::Duration;

mod capture;
mod monitor;
mod slots;

pub use capture::{
    list_soundcards, migrate_device_name, InputChannel, SoundcardDeviceInfo, SoundcardFormatInfo,
};
pub use monitor::{
    decode_soundcard_streaming_decodes, decode_soundcard_streaming_tagged, SoundcardDecodeOptions,
};

#[cfg(test)]
pub(crate) use capture::failed_capture;
pub(crate) use capture::{capture_now_ms, start_input_stream, AudioCapture, CaptureRecvError};
pub(crate) use slots::{peak_level, PeriodAccumulator, SlotStage};

pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(slots::SLOT_SECONDS + 8);
