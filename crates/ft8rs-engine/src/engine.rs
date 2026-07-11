//! Live monitor engine (P2).
//!
//! A controllable, long-lived engine the GUI drives over channels. Two actors:
//!
//! - **Decode actor** (own thread): holds the decode session for its whole life,
//!   processes staged decode commands (wsjtx nzhsym 41/47/50 early decode, others
//!   final-only), and can `Reconfigure` in place (rebuild session, migrate hash).
//! - **Engine/control thread**: owns the cpal capture + UTC-aligned slot timing,
//!   feeds staged samples to the Decode actor, routes its decode events to the GUI
//!   and the UDP sink, and applies reconfig plans at slot boundaries.
//!
//! Because the Decode actor is separate and long-lived, a device change (L2)
//! restarts only capture — the session and all decode state survive (decision 5).
//! Config changes (L1) reconfigure the actor at the next slot
//! boundary; the control loop polls commands every ~50 ms so Stop/Shutdown are
//! prompt even mid-slot (decode runs off-thread). The existing blocking CLI path
//! in `soundcard.rs` is untouched.
//!
//! Nothing here touches `lib_wsjtx`/`lib_jtdx`.

use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ft8rs::input::audio::resample_linear;
use ft8rs::stream::profile::ProfileStreamDecodeSession;
use ft8rs::stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedWithProvenance,
    StreamSlotDecodeState,
};
use ft8rs::SlotTimestamp;

use crate::protocol::{
    DecodeRecord, DecodeStage, DxContextSnapshot, EngineCommand, EngineEvent, EngineStatus,
    HisgridSource,
};
use crate::reconfig::{plan_reconfig, EngineState, StateBucket};
use crate::report::{UdpConfig, UdpOutput};
use crate::soundcard::{
    drain_pending_audio, list_soundcards, native_samples_for_nzhsym, next_slot_start_unix_seconds,
    now_unix_millis, start_input_stream, wall_clock_slot_index,
};

const TARGET_SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: u64 = 15;
/// Mirrors the CLI's per-slot focused-decode budget for DX live (`main.rs`).
const DX_MONITOR_WATCHDOG_MS: u64 = 12_000;
/// Command/event polling granularity inside the capture loop.
const POLL: Duration = Duration::from_millis(50);
/// Bound on the decode actor's staged-command backlog (~2.5 slots of the three
/// staged messages). If the decoder can't keep up with real time the producer
/// sheds whole slots rather than growing this queue without bound (each N50
/// stage carries a full ~720 KB slot buffer).
const DECODE_QUEUE_CAP: usize = 8;

/// Handle the GUI holds to drive the engine. Dropping it shuts the engine down.
pub struct EngineHandle {
    cmd_tx: Sender<EngineCommand>,
    event_rx: Receiver<EngineEvent>,
    join: Option<JoinHandle<()>>,
}

