//! Live monitor engine.
//!
//! A controllable, long-lived engine the GUI drives over channels. Two actors:
//!
//! - **Decode actor** (supervised child process): holds its session while healthy,
//!   processes staged decode commands (WSJT-X/Hybrid nzhsym 41/47/50 early
//!   decode; JTDX/DX final-only), and can `Reconfigure` in place (rebuild session, migrate hash).
//! - **Engine/control thread**: owns the cpal capture + UTC-aligned slot timing,
//!   feeds staged samples to the Decode actor, routes its decode events to the GUI
//!   and the UDP sink, and applies reconfig plans at slot boundaries.
//!
//! Because the Decode actor is separate and long-lived, a device change (L2)
//! restarts only capture — the session and all decode state survive.
//! Config changes (L1) reconfigure the actor at the next slot
//! boundary; the control loop polls commands every ~50 ms so Stop/Shutdown are
//! prompt even mid-slot. A ten-second cumulative per-slot watchdog kills and
//! reaps overrunning native work, then starts the next slot with a fresh session.
//! The existing blocking CLI path
//! in `soundcard.rs` is untouched.
//!
//! Nothing here touches `lib_wsjtx`/`lib_jtdx`.

use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(test)]
use ft8rs::input::audio::resample_linear;
use ft8rs::stream::profile::{
    A7Snapshot, JtdxRestore, ProfileSlotState, ProfileStreamDecodeSession, SlotRecovery,
};
use ft8rs::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedWithProvenance,
    StreamSlotDecodeState,
};
use ft8rs::SlotTimestamp;

use crate::events::GuiEvents;
use crate::protocol::{
    CapturedSlot, DecodeRecord, DecodeStage, DxContextSnapshot, EngineCommand, EngineEvent,
    EngineStatus, HisgridSource, MonitorObserver,
};
use crate::reconfig::{plan_reconfig, EngineState, StateBucket};
use crate::report::{UdpConfig, UdpOutput};
use crate::soundcard::{
    list_soundcards, peak_level, start_input_stream, AudioCapture, InputChannel, PeriodAccumulator,
    SlotStage,
};

// Only the test helpers reference this now; production goes through
// `downsample_12k`, which knows the 12 kHz target itself.
#[cfg(test)]
const TARGET_SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: u64 = 15;
/// Mirrors the CLI's per-slot focused-decode budget for DX live (`main.rs`).
const DX_MONITOR_WATCHDOG_MS: u64 = 12_000;
/// Command/event polling granularity inside the capture loop.
const POLL: Duration = Duration::from_millis(50);
/// Bound on staged commands; `send_stage` waits while continuing to poll controls.
pub(crate) const DECODE_QUEUE_CAP: usize = 8;

/// Handle the GUI holds to drive the engine. Dropping it shuts the engine down.
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
    event_rx: GuiEvents,
    join: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Spawn the engine thread (starts Idle).
    pub fn spawn() -> Self {
        Self::spawn_inner(None)
    }

    /// Spawn the engine with a diagnostics observer upstream of the GUI queue.
    pub fn spawn_observed(observer: Arc<dyn MonitorObserver>) -> Self {
        Self::spawn_inner(Some(observer))
    }

    fn spawn_inner(observer: Option<Arc<dyn MonitorObserver>>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let event_rx = GuiEvents::new(observer);
        let event_tx = event_rx.clone();
        let join = thread::spawn(move || engine_loop(cmd_rx, event_tx));
        Self {
            cmd_tx,
            event_rx,
            join: Some(join),
        }
    }

    /// Send a command to the engine.
    pub fn send(&self, cmd: EngineCommand) -> Result<(), String> {
        self.cmd_tx.send(cmd).map_err(|err| err.to_string())
    }

    /// Non-blocking poll for the next engine event (call each GUI frame).
    pub fn try_recv(&self) -> Option<EngineEvent> {
        self.event_rx.try_recv()
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(EngineCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

// ── Engine control thread ───────────────────────────────────────────────────

fn engine_loop(cmd_rx: Receiver<EngineCommand>, event_tx: GuiEvents) {
    event_tx.send(EngineEvent::Status(EngineStatus::Idle));
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            EngineCommand::Shutdown => return,
            // Idle: nothing to stop, and ApplyState has no session yet — the GUI
            // sends the full desired state with StartMonitor.
            EngineCommand::StopMonitor | EngineCommand::ApplyState(_) => {}
            EngineCommand::RefreshDevices => refresh_devices(&event_tx),
            EngineCommand::StartMonitor(state) => match run_monitor(&cmd_rx, &event_tx, state) {
                MonitorExit::Stopped => {
                    event_tx.send(EngineEvent::Status(EngineStatus::Idle));
                }
                MonitorExit::Shutdown => return,
            },
        }
    }
}

enum MonitorExit {
    Stopped,
    Shutdown,
}

