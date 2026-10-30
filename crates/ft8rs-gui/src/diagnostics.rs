//! Persist startup failures even for the Windows GUI subsystem (no console).
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

struct FileLogger(Mutex<File>);

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
            || ["ft8rs", "eframe", "egui_wgpu", "wgpu"]
                .iter()
                .any(|prefix| metadata.target().starts_with(prefix))
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            if let Ok(mut file) = self.0.lock() {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                let _ = writeln!(
                    file,
                    "{}.{:03} UTC-epoch {} [{}] {}",
                    now.as_secs(),
                    now.subsec_millis(),
                    record.level(),
                    record.target(),
                    record.args(),
                );
                // Write through every entry: panic=abort never runs destructors.
                let _ = file.flush();
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.0.lock() {
            let _ = file.flush();
        }
    }
}

pub fn init() -> Option<PathBuf> {
    let mut directories = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        directories.push(PathBuf::from(local).join("ft8rs").join("logs"));
    }
    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        directories.push(parent.join("logs"));
    }
    directories.push(std::env::temp_dir().join("ft8rs").join("logs"));
    let path = directories.into_iter().find_map(|dir| {
        let (file, path) = open_log(&dir).ok()?;
        let logger = Box::leak(Box::new(FileLogger(Mutex::new(file))));
        log::set_logger(logger).ok()?;
        log::set_max_level(log::LevelFilter::Info);
        Some(path)
    });
    let previous_hook = std::panic::take_hook();
    let panic_log_path = path.clone();
    std::panic::set_hook(Box::new(move |info| {
        report_failure(
            &format!("Application panic: {info}"),
            panic_log_path.as_deref(),
        );
        previous_hook(info);
    }));
    path
}

fn open_log(dir: &Path) -> std::io::Result<(File, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("ft8rs-gui.log");
    // Keep the previous session so relaunching after a crash preserves evidence.
    if path.exists() {
        std::fs::copy(&path, dir.join("ft8rs-gui.previous.log"))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    Ok((file, path))
}

pub fn report_failure(error: &str, log_path: Option<&Path>) {
    log::error!("{error}");
    let location = log_path
        .map(|p| format!("Details: {}", p.display()))
        .unwrap_or_else(|| "Could not create a diagnostic log.".to_string());
    let message = format!("FT8.RS could not continue.\n\n{error}\n\n{location}");
    eprintln!("{message}");
    #[cfg(windows)]
    show_error_dialog(&message);
}

#[cfg(windows)]
fn show_error_dialog(message: &str) {
    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            hwnd: *mut std::ffi::c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let caption: Vec<u16> = "FT8.RS startup error"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: both UTF-16 buffers are NUL-terminated and live for this synchronous
    // call. A null owner is valid even if native window creation failed.
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarting_preserves_the_failed_session_log() {
        let dir = std::env::temp_dir().join(format!(
            "ft8rs-startup-log-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let (mut file, path) = open_log(&dir).unwrap();
        writeln!(file, "graphics initialization failed").unwrap();
        drop(file);
        let (file, _) = open_log(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        assert_eq!(
            std::fs::read_to_string(dir.join("ft8rs-gui.previous.log")).unwrap(),
            "graphics initialization failed\n"
        );
        drop(file);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
