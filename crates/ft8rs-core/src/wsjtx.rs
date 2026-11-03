//! Stateful WSJT-X FT8 reception from one UTC-aligned audio slot at a time.
//!
//! Reuse a [`Decoder`] for consecutive slots from the same receiver. Call
//! [`Decoder::reset`] before decoding unrelated recordings or changing bands.
//! [`Decoder::decode_with_callback`] delivers each nonempty native stage batch
//! as soon as that stage finishes. [`Decoder::decode`] collects those batches.
//! Both calls are synchronous; run them on a worker thread when feeding audio
//! from a real-time callback.
//! The underlying WSJT-X Rust port, thresholds and pass order are unchanged.
//!
//! ```no_run
//! use ft8rs::wsjtx::{Audio, Config, Decoder, SlotTimestamp};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut decoder = Decoder::new(Config::default())?;
//! // Replace with 15 seconds of mono PCM starting exactly at this UTC slot.
//! let samples = vec![0.0_f32; 15 * 48_000];
//! let messages = decoder.decode(
//!     SlotTimestamp::parse("261103_120000")?,
//!     Audio::mono_f32(&samples, 48_000),
//! )?;
//! for message in messages {
//!     println!("{} {} dB {} Hz {}", message.slot, message.snr_db,
//!              message.frequency_hz, message.text);
//! }
//! # Ok(())
//! # }
//! ```

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use crate::input::audio::downsample_12k;
use crate::stream::session::{StreamDecodeConfig, StreamDecodeSession};
use crate::stream::validation::validate_frequency_range;

const WORKING_SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: usize = 15;

/// Errors detected before the underlying decoder's session state is changed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    InvalidConfig(String),
    InvalidTimestamp(String),
    UnalignedSlot,
    UnsupportedSampleRate(u32),
    InvalidAudioLength { expected: usize, actual: usize },
    NonFiniteSample { index: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(message) => write!(f, "invalid WSJT-X configuration: {message}"),
            Self::InvalidTimestamp(message) => write!(f, "invalid UTC timestamp: {message}"),
            Self::UnalignedSlot => f.write_str("UTC slot must start on a 15-second boundary"),
            Self::UnsupportedSampleRate(rate) => {
                write!(
                    f,
                    "sample rate {rate} Hz must be between 12000 and 192000 Hz"
                )
            }
            Self::InvalidAudioLength { expected, actual } => {
                write!(
                    f,
                    "expected {expected} samples (15 seconds), received {actual}"
                )
            }
            Self::NonFiniteSample { index } => write!(f, "audio sample {index} is not finite"),
        }
    }
}

impl std::error::Error for Error {}

/// UTC timestamp. Parsing accepts `YYMMDD_HHMMSS`, `YYYYMMDD_HHMMSS`, or `HHMMSS`.
///
/// A time without a date must still be UTC. [`Decoder::decode`] checks the
/// 15-second slot boundary; parsing itself accepts any valid whole second.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotTimestamp(crate::stream::time::SlotTimestamp);

impl SlotTimestamp {
    pub fn parse(value: &str) -> Result<Self, Error> {
        value.parse()
    }

    pub fn from_unix_seconds_utc(seconds: i64) -> Self {
        Self(crate::stream::time::SlotTimestamp::from_unix_seconds_utc(
            seconds,
        ))
    }

    pub fn add_seconds(&self, seconds: i64) -> Self {
        Self(self.0.add_seconds(seconds))
    }
}

impl FromStr for SlotTimestamp {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        crate::stream::time::SlotTimestamp::parse(value)
            .map(Self)
            .map_err(Error::InvalidTimestamp)
    }
}

impl fmt::Display for SlotTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Borrowed mono `f32` PCM, with nominal full scale `-1.0..=1.0`.
///
/// All samples must be finite. Finite headroom above full scale is preserved,
/// without normalization or clipping. Supported rates are 12000..=192000 Hz.
/// The first sample must correspond to the supplied UTC slot start, and the
/// buffer must contain exactly 15 seconds. Validation happens in `decode`.
#[derive(Clone, Copy, Debug)]
pub struct Audio<'a> {
    samples: &'a [f32],
    sample_rate: u32,
}

impl<'a> Audio<'a> {
    pub fn mono_f32(samples: &'a [f32], sample_rate: u32) -> Self {
        Self {
            samples,
            sample_rate,
        }
    }
}

