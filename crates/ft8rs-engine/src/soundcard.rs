use ft8rs::input::audio::resample_linear;
use ft8rs::stream::profile::ProfileStreamDecodeSession;
use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};
use ft8rs::stream::{SlotTimestamp, StreamDecodeSession, StreamDecodedMessage};

use cpal::traits::StreamTrait;
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TARGET_SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: u64 = 15;
const NZHSYM_STRIDE: usize = 3456;

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
    decode_soundcard_slots(options, |decoder, timestamp, samples_12k| {
        let results = decoder.decode_slot_at(&timestamp, samples_12k);
        on_slot(timestamp, results)
    })
}

pub fn decode_soundcard_streaming_decodes<F, G>(
    options: SoundcardDecodeOptions,
    mut on_decode: F,
    mut on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let (stream, rx, sample_rate) = start_input_stream(options.device.as_deref())?;
    let (cmd_tx, event_rx) = start_decode_worker(options.config);

    let first_slot_start = next_slot_start_unix_seconds()?;
    sleep_until_unix_seconds(first_slot_start)?;
    drain_pending_audio(&rx);

    let mut collector = NativeSampleCollector::new(&rx);
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
        let mut native = Vec::with_capacity(sample_rate as usize * SLOT_SECONDS as usize);
        let deadline = Instant::now() + Duration::from_secs(SLOT_SECONDS + 8);

        let nzhsym41_native = native_samples_for_nzhsym(sample_rate, 41);
        collector.collect_until_with_events(
            &mut native,
            nzhsym41_native,
            deadline,
            &event_rx,
            &mut on_decode,
            &mut on_slot_complete,
        )?;
        let samples_12k = resample_linear(&native, sample_rate, TARGET_SAMPLE_RATE);
        send_worker_command(
            &cmd_tx,
            DecodeWorkerCommand::Nzhsym41 {
                timestamp: timestamp.clone(),
                samples_12k,
            },
        )?;

        let nzhsym47_native = native_samples_for_nzhsym(sample_rate, 47);
        collector.collect_until_with_events(
            &mut native,
            nzhsym47_native,
            deadline,
            &event_rx,
            &mut on_decode,
            &mut on_slot_complete,
        )?;
        let samples_12k = resample_linear(&native, sample_rate, TARGET_SAMPLE_RATE);
        send_worker_command(
            &cmd_tx,
            DecodeWorkerCommand::Nzhsym47 {
                timestamp: timestamp.clone(),
                samples_12k,
            },
        )?;

        let samples_per_slot = sample_rate as usize * SLOT_SECONDS as usize;
        collector.collect_until_with_events(
            &mut native,
            samples_per_slot,
            deadline,
            &event_rx,
            &mut on_decode,
            &mut on_slot_complete,
        )?;
        let samples_12k = resample_linear(&native, sample_rate, TARGET_SAMPLE_RATE);
        send_worker_command(
            &cmd_tx,
            DecodeWorkerCommand::Nzhsym50 {
                timestamp,
                samples_12k,
            },
        )?;

        drain_decode_events_until_slot_complete(&event_rx, &mut on_decode, &mut on_slot_complete)?;
        slot_index += 1;
    }

    let _ = cmd_tx.send(DecodeWorkerCommand::Stop);
    drop(stream);

    Ok(())
}

pub fn open_soundcard_stream(options: SoundcardDecodeOptions) -> Result<(), String> {
    decode_soundcard_streaming(options, |_timestamp, _rows| Ok(()))
}

fn decode_soundcard_slots<F>(options: SoundcardDecodeOptions, mut on_slot: F) -> Result<(), String>
where
    F: FnMut(&mut ProfileStreamDecodeSession, SlotTimestamp, &[f32]) -> Result<(), String>,
{
    let (stream, rx, sample_rate) = start_input_stream(options.device.as_deref())?;
    let samples_per_slot = sample_rate as usize * SLOT_SECONDS as usize;

    let first_slot_start = next_slot_start_unix_seconds()?;
    sleep_until_unix_seconds(first_slot_start)?;
    drain_pending_audio(&rx);

    let mut decoder = ProfileStreamDecodeSession::new(options.config);
    let mut collector = NativeSampleCollector::new(&rx);
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
        let mut native = Vec::with_capacity(samples_per_slot);
        let deadline = Instant::now() + Duration::from_secs(SLOT_SECONDS + 5);
        collector.collect_until(&mut native, samples_per_slot, deadline)?;
        let samples_12k = resample_linear(&native, sample_rate, TARGET_SAMPLE_RATE);
        on_slot(&mut decoder, timestamp, &samples_12k)?;
        slot_index += 1;
    }

    drop(stream);

    Ok(())
}

