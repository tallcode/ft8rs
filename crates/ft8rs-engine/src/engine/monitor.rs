//! Capture control, UTC alignment, and live reconfiguration at slot boundaries.
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};

use crate::events::GuiEvents;
use crate::protocol::{CapturedSlot, EngineCommand, EngineEvent, EngineStatus};
use crate::reconfig::{plan_reconfig, EngineState, StateBucket};
use crate::report::{UdpConfig, UdpOutput};
use crate::soundcard::{
    capture_now_ms, peak_level, start_input_stream, AudioCapture, CaptureRecvError, InputChannel,
    PeriodAccumulator, SlotStage, CAPTURE_TIMEOUT,
};

use super::{refresh_devices, DecodeCmd};

const SLOT_SECONDS: u64 = 15;
const SLOT_MS: i64 = SLOT_SECONDS as i64 * 1000;
const CLOCK_STEP_MS: u64 = 100;
/// Mirrors the CLI's per-slot focused-decode budget for DX live (`main.rs`).
const DX_MONITOR_WATCHDOG_MS: u64 = 12_000;
/// Command/event polling granularity inside the capture loop.
const POLL: Duration = Duration::from_millis(50);

pub(super) enum MonitorExit {
    Stopped,
    CaptureFailed(String),
    Shutdown,
}

#[derive(Default)]
struct PendingChanges {
    state: Option<EngineState>,
    clock: Option<(Instant, Instant)>,
}

#[derive(Default)]
pub(super) struct CaptureClock {
    previous: Option<(Instant, i64)>,
    last_step: Option<Instant>,
    next_boundary_ms: Option<i64>,
}

pub(super) enum ClockAction {
    Accept,
    Wait,
    Restart,
    Aligned,
}

impl CaptureClock {
    pub(super) fn observe(
        &mut self,
        captured_at: Instant,
        wall_ms: i64,
        reported: &mut Option<(Instant, Instant)>,
    ) -> ClockAction {
        if reported.is_some_and(|(started, _)| self.last_step.is_some_and(|step| step >= started)) {
            *reported = None;
        }
        if reported.is_some_and(|(_, completed)| captured_at < completed) {
            return ClockAction::Wait;
        }
        let previous = self.previous.replace((captured_at, wall_ms));
        let external_step = previous.is_some_and(|(before, previous_wall)| {
            let elapsed = captured_at.saturating_duration_since(before).as_millis() as i64;
            wall_ms.saturating_sub(previous_wall).abs_diff(elapsed) >= CLOCK_STEP_MS
        });
        if reported.take().is_some() || external_step {
            self.last_step = Some(captured_at);
            self.next_boundary_ms = Some((wall_ms.div_euclid(SLOT_MS) + 1) * SLOT_MS);
            return ClockAction::Restart;
        }
        match self.next_boundary_ms {
            Some(boundary) if wall_ms < boundary => ClockAction::Wait,
            Some(_) => {
                self.next_boundary_ms = None;
                ClockAction::Aligned
            }
            None => ClockAction::Accept,
        }
    }
}

pub(super) fn run_monitor(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &GuiEvents,
    id: u64,
    state: EngineState,
) -> MonitorExit {
    run_monitor_with(cmd_rx, event_tx, id, state, start_input_stream)
}