/// WSJT-X receive settings. Defaults match the existing WSJT-X stream session.
///
/// Callsigns/grid are optional lookup/AP context, not independent evidence of
/// reception. This entry point never invokes JTDX, Hybrid, DX or experiment DSP.
#[derive(Clone, Debug)]
pub struct Config {
    /// Inclusive search limits in audio Hz: 0 <= min < max <= 5000.
    pub frequency_min_hz: f64,
    pub frequency_max_hz: f64,
    /// Original WSJT-X depth, 1 (shallow) through 3 (deep).
    pub decode_depth: usize,
    /// Maximum synchronization candidates; must be greater than zero.
    pub max_candidates: usize,
    pub ap_enabled: bool,
    pub ap_cq_only: bool,
    /// Original `nQSOProgress`, 0..=5.
    pub qso_progress: usize,
    /// Original `ncontest`, 0..=8; zero selects ordinary operation.
    pub contest_mode: usize,
    /// Original `nfqso` in audio Hz; zero disables QSO focus.
    pub qso_frequency_hz: f64,
    /// Original `napwid` in Hz.
    pub ap_width_hz: f64,
    /// ASCII callsign context, passed through without rewriting.
    pub my_call: Option<String>,
    pub his_call: Option<String>,
    /// ASCII locator context, passed through without rewriting.
    pub his_grid: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let native = StreamDecodeConfig::default();
        Self {
            frequency_min_hz: native.nfa,
            frequency_max_hz: native.nfb,
            decode_depth: native.ndepth,
            max_candidates: native.ncand,
            ap_enabled: native.lft8apon,
            ap_cq_only: native.lapcqonly,
            qso_progress: native.nQSOProgress,
            contest_mode: native.ncontest,
            qso_frequency_hz: native.nfqso,
            ap_width_hz: native.napwid,
            my_call: native.mycall,
            his_call: native.hiscall,
            his_grid: native.hisgrid,
        }
    }
}

impl Config {
    fn into_native(self) -> Result<StreamDecodeConfig, Error> {
        validate_frequency_range(self.frequency_min_hz, self.frequency_max_hz)
            .map_err(Error::InvalidConfig)?;
        if !(1..=3).contains(&self.decode_depth) {
            return Err(Error::InvalidConfig("decode_depth must be 1..=3".into()));
        }
        if self.max_candidates == 0 {
            return Err(Error::InvalidConfig(
                "max_candidates must be positive".into(),
            ));
        }
        if self.qso_progress > 5 || self.contest_mode > 8 {
            return Err(Error::InvalidConfig(
                "qso_progress must be 0..=5 and contest_mode must be 0..=8".into(),
            ));
        }
        for (name, value) in [
            ("qso_frequency_hz", self.qso_frequency_hz),
            ("ap_width_hz", self.ap_width_hz),
        ] {
            if !value.is_finite() || !(0.0..=5000.0).contains(&value) {
                return Err(Error::InvalidConfig(format!(
                    "{name} must be finite and 0..=5000"
                )));
            }
        }
        for (name, value) in [
            ("my_call", self.my_call.as_deref()),
            ("his_call", self.his_call.as_deref()),
            ("his_grid", self.his_grid.as_deref()),
        ] {
            if value.is_some_and(|value| !value.is_ascii() || value.chars().any(char::is_control)) {
                return Err(Error::InvalidConfig(format!(
                    "{name} must contain printable ASCII only"
                )));
            }
        }
        Ok(StreamDecodeConfig {
            nfa: self.frequency_min_hz,
            nfb: self.frequency_max_hz,
            ndepth: self.decode_depth,
            ncand: self.max_candidates,
            lft8apon: self.ap_enabled,
            lapcqonly: self.ap_cq_only,
            nQSOProgress: self.qso_progress,
            ncontest: self.contest_mode,
            nfqso: self.qso_frequency_hz,
            napwid: self.ap_width_hz,
            mycall: self.my_call,
            hiscall: self.his_call,
            hisgrid: self.his_grid,
            ..StreamDecodeConfig::default()
        })
    }
}

/// A received message, preserving the native text, numerical values and order.
/// No AP subtype or independent confirmation is inferred from native results.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub slot: SlotTimestamp,
    pub text: String,
    pub snr_db: f64,
    pub dt_seconds: f64,
    pub frequency_hz: f64,
    pub sync: f64,
}

/// Independent WSJT-X FT8 receive session with persistent hash/AP memory.
pub struct Decoder {
    config: StreamDecodeConfig,
    session: StreamDecodeSession,
}

impl Decoder {
    /// Validate configuration and create a fresh receive session.
    pub fn new(config: Config) -> Result<Self, Error> {
        let config = config.into_native()?;
        let session = StreamDecodeSession::new(config.clone());
        Ok(Self { config, session })
    }