enum DecodeWorkerCommand {
    Nzhsym41 {
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
    Nzhsym47 {
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
    Nzhsym50 {
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
    Stop,
}

enum DecodeWorkerEvent {
    Decode {
        timestamp: SlotTimestamp,
        decode: StreamDecodedMessage,
    },
    SlotComplete {
        timestamp: SlotTimestamp,
        count: usize,
    },
    Error(String),
}

fn start_decode_worker(
    config: StreamDecodeConfig,
) -> (Sender<DecodeWorkerCommand>, Receiver<DecodeWorkerEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        if config.profile != DecodeProfile::Wsjtx {
            run_profile_worker(config, cmd_rx, event_tx);
            return;
        }
        let mut decoder = StreamDecodeSession::new(config);
        let mut slot_state = None;
        while let Ok(command) = cmd_rx.recv() {
            let result = match command {
                DecodeWorkerCommand::Nzhsym41 {
                    timestamp,
                    samples_12k,
                } => {
                    let mut state = decoder.start_slot_decode();
                    let event_tx = event_tx.clone();
                    let stage_result = decoder.decode_slot_nzhsym41_at(
                        Some(&timestamp),
                        &mut state,
                        &samples_12k,
                        |decode| {
                            event_tx
                                .send(DecodeWorkerEvent::Decode {
                                    timestamp: timestamp.clone(),
                                    decode: decode.clone(),
                                })
                                .map_err(|err| err.to_string())
                        },
                    );
                    if stage_result.is_ok() {
                        slot_state = Some(state);
                    }
                    stage_result.map(|_| None)
                }
                DecodeWorkerCommand::Nzhsym47 {
                    timestamp,
                    samples_12k,
                } => {
                    if let Some(state) = slot_state.as_mut() {
                        decoder.subtract_slot_nzhsym47(state, &samples_12k);
                        Ok(None)
                    } else {
                        Err(format!("{timestamp}: received nzhsym=47 before nzhsym=41"))
                    }
                }
                DecodeWorkerCommand::Nzhsym50 {
                    timestamp,
                    samples_12k,
                } => {
                    if let Some(state) = slot_state.take() {
                        let event_tx = event_tx.clone();
                        decoder
                            .decode_slot_nzhsym50_and_finish(state, &samples_12k, |decode| {
                                event_tx
                                    .send(DecodeWorkerEvent::Decode {
                                        timestamp: timestamp.clone(),
                                        decode: decode.clone(),
                                    })
                                    .map_err(|err| err.to_string())
                            })
                            .map(|results| {
                                Some(DecodeWorkerEvent::SlotComplete {
                                    timestamp,
                                    count: results.len(),
                                })
                            })
                    } else {
                        Err(format!("{timestamp}: received nzhsym=50 before nzhsym=41"))
                    }
                }
                DecodeWorkerCommand::Stop => break,
            };

            match result {
                Ok(Some(event)) => {
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    let _ = event_tx.send(DecodeWorkerEvent::Error(err));
                    break;
                }
            }
        }
    });
    (cmd_tx, event_rx)
}

fn run_profile_worker(
    config: StreamDecodeConfig,
    cmd_rx: Receiver<DecodeWorkerCommand>,
    event_tx: Sender<DecodeWorkerEvent>,
) {
    let mut decoder = ProfileStreamDecodeSession::new(config);
    while let Ok(command) = cmd_rx.recv() {
        let result = match command {
            DecodeWorkerCommand::Nzhsym41 { .. } | DecodeWorkerCommand::Nzhsym47 { .. } => Ok(None),
            DecodeWorkerCommand::Nzhsym50 {
                timestamp,
                samples_12k,
            } => {
                let event_tx = event_tx.clone();
                decoder
                    .decode_slot_streaming_at(&timestamp, &samples_12k, |decode| {
                        event_tx
                            .send(DecodeWorkerEvent::Decode {
                                timestamp: timestamp.clone(),
                                decode: decode.clone(),
                            })
                            .map_err(|err| err.to_string())
                    })
                    .map(|results| {
                        Some(DecodeWorkerEvent::SlotComplete {
                            timestamp,
                            count: results.len(),
                        })
                    })
            }
            DecodeWorkerCommand::Stop => break,
        };

        match result {
            Ok(Some(event)) => {
                if event_tx.send(event).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(err) => {
                let _ = event_tx.send(DecodeWorkerEvent::Error(err));
                break;
            }
        }
    }
}

fn send_worker_command(
    tx: &Sender<DecodeWorkerCommand>,
    command: DecodeWorkerCommand,
) -> Result<(), String> {
    tx.send(command)
        .map_err(|err| format!("soundcard decode worker stopped: {err}"))
}

fn handle_decode_event<F, G>(
    event: DecodeWorkerEvent,
    on_decode: &mut F,
    on_slot_complete: &mut G,
) -> Result<bool, String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    match event {
        DecodeWorkerEvent::Decode { timestamp, decode } => {
            on_decode(timestamp, &decode)?;
            Ok(false)
        }
        DecodeWorkerEvent::SlotComplete { timestamp, count } => {
            on_slot_complete(timestamp, count)?;
            Ok(true)
        }
        DecodeWorkerEvent::Error(err) => Err(err),
    }
}

fn drain_available_decode_events<F, G>(
    event_rx: &Receiver<DecodeWorkerEvent>,
    on_decode: &mut F,
    on_slot_complete: &mut G,
) -> Result<bool, String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let mut slot_complete = false;
    while let Ok(event) = event_rx.try_recv() {
        slot_complete |= handle_decode_event(event, on_decode, on_slot_complete)?;
    }
    Ok(slot_complete)
}