fn run_monitor(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &GuiEvents,
    mut state: EngineState,
) -> MonitorExit {
    let mut udp = build_udp(state.udp.as_ref(), event_tx);

    let AudioCapture {
        mut stream,
        mut rx,
        mut format,
        mut device_name,
    } = match start_input_stream(state.device.as_deref(), state.channel) {
        Ok(parts) => parts,
        Err(err) => {
            event_tx.send(EngineEvent::Error(err));
            return MonitorExit::Stopped;
        }
    };
    let mut sample_rate = format.sample_rate;
    event_tx.send(EngineEvent::CaptureStarted {
        device_name: device_name.clone(),
        format: format.clone(),
    });

    let mut config = effective_config(&state.config);
    let (dec_cmd_tx, dec_evt_rx) = crate::worker::spawn(config.clone());

    // Whole callback blocks are assigned to UTC slots by callback wall-clock
    // time and appended by sample count; there is no sample-level boundary split.
    let mut acc = PeriodAccumulator::new(sample_rate);
    let mut aligned = false;
    let mut pending: Option<EngineState> = None;
    let mut last_audio = Instant::now();
    let mut last_level = Instant::now();
    let mut channel_peaks = vec![0.0f32; format.channels as usize];
    let mut input_peak = 0.0f32;
    event_tx.send(EngineEvent::Status(EngineStatus::Monitoring));

    loop {
        match drain_commands(cmd_rx, event_tx, &mut pending) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return teardown(dec_cmd_tx, stream),
            CmdFlow::Shutdown => {
                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                drop(stream);
                return MonitorExit::Shutdown;
            }
        }
        forward_decode_events(&dec_evt_rx, event_tx, &mut udp);

        match rx.recv_timeout(POLL) {
            Ok(chunk) => {
                if let Some(err) = chunk.error {
                    event_tx.send(EngineEvent::Error(err));
                    return teardown(dec_cmd_tx, stream);
                }
                last_audio = Instant::now();
                for (peak, value) in channel_peaks.iter_mut().zip(&chunk.channel_peaks) {
                    *peak = peak.max(*value);
                }
                input_peak = input_peak.max(peak_level(&chunk.samples));
                if last_level.elapsed() >= Duration::from_millis(100) {
                    event_tx.send(EngineEvent::CaptureLevels {
                        channel_peaks: channel_peaks.clone(),
                        peak: input_peak,
                        xruns: chunk.xruns,
                    });
                    channel_peaks.fill(0.0);
                    input_peak = 0.0;
                    last_level = Instant::now();
                }
                let submitted = capture_stage_submitted(
                    Instant::now(),
                    crate::soundcard::capture_now_ms(),
                    chunk.capture_ms,
                );
                let out = acc.push(&chunk.samples, chunk.capture_ms);
                for (stage, timestamp, samples_12k) in out.stages {
                    if stage == SlotStage::N50 && aligned {
                        if let Some(diag) = &out.completed {
                            event_tx.observe_slot(CapturedSlot {
                                timestamp: &timestamp,
                                samples_12k: &samples_12k,
                                config: &config,
                                device_name: &device_name,
                                format: &format,
                                channel: state.channel,
                                native_samples: diag.samples,
                                xruns: chunk.xruns,
                                jitter_ms: diag.max_jitter_ms,
                            });
                        }
                    }
                    // Wait for decode-queue space rather than dropping the slot (the
                    // old shedding could drop *every* slot on DAX). Command handling
                    // stays responsive while waiting; capture buffers in the channel.
                    match send_stage(
                        &dec_cmd_tx,
                        DecodeCmd::Stage {
                            stage,
                            timestamp,
                            samples_12k,
                            submitted,
                            recovery_deadline: None,
                            restore_a7: None,
                            restore_jtdx: [None, None],
                            supervised: false,
                        },
                        cmd_rx,
                        &dec_evt_rx,
                        event_tx,
                        &mut udp,
                        &mut pending,
                    ) {
                        StageSend::Sent => {}
                        StageSend::Stop => return teardown(dec_cmd_tx, stream),
                        StageSend::Shutdown => {
                            let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                            drop(stream);
                            return MonitorExit::Shutdown;
                        }
                        StageSend::ActorGone => {
                            drop(stream);
                            return MonitorExit::Stopped;
                        }
                    }
                }
                if let Some(diag) = out.completed {
                    // Diagnostics require a decodable slot; reconfiguration only
                    // requires a UTC boundary, including a discarded first slot.
                    event_tx.send(EngineEvent::InputLevel(diag.peak));
                    event_tx.send(EngineEvent::CaptureDiag {
                        samples: diag.samples,
                        blocks: diag.blocks,
                        avg_block: diag.avg_block,
                        jitter_ms: diag.max_jitter_ms,
                        sample_rate,
                    });
                }
                if out.boundary_crossed {
                    aligned = true;
                    if let Some(mut new_state) = pending.take() {
                        let mut outcome = plan_reconfig(&state, &new_state);
                        if outcome.rebuild_output {
                            udp = build_udp(new_state.udp.as_ref(), event_tx);
                        }
                        if outcome.rebuild_session {
                            config = effective_config(&new_state.config);
                            let _ = dec_cmd_tx.send(DecodeCmd::Reconfigure {
                                config: config.clone(),
                                reset_dx_target: outcome.reset.contains(&StateBucket::DxTarget),
                                reset_dx_operator: outcome.reset.contains(&StateBucket::DxOperator),
                            });
                        }
                        if outcome.restart_capture {
                            // Open the new device *before* dropping the working one, so
                            // a failed switch keeps the current capture running.
                            match start_input_stream(new_state.device.as_deref(), new_state.channel)
                            {
                                Ok(capture) => {
                                    let _ = dec_cmd_tx.send(DecodeCmd::InvalidateA7Checkpoint);
                                    drop(stream);
                                    stream = capture.stream;
                                    rx = capture.rx;
                                    format = capture.format;
                                    device_name = capture.device_name;
                                    sample_rate = format.sample_rate;
                                    channel_peaks = vec![0.0; format.channels as usize];
                                    input_peak = 0.0;
                                    event_tx.send(EngineEvent::CaptureStarted {
                                        device_name: device_name.clone(),
                                        format: format.clone(),
                                    });
                                    acc = PeriodAccumulator::new(sample_rate);
                                    aligned = false;
                                    last_audio = Instant::now();
                                }
                                Err(err) => {
                                    event_tx.send(EngineEvent::Error(format!(
                                        "device switch failed, keeping current input: {err}"
                                    )));
                                    event_tx.send(EngineEvent::CaptureSelectionRejected {
                                        requested_device: new_state.device.clone(),
                                        requested_channel: new_state.channel,
                                        active_device: state.device.clone(),
                                        active_channel: state.channel,
                                    });
                                    new_state.device = state.device.clone();
                                    new_state.channel = state.channel;
                                    outcome.restart_capture = false;
                                    outcome.reset.remove(&StateBucket::Capture);
                                }
                            }
                        }
                        event_tx.send(EngineEvent::Reconfigured(outcome));
                        state = new_state;
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {
                // A live device keeps delivering; a slot-plus of silence means the
                // capture died (unplugged / a virtual driver that stopped) — fall
                // through to reconnect. Otherwise just poll again.
                if last_audio.elapsed() < Duration::from_secs(SLOT_SECONDS + 8) {
                    continue;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {} // stream died → reconnect
        }

        // Capture lost: reopen and re-arm the accumulator (it self-aligns).
        drop(stream);
        match reconnect_capture(
            cmd_rx,
            event_tx,
            state.device.as_deref(),
            state.channel,
            &mut pending,
        ) {
            Reconnect::Ok {
                capture,
                device,
                channel,
            } => {
                let _ = dec_cmd_tx.send(DecodeCmd::InvalidateA7Checkpoint);
                let mut active = state.clone();
                active.device = device;
                active.channel = channel;
                event_tx.send(EngineEvent::Reconfigured(plan_reconfig(&state, &active)));
                state = active;
                stream = capture.stream;
                rx = capture.rx;
                format = capture.format;
                device_name = capture.device_name;
                sample_rate = format.sample_rate;
                channel_peaks = vec![0.0; format.channels as usize];
                input_peak = 0.0;
                event_tx.send(EngineEvent::CaptureStarted {
                    device_name: device_name.clone(),
                    format: format.clone(),
                });
                acc = PeriodAccumulator::new(sample_rate);
                aligned = false;
                last_audio = Instant::now();
                event_tx.send(EngineEvent::Status(EngineStatus::Monitoring));
            }
            Reconnect::Stop => {
                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                return MonitorExit::Stopped;
            }
            Reconnect::Shutdown => {
                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                return MonitorExit::Shutdown;
            }
            Reconnect::Failed => {
                event_tx.send(EngineEvent::Error(
                    "audio device unavailable; monitoring stopped".to_string(),
                ));
                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                return MonitorExit::Stopped;
            }
        }
    }
}

fn teardown(dec_cmd_tx: SyncSender<DecodeCmd>, stream: cpal::Stream) -> MonitorExit {
    let _ = dec_cmd_tx.send(DecodeCmd::Stop);
    drop(stream);
    MonitorExit::Stopped
}

enum Reconnect<T = AudioCapture> {
    Ok {
        capture: T,
        device: Option<String>,
        channel: InputChannel,
    },
    Stop,
    Shutdown,
    Failed,
}

/// Try to reopen the capture device after it was lost, keeping the decode actor
/// (and its session/DX intel) alive. Polls commands between attempts so Stop /
/// Shutdown stay responsive; gives up after a bounded number of tries.
fn reconnect_capture(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &GuiEvents,
    device: Option<&str>,
    channel: InputChannel,
    pending: &mut Option<EngineState>,
) -> Reconnect {
    reconnect_capture_with(
        cmd_rx,
        event_tx,
        device,
        channel,
        pending,
        start_input_stream,
        || thread::sleep(Duration::from_millis(400)),
    )
}

fn reconnect_capture_with<T>(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &GuiEvents,
    device: Option<&str>,
    channel: InputChannel,
    pending: &mut Option<EngineState>,
    mut open: impl FnMut(Option<&str>, InputChannel) -> Result<T, String>,
    mut pause: impl FnMut(),
) -> Reconnect<T> {
    event_tx.send(EngineEvent::Error(
        "audio device lost; reconnecting…".to_string(),
    ));
    for attempt in 0..30 {
        match drain_commands(cmd_rx, event_tx, pending) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return Reconnect::Stop,
            CmdFlow::Shutdown => return Reconnect::Shutdown,
        }
        let (device, channel) = pending
            .as_ref()
            .map(|state| (state.device.as_deref(), state.channel))
            .unwrap_or((device, channel));
        if let Ok(capture) = open(device, channel) {
            return Reconnect::Ok {
                capture,
                device: device.map(str::to_owned),
                channel,
            };
        }
        if attempt + 1 < 30 {
            pause();
        }
    }
    Reconnect::Failed
}

/// Outcome of handing a staged decode command to the actor.
enum StageSend {
    Sent,
    Stop,
    Shutdown,
    /// The decode actor thread is gone (channel disconnected).
    ActorGone,
}

/// Enqueue a decode stage, waiting for queue space if the actor is momentarily
/// behind rather than dropping the slot. While waiting it keeps forwarding decode
/// events and processing control commands, so Stop/Shutdown/reconfig stay
/// responsive and the GUI keeps updating. Audio buffers in the unbounded capture
/// channel meanwhile; the worker includes queue age in its staged-work deadline.
#[allow(clippy::too_many_arguments)]
fn send_stage(
    dec_cmd_tx: &SyncSender<DecodeCmd>,
    cmd: DecodeCmd,
    cmd_rx: &Receiver<EngineCommand>,
    dec_evt_rx: &Receiver<EngineEvent>,
    event_tx: &GuiEvents,
    udp: &mut Option<UdpOutput>,
    pending: &mut Option<EngineState>,
) -> StageSend {
    let mut cmd = cmd;
    loop {
        match dec_cmd_tx.try_send(cmd) {
            Ok(()) => return StageSend::Sent,
            Err(TrySendError::Disconnected(_)) => return StageSend::ActorGone,
            Err(TrySendError::Full(returned)) => {
                cmd = returned;
                forward_decode_events(dec_evt_rx, event_tx, udp);
                match drain_commands(cmd_rx, event_tx, pending) {
                    CmdFlow::Continue => {}
                    CmdFlow::Stop => return StageSend::Stop,
                    CmdFlow::Shutdown => return StageSend::Shutdown,
                }
                thread::sleep(POLL);
            }
        }
    }
}

enum CmdFlow {
    Continue,
    Stop,
    Shutdown,
}

fn drain_commands(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &GuiEvents,
    pending: &mut Option<EngineState>,
) -> CmdFlow {
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Shutdown) => return CmdFlow::Shutdown,
            Ok(EngineCommand::StopMonitor) => return CmdFlow::Stop,
            // Both ApplyState and a StartMonitor while running update the desired
            // state, applied at the next slot boundary.
            Ok(EngineCommand::ApplyState(state)) | Ok(EngineCommand::StartMonitor(state)) => {
                *pending = Some(state)
            }
            Ok(EngineCommand::RefreshDevices) => refresh_devices(event_tx),
            Err(TryRecvError::Empty) => return CmdFlow::Continue,
            Err(TryRecvError::Disconnected) => return CmdFlow::Shutdown,
        }
    }
}

