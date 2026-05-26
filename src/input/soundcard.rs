use crate::stream::session::StreamDecodeConfig;
use crate::stream::{SlotTimestamp, StreamDecodeSession, StreamDecodedMessage};

use cpal::traits::StreamTrait;
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::audio::resample_linear;

const TARGET_SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: u64 = 15;

#[derive(Clone, Debug)]
pub struct SoundcardDecodeOptions {
    pub device: Option<String>,
    pub config: StreamDecodeConfig,
    pub max_slots: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct SoundcardDeviceInfo {
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

pub fn decode_soundcard_streaming<F>(
    options: SoundcardDecodeOptions,
    mut on_slot: F,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, Vec<StreamDecodedMessage>) -> Result<(), String>,
{
    let selector = options.device.as_deref().unwrap_or("default");
    let (device, info) = select_input_device(selector)?;
    let supported_config = device.default_input_config().map_err(|err| {
        format!(
            "failed to read default input config for {}: {err}",
            info.name
        )
    })?;
    let sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;
    let samples_per_slot = sample_rate as usize * SLOT_SECONDS as usize;

    let (tx, rx) = mpsc::channel();
    let stream_config = supported_config.clone().into();
    let err_fn = |err| eprintln!("soundcard input stream error: {err}");
    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| send_f32_mono(data, channels, &tx),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| send_i16_mono(data, channels, &tx),
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| send_u16_mono(data, channels, &tx),
            err_fn,
            None,
        ),
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

    let first_slot_start = next_slot_start_unix_seconds()?;
    sleep_until_unix_seconds(first_slot_start)?;
    drain_pending_audio(&rx);

    let mut decoder = StreamDecodeSession::new(options.config);
    let mut slot_index = 0usize;
    loop {
        if options
            .max_slots
            .is_some_and(|max_slots| slot_index >= max_slots)
        {
            break;
        }

        let timestamp =
            SlotTimestamp::from_unix_seconds_utc(first_slot_start + slot_index as i64 * 15);
        let native = collect_slot_samples(&rx, samples_per_slot)?;
        let samples_12k = resample_linear(&native, sample_rate, TARGET_SAMPLE_RATE);
        let results = decoder.decode_slot(&samples_12k);
        on_slot(timestamp, results)?;
        slot_index += 1;
    }

    Ok(())
}

pub fn open_soundcard_stream(options: SoundcardDecodeOptions) -> Result<(), String> {
    decode_soundcard_streaming(options, |_timestamp, _rows| Ok(()))
}

fn input_devices_with_info() -> Result<Vec<(cpal::Device, SoundcardDeviceInfo)>, String> {
    let mut rows = Vec::new();

    for host_id in cpal::available_hosts() {
        let host = cpal::host_from_id(host_id)
            .map_err(|err| format!("failed to open audio host {host_id:?}: {err}"))?;
        let host_name = format!("{host_id:?}");
        let default_input = host
            .default_input_device()
            .and_then(|device| device.name().ok());

        for device in host
            .input_devices()
            .map_err(|err| format!("failed to enumerate {host_name} audio input devices: {err}"))?
        {
            let name = device
                .name()
                .unwrap_or_else(|_| "<unknown device>".to_string());
            let Some(input) = default_input_config(&device) else {
                continue;
            };

            let info = SoundcardDeviceInfo {
                index: rows.len(),
                host: host_name.clone(),
                is_default_input: default_input.as_deref() == Some(name.as_str()),
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
        sample_rate: config.sample_rate().0,
        sample_format: config.sample_format().to_string(),
    })
}

fn select_input_device(selector: &str) -> Result<(cpal::Device, SoundcardDeviceInfo), String> {
    let devices = input_devices_with_info()?;
    if devices.is_empty() {
        return Err("no audio input devices found".to_string());
    }

    if selector == "default" {
        if let Some((device, info)) = devices
            .iter()
            .find(|(_, info)| info.is_default_input)
            .map(|(device, info)| (device.clone(), info.clone()))
        {
            return Ok((device, info));
        }
    }

    if let Ok(index) = selector.parse::<usize>() {
        if let Some((device, info)) = devices
            .iter()
            .find(|(_, info)| info.index == index)
            .map(|(device, info)| (device.clone(), info.clone()))
        {
            return Ok((device, info));
        }
    }

    if let Some((device, info)) = devices
        .iter()
        .find(|(_, info)| info.name == selector)
        .map(|(device, info)| (device.clone(), info.clone()))
    {
        return Ok((device, info));
    }

    let available = devices
        .iter()
        .map(|(_, info)| format!("{}='{}'", info.index, info.name))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "audio input device '{selector}' not found; use one of: {available}"
    ))
}

fn send_f32_mono(data: &[f32], channels: usize, tx: &mpsc::Sender<Vec<f32>>) {
    let _ = tx.send(fold_channels(data.iter().copied(), channels));
}

fn send_i16_mono(data: &[i16], channels: usize, tx: &mpsc::Sender<Vec<f32>>) {
    let _ = tx.send(fold_channels(
        data.iter().map(|sample| *sample as f32 / 32768.0),
        channels,
    ));
}

fn send_u16_mono(data: &[u16], channels: usize, tx: &mpsc::Sender<Vec<f32>>) {
    let _ = tx.send(fold_channels(
        data.iter().map(|sample| *sample as f32 / 32768.0 - 1.0),
        channels,
    ));
}

fn fold_channels<I>(samples: I, channels: usize) -> Vec<f32>
where
    I: IntoIterator<Item = f32>,
{
    if channels <= 1 {
        return samples.into_iter().collect();
    }

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

fn collect_slot_samples(
    rx: &Receiver<Vec<f32>>,
    samples_per_slot: usize,
) -> Result<Vec<f32>, String> {
    let mut out = Vec::with_capacity(samples_per_slot);
    let deadline = Instant::now() + Duration::from_secs(SLOT_SECONDS + 5);
    while out.len() < samples_per_slot {
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "timed out collecting soundcard slot: got {}/{} samples",
                out.len(),
                samples_per_slot
            ));
        }
        let timeout = deadline.saturating_duration_since(now);
        let chunk = rx
            .recv_timeout(timeout)
            .map_err(|err| format!("soundcard input stopped while collecting audio: {err}"))?;
        let remaining = samples_per_slot - out.len();
        if chunk.len() <= remaining {
            out.extend_from_slice(&chunk);
        } else {
            out.extend_from_slice(&chunk[..remaining]);
        }
    }
    Ok(out)
}

fn drain_pending_audio(rx: &Receiver<Vec<f32>>) {
    while rx.try_recv().is_ok() {}
}

fn next_slot_start_unix_seconds() -> Result<i64, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time is before Unix epoch: {err}"))?;
    let now_millis = now.as_millis() as i64;
    Ok(((now_millis / 15_000) + 1) * 15)
}

fn sleep_until_unix_seconds(unix_seconds: i64) -> Result<(), String> {
    let target = UNIX_EPOCH + Duration::from_secs(unix_seconds as u64);
    if let Ok(duration) = target.duration_since(SystemTime::now()) {
        std::thread::sleep(duration);
    }
    Ok(())
}
