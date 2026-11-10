//! CPAL device discovery, input stream ownership, and the bounded capture inbox.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const CAPTURE_QUEUE_CAP: usize = 256;

/// A captured, channel-folded audio block tagged with the wall-clock unix-ms at
/// the moment the input callback delivered it (≈ the last sample's capture time).
/// The timestamp is what lets the accumulator place each block on the UTC grid
/// without draining — the way WSJT-X keys its buffer off `currentMSecsSinceEpoch`.
pub(crate) struct AudioChunk {
    pub samples: Vec<f32>,
    pub capture_ms: i64,
    pub captured_at: Instant,
    pub channel_peaks: Vec<f32>,
    pub xruns: u64,
}

pub(crate) struct AudioCapture<S = cpal::Stream> {
    pub stream: S,
    pub rx: AudioInbox,
    pub format: SoundcardFormatInfo,
    pub device_name: String,
}

#[derive(Default)]
struct CaptureHealth {
    error: OnceLock<String>,
    dropped_blocks: AtomicU64,
    xruns: AtomicU64,
}

pub(crate) enum CaptureRecvError {
    Timeout,
    Disconnected,
    Failed(String),
}

pub(crate) struct AudioInbox {
    samples: Receiver<AudioChunk>,
    health: Arc<CaptureHealth>,
}

impl AudioInbox {
    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<AudioChunk, CaptureRecvError> {
        let check_health = || {
            if let Some(error) = self.health.error.get() {
                return Err(CaptureRecvError::Failed(error.clone()));
            }
            let dropped = self.health.dropped_blocks.load(Ordering::Acquire);
            if dropped > 0 {
                return Err(CaptureRecvError::Failed(format!(
                    "Audio capture backlog full: {dropped} blocks dropped"
                )));
            }
            Ok(())
        };
        check_health()?;
        let result = self.samples.recv_timeout(timeout);
        check_health()?;
        result.map_err(|error| match error {
            RecvTimeoutError::Timeout => CaptureRecvError::Timeout,
            RecvTimeoutError::Disconnected => CaptureRecvError::Disconnected,
        })
    }
}

/// Which physical channel of a multi-channel capture device feeds the decoder.
///
/// Mono requests a one-channel stream, as WSJT-X does in `Audio/soundin.cpp`.
/// If the backend only supports multichannel capture, use its first channel;
/// averaging is an explicit opt-in because it can attenuate or cancel audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputChannel {
    /// Request mono; take the first channel if native mono is unavailable.
    #[default]
    Mono,
    /// Left channel only (frame index 0).
    Left,
    /// Right channel only (frame index 1).
    Right,
    /// Explicitly average all input channels.
    Mix,
}

impl InputChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            InputChannel::Left => "left",
            InputChannel::Right => "right",
            InputChannel::Mono => "mono",
            InputChannel::Mix => "mix",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Some(InputChannel::Left),
            "right" => Some(InputChannel::Right),
            "mono" => Some(InputChannel::Mono),
            "mix" => Some(InputChannel::Mix),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SoundcardDeviceInfo {
    /// Backend-qualified CPAL device ID; names are display labels only.
    pub id: String,
    pub index: usize,
    pub host: String,
    pub name: String,
    pub is_default_input: bool,
    pub input: SoundcardFormatInfo,
}

#[derive(Clone, Debug)]
pub struct SoundcardFormatInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: String,
}

pub fn list_soundcards() -> Result<Vec<SoundcardDeviceInfo>, String> {
    Ok(input_devices_with_info()?
        .into_iter()
        .map(|(_, info)| info)
        .collect())
}