/// Forward decode-actor events to the GUI, feeding decode rows to the UDP sink.
/// Sink failures are isolated (logged-and-dropped), never stopping decode.
fn forward_decode_events(
    dec_evt_rx: &Receiver<EngineEvent>,
    event_tx: &GuiEvents,
    udp: &mut Option<UdpOutput>,
) {
    while let Ok(evt) = dec_evt_rx.try_recv() {
        if let EngineEvent::Decode(record) = &evt {
            if let Some(sink) = udp.as_ref() {
                sink.on_decode_best_effort(record.timestamp.clone(), &record.row);
            }
        }
        event_tx.send(evt);
    }
}

fn refresh_devices(event_tx: &GuiEvents) {
    log::info!("Refreshing audio input devices");
    match list_soundcards() {
        Ok(devices) => {
            log::info!(
                "Audio enumeration completed: {} input devices",
                devices.len()
            );
            event_tx.send(EngineEvent::DevicesRefreshed(devices));
        }
        Err(err) => {
            log::error!("Audio enumeration failed: {err}");
            event_tx.send(EngineEvent::Error(err));
        }
    }
}

fn build_udp(config: Option<&UdpConfig>, event_tx: &GuiEvents) -> Option<UdpOutput> {
    let config = config?;
    match UdpOutput::new(config.clone()) {
        Ok(sink) => Some(sink),
        Err(err) => {
            // Output failure must not stop decode: disable sink.
            event_tx.send(EngineEvent::Error(format!("UDP disabled: {err}")));
            None
        }
    }
}

