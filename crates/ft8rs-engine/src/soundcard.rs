use ft8rs::input::audio::downsample_12k;
use ft8rs::stream::profile::{ProfileSlotState, ProfileStreamDecodeSession};
use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};
use ft8rs::stream::{SlotTimestamp, StreamDecodeSession, StreamDecodedMessage};

use cpal::traits::StreamTrait;
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

const TARGET_SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: u64 = 15;
const NZHSYM_STRIDE: usize = 3456;
/// One FT8 slot in milliseconds (the UTC period the decoder aligns to).
const SLOT_MS: i64 = SLOT_SECONDS as i64 * 1000;

/// A captured, channel-folded audio block tagged with the wall-clock unix-ms at
/// the moment the input callback delivered it (≈ the last sample's capture time).
/// The timestamp is what lets the accumulator place each block on the UTC grid
/// without draining — the way WSJT-X keys its buffer off `currentMSecsSinceEpoch`.
pub(crate) type AudioChunk = (Vec<f32>, i64);

/// Which staged decode a ready window corresponds to (nzhsym 41/47/50).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotStage {
    N41,
    N47,
    N50,
}

/// What a `PeriodAccumulator::push` produced: staged 12 kHz windows to hand the
/// decoder (in order), and, when a slot just finished, its timestamp + peak level.
#[derive(Default)]
pub(crate) struct AccOutput {
    pub stages: Vec<(SlotStage, SlotTimestamp, Vec<f32>)>,
    pub completed: Option<(SlotTimestamp, f32)>,
}

/// WSJT-X-style capture alignment. Instead of sleeping to the boundary, draining,
/// and re-collecting (which cuts the audio at a point unrelated to the driver's
/// block grid — fine for small USB blocks, but jitters by a whole block on large
/// virtual-driver blocks like FlexRadio DAX), we accumulate continuously and place
/// every incoming block on the UTC 15 s grid using its capture timestamp, splitting
/// the one block that straddles a boundary at the exact sample. Each slot therefore
/// gets its true `[boundary, boundary+15s)` window (a small constant driver-latency
/// offset aside), so the decode window no longer wanders between slots.
pub(crate) struct PeriodAccumulator {
    sample_rate: u32,
    boundary: Option<i64>, // current slot's start (unix seconds, multiple of 15)
    native: Vec<f32>,      // native-rate samples accumulated for the current slot
    stage: u8,             // 0 none, 1 N41 fired, 2 N47 fired, 3 N50 fired
    peak: f32,             // running peak amplitude of the current slot
}

impl PeriodAccumulator {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            boundary: None,
            native: Vec::new(),
            stage: 0,
            peak: 0.0,
        }
    }

    /// Feed one captured block. Returns any staged windows now ready plus (once
    /// per slot) the completed-slot marker.
    pub(crate) fn push(&mut self, chunk: &[f32], capture_ms: i64) -> AccOutput {
        let mut out = AccOutput::default();
        if chunk.is_empty() {
            return out;
        }
        let n = chunk.len();
        let rate = self.sample_rate as i64;
        if self.boundary.is_none() {
            // Anchor to the slot containing the block's *first* sample.
            let first_ms = capture_ms - (n as i64 - 1) * 1000 / rate;
            self.boundary = Some(first_ms.div_euclid(SLOT_MS) * SLOT_SECONDS as i64);
        }

        let mut start = 0usize;
        loop {
            let b = self.boundary.unwrap();
            let b_next_ms = (b + SLOT_SECONDS as i64) * 1000;
            // Number of trailing samples of the block at/after the next boundary:
            // sample j is at capture_ms-(n-1-j)*1000/rate, so j>=b_next when
            // n-1-j <= (capture_ms-b_next_ms)*rate/1000.
            let k = (capture_ms - b_next_ms) * rate / 1000;
            let after = (k + 1).clamp(0, n as i64) as usize;
            let split = (n - after).max(start);
            if split > start {
                self.append(&chunk[start..split], &mut out);
            }
            if split >= n {
                break;
            }
            // Remaining samples belong to a later slot: finish this one and advance.
            self.finalize(&mut out);
            self.reset(b + SLOT_SECONDS as i64);
            start = split;
        }
        out
    }

    /// Flush the in-progress slot (e.g. on shutdown), emitting its final stage.
    pub(crate) fn flush(&mut self) -> AccOutput {
        let mut out = AccOutput::default();
        self.finalize(&mut out);
        self.stage = 3;
        out
    }

    fn append(&mut self, samples: &[f32], out: &mut AccOutput) {
        self.peak = self.peak.max(peak_level(samples));
        self.native.extend_from_slice(samples);
        // Stream the early stages as soon as enough audio has arrived.
        if self.stage == 0 && self.native.len() >= native_samples_for_nzhsym(self.sample_rate, 41) {
            out.stages
                .push((SlotStage::N41, self.ts(), downsample_12k(&self.native, self.sample_rate)));
            self.stage = 1;
        }
        if self.stage == 1 && self.native.len() >= native_samples_for_nzhsym(self.sample_rate, 47) {
            out.stages
                .push((SlotStage::N47, self.ts(), downsample_12k(&self.native, self.sample_rate)));
            self.stage = 2;
        }
    }

    fn finalize(&mut self, out: &mut AccOutput) {
        if self.stage == 0 {
            // Never reached the early-decode point: a runt slot (monitoring started
            // mid-slot, or a capture gap) — nothing worth decoding, drop it.
            return;
        }
        let ts = self.ts();
        if self.stage == 1 {
            out.stages
                .push((SlotStage::N47, ts.clone(), downsample_12k(&self.native, self.sample_rate)));
            self.stage = 2;
        }
        if self.stage == 2 {
            out.stages
                .push((SlotStage::N50, ts.clone(), downsample_12k(&self.native, self.sample_rate)));
            self.stage = 3;
        }
        out.completed = Some((ts, self.peak));
    }

    fn reset(&mut self, boundary: i64) {
        self.boundary = Some(boundary);
        self.native.clear();
        self.stage = 0;
        self.peak = 0.0;
    }

    fn ts(&self) -> SlotTimestamp {
        SlotTimestamp::from_unix_seconds_utc(self.boundary.unwrap())
    }
}