    /// Decode exactly one UTC-aligned 15-second slot and collect all batches.
    /// Use [`Self::decode_with_callback`] to receive early results before the
    /// remaining native stages finish.
    ///
    /// Feed consecutive slots in time order, including silent ones, so native
    /// AP history advances normally. Use `reset` for a new receiving context.
    /// 12 kHz audio is borrowed directly; other rates use the existing input
    /// module's anti-alias conversion. Caller audio is never modified.
    /// Invalid input returns an error before any session state is changed.
    pub fn decode(&mut self, slot: SlotTimestamp, audio: Audio<'_>) -> Result<Vec<Message>, Error> {
        let mut messages = Vec::new();
        self.decode_with_callback(slot, audio, |batch| messages.extend(batch))?;
        Ok(messages)
    }

    /// Deliver each nonempty batch as soon as its native stage finishes.
    ///
    /// The early (`nzhsym=41`) batch is delivered before subtraction and final
    /// decoding begin. The final (`nzhsym=50`, including native AP) batch
    /// contains only new rows. Native code does not expose an in-pass callback,
    /// so results inside each stage remain grouped at that existing boundary.
    /// There are zero, one or two callbacks per slot, in native result order.
    ///
    /// Batches are owned: a callback can send them directly to a UI/channel.
    /// Callbacks run synchronously on the decoding thread and should return
    /// promptly. Returning `Ok(())` from this method signals slot completion;
    /// no empty batch is emitted. Input rules are the same as [`Self::decode`].
    ///
    /// ```no_run
    /// use ft8rs::wsjtx::{Audio, Config, Decoder, SlotTimestamp};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut decoder = Decoder::new(Config::default())?;
    /// let samples = vec![0.0_f32; 180_000];
    /// let (sender, _receiver) = std::sync::mpsc::channel();
    /// decoder.decode_with_callback(
    ///     SlotTimestamp::parse("261103_120000")?,
    ///     Audio::mono_f32(&samples, 12_000),
    ///     |batch| { let _ = sender.send(batch); },
    /// )?;
    /// // Send a separate completion event here if the consumer needs one.
    /// # Ok(())
    /// # }
    /// ```
    pub fn decode_with_callback<F>(
        &mut self,
        slot: SlotTimestamp,
        audio: Audio<'_>,
        mut on_batch: F,
    ) -> Result<(), Error>
    where
        F: FnMut(Vec<Message>),
    {
        if !slot.0.milliseconds_since_midnight().is_multiple_of(15_000) {
            return Err(Error::UnalignedSlot);
        }
        if !(WORKING_SAMPLE_RATE..=192_000).contains(&audio.sample_rate) {
            return Err(Error::UnsupportedSampleRate(audio.sample_rate));
        }
        let expected = audio.sample_rate as usize * SLOT_SECONDS;
        if audio.samples.len() != expected {
            return Err(Error::InvalidAudioLength {
                expected,
                actual: audio.samples.len(),
            });
        }
        if let Some(index) = audio.samples.iter().position(|sample| !sample.is_finite()) {
            return Err(Error::NonFiniteSample { index });
        }
        let samples = if audio.sample_rate == WORKING_SAMPLE_RATE {
            Cow::Borrowed(audio.samples)
        } else {
            Cow::Owned(downsample_12k(audio.samples, audio.sample_rate))
        };
        let message = |row: &crate::stream::session::StreamDecodedMessage| Message {
            slot: slot.clone(),
            text: row.msg.clone(),
            snr_db: row.snr,
            dt_seconds: row.dt,
            frequency_hz: row.freq,
            sync: row.sync,
        };
        let mut state = self.session.start_slot_decode();
        let mut batch = Vec::new();
        self.session
            .decode_slot_nzhsym41_at(Some(&slot.0), &mut state, &samples, |row| {
                batch.push(message(row));
                Ok(())
            })
            .expect("batch collection cannot fail");
        if !batch.is_empty() {
            on_batch(std::mem::take(&mut batch));
        }
        self.session.subtract_slot_nzhsym47(&mut state, &samples);
        self.session
            .decode_slot_nzhsym50_and_finish(state, &samples, |row| {
                batch.push(message(row));
                Ok(())
            })
            .expect("batch collection cannot fail");
        if !batch.is_empty() {
            on_batch(batch);
        }
        Ok(())
    }

    /// Clear learned hash calls and AP history, retaining configuration and
    /// explicitly configured station context, just like a fresh `Decoder`.
    pub fn reset(&mut self) {
        self.session = StreamDecodeSession::new(self.config.clone());
    }
}