impl EngineHandle {
    /// Spawn the engine thread (starts Idle).
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
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
        self.event_rx.try_recv().ok()
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

fn engine_loop(cmd_rx: Receiver<EngineCommand>, event_tx: Sender<EngineEvent>) {
    let _ = event_tx.send(EngineEvent::Status(EngineStatus::Idle));
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            EngineCommand::Shutdown => return,
            // Idle: nothing to stop, and ApplyState has no session yet — the GUI
            // sends the full desired state with StartMonitor.
            EngineCommand::StopMonitor | EngineCommand::ApplyState(_) => {}
            EngineCommand::RefreshDevices => refresh_devices(&event_tx),
            EngineCommand::StartMonitor(state) => match run_monitor(&cmd_rx, &event_tx, state) {
                MonitorExit::Stopped => {
                    let _ = event_tx.send(EngineEvent::Status(EngineStatus::Idle));
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

#[derive(Clone, Copy)]
enum Flow {
    Ready,
    Stop,
    Shutdown,
    /// The capture stream died (device unplugged / timed out): try to reconnect.
    CaptureLost,
}

fn run_monitor(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &Sender<EngineEvent>,
    mut state: EngineState,
) -> MonitorExit {
    let mut udp = build_udp(state.udp.as_ref(), event_tx);

    let (mut stream, mut rx, mut sample_rate) = match start_input_stream(state.device.as_deref()) {
        Ok(parts) => parts,
        Err(err) => {
            let _ = event_tx.send(EngineEvent::Error(err));
            return MonitorExit::Stopped;
        }
    };

    let (dec_cmd_tx, dec_evt_rx) = spawn_decode_actor(effective_config(&state.config));

    let mut pending: Option<EngineState> = None;
    let _ = event_tx.send(EngineEvent::Status(EngineStatus::Aligning));
    let mut slot_start = match align(cmd_rx, event_tx, &mut pending) {
        AlignResult::Ready(start) => start,
        AlignResult::Stop => return teardown(dec_cmd_tx, stream),
        AlignResult::Shutdown => {
            let _ = dec_cmd_tx.send(DecodeCmd::Stop);
            drop(stream);
            return MonitorExit::Shutdown;
        }
    };
    drain_pending_audio(&rx);
    let mut carry: Vec<f32> = Vec::new();
    let _ = event_tx.send(EngineEvent::Status(EngineStatus::Monitoring));

    'monitor: loop {
        // Re-lock this slot's window start to the true UTC boundary (see
        // `wall_clock_slot_index`): counting `sample_rate * 15` samples per slot
        // lets soundcard drift / clock steps slide the decode window against real
        // transmissions until decodes are silently lost over long runs.
        let boundary = match now_unix_millis() {
            Ok(now_ms) => slot_start + wall_clock_slot_index(slot_start, now_ms).max(0) * 15,
            Err(err) => {
                let _ = event_tx.send(EngineEvent::Error(err));
                return teardown(dec_cmd_tx, stream);
            }
        };
        match wait_until_boundary(cmd_rx, event_tx, &mut pending, boundary) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return teardown(dec_cmd_tx, stream),
            CmdFlow::Shutdown => {
                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                drop(stream);
                return MonitorExit::Shutdown;
            }
        }
        drain_pending_audio(&rx);
        carry.clear();

        let timestamp = SlotTimestamp::from_unix_seconds_utc(boundary);
        let deadline = Instant::now() + Duration::from_secs(SLOT_SECONDS + 8);
        let mut native: Vec<f32> = Vec::new();
        // Set when the decode actor's bounded queue is full: shed the rest of this
        // slot's stages instead of blocking capture or growing the backlog.
        let mut slot_overrun = false;

        for (nzhsym, stage) in [(41, Stage::N41), (47, Stage::N47), (50, Stage::N50)] {
            let target = if nzhsym == 50 {
                sample_rate as usize * SLOT_SECONDS as usize
            } else {
                native_samples_for_nzhsym(sample_rate, nzhsym)
            };
            match pump_until(
                &rx,
                &mut carry,
                &mut native,
                target,
                deadline,
                cmd_rx,
                &dec_evt_rx,
                event_tx,
                &mut udp,
                &mut pending,
            ) {
                Flow::Ready => {}
                Flow::Stop => return teardown(dec_cmd_tx, stream),
                Flow::Shutdown => {
                    let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                    drop(stream);
                    return MonitorExit::Shutdown;
                }
                Flow::CaptureLost => {
                    drop(stream);
                    match reconnect_capture(cmd_rx, event_tx, state.device.as_deref(), &mut pending)
                    {
                        Reconnect::Ok(parts) => {
                            stream = parts.0;
                            rx = parts.1;
                            sample_rate = parts.2;
                            let _ = event_tx.send(EngineEvent::Status(EngineStatus::Aligning));
                            slot_start = match align(cmd_rx, event_tx, &mut pending) {
                                AlignResult::Ready(start) => start,
                                AlignResult::Stop => return teardown(dec_cmd_tx, stream),
                                AlignResult::Shutdown => {
                                    let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                                    drop(stream);
                                    return MonitorExit::Shutdown;
                                }
                            };
                            drain_pending_audio(&rx);
                            carry.clear();
                            let _ = event_tx.send(EngineEvent::Status(EngineStatus::Monitoring));
                            continue 'monitor;
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
                            let _ = event_tx.send(EngineEvent::Error(
                                "audio device unavailable; monitoring stopped".to_string(),
                            ));
                            let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                            return MonitorExit::Stopped;
                        }
                    }
                }
            }
            if slot_overrun {
                // Still pumped the audio above (keeps capture real-time); just
                // don't enqueue more work for the fallen-behind decoder.
                continue;
            }
            let samples_12k = resample_linear(&native, sample_rate, TARGET_SAMPLE_RATE);
            match dec_cmd_tx.try_send(DecodeCmd::Stage {
                stage,
                timestamp: timestamp.clone(),
                samples_12k,
            }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    slot_overrun = true;
                    let _ = event_tx.send(EngineEvent::Error(format!(
                        "decode falling behind real time; dropping slot {timestamp}"
                    )));
                }
                Err(TrySendError::Disconnected(_)) => {
                    drop(stream);
                    return MonitorExit::Stopped;
                }
            }
        }

        forward_decode_events(&dec_evt_rx, event_tx, &mut udp);

        // Apply a queued reconfiguration at the slot boundary.
        if let Some(mut new_state) = pending.take() {
            let mut outcome = plan_reconfig(&state, &new_state);
            if outcome.rebuild_output {
                udp = build_udp(new_state.udp.as_ref(), event_tx);
            }
            if outcome.rebuild_session {
                let _ = dec_cmd_tx.send(DecodeCmd::Reconfigure {
                    config: effective_config(&new_state.config),
                    reset_dx_target: outcome.reset.contains(&StateBucket::DxTarget),
                    reset_dx_operator: outcome.reset.contains(&StateBucket::DxOperator),
                });
            }
            if outcome.restart_capture {
                // Open the new device *before* dropping the working one: if it
                // can't open (busy / unplugged / unsupported format) keep the
                // current capture and session running instead of tearing the
                // whole monitor session down.
                match start_input_stream(new_state.device.as_deref()) {
                    Ok((new_stream, new_rx, new_rate)) => {
                        drop(stream);
                        stream = new_stream;
                        rx = new_rx;
                        sample_rate = new_rate;
                        let _ = event_tx.send(EngineEvent::Status(EngineStatus::Aligning));
                        slot_start = match align(cmd_rx, event_tx, &mut pending) {
                            AlignResult::Ready(start) => start,
                            AlignResult::Stop => return teardown(dec_cmd_tx, stream),
                            AlignResult::Shutdown => {
                                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                                drop(stream);
                                return MonitorExit::Shutdown;
                            }
                        };
                        drain_pending_audio(&rx);
                        carry.clear();
                        let _ = event_tx.send(EngineEvent::Status(EngineStatus::Monitoring));
                    }
                    Err(err) => {
                        let _ = event_tx.send(EngineEvent::Error(format!(
                            "device switch failed, keeping current input: {err}"
                        )));
                        // Stay on the current device and report no capture restart.
                        new_state.device = state.device.clone();
                        outcome.restart_capture = false;
                        outcome.reset.remove(&StateBucket::Capture);
                    }
                }
            }
            let _ = event_tx.send(EngineEvent::Reconfigured(outcome));
            state = new_state;
            continue;
        }
    }
}

fn teardown(dec_cmd_tx: SyncSender<DecodeCmd>, stream: cpal::Stream) -> MonitorExit {
    let _ = dec_cmd_tx.send(DecodeCmd::Stop);
    drop(stream);
    MonitorExit::Stopped
}

enum AlignResult {
    Ready(i64),
    Stop,
    Shutdown,
}

enum Reconnect {
    Ok((cpal::Stream, Receiver<Vec<f32>>, u32)),
    Stop,
    Shutdown,
    Failed,
}

/// Try to reopen the capture device after it was lost, keeping the decode actor
/// (and its session/DX intel) alive. Polls commands between attempts so Stop /
/// Shutdown stay responsive; gives up after a bounded number of tries.
fn reconnect_capture(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &Sender<EngineEvent>,
    device: Option<&str>,
    pending: &mut Option<EngineState>,
) -> Reconnect {
    let _ = event_tx.send(EngineEvent::Error(
        "audio device lost; reconnecting…".to_string(),
    ));
    for attempt in 0..30 {
        match drain_commands(cmd_rx, event_tx, pending) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return Reconnect::Stop,
            CmdFlow::Shutdown => return Reconnect::Shutdown,
        }
        if let Ok(parts) = start_input_stream(device) {
            return Reconnect::Ok(parts);
        }
        if attempt + 1 < 30 {
            thread::sleep(Duration::from_millis(400));
        }
    }
    Reconnect::Failed
}