#[derive(Clone, Debug)]
pub struct SoundcardDecodeOptions {
    pub device: Option<String>,
    pub channel: InputChannel,
    pub config: StreamDecodeConfig,
    pub max_slots: Option<usize>,
}

/// Which physical channel of a multi-channel capture device feeds the decoder.
///
/// WSJT-X never averages a stereo input — it opens the device mono or picks a
/// single channel (`Audio/AudioDevice.hpp`). We mirror that: some rigs' virtual
/// cables (notably FlexRadio DAX) put the receive audio on one channel and
/// silence / an anti-phase copy / a different slice on the other, so averaging
/// `(L+R)/2` cancels or corrupts the signal and nothing decodes. Defaulting to
/// the left channel matches WSJT-X's out-of-the-box behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputChannel {
    /// Downmix all channels into one by averaging — the default, matching WSJT-X's
    /// out-of-the-box "Mono". If a virtual cable puts the audio on a single channel
    /// (or the two are anti-phase), pick Left/Right instead.
    #[default]
    Mono,
    /// Left channel only (frame index 0).
    Left,
    /// Right channel only (frame index 1).
    Right,
}

impl InputChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            InputChannel::Left => "left",
            InputChannel::Right => "right",
            InputChannel::Mono => "mono",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "left" => Some(InputChannel::Left),
            "right" => Some(InputChannel::Right),
            "mono" => Some(InputChannel::Mono),
            _ => None,
        }
    }
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

pub fn decode_soundcard_streaming_decodes<F, G>(
    options: SoundcardDecodeOptions,
    mut on_decode: F,
    mut on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let (stream, rx, sample_rate) = start_input_stream(options.device.as_deref(), options.channel)?;
    let (cmd_tx, event_rx) = start_decode_worker(options.config);

    // WSJT-X-style: no sleep-to-boundary / drain. Feed every captured block to the
    // accumulator, which grids it to UTC by its capture timestamp and streams the
    // staged windows. Sends are blocking (unbounded worker queue), so nothing is
    // ever dropped — the decoder is throttled by, never races ahead of, capture.
    let mut acc = PeriodAccumulator::new(sample_rate);
    let mut slots_done = 0usize;
    loop {
        // Surface any decodes the worker has finished.
        drain_available_decode_events(&event_rx, &mut on_decode, &mut on_slot_complete)?;

        let (chunk, capture_ms) = match rx.recv() {
            Ok(block) => block,
            Err(_) => break, // capture stopped
        };
        let out = acc.push(&chunk, capture_ms);
        for (stage, timestamp, samples_12k) in out.stages {
            send_worker_command(&cmd_tx, worker_command(stage, timestamp, samples_12k))?;
        }
        if out.completed.is_some() {
            slots_done += 1;
            if options.max_slots.is_some_and(|max| slots_done >= max) {
                break;
            }
        }
    }

    // Flush the in-progress slot, then wait for the worker to drain and finish.
    for (stage, timestamp, samples_12k) in acc.flush().stages {
        let _ = send_worker_command(&cmd_tx, worker_command(stage, timestamp, samples_12k));
    }
    let _ = cmd_tx.send(DecodeWorkerCommand::Stop);
    while let Ok(event) = event_rx.recv() {
        handle_decode_event(event, &mut on_decode, &mut on_slot_complete)?;
    }
    drop(stream);

    Ok(())
}

