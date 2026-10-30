//! Killable live-decoder boundary. Native algorithms run unchanged in a child
//! process; the capture/control thread never waits for native worker threads.
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ft8rs::{SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};
use serde::{Deserialize, Serialize};

use crate::engine::{decode_actor, DecodeCmd, DECODE_QUEUE_CAP};
use crate::protocol::{DecodeRecord, DecodeStage, DxContextSnapshot, EngineEvent};
use crate::soundcard::SlotStage;

const WORKER_ARGUMENT: &str = "--ft8rs-decode-worker";
const PREFIX: &str = "FT8RS-WORKER ";
const LIVE_BUDGET: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(5);

#[derive(Serialize, Deserialize)]
enum Request {
    Init(StreamDecodeConfig),
    Stage {
        id: u64,
        stage: SlotStage,
        timestamp: String,
        samples: Vec<f32>,
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
        if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
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
            } => (
                id,
                DecodeCmd::Stage {
                    stage,
                    timestamp: SlotTimestamp::parse(&timestamp)?,
                    samples_12k: samples,
                    submitted: Instant::now(),
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
        })
    }

    fn send(&self, request: Request) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("worker stopped")?
            .send(request)
            .map_err(|e| e.to_string())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
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
    let mut count = 0;
    loop {
        let command = match pending.pop_front().or_else(|| commands.recv().ok()) {
            Some(c) => c,
            None => return,
        };
        let (request, deadline, timestamp) = match command {
            DecodeCmd::Stop => return,
            DecodeCmd::Barrier(done) => {
                let _ = done.send(());
                continue;
            }
            DecodeCmd::Reconfigure {
                config: next,
                reset_dx_target,
                reset_dx_operator,
            } => {
                config = next;
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
            } => {
                if aborted.as_ref() == Some(&timestamp) {
                    continue;
                }
                if slot.as_ref() != Some(&timestamp) {
                    slot = Some(timestamp.clone());
                    work = Duration::ZERO;
                    count = 0;
                }
                sequence += 1;
                (
                    Request::Stage {
                        id: sequence,
                        stage,
                        timestamp: timestamp.format(),
                        samples: samples_12k,
                    },
                    (Instant::now() + budget.saturating_sub(work)).min(submitted + budget),
                    Some(timestamp),
                )
            }
        };
        let started = Instant::now();
        let result = (|| -> Result<(), String> {
            if started >= deadline {
                return Err("live decode deadline expired in queue".into());
            }
            if worker.is_none() {
                let child = start()?;
                child.send(Request::Init(config.clone()))?;
                worker = Some(child);
            }
            let child = worker.as_ref().unwrap();
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
                    return Err("live decode exceeded the 10-second budget".into());
                }
                match child
                    .output
                    .recv_timeout(POLL.min(deadline.saturating_duration_since(Instant::now())))
                {
                    Ok(Ok(Response::Done(id))) if id == sequence => return Ok(()),
                    Ok(Ok(value)) => {
                        if let Some(value) = event(value)? {
                            if matches!(value, EngineEvent::Decode(_)) {
                                count += 1;
                            }
                            if events.send(value).is_err() {
                                return Err("monitor stopped".into());
                            }
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
            // Drop performs kill + wait, including every native worker thread.
            worker.take();
            if error == "monitor stopped" {
                return;
            }
            log::warn!("{error}; worker terminated and reaped before next slot");
            let _ = events.send(EngineEvent::Error(error));
            if let Some(timestamp) = timestamp {
                aborted = Some(timestamp.clone());
                let _ = events.send(EngineEvent::SlotComplete { timestamp, count });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ft8rs::stream::profile::ProfileStreamDecodeSession;

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
        if mode == "native" {
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
                    freq: 1137.0,
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
        }
    }

    #[test]
    fn deadline_kills_cpu_work_preserves_callbacks_and_recovers_next_slot() {
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread = thread::spawn(move || {
            supervise(
                StreamDecodeConfig::default(),
                rx,
                events,
                || fixture("hang"),
                Duration::from_millis(700),
            )
        });
        let start = Instant::now();
        tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
            .unwrap();
        let mut decoded = 0;
        let mut timeout = false;
        loop {
            match results.recv_timeout(Duration::from_secs(3)).unwrap() {
                EngineEvent::Decode(_) => decoded += 1,
                EngineEvent::Error(message) => {
                    assert!(message.contains("budget"));
                    timeout = true;
                }
                EngineEvent::SlotComplete { count, .. } => {
                    assert_eq!(count, 1);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(decoded, 1);
        assert!(timeout);
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
            if matches!(
                results.recv_timeout(Duration::from_secs(3)).unwrap(),
                EngineEvent::SlotComplete { .. }
            ) {
                break;
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
        assert!(results.try_iter().any(
            |event| matches!(event, EngineEvent::Error(message) if message.contains("queue"))
        ));
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
                // Native real-time cancellation races can change a reported SNR /
                // sync slightly even without IPC. Require each original payload,
                // tones, source, frequency and DT; the wire test above separately
                // verifies bit-exact transport of every numeric field.
                assert_eq!(actual.len(), expected.len());
                for wanted in &expected {
                    assert!(
                        actual.iter().any(|row| row.decode.msg == wanted.decode.msg
                            && row.decode.itone == wanted.decode.itone
                            && row.provenance == wanted.provenance
                            && (row.decode.freq - wanted.decode.freq).abs() <= 0.01
                            && (row.decode.dt - wanted.decode.dt).abs() <= 0.001),
                        "lost native signal: {wanted:?}"
                    );
                }
            } else {
                assert_eq!(sorted(actual), sorted(expected));
            }
        }
    }
}
