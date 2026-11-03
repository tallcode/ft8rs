//! One GUI per user, protected by an OS lock (including during graphics startup).
//! The persistent lock file is never unlinked: doing so could create two inodes
//! and two simultaneous owners. Kernel locks are released even on process death.
//! A loopback-only activation channel uses an ephemeral port, not a fixed port.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui;

const ENDPOINT: &str = "gui-instance.endpoint";
const TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct Activation {
    requested: AtomicBool,
    context: Mutex<Option<egui::Context>>,
}

impl Activation {
    pub fn attach(&self, context: &egui::Context) {
        *self.context.lock().unwrap() = Some(context.clone());
        if self.requested.load(Ordering::Acquire) {
            self.request();
        }
    }

    fn request(&self) {
        self.requested.store(true, Ordering::Release);
        if let Some(context) = self.context.lock().unwrap().clone() {
            // Wake and restore even when the root viewport was minimized.
            context
                .send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
            context.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Minimized(false),
            );
            context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            context.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub fn take_request(&self) -> bool {
        self.requested.swap(false, Ordering::AcqRel)
    }
}

pub enum Startup {
    Primary(PrimaryInstance),
    Secondary,
}

pub struct PrimaryInstance {
    _lock: File,
    pub activation: Arc<Activation>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
    directory: std::path::PathBuf,
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        // Still hold the kernel lock while cleaning up the activation endpoint.
        let _ = fs::remove_file(self.directory.join(ENDPOINT));
    }
}

pub fn acquire() -> io::Result<Startup> {
    let directory = eframe::storage_dir("ft8rs")
        .ok_or_else(|| io::Error::other("Cannot locate the ft8rs user data directory"))?;
    acquire_in(&directory)
}

fn acquire_in(directory: &Path) -> io::Result<Startup> {
    fs::create_dir_all(directory)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("gui-instance.lock"))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            // The other process may still be publishing its endpoint. Retry the
            // activation handshake, but never start another GUI if it times out.
            if !notify_existing(directory)? {
                eprintln!("ft8rs is already running; window activation timed out.");
            }
            return Ok(Startup::Secondary);
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }

    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    let token = format!(
        "{:x}-{:x}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let request = format!("FT8RS-ACTIVATE {token}");
    let acknowledgement = format!("FT8RS-OK {token}");
    // Use a separate metadata file: on Windows, reading a locked file through
    // another handle is denied. A partial/stale endpoint is harmless and retried.
    fs::write(
        directory.join(ENDPOINT),
        format!("{} {token}", socket.local_addr()?.port()),
    )?;
    let activation = Arc::new(Activation::default());
    let stop = Arc::new(AtomicBool::new(false));
    let wake = Arc::clone(&activation);
    let stopped = Arc::clone(&stop);
    let listener = thread::Builder::new()
        .name("gui-activation".to_owned())
        .spawn(move || {
            let mut buffer = [0u8; 128];
            while !stopped.load(Ordering::Acquire) {
                match socket.recv_from(&mut buffer) {
                    Ok((length, peer))
                        if peer.ip().is_loopback() && buffer[..length] == *request.as_bytes() =>
                    {
                        wake.request();
                        let _ = socket.send_to(acknowledgement.as_bytes(), peer);
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock
                                | io::ErrorKind::TimedOut
                                | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => {
                        log::warn!("GUI activation listener failed: {error}");
                        break;
                    }
                }
            }
        })?;
    Ok(Startup::Primary(PrimaryInstance {
        _lock: lock,
        activation,
        stop,
        listener: Some(listener),
        directory: directory.to_owned(),
    }))
}

fn notify_existing(directory: &Path) -> io::Result<bool> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buffer = [0u8; 128];
    while Instant::now() < deadline {
        if let Ok(endpoint) = fs::read_to_string(directory.join(ENDPOINT)) {
            if let Some((port, token)) = endpoint.split_once(' ') {
                if let Ok(port) = port.parse::<u16>() {
                    socket.connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
                    let request = format!("FT8RS-ACTIVATE {token}");
                    let expected = format!("FT8RS-OK {token}");
                    if socket.send(request.as_bytes()).is_ok() {
                        if let Ok(length) = socket.recv(&mut buffer) {
                            if buffer[..length] == *expected.as_bytes() {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};

    fn directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "ft8rs-single-instance-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn child(directory: &Path, hold: bool) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "single_instance::tests::process_fixture",
                "--nocapture",
            ])
            .env("FT8RS_INSTANCE_TEST_DIR", directory)
            .env("FT8RS_INSTANCE_TEST_HOLD", if hold { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap()
    }

    fn result(child: &mut Child) -> String {
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                reader.read_line(&mut line).unwrap(),
                0,
                "child exited without reporting instance status"
            );
            if let Some((_, status)) = line.split_once("INSTANCE:") {
                let status = format!("INSTANCE:{}", status.split_whitespace().next().unwrap());
                child.stdout = Some(reader.into_inner());
                return status;
            }
        }
    }

    #[test]
    fn process_fixture() {
        let Some(directory) = std::env::var_os("FT8RS_INSTANCE_TEST_DIR") else {
            return;
        };
        match acquire_in(Path::new(&directory)).unwrap() {
            Startup::Primary(_guard) => {
                println!("INSTANCE:PRIMARY");
                std::io::stdout().flush().unwrap();
                if std::env::var("FT8RS_INSTANCE_TEST_HOLD").as_deref() == Ok("1") {
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line).unwrap();
                }
            }
            Startup::Secondary => println!("INSTANCE:SECONDARY"),
        }
    }

    #[test]
    fn second_process_activates_primary_and_clean_exit_allows_relaunch() {
        let directory = directory();
        let Startup::Primary(primary) = acquire_in(&directory).unwrap() else {
            panic!("first instance")
        };
        let mut secondary = child(&directory, false);
        assert_eq!(result(&mut secondary), "INSTANCE:SECONDARY");
        assert!(secondary.wait().unwrap().success());
        assert!(
            primary.activation.take_request(),
            "secondary must request focus"
        );
        drop(primary);
        assert!(matches!(
            acquire_in(&directory).unwrap(),
            Startup::Primary(_)
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_launches_have_one_owner_and_killed_owner_does_not_leave_a_stale_lock() {
        let directory = directory();
        let mut children: Vec<_> = (0..5).map(|_| child(&directory, true)).collect();
        let statuses: Vec<_> = children.iter_mut().map(result).collect();
        assert_eq!(
            statuses
                .iter()
                .filter(|status| *status == "INSTANCE:PRIMARY")
                .count(),
            1
        );
        for (child, status) in children.iter_mut().zip(&statuses) {
            if status == "INSTANCE:PRIMARY" {
                child.kill().unwrap()
            }
            let exit = child.wait().unwrap();
            if status == "INSTANCE:SECONDARY" {
                assert!(exit.success());
            }
        }
        assert!(
            matches!(acquire_in(&directory).unwrap(), Startup::Primary(_)),
            "kernel must release a crashed process's lock"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn activation_before_graphics_start_is_delivered_when_context_attaches() {
        let activation = Activation::default();
        activation.request();
        let context = egui::Context::default();
        activation.attach(&context);
        assert!(activation.take_request());
        assert!(!activation.take_request());
        let output = context.run_ui(egui::RawInput::default(), |_| {});
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Minimized(false))));
        assert!(commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Focus)));
    }
}
