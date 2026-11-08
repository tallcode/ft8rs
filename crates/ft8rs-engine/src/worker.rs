//! Killable live-decoder boundary. Native algorithms run unchanged in a child
//! process; the capture/control thread never waits for native worker threads.
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use ft8rs::stream::profile::{same_signal, A7Snapshot, JtdxBranch, JtdxCheckpoint, JtdxRestore};
use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::DecodeProfile;
use ft8rs::{SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};
use serde::{Deserialize, Serialize};

use crate::engine::{decode_actor, DecodeCmd, DECODE_QUEUE_CAP};
use crate::protocol::{DecodeRecord, DecodeStage, DxContextSnapshot, EngineEvent};
use crate::soundcard::SlotStage;

const WORKER_ARGUMENT: &str = "--ft8rs-decode-worker";
const PREFIX: &str = "FT8RS-WORKER ";
const LIVE_BUDGET: Duration = Duration::from_secs(10);
const RECOVERY_HEADROOM: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(5);

#[derive(Serialize, Deserialize)]
enum Request {
    Init(StreamDecodeConfig),
    Stage {
        id: u64,
        stage: SlotStage,
        timestamp: String,
        samples: Vec<f32>,
        recovery_deadline: Option<SystemTime>,
        restore_a7: Option<A7Snapshot>,
        restore_jtdx: [Option<JtdxRestore>; 2],
    },
    Reconfigure {
        id: u64,
        config: StreamDecodeConfig,
        target: bool,
        operator: bool,
    },
}

#[derive(Serialize, Deserialize)]
enum Response {
    Decode {
        timestamp: String,
        freq: f64,
        dt: f64,
        snr: f64,
        sync: f64,
        message: String,
        tones: Vec<i32>,
        provenance: ft8rs::stream::session::StreamDecodeProvenance,
        stage: DecodeStage,
    },
    SlotComplete {
        timestamp: String,
        count: usize,
    },
    A7Checkpoint {
        timestamp: String,
        snapshot: A7Snapshot,
    },
    JtdxCheckpoint {
        timestamp: String,
        branch: JtdxBranch,
        update: JtdxCheckpoint,
        publish: bool,
    },
    Dx(DxContextSnapshot),
    Error(String),
    Log(String),
    Done(u64),
}

fn write_frame(output: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    let encoded = ron::to_string(value).map_err(|e| e.to_string())?;
    writeln!(output, "{PREFIX}{encoded}")
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}

fn read_frame<T: for<'a> Deserialize<'a>>(input: &mut impl BufRead) -> Result<Option<T>, String> {
    loop {
        let mut line = String::new();
        input.read_line(&mut line).map_err(|e| e.to_string())?;
        if !line.ends_with('\n') {
            return Ok(None);
        }
        if let Some(frame) = line.strip_prefix(PREFIX) {
            return ron::from_str(frame.trim_end())
                .map(Some)
                .map_err(|e| e.to_string());
        }
    }
}

fn response(event: EngineEvent) -> Option<Response> {
    Some(match event {
        EngineEvent::Decode(record) => Response::Decode {
            timestamp: record.timestamp.format(),
            freq: record.row.freq,
            dt: record.row.dt,
            snr: record.row.snr,
            sync: record.row.sync,
            message: record.row.msg,
            tones: record.row.itone.to_vec(),
            provenance: record.provenance,
            stage: record.stage,
        },
        EngineEvent::SlotComplete { timestamp, count } => Response::SlotComplete {
            timestamp: timestamp.format(),
            count,
        },
        EngineEvent::A7Checkpoint {
            timestamp,
            snapshot,
        } => Response::A7Checkpoint {
            timestamp: timestamp.format(),
            snapshot,
        },
        EngineEvent::JtdxCheckpoint {
            timestamp,
            branch,
            update,
            publish,
        } => Response::JtdxCheckpoint {
            timestamp: timestamp.format(),
            branch,
            update,
            publish,
        },
        EngineEvent::DxContext(dx) => Response::Dx(dx),
        EngineEvent::Error(error) => Response::Error(error),
        _ => return None,
    })
}

fn event(response: Response) -> Result<Option<EngineEvent>, String> {
    Ok(Some(match response {
        Response::Decode {
            timestamp,
            freq,
            dt,
            snr,
            sync,
            message,
            tones,
            provenance,
            stage,
        } => EngineEvent::Decode(DecodeRecord {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            row: StreamDecodedMessage {
                freq,
                dt,
                snr,
                sync,
                msg: message,
                itone: tones
                    .try_into()
                    .map_err(|_| "worker returned invalid FT8 tone count")?,
            },
            provenance,
            stage,
        }),
        Response::SlotComplete { timestamp, count } => EngineEvent::SlotComplete {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            count,
        },
        Response::A7Checkpoint {
            timestamp,
            snapshot,
        } => EngineEvent::A7Checkpoint {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            snapshot,
        },
        Response::JtdxCheckpoint {
            timestamp,
            branch,
            update,
            publish,
        } => EngineEvent::JtdxCheckpoint {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            branch,
            update,
            publish,
        },
        Response::Dx(dx) => EngineEvent::DxContext(dx),
        Response::Error(error) => EngineEvent::Error(error),
        Response::Log(message) => {
            log::info!("decoder worker: {message}");
            return Ok(None);
        }
        Response::Done(_) => return Ok(None),
    }))
}