pub(crate) fn start_input_stream(
    selector: Option<&str>,
    channel: InputChannel,
) -> Result<AudioCapture, String> {
    let selector = selector.unwrap_or("default");
    let (device, info) = select_input_device(selector)?;
    let supported_config = select_input_config(&device, &info, channel)?;
    let format = SoundcardFormatInfo {
        channels: supported_config.channels(),
        sample_rate: supported_config.sample_rate(),
        sample_format: supported_config.sample_format().to_string(),
    };
    let channels = supported_config.channels() as usize;

    let (tx, samples) = mpsc::sync_channel(CAPTURE_QUEUE_CAP);
    let stream_config = supported_config.into();
    let health = Arc::new(CaptureHealth::default());
    let callback_health = Arc::clone(&health);
    let error_health = Arc::clone(&health);
    let err_fn = move |err| report_capture_error(err, &error_health);
    // Use CPAL's conversions for each PCM width, including 24-bit samples in
    // 32-bit containers. Never interpret float samples as 16-bit PCM or divide
    // already-normalized floats by 32768.
    macro_rules! build {
        ($ty:ty) => {
            device.build_input_stream(
                stream_config,
                move |data: &[$ty], _| send_samples(data, channels, channel, &tx, &callback_health),
                err_fn,
                None,
            )
        };
    }
    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::F64 => build!(f64),
        cpal::SampleFormat::I8 => build!(i8),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I24 => build!(cpal::I24),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::I64 => build!(i64),
        cpal::SampleFormat::U8 => build!(u8),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::U24 => build!(cpal::U24),
        cpal::SampleFormat::U32 => build!(u32),
        cpal::SampleFormat::U64 => build!(u64),
        sample_format => {
            return Err(format!(
                "unsupported default input sample format for {}: {sample_format:?}",
                info.name
            ))
        }
    }
    .map_err(|err| format!("failed to build input stream for {}: {err}", info.name))?;

    stream
        .play()
        .map_err(|err| format!("failed to start input stream for {}: {err}", info.name))?;

    Ok(AudioCapture {
        stream,
        rx: AudioInbox { samples, health },
        format,
        device_name: info.name,
    })
}

/// Request 48 kHz with the channel layout used by WSJT-X (mono for Mono,
/// stereo for Left/Right). WASAPI can expose f32 even when WSJT-X's Qt backend
/// exposes i16; both represent the same full scale after conversion. Prefer a
/// supported channel layout before sample type; do not open a device's maximum
/// rate just because it supports float there.
fn select_input_config(
    device: &cpal::Device,
    info: &SoundcardDeviceInfo,
    channel: InputChannel,
) -> Result<cpal::SupportedStreamConfig, String> {
    if let Ok(configs) = device.supported_input_configs() {
        let configs: Vec<_> = configs.collect();
        if let Some(config) = preferred_input_config(&configs, channel) {
            return Ok(config);
        }
    }
    // 3) Fall back to the device default.
    device
        .default_input_config()
        .map_err(|err| format!("failed to read input config for {}: {err}", info.name))
}

fn preferred_input_config(
    configs: &[cpal::SupportedStreamConfigRange],
    channel: InputChannel,
) -> Option<cpal::SupportedStreamConfig> {
    let wanted_channels = if channel == InputChannel::Mono { 1 } else { 2 };
    configs
        .iter()
        .filter(|c| {
            c.channels() > 0 && c.min_sample_rate() <= 48_000 && c.max_sample_rate() >= 48_000
        })
        .filter(|c| {
            !matches!(
                c.sample_format(),
                cpal::SampleFormat::DsdU8 | cpal::SampleFormat::DsdU16 | cpal::SampleFormat::DsdU32
            )
        })
        .min_by_key(|c| {
            (
                c.channels().abs_diff(wanted_channels),
                match c.sample_format() {
                    cpal::SampleFormat::F32 => 0,
                    cpal::SampleFormat::I16 => 1,
                    _ => 2,
                },
            )
        })
        .map(|c| (*c).with_sample_rate(48_000))
}