fn capture_stage_submitted(now: Instant, now_ms: i64, capture_ms: i64) -> Instant {
    let age = Duration::from_millis(now_ms.saturating_sub(capture_ms).max(0) as u64);
    now.checked_sub(age).unwrap_or(now)
}

fn effective_config(config: &StreamDecodeConfig) -> StreamDecodeConfig {
    let mut config = config.clone();
    config.cooperative_realtime = true;
    if config.profile == DecodeProfile::Dx {
        config.dx_monitor_watchdog_ms = Some(DX_MONITOR_WATCHDOG_MS);
    }
    config
}

// ── Decode actor ────────────────────────────────────────────────────────────

pub(crate) enum DecodeCmd {
    Stage {
        stage: SlotStage,
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
        submitted: Instant,
        recovery_deadline: Option<Instant>,
        restore_a7: Option<A7Snapshot>,
        restore_jtdx: [Option<JtdxRestore>; 2],
        supervised: bool,
    },
    InvalidateA7Checkpoint,
    Barrier(Sender<()>),
    Reconfigure {
        config: StreamDecodeConfig,
        reset_dx_target: bool,
        reset_dx_operator: bool,
    },
    Stop,
}

/// wsjtx uses the staged `StreamDecodeSession` (early decode); other profiles use
/// the unified `ProfileStreamDecodeSession` (final decode only) — mirroring the
/// shipped soundcard worker exactly, so decode behavior is unchanged.
#[expect(
    clippy::large_enum_variant,
    reason = "These long-lived sessions retain their existing inline ownership and construction"
)]
enum DecodeSession {
    Wsjtx(StreamDecodeSession),
    Profile(ProfileStreamDecodeSession),
}