struct PipeLogger;
static PIPE_LOGGER: PipeLogger = PipeLogger;
impl log::Log for PipeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("ft8rs")
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            let _ = write_frame(
                &mut std::io::stdout().lock(),
                &Response::Log(record.args().to_string()),
            );
        }
    }
    fn flush(&self) {}
}

/// Entrypoints must call this before GUI initialization or argument parsing.
/// The same executable supplies its own private decoder worker, so installed
/// packages need no extra binary and retain their existing CallDB lookup path.
pub fn run_decode_worker_if_requested() -> bool {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(WORKER_ARGUMENT)) {
        return false;
    }
    let _ = log::set_logger(&PIPE_LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    if let Err(error) = worker_main() {
        let _ = write_frame(&mut std::io::stdout().lock(), &Response::Error(error));
        std::process::exit(1);
    }
    true
}

fn worker_main() -> Result<(), String> {
    let mut input = BufReader::new(std::io::stdin());
    let Some(Request::Init(config)) = read_frame(&mut input)? else {
        return Err("worker expected initial configuration".into());
    };
    let (commands, command_rx) = mpsc::channel();
    let (events, event_rx) = mpsc::channel();
    let actor = thread::spawn(move || decode_actor(config, command_rx, events));
    while let Some(request) = read_frame(&mut input)? {
        let (id, command) = match request {
            Request::Stage {
                id,
                stage,
                timestamp,
                samples,
                recovery_deadline,
                restore_a7,
                restore_jtdx,
            } => (
                id,
                DecodeCmd::Stage {
                    stage,
                    timestamp: SlotTimestamp::parse(&timestamp)?,
                    samples_12k: samples,
                    submitted: Instant::now(),
                    recovery_deadline: recovery_deadline.map(|deadline| {
                        let remaining = deadline
                            .duration_since(SystemTime::now())
                            .unwrap_or_default()
                            .min(LIVE_BUDGET);
                        Instant::now() + remaining
                    }),
                    restore_a7,
                    restore_jtdx,
                    supervised: true,
                },
            ),
            Request::Reconfigure {
                id,
                config,
                target,
                operator,
            } => (
                id,
                DecodeCmd::Reconfigure {
                    config,
                    reset_dx_target: target,
                    reset_dx_operator: operator,
                },
            ),
            Request::Init(_) => return Err("duplicate worker initialization".into()),
        };
        let (done, done_rx) = mpsc::channel();
        commands.send(command).map_err(|e| e.to_string())?;
        commands
            .send(DecodeCmd::Barrier(done))
            .map_err(|e| e.to_string())?;
        loop {
            match event_rx.recv_timeout(POLL) {
                Ok(value) => {
                    if let Some(value) = response(value) {
                        write_frame(&mut std::io::stdout().lock(), &value)?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("decoder actor exited".into())
                }
            }
            if done_rx.try_recv().is_ok() {
                // The barrier follows all stage events in the actor. Drain them
                // before Done so callbacks never wait for all-worker completion.
                for value in event_rx.try_iter().filter_map(response) {
                    write_frame(&mut std::io::stdout().lock(), &value)?;
                }
                write_frame(&mut std::io::stdout().lock(), &Response::Done(id))?;
                break;
            }
        }
    }
    let _ = commands.send(DecodeCmd::Stop);
    let _ = actor.join();
    Ok(())
}

struct Worker {
    child: Child,
    input: Option<Sender<Request>>,
    output: Receiver<Result<Response, String>>,
    pipes: Vec<JoinHandle<()>>,
    restore_pending: [bool; 2],
    restore_jtdx_pending: bool,
    #[cfg(test)]
    kill_at_checkpoint: Option<&'static str>,
    #[cfg(test)]
    kill_at_baseline_row: Option<&'static str>,
}

impl Worker {
    fn start() -> Result<Self, String> {
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command.arg(WORKER_ARGUMENT);
        Self::start_command(command)
    }

    fn start_command(mut command: Command) -> Result<Self, String> {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut stdin = child.stdin.take().ok_or("worker stdin unavailable")?;
        let stdout = child.stdout.take().ok_or("worker stdout unavailable")?;
        let (input, input_rx) = mpsc::channel();
        let (output_tx, output) = mpsc::channel();
        let errors = output_tx.clone();
        let writer = thread::spawn(move || {
            while let Ok(request) = input_rx.recv() {
                if let Err(error) = write_frame(&mut stdin, &request) {
                    let _ = errors.send(Err(error));
                    break;
                }
            }
        });
        let reader = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                match read_frame(&mut stdout) {
                    Ok(Some(response)) => {
                        if output_tx.send(Ok(response)).is_err() {
                            break;
                        }
                    }
                    result => {
                        let _ = output_tx.send(Err(match result {
                            Err(e) => e,
                            _ => "decoder worker pipe closed".into(),
                        }));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input: Some(input),
            output,
            pipes: vec![writer, reader],
            restore_pending: [true; 2],
            restore_jtdx_pending: true,
            #[cfg(test)]
            kill_at_checkpoint: None,
            #[cfg(test)]
            kill_at_baseline_row: None,
        })
    }

    fn send(&self, request: Request) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("worker stopped")?
            .send(request)
            .map_err(|e| e.to_string())
    }

    fn stop(&mut self) {
        if self.input.is_none() {
            return;
        }
        // Kill and reap before a replacement can start. Native CPU work cannot
        // survive cancellation; closing pipes also releases blocked pipe I/O.
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.input.take();
        for pipe in self.pipes.drain(..) {
            let _ = pipe.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn parity(timestamp: &SlotTimestamp) -> usize {
    (timestamp.milliseconds_since_midnight() / 15_000 % 2) as usize
}

#[derive(Default)]
struct WorkerHistory {
    a7: [Option<(SlotTimestamp, A7Snapshot)>; 2],
    jtdx: [Option<(SlotTimestamp, JtdxRestore)>; 2],
}

impl WorkerHistory {
    fn a7_for_slot(&mut self, timestamp: &SlotTimestamp) -> &mut A7Snapshot {
        let bank = &mut self.a7[parity(timestamp)];
        if bank.as_ref().is_none_or(|(slot, _)| slot != timestamp) {
            *bank = Some((timestamp.clone(), A7Snapshot::default()));
        }
        &mut bank.as_mut().unwrap().1
    }

    fn jtdx_for_restart(&self, timestamp: &SlotTimestamp) -> [Option<JtdxRestore>; 2] {
        self.jtdx.each_ref().map(|branch| {
            branch
                .as_ref()
                .filter(|(slot, _)| {
                    slot.add_seconds(15) == *timestamp || slot.add_seconds(30) == *timestamp
                })
                .map(|(_, restore)| restore.clone())
        })
    }
}

#[derive(Default)]
struct SlotDelivery {
    rows: Vec<StreamDecodedMessage>,
    complete: bool,
}

impl SlotDelivery {
    fn forward(
        &mut self,
        value: Response,
        history: &mut WorkerHistory,
        config: &StreamDecodeConfig,
        events: &Sender<EngineEvent>,
    ) -> Result<(), String> {
        let Some(value) = event(value)? else {
            return Ok(());
        };
        let value = match value {
            EngineEvent::A7Checkpoint {
                timestamp,
                snapshot,
            } => {
                let saved = history.a7_for_slot(&timestamp);
                let mut combined = snapshot;
                combined.merge(std::mem::take(saved));
                *saved = combined;
                return Ok(());
            }
            EngineEvent::JtdxCheckpoint {
                timestamp,
                branch,
                update,
                publish,
            } => {
                let (latest, restore) = history.jtdx[branch.index()]
                    .get_or_insert_with(|| (timestamp.clone(), JtdxRestore::default()));
                *latest = timestamp.clone();
                match update {
                    JtdxCheckpoint::Complete(snapshot) => {
                        restore.completed = Some(snapshot);
                        restore.partial.clear();
                        return Ok(());
                    }
                    JtdxCheckpoint::Row(confirmed) => {
                        let slot = timestamp.format();
                        if restore.partial.last().is_none_or(|(last, _)| last != &slot) {
                            // Preserve a partial bank of each parity across repeated kills.
                            if restore.partial.len() == 2 {
                                restore.partial.remove(0);
                            }
                            restore.partial.push((slot, Vec::new()));
                        }
                        let record = publish.then(|| DecodeRecord {
                            timestamp,
                            row: confirmed.row.decode.clone(),
                            provenance: if config.profile == DecodeProfile::Jtdx {
                                confirmed.row.provenance.for_jtdx_display()
                            } else {
                                StreamDecodeProvenance::Unknown
                            },
                            stage: DecodeStage::Final,
                        });
                        restore.partial.last_mut().unwrap().1.push(*confirmed);
                        let Some(record) = record else {
                            return Ok(());
                        };
                        EngineEvent::Decode(record)
                    }
                }
            }
            other => other,
        };
        let value = match value {
            EngineEvent::Decode(record) => {
                let native_seed = matches!(
                    config.profile,
                    DecodeProfile::Wsjtx | DecodeProfile::WsjtxPlus
                ) && record.provenance.is_wsjtx_history_seed();
                if config.profile == DecodeProfile::Hybrid || native_seed {
                    let frequency =
                        if native_seed && record.provenance == StreamDecodeProvenance::A8List {
                            config.nfqso
                        } else {
                            record.row.freq
                        };
                    history
                        .a7_for_slot(&record.timestamp)
                        .record(&record.row, frequency);
                }
                if self.rows.iter().any(|row| {
                    (config.profile != DecodeProfile::Dx
                        || (row.freq - record.row.freq).abs() <= 3.0)
                        && same_signal(row, &record.row)
                }) {
                    return Ok(());
                }
                self.rows.push(record.row.clone());
                EngineEvent::Decode(record)
            }
            EngineEvent::SlotComplete { timestamp, .. } => {
                if self.complete {
                    return Ok(());
                }
                self.complete = true;
                EngineEvent::SlotComplete {
                    timestamp,
                    count: self.rows.len(),
                }
            }
            other => other,
        };
        events.send(value).map_err(|_| "monitor stopped".into())
    }
}

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
            DecodeCmd::InvalidateA7Checkpoint => {
                history = WorkerHistory::default();
                continue;
            }
            DecodeCmd::Reconfigure {
                config: next,
                reset_dx_target,
                reset_dx_operator,
            } => {
                config = next;
                history = WorkerHistory::default();
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
                while let Ok(command) = commands.try_recv() {
                    if matches!(command, DecodeCmd::Stop) {
                        return Err("monitor stopped".into());
                    }
                    pending.push_back(command);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ft8rs::stream::profile::ProfileStreamDecodeSession;
    use ft8rs::stream::session::StreamDecodeProvenance;

    const HANG_RECOVERY_ROWS: [StreamDecodeProvenance; 5] = [
        StreamDecodeProvenance::Unknown,
        StreamDecodeProvenance::CooperativeRecovery,
        StreamDecodeProvenance::PlusCompatibility,
        StreamDecodeProvenance::CandidateRetry,
        StreamDecodeProvenance::Experiment,
    ];

    fn fixture(mode: &str) -> Result<Worker, String> {
        if mode == "native" {
            if let Some(executable) = std::env::var_os("FT8RS_WORKER_EXECUTABLE") {
                let mut command = Command::new(executable);
                command.arg(WORKER_ARGUMENT);
                return Worker::start_command(command);
            }
        }
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "worker::tests::worker_fixture", "--nocapture"])
            .env("FT8RS_WORKER_TEST_FIXTURE", mode);
        Worker::start_command(command)
    }

    #[test]
    fn worker_fixture() {
        let Ok(mode) = std::env::var("FT8RS_WORKER_TEST_FIXTURE") else {
            return;
        };
        if mode == "native" || mode.starts_with("pause-baseline:") {
            worker_main().unwrap();
            return;
        }
        let mut input = BufReader::new(std::io::stdin());
        assert!(matches!(
            read_frame::<Request>(&mut input).unwrap(),
            Some(Request::Init(_))
        ));
        while let Some(Request::Stage {
            id,
            timestamp,
            stage,
            ..
        }) = read_frame(&mut input).unwrap()
        {
            write_frame(
                &mut std::io::stdout().lock(),
                &Response::Decode {
                    timestamp: timestamp.clone(),
                    freq: if mode == "cumulative" && stage == SlotStage::N47 {
                        1150.0
                    } else {
                        1137.0
                    },
                    dt: 2.4,
                    snr: -8.0,
                    sync: 1.0,
                    message: "JA8IIK/P SV9GPV -18".into(),
                    tones: vec![0; 79],
                    provenance: ft8rs::stream::session::StreamDecodeProvenance::Regular,
                    stage: DecodeStage::Final,
                },
            )
            .unwrap();
            if timestamp.ends_with("120000") && (mode != "cumulative" || stage == SlotStage::N50) {
                if mode == "hang-multi" {
                    for (index, provenance) in HANG_RECOVERY_ROWS.into_iter().enumerate() {
                        write_frame(
                            &mut std::io::stdout().lock(),
                            &Response::Decode {
                                timestamp: timestamp.clone(),
                                freq: 1200.0 + 10.0 * index as f64,
                                dt: 0.1,
                                snr: -8.0,
                                sync: 1.0,
                                message: "CQ K1ABC FN42".into(),
                                tones: vec![0; 79],
                                provenance,
                                stage: DecodeStage::Final,
                            },
                        )
                        .unwrap();
                    }
                }
                // An intentionally uncooperative CPU task. It can only stop
                // when its containing process is terminated by the watchdog.
                loop {
                    std::hint::black_box(17_u64.wrapping_mul(31));
                }
            }
            if mode == "cumulative" {
                thread::sleep(Duration::from_millis(400));
            }
            if stage == SlotStage::N50 {
                write_frame(
                    &mut std::io::stdout().lock(),
                    &Response::SlotComplete {
                        timestamp,
                        count: 1,
                    },
                )
                .unwrap();
            }
            write_frame(&mut std::io::stdout().lock(), &Response::Done(id)).unwrap();
        }
    }

    fn stage(time: &str, stage: SlotStage, samples: Vec<f32>) -> DecodeCmd {
        DecodeCmd::Stage {
            timestamp: SlotTimestamp::parse(time).unwrap(),
            stage,
            samples_12k: samples,
            submitted: Instant::now(),
            recovery_deadline: None,
            restore_a7: None,
            restore_jtdx: [None, None],
            supervised: false,
        }
    }

    #[test]
    fn killed_hybrid_worker_preserves_rows_and_restores_a7_thirty_seconds_later() {
        use ft8rs::stream::session::StreamDecodeProvenance::A7Memory;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ft8rs-core/tests/ft8/230208_140300.wav");
        let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
        let start = SlotTimestamp::parse("230208_140300").unwrap();
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            cooperative_realtime: true,
            ..Default::default()
        };
        let (tx, commands) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let supervisor = thread::spawn(move || {
            let mut starts = 0;
            supervise(
                config,
                commands,
                events,
                || {
                    let mut child = fixture("native")?;
                    if starts == 0 {
                        child.kill_at_checkpoint = Some("230208_140300");
                    }
                    starts += 1;
                    Ok(child)
                },
                LIVE_BUDGET,
            );
            starts
        });

        let (slots, _) = audio.samples.as_chunks::<180_000>();
        for (index, samples) in slots.iter().take(3).enumerate() {
            let timestamp = start.add_seconds(index as i64 * 15);
            for (kind, len) in [
                (SlotStage::N41, 141696),
                (SlotStage::N47, 162432),
                (SlotStage::N50, 180000),
            ] {
                tx.send(stage(&timestamp.format(), kind, samples[..len].to_vec()))
                    .unwrap();
            }
            let mut rows: Vec<DecodeRecord> = Vec::new();
            loop {
                match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                    EngineEvent::Decode(record) => {
                        assert_eq!(record.timestamp, timestamp);
                        assert!(
                            !rows.iter().any(|old| old.row.msg == record.row.msg
                                && (old.row.freq - record.row.freq).abs() <= 5.0
                                && (old.row.dt - record.row.dt).abs() <= 0.3),
                            "duplicate row: {record:?}"
                        );
                        rows.push(record);
                    }
                    EngineEvent::SlotComplete {
                        timestamp: completed,
                        count,
                    } => {
                        assert_eq!(completed, timestamp);
                        assert_eq!(count, rows.len());
                        break;
                    }
                    EngineEvent::Error(error) => panic!("live worker failed: {error}"),
                    _ => {}
                }
            }
            if index == 0 {
                assert!(rows.iter().any(|row| row.row.msg == "PD2RLD R6KEE KN75"));
            }
            if index == 2 {
                assert!(rows
                    .iter()
                    .any(|row| row.row.msg == "CQ R6KEE KN75" && row.provenance == A7Memory));
            }
        }
        tx.send(DecodeCmd::Stop).unwrap();
        assert_eq!(
            supervisor.join().unwrap(),
            2,
            "one real child must have been replaced"
        );
        assert!(results.try_recv().is_err());
    }

    #[test]
    fn killed_in_base_search_keeps_confirmations_and_restores_native_history() {
        use StreamDecodeProvenance::{A7Memory, JtdxDeep};

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ft8rs-core/tests/ft8/230208_140300.wav");
        let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
        let start = SlotTimestamp::parse("230208_140300").unwrap();
        for (profile, seed, recovered, provenance) in [
            (
                DecodeProfile::Wsjtx,
                "PD2RLD R6KEE KN75",
                "CQ R6KEE KN75",
                A7Memory,
            ),
            (
                DecodeProfile::Hybrid,
                "PD2RLD R6KEE KN75",
                "CQ R6KEE KN75",
                A7Memory,
            ),
            (
                DecodeProfile::Jtdx,
                "CQ D1DX KN87",
                "CQ D1DX KN87",
                JtdxDeep,
            ),
        ] {
            let config = StreamDecodeConfig {
                profile,
                cooperative_realtime: true,
                ..Default::default()
            };
            let mut cold = ProfileStreamDecodeSession::new(config.clone());
            assert!(
                !cold
                    .decode_slot_at(&start.add_seconds(30), &audio.samples[360_000..540_000])
                    .iter()
                    .any(|row| row.msg == recovered),
                "{profile:?} control unexpectedly decoded {recovered}"
            );
            let (tx, commands) = mpsc::channel();
            let (events, results) = mpsc::channel();
            let supervisor = thread::spawn(move || {
                let mut starts = 0;
                supervise(
                    config,
                    commands,
                    events,
                    || {
                        let child = if starts == 0 {
                            let mut child = fixture(&format!("pause-baseline:{seed}"))?;
                            child.kill_at_baseline_row = Some(seed);
                            child
                        } else {
                            fixture("native")?
                        };
                        starts += 1;
                        // The first child parks inside the actual confirmation callback.
                        // The supervisor always terminates and reaps it.
                        Ok(child)
                    },
                    LIVE_BUDGET,
                );
                starts
            });

            let (slots, _) = audio.samples.as_chunks::<180_000>();
            for (index, samples) in slots.iter().take(3).enumerate() {
                let timestamp = start.add_seconds(index as i64 * 15);
                for (kind, len) in [
                    (SlotStage::N41, 141696),
                    (SlotStage::N47, 162432),
                    (SlotStage::N50, 180000),
                ] {
                    let input = if index == 0 && kind == SlotStage::N41 {
                        // Ensure the classic seed is confirmed in the N50 search.
                        vec![0.0; len]
                    } else {
                        samples[..len].to_vec()
                    };
                    tx.send(stage(&timestamp.format(), kind, input)).unwrap();
                }
                let mut rows: Vec<DecodeRecord> = Vec::new();
                loop {
                    match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                        EngineEvent::Decode(record) => {
                            assert_eq!(record.timestamp, timestamp);
                            assert!(!rows.iter().any(|old| same_signal(&old.row, &record.row)));
                            rows.push(record);
                        }
                        EngineEvent::SlotComplete {
                            timestamp: completed,
                            count,
                        } => {
                            assert_eq!(completed, timestamp);
                            assert_eq!(count, rows.len());
                            break;
                        }
                        EngineEvent::Error(error) => panic!("{profile:?}: {error}"),
                        _ => {}
                    }
                }
                if index == 0 {
                    assert!(rows
                        .iter()
                        .any(|row| row.row.msg == seed && row.stage == DecodeStage::Final));
                }
                if index == 2 {
                    assert!(
                        rows.iter()
                            .any(|row| row.row.msg == recovered && row.provenance == provenance),
                        "{profile:?} lost history for {recovered}"
                    );
                }
            }
            tx.send(DecodeCmd::Stop).unwrap();
            assert_eq!(
                supervisor.join().unwrap(),
                2,
                "{profile:?} did not replace exactly one worker"
            );
            assert!(results.try_recv().is_err());
        }
    }

    #[test]
    fn hybrid_uses_a_jtdx_only_confirmation_as_a7_seed() {
        use ft8rs::stream::profile::SlotRecovery;

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ft8rs-core/tests/ft8/230208_140300.wav");
        let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
        let seed_slot = SlotTimestamp::parse("230208_140300").unwrap();
        let next = seed_slot.add_seconds(30);
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            cooperative_realtime: true,
            ..Default::default()
        };
        let mut peer = ProfileStreamDecodeSession::new(StreamDecodeConfig {
            profile: DecodeProfile::Jtdx,
            ..config.clone()
        });
        let mut history = WorkerHistory::default();
        let mut delivery = SlotDelivery::default();
        let (events, received) = mpsc::channel();
        let mut checkpoint = |branch, update, publish| {
            if matches!(&update, JtdxCheckpoint::Row(entry)
                if entry.row.decode.msg == "PD2RLD R6KEE KN75")
            {
                delivery
                    .forward(
                        Response::JtdxCheckpoint {
                            timestamp: seed_slot.format(),
                            branch,
                            update,
                            publish,
                        },
                        &mut history,
                        &config,
                        &events,
                    )
                    .unwrap();
            }
        };
        let state = peer.start_slot();
        peer.finish_slot_nzhsym50(
            &seed_slot,
            state,
            &audio.samples[..180_000],
            SlotRecovery {
                jtdx_checkpoint: Some(&mut checkpoint),
                ..Default::default()
            },
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(received.try_iter().count(), 1);

        let mut hybrid = ProfileStreamDecodeSession::new(config);
        hybrid.restore_a7(&next, history.a7[parity(&next)].take().unwrap().1);
        let mut decoded = false;
        hybrid
            .decode_slot_streaming_with_provenance_at(
                &next,
                &audio.samples[360_000..540_000],
                |row| {
                    decoded |= row.decode.msg == "CQ R6KEE KN75"
                        && row.provenance == StreamDecodeProvenance::A7Memory;
                    Ok(())
                },
            )
            .unwrap();
        assert!(
            decoded,
            "the WSJT-X branch must be able to use a JTDX-only seed"
        );
    }

    #[test]
    fn deadline_silently_kills_cpu_work_preserves_callbacks_and_recovers_next_slot() {
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread = thread::spawn(move || {
            supervise(
                StreamDecodeConfig::default(),
                rx,
                events,
                || fixture("hang-multi"),
                Duration::from_millis(700),
            )
        });
        let start = Instant::now();
        tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
            .unwrap();
        let mut decoded = Vec::new();
        loop {
            match results.recv_timeout(Duration::from_secs(3)).unwrap() {
                EngineEvent::Decode(record) => decoded.push(record.provenance),
                EngineEvent::Error(message) => {
                    panic!("normal deadline must not show a GUI error: {message}");
                }
                EngineEvent::SlotComplete { count, .. } => {
                    assert_eq!(count, 1 + HANG_RECOVERY_ROWS.len());
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(decoded[0], StreamDecodeProvenance::Regular);
        assert_eq!(decoded[1..], HANG_RECOVERY_ROWS);
        assert!(start.elapsed() < Duration::from_secs(2));
        tx.send(stage("261030_120015", SlotStage::N50, vec![0.0; 32]))
            .unwrap();
        loop {
            match results.recv_timeout(Duration::from_secs(3)).unwrap() {
                EngineEvent::Decode(record) => assert_eq!(record.timestamp.format_time(), "120015"),
                EngineEvent::SlotComplete { timestamp, .. } => {
                    assert_eq!(timestamp.format_time(), "120015");
                    break;
                }
                EngineEvent::Error(message) => panic!("next slot failed: {message}"),
                _ => {}
            }
        }
        tx.send(DecodeCmd::Stop).unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn stop_preempts_an_uncooperative_decoder() {
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread = thread::spawn(move || {
            supervise(
                StreamDecodeConfig::default(),
                rx,
                events,
                || fixture("hang"),
                LIVE_BUDGET,
            )
        });
        tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
            .unwrap();
        assert!(matches!(
            results.recv_timeout(Duration::from_secs(3)).unwrap(),
            EngineEvent::Decode(_)
        ));
        let start = Instant::now();
        tx.send(DecodeCmd::Stop).unwrap();
        thread.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn budget_is_cumulative_across_early_and_final_stages() {
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread = thread::spawn(move || {
            supervise(
                StreamDecodeConfig::default(),
                rx,
                events,
                || fixture("cumulative"),
                Duration::from_millis(1200),
            )
        });
        for kind in [SlotStage::N41, SlotStage::N47] {
            tx.send(stage("261030_120000", kind, vec![0.0; 32]))
                .unwrap();
            assert!(matches!(
                results.recv_timeout(Duration::from_secs(3)).unwrap(),
                EngineEvent::Decode(_)
            ));
            // Each fake stage works for 400 ms. Submit the next capture stage
            // only after the previous one has finished, excluding queue age.
            thread::sleep(Duration::from_millis(450));
        }
        let final_start = Instant::now();
        tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
            .unwrap();
        loop {
            match results.recv_timeout(Duration::from_secs(3)).unwrap() {
                EngineEvent::SlotComplete { .. } => break,
                EngineEvent::Error(message) => {
                    panic!("cumulative deadline must be silent: {message}")
                }
                _ => {}
            }
        }
        assert!(
            final_start.elapsed() < Duration::from_millis(800),
            "early-stage work must reduce the final-stage budget"
        );
        tx.send(DecodeCmd::Stop).unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn expired_queued_slots_are_skipped_without_spawning_native_work() {
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        tx.send(DecodeCmd::Stage {
            timestamp: SlotTimestamp::parse("261030_120000").unwrap(),
            stage: SlotStage::N41,
            samples_12k: vec![],
            submitted: Instant::now() - Duration::from_secs(2),
            recovery_deadline: None,
            restore_a7: None,
            restore_jtdx: [None, None],
            supervised: false,
        })
        .unwrap();
        tx.send(DecodeCmd::Stop).unwrap();
        supervise(
            StreamDecodeConfig::default(),
            rx,
            events,
            || panic!("expired work must not start a child"),
            Duration::from_millis(100),
        );
        let results: Vec<_> = results.try_iter().collect();
        assert!(!results
            .iter()
            .any(|event| matches!(event, EngineEvent::Error(_))));
        assert!(results
            .iter()
            .any(|event| matches!(event, EngineEvent::SlotComplete { count: 0, .. })));
    }

    #[test]
    fn actual_worker_failures_still_reach_the_gui() {
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        tx.send(stage("261030_120000", SlotStage::N50, vec![]))
            .unwrap();
        tx.send(DecodeCmd::Stop).unwrap();
        supervise(
            StreamDecodeConfig::default(),
            rx,
            events,
            || Err("could not start decoder worker".into()),
            LIVE_BUDGET,
        );
        assert!(results.try_iter().any(|event| matches!(event,
            EngineEvent::Error(message) if message == "could not start decoder worker")));
    }

    #[test]
    fn ipc_roundtrip_preserves_every_config_and_decode_field() {
        let config = StreamDecodeConfig {
            hybrid_experiment: true,
            mycall: Some("BG5ATV".into()),
            nfa: 201.123456789,
            nfb: 2999.123456789,
            ..Default::default()
        };
        let mut frame = Vec::new();
        write_frame(&mut frame, &Request::Init(config.clone())).unwrap();
        let Some(Request::Init(restored)) = read_frame(&mut std::io::Cursor::new(frame)).unwrap()
        else {
            panic!("wrong frame");
        };
        assert_eq!(format!("{config:?}"), format!("{restored:?}"));
        let record = DecodeRecord {
            timestamp: SlotTimestamp::parse("261030_120000").unwrap(),
            row: StreamDecodedMessage {
                freq: 1000.1234567890123,
                dt: -0.1234567890123,
                snr: -20.1234567890123,
                sync: 0.1234567890123,
                msg: "CQ K1ABC FN42".into(),
                itone: [7; 79],
            },
            provenance: ft8rs::stream::session::StreamDecodeProvenance::Experiment,
            stage: DecodeStage::Final,
        };
        let expected = format!("{record:?}");
        let mut frame = Vec::new();
        write_frame(&mut frame, &response(EngineEvent::Decode(record)).unwrap()).unwrap();
        let restored: Response = read_frame(&mut std::io::Cursor::new(frame))
            .unwrap()
            .unwrap();
        let Some(EngineEvent::Decode(restored)) = event(restored).unwrap() else {
            panic!("wrong event");
        };
        assert_eq!(expected, format!("{restored:?}"));
    }

    #[test]
    #[ignore = "serial optimized 285-second recording comparison through the production worker"]
    fn native_process_preserves_each_long_recording_message() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ft8rs-core/tests/ft8/230208_140300.wav");
        let samples = ft8rs::input::audio::read_wav_mono_f32(path)
            .unwrap()
            .samples;
        let config = StreamDecodeConfig {
            profile: ft8rs::DecodeProfile::Hybrid,
            ..Default::default()
        };
        let mut original = ProfileStreamDecodeSession::new(config.clone());
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread =
            thread::spawn(move || supervise(config, rx, events, || fixture("native"), LIVE_BUDGET));
        let start = SlotTimestamp::parse("230208_140300").unwrap();
        let mut retained = 0;
        for (index, samples) in samples.as_chunks::<180000>().0.iter().enumerate() {
            let timestamp = start.add_seconds(index as i64 * 15);
            let mut state = original.start_slot();
            let mut expected = Vec::new();
            original
                .decode_slot_nzhsym41_streaming_with_provenance(
                    &timestamp,
                    &mut state,
                    &samples[..141696],
                    |row| {
                        expected.push(format!("{row:?}"));
                        Ok(())
                    },
                )
                .unwrap();
            original.subtract_slot_nzhsym47(&mut state, &samples[..162432]);
            original
                .decode_slot_nzhsym50_streaming_with_provenance(
                    &timestamp,
                    state,
                    expected.len(),
                    samples,
                    |row| {
                        expected.push(format!("{row:?}"));
                        Ok(())
                    },
                )
                .unwrap();
            for (kind, len) in [
                (SlotStage::N41, 141696),
                (SlotStage::N47, 162432),
                (SlotStage::N50, 180000),
            ] {
                tx.send(stage(&timestamp.format(), kind, samples[..len].to_vec()))
                    .unwrap();
            }
            let mut actual = Vec::new();
            loop {
                match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                    EngineEvent::Decode(record) => actual.push(format!(
                        "{:?}",
                        ft8rs::stream::session::StreamDecodedWithProvenance {
                            decode: record.row,
                            provenance: record.provenance
                        }
                    )),
                    EngineEvent::SlotComplete { .. } => break,
                    EngineEvent::Error(error) => panic!("slot {index} failed: {error}"),
                    _ => {}
                }
            }
            expected.sort();
            actual.sort();
            assert_eq!(
                actual, expected,
                "slot {index} lost or changed native output"
            );
            retained += actual.len();
        }
        tx.send(DecodeCmd::Stop).unwrap();
        thread.join().unwrap();
        assert!(
            retained >= 476,
            "long recording retained only {retained} rows"
        );
        eprintln!("production worker retained all {retained} native rows with exact fields");
    }

    #[test]
    fn native_process_streaming_matches_in_process_hybrid() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../ft8rs-core/tests/ft8/210703_133430.wav");
        let samples = ft8rs::input::audio::read_wav_mono_f32(path)
            .unwrap()
            .samples;
        for realtime in [false, true] {
            let config = StreamDecodeConfig {
                profile: ft8rs::DecodeProfile::Hybrid,
                cooperative_realtime: realtime,
                ..Default::default()
            };
            let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
            let mut session = ProfileStreamDecodeSession::new(config.clone());
            let mut state = session.start_slot();
            let mut expected = Vec::new();
            session
                .decode_slot_nzhsym41_streaming_with_provenance(
                    &timestamp,
                    &mut state,
                    &samples[..141696],
                    |row| {
                        expected.push(row.clone());
                        Ok(())
                    },
                )
                .unwrap();
            session.subtract_slot_nzhsym47(&mut state, &samples[..162432]);
            session
                .decode_slot_nzhsym50_streaming_with_provenance(
                    &timestamp,
                    state,
                    expected.len(),
                    &samples,
                    |row| {
                        expected.push(row.clone());
                        Ok(())
                    },
                )
                .unwrap();
            let (tx, rx) = mpsc::channel();
            let (events, results) = mpsc::channel();
            let thread = thread::spawn(move || {
                supervise(config, rx, events, || fixture("native"), LIVE_BUDGET)
            });
            let mut actual = Vec::new();
            for (kind, len) in [
                (SlotStage::N41, 141696),
                (SlotStage::N47, 162432),
                (SlotStage::N50, 180000),
            ] {
                tx.send(stage("210703_133430", kind, samples[..len].to_vec()))
                    .unwrap();
                // Do not serialize later-stage capture time as if it was old queued
                // work. Native capture actually submits stages at different times.
                if kind != SlotStage::N50 {
                    thread::sleep(Duration::from_millis(40));
                }
            }
            loop {
                match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                    EngineEvent::Decode(record) => {
                        actual.push(ft8rs::stream::session::StreamDecodedWithProvenance {
                            decode: record.row,
                            provenance: record.provenance,
                        })
                    }
                    EngineEvent::SlotComplete { .. } => break,
                    EngineEvent::Error(error) => panic!("native child failed: {error}"),
                    _ => {}
                }
            }
            tx.send(DecodeCmd::Stop).unwrap();
            thread.join().unwrap();
            let sorted = |rows: Vec<ft8rs::stream::session::StreamDecodedWithProvenance>| {
                let mut rows: Vec<_> = rows.into_iter().map(|row| format!("{row:?}")).collect();
                rows.sort();
                rows
            };
            if realtime {
                // The first branch to report a signal chooses the representative.
                // IPC can affect scheduling, so compare unique signals here;
                // the wire test separately checks every encoded field.
                assert_eq!(actual.len(), expected.len());
                for wanted in &expected {
                    assert!(
                        actual
                            .iter()
                            .any(|row| same_signal(&row.decode, &wanted.decode)),
                        "lost native signal: {:?}",
                        wanted.decode.msg
                    );
                }
            } else {
                assert_eq!(sorted(actual), sorted(expected));
            }
        }
    }
}