fn input_devices_with_info() -> Result<Vec<(cpal::Device, SoundcardDeviceInfo)>, String> {
    let mut rows = Vec::new();

    for host_id in cpal::available_hosts() {
        log::info!(target: "ft8rs_engine::soundcard", "Opening audio host {host_id:?}");
        let host = cpal::host_from_id(host_id)
            .map_err(|err| format!("failed to open audio host {host_id:?}: {err}"))?;
        let host_name = format!("{host_id:?}");
        log::info!(target: "ft8rs_engine::soundcard", "Querying default input on {host_name}");
        let default_input = host
            .default_input_device()
            .and_then(|device| device.id().ok().map(|id| id.to_string()));

        for device in host
            .input_devices()
            .map_err(|err| format!("failed to enumerate {host_name} audio input devices: {err}"))?
        {
            let name = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "<unknown device>".to_string());
            log::info!(target: "ft8rs_engine::soundcard", "Querying input format: {host_name} / {name}");
            let Some(input) = default_input_config(&device) else {
                log::warn!(target: "ft8rs_engine::soundcard", "Skipping input without a default format: {host_name} / {name}");
                continue;
            };
            log::info!(target: "ft8rs_engine::soundcard", "Audio input: {host_name} / {name}: {input:?}");

            let id = match device.id() {
                Ok(id) => id.to_string(),
                Err(err) => {
                    log::warn!(target: "ft8rs_engine::soundcard",
                        "Skipping input without device identity: {host_name} / {name}: {err}"
                    );
                    continue;
                }
            };
            let info = SoundcardDeviceInfo {
                is_default_input: default_input.as_deref() == Some(id.as_str()),
                id,
                index: rows.len(),
                host: host_name.clone(),
                name,
                input,
            };
            rows.push((device, info));
        }
    }

    for (index, (_, info)) in rows.iter_mut().enumerate() {
        info.index = index;
    }

    Ok(rows)
}

fn default_input_config(device: &cpal::Device) -> Option<SoundcardFormatInfo> {
    let config = device.default_input_config().ok()?;
    Some(SoundcardFormatInfo {
        channels: config.channels(),
        sample_rate: config.sample_rate(),
        sample_format: config.sample_format().to_string(),
    })
}

fn select_input_device(selector: &str) -> Result<(cpal::Device, SoundcardDeviceInfo), String> {
    let devices = input_devices_with_info()?;
    if devices.is_empty() {
        return Err("no audio input devices found".to_string());
    }

    let infos: Vec<_> = devices.iter().map(|(_, info)| info.clone()).collect();
    let index = resolve_input_selector(selector, &infos)?;
    Ok(devices.into_iter().nth(index).unwrap())
}

fn resolve_input_selector(
    selector: &str,
    devices: &[SoundcardDeviceInfo],
) -> Result<usize, String> {
    if let Some(id) = selector.strip_prefix("id:") {
        return devices
            .iter()
            .position(|info| info.id == id)
            .ok_or_else(|| {
                format!("saved audio input '{selector}' is unavailable; select an input device")
            });
    }
    if selector == "default" {
        if let Some(index) = devices.iter().position(|info| info.is_default_input) {
            return Ok(index);
        }
    }
    if let Ok(index) = selector.parse::<usize>() {
        if let Some(position) = devices.iter().position(|info| info.index == index) {
            return Ok(position);
        }
    }
    let mut matches = devices
        .iter()
        .enumerate()
        .filter(|(_, info)| info.name == selector);
    if let Some((index, _)) = matches.next() {
        if matches.next().is_some() {
            return Err(format!(
                "audio input name '{selector}' is ambiguous; select a device by index or ID"
            ));
        }
        return Ok(index);
    }
    let available = devices
        .iter()
        .map(|info| format!("{}='{}'", info.index, info.name))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "audio input device '{selector}' not found; use one of: {available}"
    ))
}

/// Migrate a persisted GUI name only when it uniquely identifies an input.
/// Missing selections stay explicit and will fail on Start instead of falling back.
pub fn migrate_device_name(
    selector: &str,
    devices: &[SoundcardDeviceInfo],
) -> Result<String, String> {
    if selector.starts_with("id:") {
        return Ok(selector.to_owned());
    }
    let mut matches = devices.iter().filter(|info| info.name == selector);
    let Some(device) = matches.next() else {
        return Ok(selector.to_owned());
    };
    if matches.next().is_some() {
        return Err(format!(
            "saved audio input name '{selector}' is ambiguous; select the intended input again"
        ));
    }
    Ok(format!("id:{}", device.id))
}

