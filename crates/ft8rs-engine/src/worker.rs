//! Live worker supervision, stage deadlines, and process replacement.
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ft8rs::{DecodeProfile, StreamDecodeConfig};

use crate::engine::{DecodeCmd, DECODE_QUEUE_CAP};
#[cfg(test)]
use crate::protocol::DecodeStage;
use crate::protocol::EngineEvent;
use crate::reconfig::{config_diff, ConfigField};
use crate::soundcard::SlotStage;
#[cfg(test)]
use ft8rs::stream::profile::JtdxCheckpoint;

mod child;
mod delivery;
mod ipc;
mod process;
#[cfg(test)]
mod tests;

pub use child::run_decode_worker_if_requested;
#[cfg(test)]
pub(crate) use tests::pause_fixture_after_confirmation;

use delivery::{parity, SlotDelivery, WorkerHistory};
use ipc::{Request, Response};
use process::Worker;

const WORKER_ARGUMENT: &str = "--ft8rs-decode-worker";
const LIVE_BUDGET: Duration = Duration::from_secs(10);
const RECOVERY_HEADROOM: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(5);

pub(crate) fn spawn(config: StreamDecodeConfig) -> (SyncSender<DecodeCmd>, Receiver<EngineEvent>) {
    let (commands, input) = mpsc::sync_channel(DECODE_QUEUE_CAP);
    let (output, events) = mpsc::channel();
    thread::spawn(move || supervise(config, input, output, Worker::start, LIVE_BUDGET));
    (commands, events)
}