/// Sleep (interruptibly) until a specific unix-second boundary, polling commands
/// so Stop/Shutdown stay responsive. Returns immediately if the boundary is
/// already in the past (slow device / decode lag). Used to re-lock each slot's
/// window to UTC without dropping whole slots in steady state.
fn wait_until_boundary(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &Sender<EngineEvent>,
    pending: &mut Option<EngineState>,
    boundary_secs: i64,
) -> CmdFlow {
    let target = UNIX_EPOCH + Duration::from_secs(boundary_secs.max(0) as u64);
    while let Ok(remaining) = target.duration_since(SystemTime::now()) {
        match drain_commands(cmd_rx, event_tx, pending) {
            CmdFlow::Continue => {}
            other => return other,
        }
        thread::sleep(remaining.min(POLL));
    }
    CmdFlow::Continue
}

/// Sleep (interruptibly) until the next UTC slot boundary, returning its unix
/// second. Polls the command channel so Stop/Shutdown stay responsive.
fn align(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &Sender<EngineEvent>,
    pending: &mut Option<EngineState>,
) -> AlignResult {
    let start = match next_slot_start_unix_seconds() {
        Ok(start) => start,
        Err(err) => {
            let _ = event_tx.send(EngineEvent::Error(err));
            return AlignResult::Stop;
        }
    };
    let target = UNIX_EPOCH + Duration::from_secs(start as u64);
    while let Ok(remaining) = target.duration_since(SystemTime::now()) {
        match drain_commands(cmd_rx, event_tx, pending) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return AlignResult::Stop,
            CmdFlow::Shutdown => return AlignResult::Shutdown,
        }
        thread::sleep(remaining.min(POLL));
    }
    AlignResult::Ready(start)
}

