use ft8rs::input::audio::downsample_12k;
use ft8rs::stream::profile::{ProfileSlotState, ProfileStreamDecodeSession};
use ft8rs::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance, StreamDecodedWithProvenance,
};
use ft8rs::stream::{SlotTimestamp, StreamDecodeSession, StreamDecodedMessage};

use cpal::traits::StreamTrait;
use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
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
pub(crate) struct AudioChunk {
    pub samples: Vec<f32>,
    pub capture_ms: i64,
    pub channel_peaks: Vec<f32>,
    pub xruns: u64,
    pub error: Option<String>,
}

pub(crate) struct AudioCapture {
    pub stream: cpal::Stream,
    pub rx: Receiver<AudioChunk>,
    pub format: SoundcardFormatInfo,
    pub device_name: String,
}

/// Which staged decode a ready window corresponds to (nzhsym 41/47/50).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum SlotStage {
    N41,
    N47,
    N50,
}

/// Diagnostics for one just-completed slot — surfaced to the GUI so a single live
/// test reveals how the driver actually delivers audio (block size / count), how
/// full and well-timed the decode window is, and whether the block *arrival* times
/// (`capture_ms`) are smooth or jittery. High `max_jitter_ms` on a driver that
/// still decodes elsewhere is the tell that capture-timestamps are unreliable.
pub(crate) struct SlotDiag {
    pub peak: f32,
    pub samples: usize,     // native samples in the slot's decode window
    pub blocks: u32,        // capture callbacks that fed this slot
    pub avg_block: u32,     // mean block size in samples
    pub max_jitter_ms: i64, // worst deviation of block arrival from smooth real-time
}

/// What a `PeriodAccumulator::push` produced: staged 12 kHz windows to hand the
/// decoder (in order), and, when a slot just finished, its diagnostics.
#[derive(Default)]
pub(crate) struct AccOutput {
    pub stages: Vec<(SlotStage, SlotTimestamp, Vec<f32>)>,
    pub completed: Option<SlotDiag>,
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
    // Per-slot capture diagnostics.
    blocks: u32,
    slot_t0_ms: Option<i64>, // delivery time just before the slot's first sample
    max_jitter_ms: i64,
}