fn supervise(
    mut config: StreamDecodeConfig,
    commands: Receiver<DecodeCmd>,
    events: Sender<EngineEvent>,
    mut start: impl FnMut() -> Result<Worker, String>,
    budget: Duration,
) {
    let mut worker: Option<Worker> = None;
    let mut pending = VecDeque::new();
    let mut sequence = 0;
    let mut slot = None;
    let mut work = Duration::ZERO;
    let mut aborted = None;
    let mut delivery = SlotDelivery::default();
    let mut history = WorkerHistory::default();
    loop {
        let command = match pending.pop_front().or_else(|| commands.recv().ok()) {
            Some(c) => c,
            None => return,
        };
        let (mut request, deadline, timestamp) = match command {
            DecodeCmd::Stop => return,
            DecodeCmd::Barrier(done) => {
                let _ = done.send(());
                continue;
            }
            DecodeCmd::InvalidateWorkerHistory => {
                history = WorkerHistory::default();
                continue;
            }
            DecodeCmd::Reconfigure {
                config: next,
                reset_dx_target,
                reset_dx_operator,
            } => {
                // A new receive target does not invalidate observations at
                // the frequency and time stored in the native A7 table.
                let changed = config_diff(&config, &next);
                let keep_a7 = matches!(
                    config.profile,
                    DecodeProfile::Wsjtx | DecodeProfile::WsjtxPlus | DecodeProfile::Hybrid
                ) && !changed.is_empty()
                    && changed.iter().all(|field| {
                        matches!(
                            field,
                            ConfigField::Nfqso | ConfigField::HisCall | ConfigField::HisGrid
                        )
                    });
                let a7 = keep_a7.then(|| std::mem::take(&mut history.a7));
                config = next;
                history = WorkerHistory::default();
                if let Some(a7) = a7 {
                    history.a7 = a7;
                    if let Some(child) = &mut worker {
                        child.restore_pending = [true; 2];
                    }
                }
                sequence += 1;
                (
                    Request::Reconfigure {
                        id: sequence,
                        config: config.clone(),
                        target: reset_dx_target,
                        operator: reset_dx_operator,
                    },
                    Instant::now() + budget,
                    None,
                )
            }
            DecodeCmd::Stage {
                stage,
                timestamp,
                samples_12k,
                submitted,
                ..
            } => {
                if aborted.as_ref() == Some(&timestamp) {
                    continue;
                }
                if slot.as_ref() != Some(&timestamp) {
                    slot = Some(timestamp.clone());
                    work = Duration::ZERO;
                    delivery = SlotDelivery::default();
                }
                sequence += 1;
                let now = Instant::now();
                let deadline = (now + budget.saturating_sub(work)).min(submitted + budget);
                let recovery_deadline = (stage == SlotStage::N50
                    && matches!(
                        config.profile,
                        DecodeProfile::Hybrid | DecodeProfile::WsjtxPlus
                    ))
                .then(|| {
                    SystemTime::now()
                        + deadline
                            .saturating_duration_since(now)
                            .saturating_sub(RECOVERY_HEADROOM)
                });
                (
                    Request::Stage {
                        id: sequence,
                        stage,
                        timestamp: timestamp.format(),
                        samples: samples_12k,
                        recovery_deadline,
                        restore_a7: None,
                        restore_jtdx: [None, None],
                    },
                    deadline,
                    Some(timestamp),
                )
            }
        };
        let started = Instant::now();
        let mut deadline_expired = false;
        let result = (|| -> Result<(), String> {
            if started >= deadline {
                deadline_expired = true;
                return Err("live decode deadline expired in queue".into());
            }
            if worker.is_none() {
                let child = start()?;
                child.send(Request::Init(config.clone()))?;
                worker = Some(child);
            }
            let child = worker.as_mut().unwrap();
            if let (
                Some(timestamp),
                Request::Stage {
                    stage: SlotStage::N41,
                    restore_a7,
                    restore_jtdx,
                    ..
                },
            ) = (&timestamp, &mut request)
            {
                let index = parity(timestamp);
                if config.profile == DecodeProfile::Hybrid
                    || std::mem::take(&mut child.restore_pending[index])
                {
                    *restore_a7 = history.a7[index]
                        .as_ref()
                        .filter(|(previous, _)| previous.add_seconds(30) == *timestamp)
                        .map(|(_, snapshot)| snapshot.clone());
                }
                if std::mem::take(&mut child.restore_jtdx_pending) {
                    *restore_jtdx = history.jtdx_for_restart(timestamp);
                }
            }
            child.send(request)?;
            loop {
                // Stop/Shutdown must preempt even a blocked native FEC call.
                loop {
                    match commands.try_recv() {
                        Ok(DecodeCmd::Stop) | Err(mpsc::TryRecvError::Disconnected) => {
                            return Err("monitor stopped".into());
                        }
                        Ok(command) => pending.push_back(command),
                        Err(mpsc::TryRecvError::Empty) => break,
                    }
                }
                if started.elapsed() >= budget || Instant::now() >= deadline {
                    deadline_expired = true;
                    return Err("live decode exceeded the 10-second budget".into());
                }
                match child
                    .output
                    .recv_timeout(POLL.min(deadline.saturating_duration_since(Instant::now())))
                {
                    Ok(Ok(Response::Done(id))) if id == sequence => return Ok(()),
                    Ok(Ok(value)) => {
                        #[cfg(test)]
                        let force_timeout = match &value {
                            Response::A7Checkpoint { timestamp, .. } => {
                                child.kill_at_checkpoint == Some(timestamp.as_str())
                            }
                            Response::Decode {
                                message,
                                stage: DecodeStage::Final,
                                ..
                            } => child.kill_at_baseline_row == Some(message.as_str()),
                            Response::JtdxCheckpoint {
                                update: JtdxCheckpoint::Row(entry),
                                ..
                            } => child.kill_at_baseline_row == Some(entry.row.decode.msg.as_str()),
                            _ => false,
                        };
                        delivery.forward(value, &mut history, &config, &events)?;
                        #[cfg(test)]
                        if force_timeout {
                            deadline_expired = true;
                            return Err("test forced a decoder timeout".into());
                        }
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err("decoder worker disconnected".into())
                    }
                }
            }
        })();
        if timestamp.is_some() {
            work += started.elapsed();
        }
        if let Err(error) = result {
            if error == "monitor stopped" {
                return;
            }
            if let Some(mut child) = worker.take() {
                // Reap the process before draining. The reader finishes at EOF,
                // so only complete frames already written by this worker survive.
                child.stop();
                for frame in child.output.try_iter().filter_map(Result::ok) {
                    if delivery
                        .forward(frame, &mut history, &config, &events)
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if let Some(timestamp) = &timestamp {
                log::warn!(
                    "slot {}: {error}; worker terminated and reaped before next slot",
                    timestamp.format()
                );
            } else {
                log::warn!("{error}; worker terminated and reaped before next slot");
            }
            // A deadline is normal live scheduling: keep the diagnostic log
            // without showing a GUI error. Actual worker failures still surface.
            if !deadline_expired {
                let _ = events.send(EngineEvent::Error(error));
            }
            if let Some(timestamp) = timestamp {
                aborted = Some(timestamp.clone());
                if !delivery.complete {
                    let _ = events.send(EngineEvent::SlotComplete {
                        timestamp,
                        count: delivery.rows.len(),
                    });
                }
            }
        }
    }
}
