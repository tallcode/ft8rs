//! Parent-side child process and its pipe reader/writer threads.
use std::io::BufReader;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use super::ipc::{read_frame, write_frame, Request, Response};
use super::WORKER_ARGUMENT;

pub(super) struct Worker {
    child: Child,
    input: Option<Sender<Request>>,
    pub(super) output: Receiver<Result<Response, String>>,
    pipes: Vec<JoinHandle<()>>,
    pub(super) restore_pending: [bool; 2],
    pub(super) restore_jtdx_pending: bool,
    #[cfg(test)]
    pub(super) kill_at_checkpoint: Option<&'static str>,
    #[cfg(test)]
    pub(super) kill_at_baseline_row: Option<&'static str>,
}

impl Worker {
    pub(super) fn start() -> Result<Self, String> {
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command.arg(WORKER_ARGUMENT);
        Self::start_command(command)
    }

    pub(super) fn start_command(mut command: Command) -> Result<Self, String> {
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

    pub(super) fn send(&self, request: Request) -> Result<(), String> {
        self.input
            .as_ref()
            .ok_or("worker stopped")?
            .send(request)
            .map_err(|e| e.to_string())
    }

    pub(super) fn stop(&mut self) {
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
