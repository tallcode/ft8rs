use super::{Error, Sample};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output};

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn checked(
    output: std::io::Result<Output>,
    cancelled: impl FnOnce(&Output) -> bool,
) -> Result<(), Error> {
    let output = output.map_err(|err| Error::System(err.to_string()))?;
    if output.status.success() {
        Ok(())
    } else if cancelled(&output) {
        Err(Error::Cancelled)
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        let detail = error.trim();
        if !detail.is_empty() {
            log::warn!("System clock command failed: {detail}");
        }
        Err(Error::System(output.status.to_string()))
    }
}

#[cfg(target_os = "macos")]
pub(super) fn apply(sample: Sample) -> Result<(), Error> {
    // Execute Apple's system-owned writer after authorization. Passing only the
    // winning numeric IP makes its fresh sample independent of DNS and auth delay.
    let script = r#"on run argv
        do shell script ("/usr/bin/sntp -S -t 3 -n 1 " & quoted form of (item 1 of argv)) with administrator privileges
    end run"#;
    checked(
        Command::new("/usr/bin/osascript")
            .args(["-e", script, &sample.peer.ip().to_string()])
            .output(),
        |output| String::from_utf8_lossy(&output.stderr).contains("(-128)"),
    )
}

#[cfg(target_os = "linux")]
pub(super) fn apply(sample: Sample) -> Result<(), Error> {
    // GNU date evaluates the relative offset after polkit authorization.
    let micros = sample.offset_us.unsigned_abs();
    let sign = if sample.offset_us < 0 { '-' } else { '+' };
    let adjustment = format!(
        "--set={sign}{}.{:06} seconds",
        micros / 1_000_000,
        micros % 1_000_000
    );
    checked(
        Command::new("/usr/bin/pkexec")
            .args([
                "--disable-internal-agent",
                "/usr/bin/date",
                "--utc",
                &adjustment,
            ])
            .output(),
        |output| output.status.code() == Some(126),
    )
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(super) fn apply(_: Sample) -> Result<(), Error> {
    Err(Error::System(
        "System clock setting is unavailable on this platform".into(),
    ))
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, SYSTEMTIME, WAIT_OBJECT_0};
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemTimePreciseAsFileTime, SetSystemTime,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::System::Time::FileTimeToSystemTime;
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    use super::{Error, Sample};

    const HELPER: &str = "--ft8rs-set-clock";

    fn set_offset(offset_us: i64) -> io::Result<()> {
        let mut time = FILETIME::default();
        // SAFETY: the API writes a FILETIME to the supplied initialized storage.
        unsafe { GetSystemTimePreciseAsFileTime(&mut time) };
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        let target = ticks
            .checked_add_signed(offset_us.saturating_mul(10))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "system clock offset out of range",
                )
            })?;
        let time = FILETIME {
            dwLowDateTime: target as u32,
            dwHighDateTime: (target >> 32) as u32,
        };
        let mut utc = SYSTEMTIME::default();
        // SAFETY: the input and output structures are valid for these synchronous calls.
        if unsafe { FileTimeToSystemTime(&time, &mut utc) } == 0
            || unsafe { SetSystemTime(&utc) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    pub(super) fn apply(sample: Sample) -> Result<(), Error> {
        if let Err(error) = set_offset(sample.offset_us) {
            if !matches!(error.raw_os_error(), Some(5 | 1314)) {
                return Err(Error::System(error.to_string()));
            }
        } else {
            return Ok(());
        }

        let executable = std::env::current_exe().map_err(|err| Error::System(err.to_string()))?;
        let executable = wide(executable.as_os_str());
        let verb = wide(OsStr::new("runas"));
        let parameters = wide(OsStr::new(&format!("{HELPER} {}", sample.peer)));
        let mut request = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: verb.as_ptr(),
            lpFile: executable.as_ptr(),
            lpParameters: parameters.as_ptr(),
            nShow: SW_HIDE,
            ..Default::default()
        };
        // SAFETY: the request and nul-terminated strings live through this call.
        if unsafe { ShellExecuteExW(&mut request) } == 0 {
            let error = io::Error::last_os_error();
            return Err(if error.raw_os_error() == Some(1223) {
                Error::Cancelled
            } else {
                Error::System(error.to_string())
            });
        }
        let process = request.hProcess;
        if process.is_null() {
            return Err(Error::System("No handle to the system clock helper".into()));
        }
        let mut code = 0;
        // SAFETY: ShellExecuteExW returned a process handle that we own.
        let exited = unsafe {
            let exited = WaitForSingleObject(process, INFINITE) == WAIT_OBJECT_0
                && GetExitCodeProcess(process, &mut code) != 0;
            CloseHandle(process);
            exited
        };
        match (exited, code) {
            (true, 0) => Ok(()),
            (true, 2) => Err(Error::NoResponse),
            _ => Err(Error::System(
                "The elevated system clock write failed".into(),
            )),
        }
    }

    pub(crate) fn run_write_helper() {
        let mut args = std::env::args_os().skip(1);
        if !args.next().is_some_and(|arg| arg == HELPER) {
            return;
        }
        let peer = args
            .next()
            .and_then(|arg| arg.into_string().ok())
            .and_then(|arg| arg.parse::<std::net::SocketAddr>().ok());
        let code = match peer {
            Some(peer) if peer.port() == 123 && args.next().is_none() => {
                match crate::clock_sync::refresh_peer(peer) {
                    Ok(sample) if set_offset(sample.offset_us).is_ok() => 0,
                    Ok(_) => 1,
                    Err(_) => 2,
                }
            }
            _ => 1,
        };
        std::process::exit(code);
    }
}

#[cfg(windows)]
pub(super) fn apply(sample: Sample) -> Result<(), Error> {
    windows::apply(sample)
}

#[cfg(windows)]
pub(crate) use windows::run_write_helper;