#[allow(clippy::too_many_arguments)]
fn pump_until(
    rx: &Receiver<Vec<f32>>,
    carry: &mut Vec<f32>,
    out: &mut Vec<f32>,
    target_len: usize,
    deadline: Instant,
    cmd_rx: &Receiver<EngineCommand>,
    dec_evt_rx: &Receiver<EngineEvent>,
    event_tx: &Sender<EngineEvent>,
    udp: &mut Option<UdpOutput>,
    pending: &mut Option<EngineState>,
) -> Flow {
    take_from_carry(carry, out, target_len);
    while out.len() < target_len {
        forward_decode_events(dec_evt_rx, event_tx, udp);
        match drain_commands(cmd_rx, event_tx, pending) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return Flow::Stop,
            CmdFlow::Shutdown => return Flow::Shutdown,
        }
        let now = Instant::now();
        if now >= deadline {
            // No audio for a whole slot+: treat the capture as lost and reconnect.
            return Flow::CaptureLost;
        }
        let timeout = deadline.saturating_duration_since(now).min(POLL);
        match rx.recv_timeout(timeout) {
            Ok(chunk) => append_with_carry(out, carry, target_len, chunk),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Flow::CaptureLost,
        }
    }
    forward_decode_events(dec_evt_rx, event_tx, udp);
    Flow::Ready
}

enum CmdFlow {
    Continue,
    Stop,
    Shutdown,
}

