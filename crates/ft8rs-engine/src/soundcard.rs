use ft8rs::input::audio::downsample_12k;
use ft8rs::stream::profile::{ProfileSlotState, ProfileStreamDecodeSession};
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
    let (stream, rx, sample_rate) = start_input_stream(options.device.as_deref(), options.channel)?;
    let (cmd_tx, event_rx) = start_decode_worker(options.config);

    // Fixed phase anchor (a UTC 15 s boundary). Each slot re-derives its window
    // from the wall clock relative to this anchor so soundcard sample-rate drift
    // and clock steps cannot accumulate into a decode-window offset.
    let slot_start = next_slot_start_unix_seconds()?;
    sleep_until_unix_seconds(slot_start)?;
    drain_pending_audio(&rx);

    let mut collector = NativeSampleCollector::new(&rx);
    let mut slots_done = 0usize;
    loop {
        if options
            .max_slots
            .is_some_and(|max_slots| slots_done >= max_slots)
        {
            break;
        }

        // Re-lock this slot's window start to the true UTC boundary.
        let boundary =
            slot_start + wall_clock_slot_index(slot_start, now_unix_millis()?).max(0) * 15;
        sleep_until_unix_seconds(boundary)?;
        drain_pending_audio(&rx);
        collector.clear_carry();

        let timestamp = SlotTimestamp::from_unix_seconds_utc(boundary);
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
        let samples_12k = downsample_12k(&native, sample_rate);
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
        let samples_12k = downsample_12k(&native, sample_rate);
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
        let samples_12k = downsample_12k(&native, sample_rate);
        send_worker_command(
            &cmd_tx,
            DecodeWorkerCommand::Nzhsym50 {
                timestamp,
                samples_12k,
            },
        )?;

        drain_decode_events_until_slot_complete(&event_rx, &mut on_decode, &mut on_slot_complete)?;
        slots_done += 1;
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
    let (stream, rx, sample_rate) = start_input_stream(options.device.as_deref(), options.channel)?;
    let samples_per_slot = sample_rate as usize * SLOT_SECONDS as usize;

    // Fixed phase anchor; each slot re-locks to the wall clock (see
    // `wall_clock_slot_index`) so soundcard drift cannot accumulate.
    let slot_start = next_slot_start_unix_seconds()?;
    sleep_until_unix_seconds(slot_start)?;
    drain_pending_audio(&rx);

    let mut decoder = ProfileStreamDecodeSession::new(options.config);
    let mut collector = NativeSampleCollector::new(&rx);
    let mut slots_done = 0usize;
    loop {
        if options
            .max_slots
            .is_some_and(|max_slots| slots_done >= max_slots)
        {
            break;
        }

        let boundary =
            slot_start + wall_clock_slot_index(slot_start, now_unix_millis()?).max(0) * 15;
        sleep_until_unix_seconds(boundary)?;
        drain_pending_audio(&rx);
        collector.clear_carry();

        let timestamp = SlotTimestamp::from_unix_seconds_utc(boundary);
        let mut native = Vec::with_capacity(samples_per_slot);
        let deadline = Instant::now() + Duration::from_secs(SLOT_SECONDS + 5);
        collector.collect_until(&mut native, samples_per_slot, deadline)?;
        let samples_12k = downsample_12k(&native, sample_rate);
        on_slot(&mut decoder, timestamp, &samples_12k)?;
        slots_done += 1;
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
    channel: InputChannel,
) -> Result<(cpal::Stream, Receiver<Vec<f32>>, u32), String> {
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

fn send_f32(data: &[f32], channels: usize, channel: InputChannel, tx: &mpsc::Sender<Vec<f32>>) {
    let _ = tx.send(fold_channels(data.iter().copied(), channels, channel));
}

fn send_i16(data: &[i16], channels: usize, channel: InputChannel, tx: &mpsc::Sender<Vec<f32>>) {
    let _ = tx.send(fold_channels(
        data.iter().map(|sample| *sample as f32 / 32768.0),
        channels,
        channel,
    ));
}

fn send_u16(data: &[u16], channels: usize, channel: InputChannel, tx: &mpsc::Sender<Vec<f32>>) {
    let _ = tx.send(fold_channels(
        data.iter().map(|sample| *sample as f32 / 32768.0 - 1.0),
        channels,
        channel,
    ));
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

    /// Drop any samples carried past the previous slot's target. Used when a slot
    /// re-locks to its UTC boundary so the next window starts from post-boundary
    /// audio rather than stale leftovers.
    fn clear_carry(&mut self) {
        self.carry.clear();
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

pub(crate) fn now_unix_millis() -> Result<i64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time is before Unix epoch: {err}"))?
        .as_millis() as i64)
}

/// Re-derive, from the wall clock, the index of the 15 s UTC slot whose start
/// boundary should be filled next, relative to the fixed phase anchor
/// `slot_start_secs` (a unix second that is a multiple of 15).
///
/// The monitor loops must recompute this every slot instead of a free-running
/// `+1` sample counter: the soundcard delivers samples at its *true* rate, which
/// differs from the nominal `sample_rate` by tens of ppm, so counting
/// `sample_rate * 15` samples per slot lets the decode window slide against UTC
/// (~4 s/day at ±50 ppm) until real transmissions fall outside the sync-search
/// window and decodes are silently lost. Deriving the index from the clock and
/// re-locking to the boundary each slot bounds the offset to a single slot's
/// processing lag, so drift and NTP clock steps cannot accumulate.
///
/// Rounds to the nearest boundary so being a hair early (fast device) or late
/// (slow device / decode lag) never mislabels a slot. Returns a negative index
/// only if the clock stepped back before the anchor; callers clamp/re-anchor.
pub(crate) fn wall_clock_slot_index(slot_start_secs: i64, now_ms: i64) -> i64 {
    let delta_ms = now_ms - slot_start_secs * 1_000;
    (delta_ms + 7_500).div_euclid(15_000)
}

pub(crate) fn sleep_until_unix_seconds(unix_seconds: i64) -> Result<(), String> {
    let target = UNIX_EPOCH + Duration::from_secs(unix_seconds as u64);
    if let Ok(duration) = target.duration_since(SystemTime::now()) {
        std::thread::sleep(duration);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{fold_channels, wall_clock_slot_index, InputChannel};

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

    const ANCHOR: i64 = 1_700_000_010; // a unix second that is a multiple of 15

    #[test]
    fn exact_boundary_maps_to_its_index() {
        for k in 0..10 {
            let now_ms = (ANCHOR + k * 15) * 1_000;
            assert_eq!(wall_clock_slot_index(ANCHOR, now_ms), k);
        }
    }

    #[test]
    fn slightly_early_or_late_rounds_to_nearest_boundary() {
        for k in 1..10 {
            let boundary_ms = (ANCHOR + k * 15) * 1_000;
            // Fast device: finished a hair before the boundary.
            assert_eq!(wall_clock_slot_index(ANCHOR, boundary_ms - 200), k);
            // Slow device / decode lag: a hair after the boundary.
            assert_eq!(wall_clock_slot_index(ANCHOR, boundary_ms + 200), k);
        }
    }

    #[test]
    fn accumulated_half_slot_drift_advances_the_index_once() {
        // ~7.4 s past boundary k still fills slot k; past the half-slot it jumps
        // to k+1 (a clean resync rather than unbounded silent drift).
        let boundary_ms = (ANCHOR + 4 * 15) * 1_000;
        assert_eq!(wall_clock_slot_index(ANCHOR, boundary_ms + 7_400), 4);
        assert_eq!(wall_clock_slot_index(ANCHOR, boundary_ms + 7_600), 5);
    }

    #[test]
    fn clock_step_forward_skips_the_uncaptured_slots() {
        let now_ms = (ANCHOR + 5 * 15) * 1_000; // jumped ahead 5 slots
        assert_eq!(wall_clock_slot_index(ANCHOR, now_ms), 5);
    }

    #[test]
    fn clock_before_anchor_is_negative_for_caller_to_clamp() {
        let now_ms = (ANCHOR - 30) * 1_000;
        assert!(wall_clock_slot_index(ANCHOR, now_ms) < 0);
    }
}
