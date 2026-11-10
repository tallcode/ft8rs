//! Control handle and command routing for the live GUI monitor.
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::events::GuiEvents;
use crate::protocol::{EngineCommand, EngineEvent, EngineStatus, MonitorObserver};
use crate::soundcard::list_soundcards;

mod actor;
mod monitor;
#[cfg(test)]
mod tests;

pub(crate) use actor::{decode_actor, DecodeCmd};

use monitor::{run_monitor, MonitorExit};

/// Bound on staged commands; the monitor waits while continuing to poll controls.
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

    pub fn clock_adjustment_reporter(&self) -> impl FnOnce(Instant, Instant) + Send + 'static {
        let commands = self.cmd_tx.clone();
        move |started, completed| {
            let _ = commands.send(EngineCommand::ClockAdjusted { started, completed });
        }
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
    event_tx.send(EngineEvent::Status {
        id: 0,
        status: EngineStatus::Idle,
    });
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            EngineCommand::Shutdown => return,
            // Idle: nothing to stop, and ApplyState has no session yet — the GUI
            // sends the full desired state with StartMonitor.
            EngineCommand::StopMonitor | EngineCommand::ApplyState(_) => {}
            EngineCommand::ClockAdjusted { completed, .. } => {
                event_tx.send(EngineEvent::ClockDiscontinuity {
                    at_ms: crate::soundcard::capture_now_ms(),
                    observed: completed,
                });
            }
            EngineCommand::RefreshDevices => refresh_devices(&event_tx),
            EngineCommand::StartMonitor { id, state } => {
                let status = match run_monitor(&cmd_rx, &event_tx, id, state) {
                    MonitorExit::Stopped => EngineStatus::Idle,
                    MonitorExit::CaptureFailed(error) => {
                        log::warn!("Audio monitoring stopped: {error}");
                        EngineStatus::Error(error)
                    }
                    MonitorExit::Shutdown => return,
                };
                event_tx.send(EngineEvent::Status { id, status });
            }
        }
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