impl PeriodAccumulator {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            boundary: None,
            native: Vec::new(),
            stage: 0,
            peak: 0.0,
            blocks: 0,
            slot_t0_ms: None,
            max_jitter_ms: 0,
        }
    }

    /// Feed one captured block. Returns any staged windows now ready plus (once
    /// per slot) the completed-slot marker.
    ///
    /// Attributes the *whole* block to the UTC slot of its capture time and
    /// appends its samples by count — exactly like WSJT-X, whose `writeData` keys a
    /// once-per-block wall-clock check to decide only *whether* it crossed the slot
    /// boundary (resetting `kin`), and otherwise packs samples densely by index. It
    /// never uses a per-block timestamp to position individual samples. That coarse
    /// decision tolerates block-delivery-time jitter (as on cpal's WASAPI capture);
    /// a finer sub-sample split does not, and smears the window on jittery drivers.
    pub(crate) fn push(&mut self, chunk: &[f32], capture_ms: i64) -> AccOutput {
        let mut out = AccOutput::default();
        if chunk.is_empty() {
            return out;
        }
        let slot = capture_ms.div_euclid(SLOT_MS) * SLOT_SECONDS as i64;
        match self.boundary {
            None => self.boundary = Some(slot),
            Some(b) if slot > b => {
                // Crossed into a new slot: finish the old one, start fresh.
                self.finalize(&mut out);
                self.reset(slot);
            }
            // Clock stepped back into an already-finished slot: ignore this block.
            Some(b) if slot < b => return out,
            _ => {}
        }
        // Capture diagnostics: measure how far this block's arrival time deviates
        // from smooth real-time delivery (t0 + samples/rate). Small = clean timing;
        // large = bursty/jittery driver, i.e. `capture_ms` can't be trusted for fine
        // sample placement (the reason we grid whole blocks, not sub-samples).
        let n = chunk.len() as i64;
        let rate = self.sample_rate as i64;
        self.blocks += 1;
        match self.slot_t0_ms {
            None => self.slot_t0_ms = Some(capture_ms - n * 1000 / rate),
            Some(t0) => {
                let expected = t0 + (self.native.len() as i64 + n) * 1000 / rate;
                self.max_jitter_ms = self.max_jitter_ms.max((capture_ms - expected).abs());
            }
        }
        self.append(chunk, &mut out);
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
            out.stages.push((
                SlotStage::N41,
                self.ts(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 1;
        }
        if self.stage == 1 && self.native.len() >= native_samples_for_nzhsym(self.sample_rate, 47) {
            out.stages.push((
                SlotStage::N47,
                self.ts(),
                downsample_12k(&self.native, self.sample_rate),
            ));
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
            out.stages.push((
                SlotStage::N47,
                ts.clone(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 2;
        }
        if self.stage == 2 {
            out.stages.push((
                SlotStage::N50,
                ts.clone(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 3;
        }
        out.completed = Some(SlotDiag {
            peak: self.peak,
            samples: self.native.len(),
            blocks: self.blocks,
            avg_block: self.native.len() as u32 / self.blocks.max(1),
            max_jitter_ms: self.max_jitter_ms,
        });
    }

    fn reset(&mut self, boundary: i64) {
        self.boundary = Some(boundary);
        self.native.clear();
        self.stage = 0;
        self.peak = 0.0;
        self.blocks = 0;
        self.slot_t0_ms = None;
        self.max_jitter_ms = 0;
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
    on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    decode_soundcard_streaming_tagged(
        options,
        |time, row| on_decode(time, &row.decode),
        on_slot_complete,
    )
}

pub fn decode_soundcard_streaming_tagged<F, G>(
    options: SoundcardDecodeOptions,
    mut on_decode: F,
    mut on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedWithProvenance) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let AudioCapture {
        stream, rx, format, ..
    } = start_input_stream(options.device.as_deref(), options.channel)?;
    let sample_rate = format.sample_rate;
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

        let chunk = match rx.recv() {
            Ok(block) => block,
            Err(_) => break, // capture stopped
        };
        if let Some(err) = chunk.error {
            return Err(err);
        }
        let out = acc.push(&chunk.samples, chunk.capture_ms);
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

#[expect(
    clippy::large_enum_variant,
    reason = "Keep decoded messages inline to avoid an extra allocation per worker event"
)]
enum DecodeWorkerEvent {
    Decode {
        timestamp: SlotTimestamp,
        decode: StreamDecodedWithProvenance,
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
                                    decode: StreamDecodedWithProvenance {
                                        decode: decode.clone(),
                                        provenance: StreamDecodeProvenance::Unknown,
                                    },
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
                                        decode: StreamDecodedWithProvenance {
                                            decode: decode.clone(),
                                            provenance: StreamDecodeProvenance::Unknown,
                                        },
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
                                decode: row.clone(),
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
                                        decode: row.clone(),
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
    F: FnMut(SlotTimestamp, &StreamDecodedWithProvenance) -> Result<(), String>,
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
    F: FnMut(SlotTimestamp, &StreamDecodedWithProvenance) -> Result<(), String>,
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

    let (tx, rx) = mpsc::channel();
    let stream_config = supported_config.into();
    let error_tx = tx.clone();
    let xruns = Arc::new(AtomicU64::new(0));
    let error_xruns = Arc::clone(&xruns);
    let err_fn = move |err| report_capture_error(err, &error_tx, &error_xruns);
    // Use CPAL's conversions for each PCM width, including 24-bit samples in
    // 32-bit containers. Never interpret float samples as 16-bit PCM or divide
    // already-normalized floats by 32768.
    macro_rules! build {
        ($ty:ty) => {
            device.build_input_stream(
                stream_config,
                move |data: &[$ty], _| send_samples(data, channels, channel, &tx, &xruns),
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
        rx,
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

/// Peak absolute amplitude of a sample buffer (0.0 when empty) — feeds the GUI
/// input-level readout so the operator can tell silence (dead capture) from signal.
pub(crate) fn peak_level(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

fn input_devices_with_info() -> Result<Vec<(cpal::Device, SoundcardDeviceInfo)>, String> {
    let mut rows = Vec::new();

    for host_id in cpal::available_hosts() {
        log::info!("Opening audio host {host_id:?}");
        let host = cpal::host_from_id(host_id)
            .map_err(|err| format!("failed to open audio host {host_id:?}: {err}"))?;
        let host_name = format!("{host_id:?}");
        log::info!("Querying default input on {host_name}");
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
            log::info!("Querying input format: {host_name} / {name}");
            let Some(input) = default_input_config(&device) else {
                log::warn!("Skipping input without a default format: {host_name} / {name}");
                continue;
            };
            log::info!("Audio input: {host_name} / {name}: {input:?}");

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
fn report_capture_error(err: cpal::Error, tx: &Sender<AudioChunk>, xruns: &AtomicU64) {
    if err.kind() == cpal::ErrorKind::Xrun {
        xruns.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let _ = tx.send(AudioChunk {
        samples: Vec::new(),
        capture_ms: capture_now_ms(),
        channel_peaks: Vec::new(),
        xruns: xruns.load(Ordering::Relaxed),
        error: Some(format!("soundcard input stream error: {err}")),
    });
}

fn send_samples<T>(
    data: &[T],
    channels: usize,
    channel: InputChannel,
    tx: &Sender<AudioChunk>,
    xruns: &AtomicU64,
) where
    T: cpal::Sample,
    f32: cpal::FromSample<T>,
{
    let capture_ms = capture_now_ms();
    let mut channel_peaks = vec![0.0f32; channels];
    let samples = data.iter().enumerate().map(|(i, &sample)| {
        let value = sample.to_sample::<f32>();
        // A malformed float from a driver must not poison an entire slot's FFT.
        let value = if value.is_finite() { value } else { 0.0 };
        channel_peaks[i % channels] = channel_peaks[i % channels].max(value.abs());
        value
    });
    let samples = fold_channels(samples, channels, channel);
    let _ = tx.send(AudioChunk {
        samples,
        capture_ms,
        channel_peaks,
        xruns: xruns.load(Ordering::Relaxed),
        error: None,
    });
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

    #[test]
    fn capture_xruns_do_not_stop_audio_and_reach_diagnostics() {
        use super::{report_capture_error, send_samples, AtomicU64};
        let (tx, rx) = std::sync::mpsc::channel();
        let xruns = AtomicU64::new(0);
        // A driver may flag more than one startup packet. Neither notification
        // may enter the fatal-error queue consumed by both GUI and CLI.
        for _ in 0..2 {
            report_capture_error(cpal::ErrorKind::Xrun.into(), &tx, &xruns);
        }
        assert!(matches!(
            rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        for _ in 0..2 {
            send_samples(&[0.25f32, -0.5], 1, InputChannel::Mono, &tx, &xruns);
            let chunk = rx.recv().unwrap();
            assert_eq!(chunk.samples, [0.25, -0.5]);
            assert_eq!(chunk.channel_peaks, [0.5]);
            assert_eq!(chunk.xruns, 2);
            assert!(chunk.error.is_none());
        }
    }

    #[test]
    fn capture_fatal_errors_keep_their_kind_independent_stop_behavior() {
        use super::{report_capture_error, AtomicU64};
        let (tx, rx) = std::sync::mpsc::channel();
        let xruns = AtomicU64::new(0);
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::PermissionDenied,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::BackendError,
        ] {
            // Even text identical to an xrun must remain fatal if its error kind
            // is not Xrun. Never classify backend errors by matching strings.
            report_capture_error(
                cpal::Error::with_message(kind, "A buffer underrun or overrun occurred."),
                &tx,
                &xruns,
            );
            let chunk = rx.recv().unwrap();
            assert!(chunk.samples.is_empty());
            assert_eq!(chunk.xruns, 0);
            assert_eq!(
                chunk.error.as_deref(),
                Some("soundcard input stream error: A buffer underrun or overrun occurred.")
            );
        }
    }

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
                // A slot finishes when its N50 window is emitted.
                if st == SlotStage::N50 {
                    done.push(ts.clone());
                }
                stages.push((st, ts, s.len()));
            }
        }
        (stages, done)
    }

    #[test]
    fn whole_capture_blocks_can_exceed_fifteen_seconds() {
        const B: i64 = 1_700_000_010;
        let mut acc = PeriodAccumulator::new(SR);
        // 1024-sample driver blocks don't divide a 15 s period. In a continuous
        // stream some slots contain one extra block; this is valid capture.
        let (stages, _) = drive(&mut acc, 1024, SR as usize * 61 / 1024, B * 1000);
        let lengths: Vec<_> = stages
            .iter()
            .filter(|(stage, _, _)| *stage == SlotStage::N50)
            .map(|(_, _, len)| *len)
            .collect();
        assert!(lengths.iter().any(|&len| len > 180_000), "{lengths:?}");
    }

    #[test]
    fn full_slot_emits_all_three_stages_with_near_full_window() {
        const B: i64 = 1_700_000_010; // a multiple of 15
        let mut acc = PeriodAccumulator::new(SR);
        // 32 half-second blocks = 16 s, so slot B fills and the next one begins.
        let (stages, done) = drive(&mut acc, SR as usize / 2, 32, B * 1000);

        let kinds: Vec<_> = stages.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(kinds, vec![SlotStage::N41, SlotStage::N47, SlotStage::N50]);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        assert!(stages.iter().all(|(_, ts, _)| *ts == ts_b));
        assert_eq!(done, vec![ts_b]);
        // N50 window covers the slot up to the block that crossed the boundary — at
        // least the ~14.4 s (172 800 @ 12 kHz) the decoder consumes.
        let (_, _, n50_len) = *stages.last().unwrap();
        assert!(
            (172_800..=180_000).contains(&n50_len),
            "N50 window should be a near-full slot, got {n50_len}"
        );
    }

    #[test]
    fn each_whole_block_grids_to_its_slot_and_both_decode() {
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

    // Synthetic opposite-phase input: averaging cancels it; Mono must retain
    // signal. This is a regression case, not a claim about every DAX driver.
    #[test]
    fn antiphase_stereo_cancels_under_mono_but_survives_single_channel() {
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
            let (tx, rx) = std::sync::mpsc::channel();
            super::send_samples(data, 2, InputChannel::Mono, &tx, &super::AtomicU64::new(0));
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