fn run_monitor_with<S>(
    cmd_rx: &Receiver<EngineCommand>,
    event_tx: &GuiEvents,
    id: u64,
    mut state: EngineState,
    mut open_input: impl FnMut(Option<&str>, InputChannel) -> Result<AudioCapture<S>, String>,
) -> MonitorExit {
    let mut udp = build_udp(state.udp.as_ref(), event_tx);

    let AudioCapture {
        mut stream,
        mut rx,
        mut format,
        mut device_name,
    } = match open_input(state.device.as_deref(), state.channel) {
        Ok(parts) => parts,
        Err(err) => return MonitorExit::CaptureFailed(err),
    };
    let mut sample_rate = format.sample_rate;
    event_tx.send(EngineEvent::CaptureStarted {
        device_name: device_name.clone(),
        format: format.clone(),
    });

    let mut config = effective_config(&state.config);
    let (mut dec_cmd_tx, mut dec_evt_rx) = crate::worker::spawn(config.clone());

    // Whole callback blocks are assigned to UTC slots by callback wall-clock
    // time and appended by sample count; there is no sample-level boundary split.
    let mut acc = PeriodAccumulator::new(sample_rate);
    let mut aligned = false;
    let mut capture_clock = CaptureClock::default();
    let mut pending = PendingChanges::default();
    let mut last_audio = Instant::now();
    let mut last_level = Instant::now();
    let mut channel_peaks = vec![0.0f32; format.channels as usize];
    let mut input_peak = 0.0f32;
    event_tx.send(EngineEvent::Status {
        id,
        status: EngineStatus::Monitoring,
    });

    'monitor: loop {
        match drain_commands(cmd_rx, event_tx, &mut pending) {
            CmdFlow::Continue => {}
            CmdFlow::Stop => return teardown(dec_cmd_tx, stream),
            CmdFlow::Shutdown => {
                let _ = dec_cmd_tx.send(DecodeCmd::Stop);
                drop(stream);
                return MonitorExit::Shutdown;
            }
        }
        if pending.clock.is_none() {
            forward_decode_events(&dec_evt_rx, event_tx, &mut udp);
        }

        match rx.recv_timeout(POLL) {
            Ok(chunk) => {
                last_audio = Instant::now();
                match capture_clock.observe(chunk.captured_at, chunk.capture_ms, &mut pending.clock)
                {
                    ClockAction::Accept => {}
                    ClockAction::Wait => continue,
                    ClockAction::Restart => {
                        // Disconnection preempts the old worker and its queued
                        // work. This keeps old AP/history and late rows out of
                        // the new UTC timeline while the audio stream stays open.
                        (dec_cmd_tx, dec_evt_rx) = crate::worker::spawn(config.clone());
                        acc = PeriodAccumulator::new(sample_rate);
                        aligned = false;
                        event_tx.send(EngineEvent::ClockDiscontinuity {
                            at_ms: chunk.capture_ms,
                            observed: chunk.captured_at,
                        });
                        event_tx.send(EngineEvent::Status {
                            id,
                            status: EngineStatus::Aligning,
                        });
                        continue;
                    }
                    ClockAction::Aligned => {
                        aligned = true;
                        event_tx.send(EngineEvent::Status {
                            id,
                            status: EngineStatus::Monitoring,
                        });
                    }
                }
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
                let submitted =
                    capture_stage_submitted(Instant::now(), capture_now_ms(), chunk.capture_ms);
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
                        StageSend::ClockAdjusted => continue 'monitor,
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
                    if let Some(mut new_state) = pending.state.take() {
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
                            match open_input(new_state.device.as_deref(), new_state.channel) {
                                Ok(capture) => {
                                    let _ = dec_cmd_tx.send(DecodeCmd::InvalidateWorkerHistory);
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
                                        id,
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
            Err(CaptureRecvError::Timeout) => {
                if last_audio.elapsed() < CAPTURE_TIMEOUT {
                    continue;
                }
                return MonitorExit::CaptureFailed(format!("No audio received from {device_name}"));
            }
            Err(CaptureRecvError::Disconnected) => {
                return MonitorExit::CaptureFailed(format!(
                    "Audio input disconnected: {device_name}"
                ));
            }
            Err(CaptureRecvError::Failed(error)) => return MonitorExit::CaptureFailed(error),
        }
    }
}

fn teardown<S>(dec_cmd_tx: SyncSender<DecodeCmd>, stream: S) -> MonitorExit {
    let _ = dec_cmd_tx.send(DecodeCmd::Stop);
    drop(stream);
    MonitorExit::Stopped
}

/// Outcome of handing a staged decode command to the actor.
enum StageSend {
    Sent,
    ClockAdjusted,
    Stop,
    Shutdown,
    /// The decode actor thread is gone (channel disconnected).
    ActorGone,
}

/// Enqueue a decode stage, waiting for queue space if the actor is momentarily
/// behind rather than dropping the slot. While waiting it keeps forwarding decode
/// events and processing control commands, so Stop/Shutdown/reconfig stay
/// responsive and the GUI keeps updating. The worker includes queue age in its
/// staged-work deadline.
#[allow(clippy::too_many_arguments)]
fn send_stage(
    dec_cmd_tx: &SyncSender<DecodeCmd>,
    cmd: DecodeCmd,
    cmd_rx: &Receiver<EngineCommand>,
    dec_evt_rx: &Receiver<EngineEvent>,
    event_tx: &GuiEvents,
    udp: &mut Option<UdpOutput>,
    pending: &mut PendingChanges,
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
                if pending.clock.is_some() {
                    return StageSend::ClockAdjusted;
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
    pending: &mut PendingChanges,
) -> CmdFlow {
    loop {
        match cmd_rx.try_recv() {
            Ok(EngineCommand::Shutdown) => return CmdFlow::Shutdown,
            Ok(EngineCommand::StopMonitor) => return CmdFlow::Stop,
            // Both ApplyState and a StartMonitor while running update the desired
            // state, applied at the next slot boundary.
            Ok(EngineCommand::ApplyState(state))
            | Ok(EngineCommand::StartMonitor { state, .. }) => pending.state = Some(state),
            Ok(EngineCommand::ClockAdjusted { started, completed }) => {
                pending.clock = Some((started, completed));
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

pub(super) fn capture_stage_submitted(now: Instant, now_ms: i64, capture_ms: i64) -> Instant {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundcard::failed_capture;
    use std::cell::Cell;
    use std::sync::mpsc;

    #[test]
    fn device_callback_failure_stops_monitoring_and_releases_the_input() {
        struct InputGuard<'a>(&'a Cell<bool>);
        impl Drop for InputGuard<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }

        let (_commands, receiver) = mpsc::channel();
        let events = GuiEvents::default();
        let released = Cell::new(false);
        let mut stream = Some(InputGuard(&released));
        let result = run_monitor_with(
            &receiver,
            &events,
            1,
            EngineState {
                device: None,
                channel: InputChannel::Mono,
                config: StreamDecodeConfig::default(),
                udp: None,
            },
            |_, _| {
                Ok(failed_capture(
                    stream
                        .take()
                        .expect("capture must wait for a new start command"),
                    cpal::Error::with_message(
                        cpal::ErrorKind::DeviceNotAvailable,
                        "input unplugged",
                    ),
                ))
            },
        );

        assert!(
            matches!(result, MonitorExit::CaptureFailed(reason) if reason.contains("input unplugged"))
        );
        assert!(released.get());
    }
}