fn build_session(config: &StreamDecodeConfig) -> DecodeSession {
    if config.profile == DecodeProfile::Wsjtx {
        DecodeSession::Wsjtx(StreamDecodeSession::new(config.clone()))
    } else {
        DecodeSession::Profile(ProfileStreamDecodeSession::new(config.clone()))
    }
}

/// Rebuild the decode session for a new config, carrying forward DX intel where
/// valid (dx → dx). wsjtx uses the staged session; everything else the unified
/// one. The DX carry-over honors the reconfig plan's reset flags.
fn reconfigure_session(
    old: DecodeSession,
    config: StreamDecodeConfig,
    reset_dx_target: bool,
    reset_dx_operator: bool,
) -> DecodeSession {
    if config.profile == DecodeProfile::Wsjtx {
        return DecodeSession::Wsjtx(StreamDecodeSession::new(config));
    }
    match old {
        DecodeSession::Profile(profile) => {
            DecodeSession::Profile(profile.reconfigure(config, reset_dx_target, reset_dx_operator))
        }
        DecodeSession::Wsjtx(_) => DecodeSession::Profile(ProfileStreamDecodeSession::new(config)),
    }
}

fn export_hash(session: &DecodeSession) -> Vec<String> {
    match session {
        DecodeSession::Wsjtx(s) => s.export_regular_hash_calls(),
        DecodeSession::Profile(p) => p.export_hash_calls(),
    }
}

fn import_hash(session: &mut DecodeSession, calls: &[String]) {
    match session {
        DecodeSession::Wsjtx(s) => s.import_hash_calls(calls),
        DecodeSession::Profile(p) => p.import_hash_calls(calls),
    }
}

fn map_dx_snapshot(snapshot: ft8rs::decode::dx::DxSnapshot) -> DxContextSnapshot {
    use ft8rs::decode::dx::HisgridSource as Core;
    let hisgrid_source = match snapshot.hisgrid_source {
        Core::None => None,
        Core::User => Some(HisgridSource::User),
        Core::Harvested => Some(HisgridSource::Harvested),
    };
    DxContextSnapshot {
        target: snapshot.target,
        foci: snapshot.foci,
        tx_parity: snapshot.tx_parity,
        hisgrid: snapshot.hisgrid,
        hisgrid_source,
        dt: snapshot.dt,
    }
}

fn send_record(
    evt_tx: &Sender<EngineEvent>,
    timestamp: &SlotTimestamp,
    row: &StreamDecodedWithProvenance,
    stage: DecodeStage,
) {
    let _ = evt_tx.send(EngineEvent::Decode(DecodeRecord {
        timestamp: timestamp.clone(),
        row: row.decode.clone(),
        provenance: row.provenance,
        stage,
    }));
}

#[cfg(test)]
fn pause_fixture_after_confirmation(timestamp: &SlotTimestamp, message: &str) {
    if timestamp.format() == "230208_140300"
        && std::env::var("FT8RS_WORKER_TEST_FIXTURE")
            .ok()
            .and_then(|mode| mode.strip_prefix("pause-baseline:").map(str::to_owned))
            .as_deref()
            == Some(message)
    {
        loop {
            thread::park();
        }
    }
}

#[cfg(test)]
fn spawn_decode_actor(
    config: StreamDecodeConfig,
) -> (SyncSender<DecodeCmd>, Receiver<EngineEvent>) {
    // Test-only in-process actor queue. Tests feed with blocking sends;
    // production uses the supervised worker and responsive `send_stage` loop.
    let (cmd_tx, cmd_rx) = mpsc::sync_channel::<DecodeCmd>(DECODE_QUEUE_CAP);
    let (evt_tx, evt_rx) = mpsc::channel::<EngineEvent>();
    thread::spawn(move || decode_actor(config, cmd_rx, evt_tx));
    (cmd_tx, evt_rx)
}