fn worker_command(
    stage: SlotStage,
    timestamp: SlotTimestamp,
    samples_12k: Vec<f32>,
) -> DecodeWorkerCommand {
    match stage {
        SlotStage::N41 => DecodeWorkerCommand::Nzhsym41 {
            timestamp,
            samples_12k,
        },
        SlotStage::N47 => DecodeWorkerCommand::Nzhsym47 {
            timestamp,
            samples_12k,
        },
        SlotStage::N50 => DecodeWorkerCommand::Nzhsym50 {
            timestamp,
            samples_12k,
        },
    }
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
    // Staged so hybrid streams early WSJT-X rows before the slot boundary (jtdx/dx
    // are stateless no-ops at nzhsym=41/47 and decode fully at nzhsym=50).
    let mut slot_state: Option<(ProfileSlotState, usize)> = None;
    while let Ok(command) = cmd_rx.recv() {
        let result = match command {
            DecodeWorkerCommand::Nzhsym41 {
                timestamp,
                samples_12k,
            } => {
                let mut state = decoder.start_slot();
                let event_tx = event_tx.clone();
                match decoder.decode_slot_nzhsym41_streaming_with_provenance(
                    &timestamp,
                    &mut state,
                    &samples_12k,
                    |row| {
                        event_tx
                            .send(DecodeWorkerEvent::Decode {
                                timestamp: timestamp.clone(),
                                decode: row.decode.clone(),
                            })
                            .map_err(|err| err.to_string())
                    },
                ) {
                    Ok(early_count) => {
                        slot_state = Some((state, early_count));
                        Ok(None)
                    }
                    Err(err) => Err(err),
                }
            }
            DecodeWorkerCommand::Nzhsym47 {
                timestamp: _,
                samples_12k,
            } => {
                if let Some((state, _)) = slot_state.as_mut() {
                    decoder.subtract_slot_nzhsym47(state, &samples_12k);
                }
                Ok(None)
            }
            DecodeWorkerCommand::Nzhsym50 {
                timestamp,
                samples_12k,
            } => {
                if let Some((state, early_count)) = slot_state.take() {
                    let event_tx = event_tx.clone();
                    decoder
                        .decode_slot_nzhsym50_streaming_with_provenance(
                            &timestamp,
                            state,
                            early_count,
                            &samples_12k,
                            |row| {
                                event_tx
                                    .send(DecodeWorkerEvent::Decode {
                                        timestamp: timestamp.clone(),
                                        decode: row.decode.clone(),
                                    })
                                    .map_err(|err| err.to_string())
                            },
                        )
                        .map(|count| Some(DecodeWorkerEvent::SlotComplete { timestamp, count }))
                } else {
                    Ok(None)
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


pub(crate) fn start_input_stream(
    selector: Option<&str>,
    channel: InputChannel,
) -> Result<(cpal::Stream, Receiver<AudioChunk>, u32), String> {
    let selector = selector.unwrap_or("default");
    let (device, info) = select_input_device(selector)?;
    let supported_config = select_input_config(&device, &info)?;
    let sample_rate = supported_config.sample_rate();
    let channels = supported_config.channels() as usize;

    let (tx, rx) = mpsc::channel();
    let stream_config = supported_config.clone().into();
    let err_fn = |err| eprintln!("soundcard input stream error: {err}");
    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _| send_f32(data, channels, channel, &tx),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _| send_i16(data, channels, channel, &tx),
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            stream_config,
            move |data: &[u16], _| send_u16(data, channels, channel, &tx),
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

/// Choose the capture format, preferring 48 kHz / f32. FlexRadio DAX (and many
/// virtual cables) advertise that as their only real format and, in shared mode,
/// won't resample or reformat. `default_input_config()` returns whatever Windows
/// currently reports as the endpoint's mix format — which the operator can change
/// (Sound → device → Advanced → Default Format) and which need not be the rate the
/// driver actually streams; opening a mismatched format there yields a *silent*
/// stream rather than an error. Explicitly matching a device-supported f32/48k
/// config (per the DAX guidance) avoids that. Falls back to the device default for
/// ordinary cards and other platforms.
fn select_input_config(
    device: &cpal::Device,
    info: &SoundcardDeviceInfo,
) -> Result<cpal::SupportedStreamConfig, String> {
    const PREFERRED_RATE: u32 = 48_000;
    if let Ok(configs) = device.supported_input_configs() {
        let configs: Vec<_> = configs.collect();
        // 1) f32 @ 48 kHz — DAX's native format. (cpal's SampleRate is a u32 alias.)
        if let Some(range) = configs.iter().find(|c| {
            c.sample_format() == cpal::SampleFormat::F32
                && c.min_sample_rate() <= PREFERRED_RATE
                && c.max_sample_rate() >= PREFERRED_RATE
        }) {
            return Ok(range.clone().with_sample_rate(PREFERRED_RATE));
        }
        // 2) Any f32 config (still avoids an int format the driver may not stream).
        if let Some(range) = configs
            .iter()
            .find(|c| c.sample_format() == cpal::SampleFormat::F32)
        {
            return Ok(range.clone().with_max_sample_rate());
        }
    }
    // 3) Fall back to the device default.
    device
        .default_input_config()
        .map_err(|err| format!("failed to read input config for {}: {err}", info.name))
}

/// Peak absolute amplitude of a sample buffer (0.0 when empty) — feeds the GUI
/// input-level readout so the operator can tell silence (dead capture) from signal.
pub(crate) fn peak_level(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

fn input_devices_with_info() -> Result<Vec<(cpal::Device, SoundcardDeviceInfo)>, String> {
    let mut rows = Vec::new();

    for host_id in cpal::available_hosts() {
        let host = cpal::host_from_id(host_id)
            .map_err(|err| format!("failed to open audio host {host_id:?}: {err}"))?;
        let host_name = format!("{host_id:?}");
        let default_input = host
            .default_input_device()
            .and_then(|device| device.description().ok().map(|d| d.name().to_string()));

        for device in host
            .input_devices()
            .map_err(|err| format!("failed to enumerate {host_name} audio input devices: {err}"))?
        {
            let name = device
                .description()
                .map(|d| d.name().to_string())
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
        sample_rate: config.sample_rate(),
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

/// Wall-clock unix-ms at the input callback — the block's approximate capture
/// time, stamped here (like WSJT-X's `currentMSecsSinceEpoch()` in `writeData`) so
/// the accumulator can grid it to UTC regardless of how long it queues downstream.
fn capture_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn send_f32(data: &[f32], channels: usize, channel: InputChannel, tx: &mpsc::Sender<AudioChunk>) {
    let _ = tx.send((fold_channels(data.iter().copied(), channels, channel), capture_now_ms()));
}

fn send_i16(data: &[i16], channels: usize, channel: InputChannel, tx: &mpsc::Sender<AudioChunk>) {
    let folded = fold_channels(
        data.iter().map(|sample| *sample as f32 / 32768.0),
        channels,
        channel,
    );
    let _ = tx.send((folded, capture_now_ms()));
}

fn send_u16(data: &[u16], channels: usize, channel: InputChannel, tx: &mpsc::Sender<AudioChunk>) {
    let folded = fold_channels(
        data.iter().map(|sample| *sample as f32 / 32768.0 - 1.0),
        channels,
        channel,
    );
    let _ = tx.send((folded, capture_now_ms()));
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
        InputChannel::Mono => {
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
        InputChannel::Left | InputChannel::Right => {
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


pub(crate) fn native_samples_for_nzhsym(sample_rate: u32, nzhsym: usize) -> usize {
    let samples_12k = nzhsym * NZHSYM_STRIDE;
    ((samples_12k as u64 * sample_rate as u64) + TARGET_SAMPLE_RATE as u64 - 1) as usize
        / TARGET_SAMPLE_RATE as usize
}


#[cfg(test)]
mod tests {
    use super::{fold_channels, InputChannel, PeriodAccumulator, SlotStage};
    use ft8rs::SlotTimestamp;

    const SR: u32 = 12_000; // downsample_12k is a passthrough here, keeping buffers small

    /// Feed `chunks` blocks of `chunk_len` samples each, at real-time capture
    /// timestamps starting from `start_ms`, collecting every emitted stage and the
    /// completed-slot markers.
    fn drive(
        acc: &mut PeriodAccumulator,
        chunk_len: usize,
        chunks: usize,
        start_ms: i64,
    ) -> (Vec<(SlotStage, SlotTimestamp, usize)>, Vec<SlotTimestamp>) {
        let mut stages = Vec::new();
        let mut done = Vec::new();
        let block = vec![0.1f32; chunk_len];
        for c in 0..chunks {
            let capture_ms = start_ms + ((c as i64 + 1) * chunk_len as i64 * 1000) / SR as i64;
            let out = acc.push(&block, capture_ms);
            for (st, ts, s) in out.stages {
                stages.push((st, ts, s.len()));
            }
            if let Some((ts, _peak)) = out.completed {
                done.push(ts);
            }
        }
        (stages, done)
    }

    #[test]
    fn full_slot_emits_all_three_stages_with_full_window() {
        const B: i64 = 1_700_000_010; // a multiple of 15
        let mut acc = PeriodAccumulator::new(SR);
        // 32 half-second blocks = 16 s, so slot B fills and the next one begins.
        let (stages, done) = drive(&mut acc, SR as usize / 2, 32, B * 1000);

        let kinds: Vec<_> = stages.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(kinds, vec![SlotStage::N41, SlotStage::N47, SlotStage::N50]);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        assert!(stages.iter().all(|(_, ts, _)| *ts == ts_b));
        assert_eq!(done, vec![ts_b]);
        // N50 window must be ~a full 15 s slot (no tail lost to the straddling block).
        let (_, _, n50_len) = *stages.last().unwrap();
        assert!(
            n50_len >= 179_000 && n50_len <= 180_000,
            "N50 window should be a full slot, got {n50_len}"
        );
    }

    #[test]
    fn straddling_block_splits_so_next_slot_starts_at_its_boundary() {
        const B: i64 = 1_700_000_010;
        let mut acc = PeriodAccumulator::new(SR);
        // Run through slot B and well into slot B+15 so both finalize.
        let (stages, done) = drive(&mut acc, SR as usize / 2, 62, B * 1000);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        let ts_b1 = SlotTimestamp::from_unix_seconds_utc(B + 15);
        assert_eq!(done, vec![ts_b, ts_b1.clone()]);
        // Each slot produced its own N41/N47/N50.
        let for_b1: Vec<_> = stages
            .iter()
            .filter(|(_, ts, _)| *ts == ts_b1)
            .map(|(k, _, _)| *k)
            .collect();
        assert_eq!(for_b1, vec![SlotStage::N41, SlotStage::N47, SlotStage::N50]);
    }

    #[test]
    fn runt_first_slot_is_dropped_not_decoded() {
        const B: i64 = 1_700_000_010;
        let mut acc = PeriodAccumulator::new(SR);
        // Start 12 s into slot B: only ~3 s of it captured -> below the N41 point.
        let start = (B * 1000) + 12_000;
        let (stages, done) = drive(&mut acc, SR as usize / 2, 40, start);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        // Slot B is a runt: no stages, not marked done.
        assert!(stages.iter().all(|(_, ts, _)| *ts != ts_b));
        assert!(!done.contains(&ts_b));
        // The next full slot decodes normally.
        let ts_b1 = SlotTimestamp::from_unix_seconds_utc(B + 15);
        assert!(done.contains(&ts_b1));
    }

    // Interleaved stereo where the right channel is the left, inverted — the
    // shape FlexRadio DAX can present. Averaging cancels it to silence; picking a
    // single channel recovers the real audio. This is the whole point of the fix.
    #[test]
    fn antiphase_stereo_cancels_under_mono_but_survives_single_channel() {
        let left = [0.1f32, -0.2, 0.3, -0.4];
        let interleaved: Vec<f32> = left.iter().flat_map(|&l| [l, -l]).collect();

        let mono = fold_channels(interleaved.iter().copied(), 2, InputChannel::Mono);
        assert!(
            mono.iter().all(|&s| s.abs() < 1e-6),
            "anti-phase average should cancel to ~0, got {mono:?}"
        );

        let l = fold_channels(interleaved.iter().copied(), 2, InputChannel::Left);
        assert_eq!(l, left);
        let r = fold_channels(interleaved.iter().copied(), 2, InputChannel::Right);
        assert_eq!(r, left.iter().map(|&s| -s).collect::<Vec<_>>());
    }

    #[test]
    fn mono_device_passes_through_regardless_of_selection() {
        let samples = [0.5f32, -0.25, 0.125];
        for ch in [InputChannel::Left, InputChannel::Right, InputChannel::Mono] {
            assert_eq!(fold_channels(samples.iter().copied(), 1, ch), samples);
        }
    }
}