fn drain_commands(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &Sender<EngineEvent>,
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
    event_tx: &Sender<EngineEvent>,
    udp: &mut Option<UdpOutput>,
) {
    while let Ok(evt) = dec_evt_rx.try_recv() {
        if let EngineEvent::Decode(record) = &evt {
            if let Some(sink) = udp.as_ref() {
                let _ = sink.on_decode(record.timestamp.clone(), &record.row);
            }
        }
        let _ = event_tx.send(evt);
    }
}

fn refresh_devices(event_tx: &Sender<EngineEvent>) {
    match list_soundcards() {
        Ok(devices) => {
            let _ = event_tx.send(EngineEvent::DevicesRefreshed(devices));
        }
        Err(err) => {
            let _ = event_tx.send(EngineEvent::Error(err));
        }
    }
}

fn build_udp(config: Option<&UdpConfig>, event_tx: &Sender<EngineEvent>) -> Option<UdpOutput> {
    let config = config?;
    match UdpOutput::new(config.clone()) {
        Ok(sink) => Some(sink),
        Err(err) => {
            // Output failure must not stop decode: disable sink.
            let _ = event_tx.send(EngineEvent::Error(format!("UDP disabled: {err}")));
            None
        }
    }
}

fn effective_config(config: &StreamDecodeConfig) -> StreamDecodeConfig {
    let mut config = config.clone();
    if config.profile == DecodeProfile::Dx {
        config.dx_monitor_watchdog_ms = Some(DX_MONITOR_WATCHDOG_MS);
    }
    config
}

fn take_from_carry(carry: &mut Vec<f32>, out: &mut Vec<f32>, target_len: usize) {
    if carry.is_empty() || out.len() >= target_len {
        return;
    }
    let needed = target_len - out.len();
    if carry.len() <= needed {
        out.extend_from_slice(carry);
        carry.clear();
    } else {
        out.extend_from_slice(&carry[..needed]);
        carry.drain(..needed);
    }
}

fn append_with_carry(out: &mut Vec<f32>, carry: &mut Vec<f32>, target_len: usize, chunk: Vec<f32>) {
    let remaining = target_len - out.len();
    if chunk.len() <= remaining {
        out.extend_from_slice(&chunk);
    } else {
        out.extend_from_slice(&chunk[..remaining]);
        carry.extend_from_slice(&chunk[remaining..]);
    }
}

// ── Decode actor ────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Stage {
    N41,
    N47,
    N50,
}

