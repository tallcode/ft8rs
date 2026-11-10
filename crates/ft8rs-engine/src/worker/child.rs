//! Private worker entry point and stdin/stdout request loop in the child process.
use std::io::BufReader;
use std::sync::mpsc;
use std::thread;
use std::time::{Instant, SystemTime};

use ft8rs::SlotTimestamp;

use crate::engine::{decode_actor, DecodeCmd};

use super::ipc::{read_frame, response, write_frame, Request, Response};
use super::{LIVE_BUDGET, POLL, WORKER_ARGUMENT};

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

/// Call this from the GUI entry point before initializing the window.
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

pub(super) fn worker_main() -> Result<(), String> {
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