pub(crate) fn decode_actor(
    config: StreamDecodeConfig,
    cmd_rx: Receiver<DecodeCmd>,
    evt_tx: Sender<EngineEvent>,
) {
    let mut experiment_enabled = config.hybrid_experiment;
    let mut session = build_session(&config);
    // Classic WSJT-X preserves staged audio and duplicate tracking through N50.
    let mut slot_state: Option<StreamSlotDecodeState> = None;
    // JTDX/DX only run their searches at N50; their sessions persist across slots.
    let mut profile_slot: Option<(ProfileSlotState, usize)> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            DecodeCmd::Stop => break,
            DecodeCmd::InvalidateA7Checkpoint => {}
            DecodeCmd::Barrier(done) => {
                let _ = done.send(());
            }
            DecodeCmd::Reconfigure {
                config,
                reset_dx_target,
                reset_dx_operator,
            } => {
                experiment_enabled = config.hybrid_experiment;
                let calls = export_hash(&session);
                session = reconfigure_session(session, config, reset_dx_target, reset_dx_operator);
                import_hash(&mut session, &calls);
                slot_state = None;
                profile_slot = None;
            }
            DecodeCmd::Stage {
                stage,
                timestamp,
                samples_12k,
                submitted: _,
                recovery_deadline,
                restore_a7,
                restore_jtdx,
                supervised,
            } => match (&mut session, stage) {
                (DecodeSession::Wsjtx(s), SlotStage::N41) => {
                    if let Some(snapshot) = restore_a7 {
                        s.restore_a7(&timestamp, snapshot);
                    }
                    let mut state = s.start_slot_decode();
                    match s.decode_slot_nzhsym41_tagged(
                        Some(&timestamp),
                        &mut state,
                        &samples_12k,
                        |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Early);
                            Ok(())
                        },
                    ) {
                        Ok(()) => slot_state = Some(state),
                        Err(err) => {
                            let _ = evt_tx.send(EngineEvent::Error(err));
                        }
                    }
                }
                (DecodeSession::Wsjtx(s), SlotStage::N47) => {
                    if let Some(state) = slot_state.as_mut() {
                        s.subtract_slot_nzhsym47(state, &samples_12k);
                    }
                }
                (DecodeSession::Wsjtx(s), SlotStage::N50) => {
                    if let Some(state) = slot_state.take() {
                        match s.decode_slot_nzhsym50_tagged(state, &samples_12k, |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Final);
                            #[cfg(test)]
                            pause_fixture_after_confirmation(&timestamp, &row.decode.msg);
                            Ok(())
                        }) {
                            Ok(count) => {
                                let _ = evt_tx.send(EngineEvent::SlotComplete { timestamp, count });
                            }
                            Err(err) => {
                                let _ = evt_tx.send(EngineEvent::Error(err));
                            }
                        }
                    }
                }
                (DecodeSession::Profile(p), SlotStage::N41) => {
                    // Hybrid streams early WSJT-X rows here (before the slot
                    // boundary); jtdx/dx are no-ops that just open a stateless slot.
                    if let Some(snapshot) = restore_a7 {
                        p.restore_a7(&timestamp, snapshot);
                    }
                    p.restore_jtdx(restore_jtdx);
                    let mut state = p.start_slot();
                    match p.decode_slot_nzhsym41_streaming_with_provenance(
                        &timestamp,
                        &mut state,
                        &samples_12k,
                        |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Early);
                            Ok(())
                        },
                    ) {
                        Ok(early_count) => profile_slot = Some((state, early_count)),
                        Err(err) => {
                            let _ = evt_tx.send(EngineEvent::Error(err));
                        }
                    }
                }
                (DecodeSession::Profile(p), SlotStage::N47) => {
                    if let Some((state, _)) = profile_slot.as_mut() {
                        p.subtract_slot_nzhsym47(state, &samples_12k);
                    }
                }
                (DecodeSession::Profile(p), SlotStage::N50) => {
                    // Finish the slot: hybrid emits the remaining WSJT-X rows then
                    // JTDX-unique rows; jtdx/dx run their full decode here.
                    if let Some((state, _)) = profile_slot.take() {
                        let mut checkpoint = |snapshot| {
                            let _ = evt_tx.send(EngineEvent::A7Checkpoint {
                                timestamp: timestamp.clone(),
                                snapshot,
                            });
                        };
                        let mut jtdx_checkpoint = |branch, update, publish| {
                            #[cfg(test)]
                            let message = match &update {
                                ft8rs::stream::profile::JtdxCheckpoint::Row(entry) => {
                                    Some(entry.row.decode.msg.clone())
                                }
                                _ => None,
                            };
                            let _ = evt_tx.send(EngineEvent::JtdxCheckpoint {
                                timestamp: timestamp.clone(),
                                branch,
                                update,
                                publish,
                            });
                            #[cfg(test)]
                            if let Some(message) = message {
                                pause_fixture_after_confirmation(&timestamp, &message);
                            }
                        };
                        let result = p.finish_slot_nzhsym50(
                            &timestamp,
                            state,
                            &samples_12k,
                            SlotRecovery {
                                deadline: recovery_deadline,
                                checkpoint: supervised.then_some(&mut checkpoint),
                                jtdx_checkpoint: supervised.then_some(&mut jtdx_checkpoint),
                            },
                            |row| {
                                send_record(&evt_tx, &timestamp, row, DecodeStage::Final);
                                #[cfg(test)]
                                pause_fixture_after_confirmation(&timestamp, &row.decode.msg);
                                Ok(())
                            },
                        );
                        if let ProfileStreamDecodeSession::Hybrid(session) = p {
                            log::info!(
                                "Hybrid slot {}: experiment_enabled={} samples={} diagnostics={:?}",
                                timestamp.format_time(),
                                experiment_enabled,
                                samples_12k.len(),
                                session.last_diagnostics().experiment,
                            );
                        }
                        match result {
                            Ok(count) => {
                                if let Some(snapshot) = p.dx_context_snapshot() {
                                    let _ = evt_tx
                                        .send(EngineEvent::DxContext(map_dx_snapshot(snapshot)));
                                }
                                let _ = evt_tx.send(EngineEvent::SlotComplete { timestamp, count });
                            }
                            Err(err) => {
                                let _ = evt_tx.send(EngineEvent::Error(err));
                            }
                        }
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NZHSYM_STRIDE: usize = 3456;
    const SHORT_FIXTURE: &str = "../ft8rs-core/tests/ft8/210703_133430.wav";

    fn load_fixture_12k() -> Vec<f32> {
        let audio = ft8rs::input::audio::read_wav_mono_f32(SHORT_FIXTURE)
            .expect("versioned short recording must be readable");
        resample_linear(&audio.samples, audio.sample_rate, TARGET_SAMPLE_RATE)
    }

    #[test]
    fn reconnect_uses_latest_selection_received_between_attempts() {
        let (tx, rx) = mpsc::channel();
        let events = GuiEvents::default();
        let mut pending = None;
        let mut attempts = Vec::new();
        let result = reconnect_capture_with(
            &rx,
            &events,
            Some("old"),
            InputChannel::Left,
            &mut pending,
            |device, channel| {
                attempts.push((device.map(str::to_owned), channel));
                if attempts.len() == 1 {
                    tx.send(EngineCommand::ApplyState(EngineState {
                        device: Some("new".into()),
                        channel: InputChannel::Right,
                        config: StreamDecodeConfig::default(),
                        udp: None,
                    }))
                    .unwrap();
                    Err("old device unavailable".into())
                } else {
                    Ok(())
                }
            },
            || {},
        );
        assert_eq!(
            attempts,
            vec![
                (Some("old".into()), InputChannel::Left),
                (Some("new".into()), InputChannel::Right)
            ]
        );
        assert!(
            matches!(result, Reconnect::Ok {device:Some(device),channel:InputChannel::Right,..} if device=="new")
        );
        assert!(
            pending.is_some(),
            "decode/output config remains pending until a boundary"
        );
    }

    #[test]
    fn capture_queue_age_is_included_in_the_decode_deadline() {
        let now = Instant::now();
        assert_eq!(
            capture_stage_submitted(now, 25_000, 10_000),
            now - Duration::from_secs(15)
        );
        assert_eq!(capture_stage_submitted(now, 10_000, 25_000), now);
    }

    fn feed_slot(cmd_tx: &SyncSender<DecodeCmd>, ts: &SlotTimestamp, s12k: &[f32]) {
        let slice = |n: usize| s12k[..(n * NZHSYM_STRIDE).min(s12k.len())].to_vec();
        for (n, stage) in [
            (41, SlotStage::N41),
            (47, SlotStage::N47),
            (50, SlotStage::N50),
        ] {
            let samples = if n == 50 { s12k.to_vec() } else { slice(n) };
            cmd_tx
                .send(DecodeCmd::Stage {
                    stage,
                    timestamp: ts.clone(),
                    samples_12k: samples,
                    submitted: Instant::now(),
                    recovery_deadline: None,
                    restore_a7: None,
                    restore_jtdx: [None, None],
                    supervised: false,
                })
                .unwrap();
        }
    }

    fn drain_until_slot_complete(rx: &Receiver<EngineEvent>) -> (usize, usize) {
        let mut decodes = 0usize;
        loop {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(EngineEvent::Decode(_)) => decodes += 1,
                Ok(EngineEvent::SlotComplete { count, .. }) => return (decodes, count),
                Ok(EngineEvent::Error(err)) => panic!("decode actor error: {err}"),
                Ok(_) => {}
                Err(err) => panic!("timed out waiting for slot complete: {err}"),
            }
        }
    }

    #[test]
    fn decode_actor_reproduces_wsjtx_short_slot_and_reconfigures() {
        let s12k = load_fixture_12k();
        let ts = SlotTimestamp::parse("210703_133430").unwrap();

        let (cmd_tx, evt_rx) = spawn_decode_actor(StreamDecodeConfig::default());

        // wsjtx staged decode of the short fixture.
        feed_slot(&cmd_tx, &ts, &s12k);
        let (_early_or_final, count) = drain_until_slot_complete(&evt_rx);
        assert!(
            count >= 15,
            "wsjtx short slot decoded too few rows: {count}"
        );

        // Reconfigure to jtdx in place (hash migrated) and decode again — the
        // actor must survive the rebuild and keep producing.
        let jtdx = StreamDecodeConfig {
            profile: DecodeProfile::Jtdx,
            ..Default::default()
        };
        cmd_tx
            .send(DecodeCmd::Reconfigure {
                config: jtdx,
                reset_dx_target: false,
                reset_dx_operator: false,
            })
            .unwrap();
        feed_slot(&cmd_tx, &ts, &s12k);
        let (_d2, count2) = drain_until_slot_complete(&evt_rx);
        assert!(
            count2 >= 15,
            "jtdx short slot decoded too few rows: {count2}"
        );

        cmd_tx.send(DecodeCmd::Stop).unwrap();
    }

    /// The hybrid decode actor (staged N41/N47/N50), over the LONG fixture where
    /// hybrid genuinely exceeds the WSJT-X baseline (jtdx-unique rows exist), must
    /// emit the same total as a one-shot hybrid decode — i.e. it must not drop
    /// jtdx-unique rows. The short fixture can't catch this (there hybrid==wsjtx,
    /// jtdx fully overlaps). Slow; run with --ignored.
    #[test]
    #[ignore = "slow long-fixture actor gate; run with --release --ignored"]
    fn decode_actor_hybrid_long_emits_jtdx_unique() {
        use ft8rs::stream::profile::ProfileStreamDecodeSession;
        const LONG_FIXTURE: &str = "../ft8rs-core/tests/ft8/230208_140300.wav";
        let audio = ft8rs::input::audio::read_wav_mono_f32(LONG_FIXTURE).unwrap();
        let s12k = resample_linear(&audio.samples, audio.sample_rate, TARGET_SAMPLE_RATE);
        let sps = 15 * TARGET_SAMPLE_RATE as usize;
        let nseg = s12k.len().div_ceil(sps);
        let start = SlotTimestamp::parse("230208_140300").unwrap();
        let hyb = || StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            ..Default::default()
        };

        let mut oneshot = ProfileStreamDecodeSession::new(hyb());
        let mut oneshot_total = 0usize;
        for seg in 0..nseg {
            let ts = start.add_seconds((seg * 15) as i64);
            let data = &s12k[seg * sps..((seg + 1) * sps).min(s12k.len())];
            oneshot_total += oneshot.decode_slot_at(&ts, data).len();
        }

        let (cmd_tx, evt_rx) = spawn_decode_actor(hyb());
        let mut actor_emitted = 0usize;
        for seg in 0..nseg {
            let ts = start.add_seconds((seg * 15) as i64);
            let data = &s12k[seg * sps..((seg + 1) * sps).min(s12k.len())];
            feed_slot(&cmd_tx, &ts, data);
            let (emitted, _count) = drain_until_slot_complete(&evt_rx);
            actor_emitted += emitted;
        }
        cmd_tx.send(DecodeCmd::Stop).unwrap();

        assert!(
            oneshot_total > 424,
            "sanity: long hybrid should exceed wsjtx baseline (jtdx-unique exists), got {oneshot_total}"
        );
        assert_eq!(
            actor_emitted, oneshot_total,
            "hybrid actor emitted {actor_emitted} rows over the long fixture but one-shot has {oneshot_total} (jtdx-unique dropped in the staged engine path?)"
        );
    }

    /// The hybrid decode actor (staged N41/N47/N50) must emit the same rows as a
    /// one-shot hybrid decode of the same slot — in particular it must not drop
    /// JTDX-unique rows. Guards the monitor early-decode staging.
    #[test]
    fn decode_actor_hybrid_matches_oneshot() {
        use ft8rs::stream::profile::ProfileStreamDecodeSession;
        let s12k = load_fixture_12k();
        let ts = SlotTimestamp::parse("210703_133430").unwrap();
        let hybrid_cfg = || StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            ..Default::default()
        };

        // Reference: one-shot hybrid decode (file path) of the whole slot.
        let mut oneshot = ProfileStreamDecodeSession::new(hybrid_cfg());
        let oneshot_count = oneshot.decode_slot_at(&ts, &s12k).len();

        // Actor: staged N41/N47/N50 exactly as the monitor engine drives it.
        let (cmd_tx, evt_rx) = spawn_decode_actor(hybrid_cfg());
        feed_slot(&cmd_tx, &ts, &s12k);
        let (emitted, count) = drain_until_slot_complete(&evt_rx);
        cmd_tx.send(DecodeCmd::Stop).unwrap();

        assert_eq!(
            count, oneshot_count,
            "hybrid actor slot count {count} != one-shot {oneshot_count} (jtdx-unique dropped?)"
        );
        assert_eq!(
            emitted, oneshot_count,
            "hybrid actor emitted {emitted} rows but slot count is {oneshot_count}"
        );
    }
}