/// Wall-clock unix-ms at the input callback — the block's approximate capture
/// time, stamped here (like WSJT-X's `currentMSecsSinceEpoch()` in `writeData`) so
/// the accumulator can grid it to UTC regardless of how long it queues downstream.
pub(crate) fn capture_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// CPAL 0.18.2 reports WASAPI capture discontinuities via the error callback,
/// then continues delivering samples. These are diagnostics, not a dead stream.
/// Keep the callback lightweight and carry the count with the next audio block;
/// all other errors retain the existing fatal GUI/CLI behavior.
fn report_capture_error(err: cpal::Error, health: &CaptureHealth) {
    if err.kind() == cpal::ErrorKind::Xrun {
        health.xruns.fetch_add(1, Ordering::Relaxed);
    } else {
        let _ = health
            .error
            .set(format!("Soundcard input stream error: {err}"));
    }
}

#[cfg(test)]
pub(crate) fn failed_capture<S>(stream: S, error: cpal::Error) -> AudioCapture<S> {
    let (_tx, samples) = mpsc::sync_channel(1);
    let health = Arc::new(CaptureHealth::default());
    report_capture_error(error, &health);
    AudioCapture {
        stream,
        rx: AudioInbox { samples, health },
        format: SoundcardFormatInfo {
            channels: 1,
            sample_rate: 48_000,
            sample_format: "F32".into(),
        },
        device_name: "fixture input".into(),
    }
}

fn send_samples<T>(
    data: &[T],
    channels: usize,
    channel: InputChannel,
    tx: &SyncSender<AudioChunk>,
    health: &CaptureHealth,
) where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    let capture_ms = capture_now_ms();
    let captured_at = Instant::now();
    let mut channel_peaks = vec![0.0f32; channels];
    let samples = data.iter().enumerate().map(|(i, &sample)| {
        let value = sample.to_sample::<f32>();
        // A malformed float from a driver must not poison an entire slot's FFT.
        let value = if value.is_finite() { value } else { 0.0 };
        channel_peaks[i % channels] = channel_peaks[i % channels].max(value.abs());
        value
    });
    let samples = fold_channels(samples, channels, channel);
    if let Err(TrySendError::Full(_)) = tx.try_send(AudioChunk {
        samples,
        capture_ms,
        captured_at,
        channel_peaks,
        xruns: health.xruns.load(Ordering::Relaxed),
    }) {
        health.dropped_blocks.fetch_add(1, Ordering::Release);
    }
}