fn drain_decode_events_until_slot_complete<F, G>(
    event_rx: &Receiver<DecodeWorkerEvent>,
    on_decode: &mut F,
    on_slot_complete: &mut G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    loop {
        if drain_available_decode_events(event_rx, on_decode, on_slot_complete)? {
            return Ok(());
        }
        let event = event_rx
            .recv()
            .map_err(|err| format!("soundcard decode worker stopped: {err}"))?;
        if handle_decode_event(event, on_decode, on_slot_complete)? {
            return Ok(());
        }
    }
}

pub(crate) fn start_input_stream(
    selector: Option<&str>,
) -> Result<(cpal::Stream, Receiver<Vec<f32>>, u32), String> {
    let selector = selector.unwrap_or("default");
    let (device, info) = select_input_device(selector)?;
    let supported_config = device.default_input_config().map_err(|err| {
        format!(
            "failed to read default input config for {}: {err}",
            info.name
        )
    })?;
    let sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;

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

    Ok((stream, rx, sample_rate))
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

struct NativeSampleCollector<'a> {
    rx: &'a Receiver<Vec<f32>>,
    carry: Vec<f32>,
}

impl<'a> NativeSampleCollector<'a> {
    fn new(rx: &'a Receiver<Vec<f32>>) -> Self {
        Self {
            rx,
            carry: Vec::new(),
        }
    }

    fn collect_until(
        &mut self,
        out: &mut Vec<f32>,
        target_len: usize,
        deadline: Instant,
    ) -> Result<(), String> {
        self.take_from_carry(out, target_len);
        while out.len() < target_len {
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "timed out collecting soundcard slot: got {}/{} samples",
                    out.len(),
                    target_len
                ));
            }
            let timeout = deadline.saturating_duration_since(now);
            let chunk = self
                .rx
                .recv_timeout(timeout)
                .map_err(|err| format!("soundcard input stopped while collecting audio: {err}"))?;
            let remaining = target_len - out.len();
            if chunk.len() <= remaining {
                out.extend_from_slice(&chunk);
            } else {
                out.extend_from_slice(&chunk[..remaining]);
                self.carry.extend_from_slice(&chunk[remaining..]);
            }
        }
        Ok(())
    }

    fn collect_until_with_events<F, G>(
        &mut self,
        out: &mut Vec<f32>,
        target_len: usize,
        deadline: Instant,
        event_rx: &Receiver<DecodeWorkerEvent>,
        on_decode: &mut F,
        on_slot_complete: &mut G,
    ) -> Result<(), String>
    where
        F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
        G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
    {
        self.take_from_carry(out, target_len);
        while out.len() < target_len {
            let _ = drain_available_decode_events(event_rx, on_decode, on_slot_complete)?;
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "timed out collecting soundcard slot: got {}/{} samples",
                    out.len(),
                    target_len
                ));
            }
            let timeout = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(50));
            match self.rx.recv_timeout(timeout) {
                Ok(chunk) => {
                    let remaining = target_len - out.len();
                    if chunk.len() <= remaining {
                        out.extend_from_slice(&chunk);
                    } else {
                        out.extend_from_slice(&chunk[..remaining]);
                        self.carry.extend_from_slice(&chunk[remaining..]);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(err) => {
                    return Err(format!(
                        "soundcard input stopped while collecting audio: {err}"
                    ))
                }
            }
        }
        let _ = drain_available_decode_events(event_rx, on_decode, on_slot_complete)?;
        Ok(())
    }

    fn take_from_carry(&mut self, out: &mut Vec<f32>, target_len: usize) {
        if self.carry.is_empty() || out.len() >= target_len {
            return;
        }
        let needed = target_len - out.len();
        if self.carry.len() <= needed {
            out.extend_from_slice(&self.carry);
            self.carry.clear();
        } else {
            out.extend_from_slice(&self.carry[..needed]);
            self.carry.drain(..needed);
        }
    }
}

pub(crate) fn native_samples_for_nzhsym(sample_rate: u32, nzhsym: usize) -> usize {
    let samples_12k = nzhsym * NZHSYM_STRIDE;
    ((samples_12k as u64 * sample_rate as u64) + TARGET_SAMPLE_RATE as u64 - 1) as usize
        / TARGET_SAMPLE_RATE as usize
}

pub(crate) fn drain_pending_audio(rx: &Receiver<Vec<f32>>) {
    while rx.try_recv().is_ok() {}
}

pub(crate) fn next_slot_start_unix_seconds() -> Result<i64, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time is before Unix epoch: {err}"))?;
    let now_millis = now.as_millis() as i64;
    Ok(((now_millis / 15_000) + 1) * 15)
}

pub(crate) fn sleep_until_unix_seconds(unix_seconds: i64) -> Result<(), String> {
    let target = UNIX_EPOCH + Duration::from_secs(unix_seconds as u64);
    if let Ok(duration) = target.duration_since(SystemTime::now()) {
        std::thread::sleep(duration);
    }
    Ok(())
}