enum DecodeCmd {
    Stage {
        stage: Stage,
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
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

fn spawn_decode_actor(
    config: StreamDecodeConfig,
) -> (SyncSender<DecodeCmd>, Receiver<EngineEvent>) {
    // Bounded so a decoder that falls behind real time sheds slots (see the
    // producer's `try_send`) instead of growing an unbounded backlog of slot
    // buffers. Stop/Reconfigure use a blocking `send`; they are rare and must
    // not be dropped, and only ever wait for one in-flight slot to drain.
    let (cmd_tx, cmd_rx) = mpsc::sync_channel::<DecodeCmd>(DECODE_QUEUE_CAP);
    let (evt_tx, evt_rx) = mpsc::channel::<EngineEvent>();
    thread::spawn(move || decode_actor(config, cmd_rx, evt_tx));
    (cmd_tx, evt_rx)
}

fn decode_actor(
    config: StreamDecodeConfig,
    cmd_rx: Receiver<DecodeCmd>,
    evt_tx: Sender<EngineEvent>,
) {
    let mut session = build_session(&config);
    // wsjtx staged state plus the count of early rows already emitted at nzhsym=41.
    let mut slot_state: Option<(StreamSlotDecodeState, usize)> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            DecodeCmd::Stop => break,
            DecodeCmd::Reconfigure {
                config,
                reset_dx_target,
                reset_dx_operator,
            } => {
                let calls = export_hash(&session);
                session = reconfigure_session(session, config, reset_dx_target, reset_dx_operator);
                import_hash(&mut session, &calls);
                slot_state = None;
            }
            DecodeCmd::Stage {
                stage,
                timestamp,
                samples_12k,
            } => match (&mut session, stage) {
                (DecodeSession::Wsjtx(s), Stage::N41) => {
                    let mut state = s.start_slot_decode();
                    match s.decode_slot_nzhsym41_with_provenance_at(
                        Some(&timestamp),
                        &mut state,
                        &samples_12k,
                    ) {
                        Ok(early) => {
                            for row in &early {
                                send_record(&evt_tx, &timestamp, row, DecodeStage::Early);
                            }
                            slot_state = Some((state, early.len()));
                        }
                        Err(err) => {
                            let _ = evt_tx.send(EngineEvent::Error(err));
                        }
                    }
                }
                (DecodeSession::Wsjtx(s), Stage::N47) => {
                    if let Some((state, _)) = slot_state.as_mut() {
                        s.subtract_slot_nzhsym47(state, &samples_12k);
                    }
                }
                (DecodeSession::Wsjtx(s), Stage::N50) => {
                    if let Some((state, early_count)) = slot_state.take() {
                        match s.decode_slot_nzhsym50_and_finish_with_provenance(state, &samples_12k)
                        {
                            Ok(all) => {
                                for row in all.iter().skip(early_count) {
                                    send_record(&evt_tx, &timestamp, row, DecodeStage::Final);
                                }
                                let _ = evt_tx.send(EngineEvent::SlotComplete {
                                    timestamp,
                                    count: all.len(),
                                });
                            }
                            Err(err) => {
                                let _ = evt_tx.send(EngineEvent::Error(err));
                            }
                        }
                    }
                }
                (DecodeSession::Profile(_), Stage::N41)
                | (DecodeSession::Profile(_), Stage::N47) => {}
                (DecodeSession::Profile(p), Stage::N50) => {
                    // Stream rows as the core produces them (hybrid emits the
                    // WSJT-X pass first, then JTDX) so early decodes show without
                    // waiting for the deep pass to finish.
                    let result = p.decode_slot_streaming_with_provenance_at(
                        &timestamp,
                        &samples_12k,
                        |row| {
                            send_record(&evt_tx, &timestamp, row, DecodeStage::Final);
                            Ok(())
                        },
                    );
                    match result {
                        Ok(count) => {
                            if let Some(snapshot) = p.dx_context_snapshot() {
                                let _ =
                                    evt_tx.send(EngineEvent::DxContext(map_dx_snapshot(snapshot)));
                            }
                            let _ = evt_tx.send(EngineEvent::SlotComplete { timestamp, count });
                        }
                        Err(err) => {
                            let _ = evt_tx.send(EngineEvent::Error(err));
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
    use std::path::Path;

    const NZHSYM_STRIDE: usize = 3456;
    const SHORT_FIXTURE: &str = "../ft8rs-core/tests/ft8/210703_133430.wav";

    fn load_fixture_12k() -> Option<Vec<f32>> {
        if !Path::new(SHORT_FIXTURE).exists() {
            eprintln!("skipping decode-actor test: fixture {SHORT_FIXTURE} not present");
            return None;
        }
        let audio = ft8rs::input::audio::read_wav_mono_f32(SHORT_FIXTURE).ok()?;
        Some(resample_linear(
            &audio.samples,
            audio.sample_rate,
            TARGET_SAMPLE_RATE,
        ))
    }

    fn feed_slot(cmd_tx: &SyncSender<DecodeCmd>, ts: &SlotTimestamp, s12k: &[f32]) {
        let slice = |n: usize| s12k[..(n * NZHSYM_STRIDE).min(s12k.len())].to_vec();
        for (n, stage) in [(41, Stage::N41), (47, Stage::N47), (50, Stage::N50)] {
            let samples = if n == 50 { s12k.to_vec() } else { slice(n) };
            cmd_tx
                .send(DecodeCmd::Stage {
                    stage,
                    timestamp: ts.clone(),
                    samples_12k: samples,
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
        let Some(s12k) = load_fixture_12k() else {
            return;
        };
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
        let mut jtdx = StreamDecodeConfig::default();
        jtdx.profile = DecodeProfile::Jtdx;
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
}