/// Reduce an interleaved multi-channel frame stream to one channel per the
/// selected `InputChannel` (see its doc for why averaging is *not* the default).
fn fold_channels<I>(samples: I, channels: usize, channel: InputChannel) -> Vec<f32>
where
    I: IntoIterator<Item = f32>,
{
    if channels <= 1 {
        return samples.into_iter().collect();
    }

    match channel {
        InputChannel::Mix => {
            let mut out = Vec::new();
            let mut acc = 0.0f32;
            let mut pos = 0usize;
            for sample in samples {
                acc += sample;
                pos += 1;
                if pos == channels {
                    out.push(acc / channels as f32);
                    acc = 0.0;
                    pos = 0;
                }
            }
            out
        }
        InputChannel::Mono | InputChannel::Left | InputChannel::Right => {
            // Right clamps to the last channel on mono-but-reported-multichannel
            // edge cases so we never silently emit an empty buffer.
            let target = match channel {
                InputChannel::Right => (channels - 1).min(1),
                _ => 0,
            };
            samples
                .into_iter()
                .enumerate()
                .filter_map(|(i, sample)| (i % channels == target).then_some(sample))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_input_names_require_identity_and_legacy_migration_is_explicit() {
        use super::{
            migrate_device_name, resolve_input_selector, SoundcardDeviceInfo, SoundcardFormatInfo,
        };
        let mut devices: Vec<_> = (0..2)
            .map(|index| SoundcardDeviceInfo {
                id: format!("Host:input-{index}"),
                index,
                host: "Host".into(),
                name: "USB Audio".into(),
                is_default_input: index == 1,
                input: SoundcardFormatInfo {
                    channels: 1,
                    sample_rate: 48_000,
                    sample_format: "f32".into(),
                },
            })
            .collect();
        assert!(resolve_input_selector("USB Audio", &devices).is_err());
        assert!(migrate_device_name("USB Audio", &devices).is_err());
        assert_eq!(
            resolve_input_selector("id:Host:input-1", &devices).unwrap(),
            1
        );
        assert_eq!(resolve_input_selector("default", &devices).unwrap(), 1);
        assert_eq!(resolve_input_selector("0", &devices).unwrap(), 0);
        devices.reverse();
        assert_eq!(
            resolve_input_selector("id:Host:input-1", &devices).unwrap(),
            0
        );
        assert!(resolve_input_selector("id:missing", &devices).is_err());
        devices.pop();
        assert_eq!(
            migrate_device_name("USB Audio", &devices).unwrap(),
            "id:Host:input-1"
        );
    }

    #[test]
    fn capture_xruns_do_not_stop_audio_and_reach_diagnostics() {
        use super::{report_capture_error, send_samples, CaptureHealth};
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let health = CaptureHealth::default();
        // A driver may flag more than one startup packet. Neither notification
        // may enter the fatal-error queue consumed by both GUI and CLI.
        for _ in 0..2 {
            report_capture_error(cpal::ErrorKind::Xrun.into(), &health);
        }
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        for _ in 0..2 {
            send_samples(&[0.25f32, -0.5], 1, InputChannel::Mono, &tx, &health);
            let chunk = rx.recv().unwrap();
            assert_eq!(chunk.samples, [0.25, -0.5]);
            assert_eq!(chunk.channel_peaks, [0.5]);
            assert_eq!(chunk.xruns, 2);
            assert!(health.error.get().is_none());
        }
    }

    #[test]
    fn capture_errors_other_than_xrun_are_fatal() {
        use super::{report_capture_error, AudioInbox, CaptureHealth, CaptureRecvError};
        use std::{sync::Arc, time::Duration};
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::BackendError,
        ] {
            // Even text identical to an xrun must remain fatal if its error kind
            // is not Xrun. Never classify backend errors by matching strings.
            let (_tx, samples) = std::sync::mpsc::sync_channel(1);
            let health = Arc::new(CaptureHealth::default());
            report_capture_error(
                cpal::Error::with_message(kind, "A buffer underrun or overrun occurred."),
                &health,
            );
            let inbox = AudioInbox { samples, health };
            assert!(matches!(
                inbox.recv_timeout(Duration::ZERO),
                Err(CaptureRecvError::Failed(message))
                    if message.contains("A buffer underrun or overrun occurred.")
            ));
        }
    }

    #[test]
    fn capture_overflow_is_reported_even_when_the_audio_mailbox_is_full() {
        use super::{send_samples, AudioInbox, CaptureHealth, CaptureRecvError};
        use std::{sync::Arc, time::Duration};
        let (tx, samples) = std::sync::mpsc::sync_channel(1);
        let health = Arc::new(CaptureHealth::default());
        let inbox = AudioInbox {
            samples,
            health: health.clone(),
        };
        for _ in 0..3 {
            send_samples(&[0.25f32], 1, InputChannel::Mono, &tx, &health);
        }
        assert!(matches!(
            inbox.recv_timeout(Duration::ZERO),
            Err(CaptureRecvError::Failed(message)) if message.contains("2 blocks dropped")
        ));
    }

    #[test]
    fn antiphase_stereo_cancels_under_mix_but_survives_single_channel() {
        let left = [0.1f32, -0.2, 0.3, -0.4];
        let interleaved: Vec<f32> = left.iter().flat_map(|&l| [l, -l]).collect();

        let mono = fold_channels(interleaved.iter().copied(), 2, InputChannel::Mix);
        assert!(
            mono.iter().all(|&s| s.abs() < 1e-6),
            "anti-phase average should cancel to ~0, got {mono:?}"
        );

        let l = fold_channels(interleaved.iter().copied(), 2, InputChannel::Left);
        assert_eq!(l, left);
        assert_eq!(
            fold_channels(interleaved.iter().copied(), 2, InputChannel::Mono),
            left
        );
        let r = fold_channels(interleaved.iter().copied(), 2, InputChannel::Right);
        assert_eq!(r, left.iter().map(|&s| -s).collect::<Vec<_>>());
    }

    #[test]
    fn mono_device_passes_through_regardless_of_selection() {
        let samples = [0.5f32, -0.25, 0.125];
        for ch in [
            InputChannel::Left,
            InputChannel::Right,
            InputChannel::Mono,
            InputChannel::Mix,
        ] {
            assert_eq!(fold_channels(samples.iter().copied(), 1, ch), samples);
        }
    }

    #[test]
    fn mono_does_not_halve_single_sided_audio() {
        let data = [0.25, 0.0, -0.5, 0.0];
        assert_eq!(fold_channels(data, 2, InputChannel::Mono), [0.25, -0.5]);
        assert_eq!(fold_channels(data, 2, InputChannel::Mix), [0.125, -0.25]);
    }

    #[test]
    fn capture_format_prefers_requested_layout_at_48k_and_uses_device_fallback() {
        use cpal::{SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};
        let range = |channels, rate, format| {
            SupportedStreamConfigRange::new(
                channels,
                rate,
                rate,
                SupportedBufferSize::Unknown,
                format,
            )
        };
        let configs = [
            range(2, 48_000, SampleFormat::F32),
            range(1, 48_000, SampleFormat::I16),
            range(1, 192_000, SampleFormat::F32),
        ];
        let mono = super::preferred_input_config(&configs, InputChannel::Mono).unwrap();
        assert_eq!(
            (mono.channels(), mono.sample_rate(), mono.sample_format()),
            (1, 48_000, SampleFormat::I16)
        );
        for ch in [InputChannel::Left, InputChannel::Right, InputChannel::Mix] {
            let config = super::preferred_input_config(&configs, ch).unwrap();
            assert_eq!(config.channels(), 2);
        }
        let stereo_only = super::preferred_input_config(&configs[..1], InputChannel::Mono).unwrap();
        assert_eq!(stereo_only.channels(), 2);
        assert!(super::preferred_input_config(&configs[2..], InputChannel::Mono).is_none());
    }

    #[test]
    fn pcm_widths_and_float_capture_have_equal_amplitudes_and_channel_levels() {
        fn capture<T: cpal::Sample>(data: &[T]) -> super::AudioChunk
        where
            f32: cpal::FromSample<T>,
        {
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            super::send_samples(
                data,
                2,
                InputChannel::Mono,
                &tx,
                &super::CaptureHealth::default(),
            );
            rx.recv().unwrap()
        }
        let expected = [0.25, -0.5];
        let floats = capture(&[0.25f32, -0.25, -0.5, 0.5]);
        assert_eq!(floats.samples, expected);
        assert_eq!(floats.channel_peaks, [0.5, 0.5]);
        assert_eq!(capture(&[8192i16, -8192, -16384, 16384]).samples, expected);
        assert_eq!(capture(&[40960u16, 24576, 16384, 49152]).samples, expected);
        assert_eq!(
            capture(&[536870912i32, -536870912, -1073741824, 1073741824]).samples,
            expected
        );
        assert_eq!(capture(&[0.25f64, -0.25, -0.5, 0.5]).samples, expected);
        let quiet = capture(&[1e-7f32, 0.0, -1e-7, 0.0]);
        assert_eq!(quiet.samples, [1e-7, -1e-7]);
        let invalid = capture(&[f32::NAN, 0.0, f32::INFINITY, 0.0]);
        assert_eq!(invalid.samples, [0.0, 0.0]);
    }
}
