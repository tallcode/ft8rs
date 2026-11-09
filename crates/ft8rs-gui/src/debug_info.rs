//! Session-only diagnostics for the live GUI.
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ft8rs::stream::session::StreamDecodeConfig;
use ft8rs::SlotTimestamp;
use ft8rs_engine::protocol::{CapturedSlot, DecodeRecord, EngineEvent, MonitorObserver};
use ft8rs_engine::soundcard::SoundcardFormatInfo;
use serde_json::{json, Value};

use crate::wsjtx_udp::ExternalDecode;

const SLOT_MS: i64 = 15_000;
const KEEP_SLOTS: i64 = 8;
const SLOTS_PER_DAY: i64 = 86_400_000 / SLOT_MS;
const GRACE_MS: i64 = 10_000;
const MAX_DECODE_ROWS: usize = 512;
const MAX_LOG_LINES: usize = 256;
const MAX_LOG_BYTES: usize = 4096;
const AUDIO_RATE: u32 = 12_000;

pub(crate) fn eligible(call: &str) -> bool {
    call.trim().eq_ignore_ascii_case("BG5ATV")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn slot_timestamp(slot: i64) -> SlotTimestamp {
    SlotTimestamp::from_unix_seconds_utc(slot * (SLOT_MS / 1000))
}

// The UDP protocol only supplies time of day. Resolve the date against
// reception, so a late 23:59:45 packet after midnight keeps yesterday's slot.
fn received_slot(ms_since_midnight: u32, received_ms: i64) -> i64 {
    let now = received_ms.div_euclid(SLOT_MS);
    let today = now.div_euclid(SLOTS_PER_DAY) * SLOTS_PER_DAY;
    let slot = today + i64::from(ms_since_midnight) / SLOT_MS;
    [slot - SLOTS_PER_DAY, slot, slot + SLOTS_PER_DAY]
        .into_iter()
        .min_by_key(|candidate| candidate.abs_diff(now))
        .unwrap()
}

#[derive(Clone)]
struct Audio {
    samples: Arc<[f32]>,
    config: StreamDecodeConfig,
    device_name: String,
    format: SoundcardFormatInfo,
    channel: &'static str,
    native_samples: usize,
    xruns: u64,
    jitter_ms: i64,
}

#[derive(Clone)]
struct Observed<T> {
    at_ms: i64,
    value: T,
}

#[derive(Clone, Default)]
struct Slot {
    audio: Option<Audio>,
    local: Vec<Observed<DecodeRecord>>,
    udp: Vec<Observed<(SocketAddr, ExternalDecode)>>,
    logs: Vec<Observed<String>>,
    dropped_local: usize,
    dropped_udp: usize,
    dropped_logs: usize,
}

fn push_bounded<T>(rows: &mut Vec<T>, row: T, limit: usize, dropped: &mut usize) {
    if rows.len() < limit {
        rows.push(row);
    } else {
        *dropped = dropped.saturating_add(1);
    }
}

#[derive(Clone, Copy)]
struct Window {
    first: i64,
    last: i64,
}

impl Window {
    fn retains(self, slot: i64) -> bool {
        // Post-boundary diagnostics can land in the following slot.
        (self.first..=self.last + 1).contains(&slot)
    }

    fn cutoff_ms(self) -> i64 {
        (self.last + 1) * SLOT_MS + GRACE_MS
    }
}

struct Request {
    window: Window,
    requested_ms: i64,
    ready_at: Instant,
    udp_listener: Option<String>,
}

struct Session {
    first: i64,
    slots: BTreeMap<i64, Arc<Slot>>,
    latest_audio: Option<i64>,
    pending: Option<Request>,
    cancelled: Arc<Mutex<bool>>,
}

impl Session {
    fn window(&self) -> Option<Window> {
        self.latest_audio.map(|last| Window {
            first: self.first.max(last - KEEP_SLOTS + 1),
            last,
        })
    }

    fn prune(&mut self, at_ms: i64) {
        let current = at_ms.div_euclid(SLOT_MS);
        let recent = self.window();
        let pending = self.pending.as_ref().map(|req| req.window);
        self.slots.retain(|&slot, _| {
            (current - KEEP_SLOTS..=current).contains(&slot)
                || recent.is_some_and(|window| window.retains(slot))
                || pending.is_some_and(|window| window.retains(slot))
        });
    }

    fn decode_slot(&mut self, slot: i64, at_ms: i64) -> Option<&mut Slot> {
        let current = at_ms.div_euclid(SLOT_MS);
        if slot < self.first || !(current - KEEP_SLOTS..=current).contains(&slot) {
            return None;
        }
        self.prune(at_ms);
        Some(Arc::make_mut(self.slots.entry(slot).or_default()))
    }
}

#[derive(Default)]
pub(crate) struct Recorder {
    active: AtomicBool,
    session: Mutex<Option<Session>>,
}

impl Recorder {
    fn set_active(&self, enabled: bool, at_ms: i64) {
        let mut session = self.session.lock().unwrap();
        if enabled {
            if session.is_none() {
                *session = Some(Session {
                    first: at_ms.div_euclid(SLOT_MS) + 1,
                    slots: BTreeMap::new(),
                    latest_audio: None,
                    pending: None,
                    cancelled: Arc::new(Mutex::new(false)),
                });
            }
            self.active.store(true, Ordering::Release);
        } else {
            self.active.store(false, Ordering::Release);
            let previous = session.take();
            drop(session);
            if let Some(previous) = previous {
                *previous.cancelled.lock().unwrap() = true;
            }
        }
    }

    fn record(&self, f: impl FnOnce(&mut Session)) {
        if self.active.load(Ordering::Acquire) {
            if let Some(session) = self.session.lock().unwrap().as_mut() {
                f(session);
            }
        }
    }

    pub(crate) fn udp(&self, peer: SocketAddr, decode: &ExternalDecode) {
        if self.active.load(Ordering::Acquire) {
            self.record_udp(now_ms(), peer, decode);
        }
    }

    fn record_udp(&self, at_ms: i64, peer: SocketAddr, decode: &ExternalDecode) {
        self.record(|session| {
            let slot = received_slot(decode.ms_since_midnight, at_ms);
            if let Some(data) = session.decode_slot(slot, at_ms) {
                push_bounded(
                    &mut data.udp,
                    Observed {
                        at_ms,
                        value: (peer, decode.clone()),
                    },
                    MAX_DECODE_ROWS,
                    &mut data.dropped_udp,
                );
            }
        });
    }

    pub(crate) fn log(&self, at_ms: i64, line: fmt::Arguments<'_>) {
        self.record(|session| {
            let slot = at_ms.div_euclid(SLOT_MS);
            if slot < session.first {
                return;
            }
            session.prune(at_ms);
            let data = Arc::make_mut(session.slots.entry(slot).or_default());
            if data.logs.len() == MAX_LOG_LINES {
                data.dropped_logs = data.dropped_logs.saturating_add(1);
                return;
            }
            let mut value = line.to_string();
            value.truncate(value.floor_char_boundary(MAX_LOG_BYTES.min(value.len())));
            data.logs.push(Observed { at_ms, value });
        });
    }

    fn audio(&self, slot: i64, at_ms: i64, captured: CapturedSlot<'_>) {
        self.record(|session| {
            if slot < session.first || session.latest_audio.is_some_and(|latest| slot <= latest) {
                return;
            }
            session.latest_audio = Some(slot);
            session.prune(at_ms);
            Arc::make_mut(session.slots.entry(slot).or_default()).audio = Some(Audio {
                samples: Arc::from(captured.samples_12k),
                config: captured.config.clone(),
                device_name: captured.device_name.to_owned(),
                format: captured.format.clone(),
                channel: captured.channel.as_str(),
                native_samples: captured.native_samples,
                xruns: captured.xruns,
                jitter_ms: captured.jitter_ms,
            });
        });
    }

    fn local(&self, at_ms: i64, decode: &DecodeRecord) {
        self.record(|session| {
            let slot = received_slot(decode.timestamp.milliseconds_since_midnight(), at_ms);
            if decode.timestamp != slot_timestamp(slot) {
                return;
            }
            if let Some(data) = session.decode_slot(slot, at_ms) {
                push_bounded(
                    &mut data.local,
                    Observed {
                        at_ms,
                        value: decode.clone(),
                    },
                    MAX_DECODE_ROWS,
                    &mut data.dropped_local,
                );
            }
        });
    }

    fn request(&self, at_ms: i64, udp_listener: Option<String>) {
        self.record(|session| {
            if let Some(window) = session.window() {
                let wait_ms = (window.cutoff_ms() - at_ms).clamp(0, GRACE_MS) as u64;
                session.pending = Some(Request {
                    window,
                    requested_ms: at_ms,
                    ready_at: Instant::now() + Duration::from_millis(wait_ms),
                    udp_listener,
                });
            }
        });
    }

    fn ready(&self, now: Instant) -> Option<Export> {
        let mut guard = self.session.lock().unwrap();
        let session = guard.as_mut()?;
        if now < session.pending.as_ref()?.ready_at {
            return None;
        }
        let request = session.pending.take()?;
        let slots = session
            .slots
            .range(request.window.first..=request.window.last + 1)
            .map(|(&slot, data)| (slot, Arc::clone(data)))
            .collect();
        Some(Export {
            request,
            slots,
            cancelled: Arc::clone(&session.cancelled),
        })
    }
}

impl MonitorObserver for Recorder {
    fn slot(&self, captured: CapturedSlot<'_>) {
        if self.active.load(Ordering::Acquire) {
            let at_ms = now_ms();
            let slot = received_slot(captured.timestamp.milliseconds_since_midnight(), at_ms);
            if *captured.timestamp == slot_timestamp(slot) {
                self.audio(slot, at_ms, captured);
            }
        }
    }

    fn event(&self, event: &EngineEvent) {
        if self.active.load(Ordering::Acquire) {
            match event {
                EngineEvent::Decode(decode) => self.local(now_ms(), decode),
                EngineEvent::Error(error) => {
                    let at_ms = now_ms();
                    self.log(
                        at_ms,
                        format_args!("{at_ms} UTC-epoch-ms ERROR [engine] {error}"),
                    );
                }
                _ => {}
            }
        }
    }
}

pub(crate) struct DebugInfo {
    pub(crate) recorder: Arc<Recorder>,
    root: Result<PathBuf, String>,
    writer: Option<JoinHandle<Result<Option<PathBuf>, String>>>,
}

impl DebugInfo {
    pub(crate) fn new(recorder: Arc<Recorder>) -> Self {
        let root = std::env::current_exe()
            .map_err(|err| err.to_string())
            .and_then(|exe| {
                let app_dir = if cfg!(target_os = "macos") {
                    exe.ancestors()
                        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
                        .and_then(Path::parent)
                        .or_else(|| exe.parent())
                } else {
                    exe.parent()
                };
                app_dir.map(|dir| dir.join("debug-info")).ok_or_else(|| {
                    format!(
                        "Cannot locate the application directory for {}",
                        exe.display()
                    )
                })
            });
        Self {
            recorder,
            root,
            writer: None,
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.recorder.active.load(Ordering::Acquire)
    }

    pub(crate) fn set_active(&self, enabled: bool) {
        self.recorder.set_active(enabled, now_ms());
    }

    pub(crate) fn can_save(&self) -> bool {
        self.writer.is_none()
            && self.active()
            && self
                .recorder
                .session
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|session| session.latest_audio.is_some() && session.pending.is_none())
    }

    pub(crate) fn save(&self, udp_listener: Option<String>) -> Result<(), String> {
        if self.can_save() {
            self.root.as_ref().map_err(Clone::clone)?;
            self.recorder.request(now_ms(), udp_listener);
        }
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Result<Option<PathBuf>, String> {
        if self.writer.as_ref().is_some_and(|task| task.is_finished()) {
            return self
                .writer
                .take()
                .unwrap()
                .join()
                .map_err(|_| "Debug information export worker stopped".to_owned())?;
        }
        if self.writer.is_none() {
            if let Some(export) = self.recorder.ready(Instant::now()) {
                let root = self.root.clone()?;
                self.writer = Some(
                    thread::Builder::new()
                        .name("ft8rs-debug-export".into())
                        .spawn(move || {
                            export
                                .write(&root)
                                .map_err(|err| format!("{}: {err}", root.display()))
                        })
                        .map_err(|err| err.to_string())?,
                );
            }
        }
        Ok(None)
    }

    pub(crate) fn open_directory(&self) -> Result<(), String> {
        let root = self.root.as_ref().map_err(Clone::clone)?;
        let command = if cfg!(target_os = "windows") {
            "explorer.exe"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        fs::create_dir_all(root).map_err(|err| format!("{}: {err}", root.display()))?;
        let mut child = Command::new(command)
            .arg(root)
            .spawn()
            .map_err(|err| format!("{}: {err}", root.display()))?;
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

impl Drop for DebugInfo {
    fn drop(&mut self) {
        self.set_active(false);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

struct Export {
    request: Request,
    slots: BTreeMap<i64, Arc<Slot>>,
    cancelled: Arc<Mutex<bool>>,
}

impl Export {
    fn write(self, root: &Path) -> io::Result<Option<PathBuf>> {
        if *self.cancelled.lock().unwrap() {
            return Ok(None);
        }
        fs::create_dir_all(root)?;
        let time = SlotTimestamp::from_unix_seconds_utc(self.request.requested_ms / 1000);
        let base = format!("{time}Z");
        let mut index = 1;
        let (destination, mut staging) = loop {
            let name = if index == 1 {
                base.clone()
            } else {
                format!("{base}_{index}")
            };
            let destination = root.join(&name);
            if !destination.try_exists()? {
                let path = root.join(format!(".{name}.partial"));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        break (
                            destination,
                            Staging {
                                path,
                                published: false,
                            },
                        )
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(err) => return Err(err),
                }
            }
            index += 1;
        };
        self.write_files(&staging.path)?;
        // Linearize publication with unchecking the capture option. Files are
        // closed before this lock, and cancellation removes only staging.
        let cancelled = self.cancelled.lock().unwrap();
        if *cancelled {
            return Ok(None);
        }
        fs::rename(&staging.path, &destination)?;
        staging.published = true;
        Ok(Some(destination))
    }

    fn write_files(&self, path: &Path) -> io::Result<()> {
        let audio_dir = path.join("audio");
        fs::create_dir(&audio_dir)?;
        let mut local = BufWriter::new(File::create(path.join("local-decodes.jsonl"))?);
        let mut udp = BufWriter::new(File::create(path.join("udp-decodes.jsonl"))?);
        let mut logs = BufWriter::new(File::create(path.join("app.log"))?);
        let mut slots = Vec::new();
        let window = self.request.window;
        let cutoff_ms = window.cutoff_ms();
        let empty = Slot::default();
        for slot in window.first..=window.last + 1 {
            if *self.cancelled.lock().unwrap() {
                return Ok(());
            }
            let data = self.slots.get(&slot).map(Arc::as_ref).unwrap_or(&empty);
            for log in data.logs.iter().filter(|log| log.at_ms <= cutoff_ms) {
                writeln!(logs, "{}", log.value)?;
            }
            if slot > window.last {
                break;
            }
            let timestamp = slot_timestamp(slot).format();
            for entry in data.local.iter().filter(|row| row.at_ms <= cutoff_ms) {
                let record = &entry.value;
                write_json_line(
                    &mut local,
                    &json!({
                        "slot_utc": timestamp, "forwarded_at_unix_ms": entry.at_ms,
                        "snr_db": record.row.snr, "dt_seconds": record.row.dt,
                        "frequency_hz": record.row.freq, "message": record.row.msg,
                        "stage": record.stage, "provenance": record.provenance,
                    }),
                )?;
            }
            for entry in data.udp.iter().filter(|row| row.at_ms <= cutoff_ms) {
                let (peer, record) = &entry.value;
                write_json_line(
                    &mut udp,
                    &json!({
                        "slot_utc": timestamp, "received_at_unix_ms": entry.at_ms,
                        "packet_ms_since_midnight": record.ms_since_midnight, "peer": peer,
                        "snr_db": record.snr, "dt_seconds": record.dt,
                        "frequency_hz": record.freq, "message": record.message,
                    }),
                )?;
            }
            let audio = if let Some(audio) = &data.audio {
                let name = format!("{timestamp}.wav");
                write_wav(&audio_dir.join(&name), &audio.samples)?;
                json!({
                    "file": format!("audio/{name}"),
                    "samples": audio.samples.len(), "sample_rate": AUDIO_RATE,
                    "channels": 1, "sample_format": "float32",
                    "short": audio.samples.len() < AUDIO_RATE as usize * 15,
                    "device": audio.device_name, "channel": audio.channel,
                    "native_sample_rate": audio.format.sample_rate,
                    "native_channels": audio.format.channels,
                    "native_sample_format": audio.format.sample_format,
                    "native_samples": audio.native_samples, "capture_xruns": audio.xruns,
                    "capture_jitter_ms": audio.jitter_ms, "decode_config": audio.config,
                })
            } else {
                Value::Null
            };
            slots.push(json!({
                "slot_utc": timestamp, "slot_start_unix_ms": slot * SLOT_MS, "audio": audio,
                "dropped_local": data.dropped_local, "dropped_udp": data.dropped_udp,
                "dropped_log_lines": data.dropped_logs,
            }));
        }
        local.flush()?;
        udp.flush()?;
        logs.flush()?;
        let mut manifest = BufWriter::new(File::create(path.join("manifest.json"))?);
        serde_json::to_writer_pretty(
            &mut manifest,
            &json!({
                "format_version": 1, "application_version": env!("FT8RS_VERSION"),
                "requested_at_unix_ms": self.request.requested_ms,
                "last_observation_unix_ms": cutoff_ms,
                "udp_listener_at_save": self.request.udp_listener, "slots": slots,
                "tail_dropped_log_lines": self.slots.get(&(window.last + 1))
                    .map_or(0, |slot| slot.dropped_logs),
            }),
        )?;
        manifest.flush()
    }
}

struct Staging {
    path: PathBuf,
    published: bool,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn write_json_line(output: &mut impl Write, row: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, row)?;
    writeln!(output)
}

fn write_wav(path: &Path, samples: &[f32]) -> io::Result<()> {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: AUDIO_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .map_err(io::Error::other)?;
    for &sample in samples {
        writer.write_sample(sample).map_err(io::Error::other)?;
    }
    writer.finalize().map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ft8rs::stream::session::StreamDecodeProvenance;
    use ft8rs::StreamDecodedMessage;
    use ft8rs_engine::protocol::DecodeStage;
    use ft8rs_engine::soundcard::InputChannel;

    const MIDNIGHT: i64 = 20_766 * SLOTS_PER_DAY;
    static TEST_DIR_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn record_audio(recorder: &Recorder, slot: i64, samples: &[f32]) {
        recorder.audio(
            slot,
            (slot + 1) * SLOT_MS,
            CapturedSlot {
                timestamp: &slot_timestamp(slot),
                samples_12k: samples,
                config: &StreamDecodeConfig::default(),
                device_name: "test input",
                format: &SoundcardFormatInfo {
                    channels: 2,
                    sample_rate: 48_000,
                    sample_format: "F32".into(),
                },
                channel: InputChannel::Left,
                native_samples: samples.len() * 4,
                xruns: 2,
                jitter_ms: 3,
            },
        );
    }

    fn test_directory() -> Staging {
        let path = std::env::temp_dir().join(format!(
            "ft8rs-debug-test-{}-{}-{}",
            std::process::id(),
            now_ms(),
            TEST_DIR_ID.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Staging {
            path,
            published: false,
        }
    }

    #[test]
    fn capture_is_station_gated_and_reenable_starts_with_an_empty_window() {
        assert!(eligible(" bg5atv "));
        assert!(!eligible("BG5ATV/P"));
        assert!(!eligible(""));

        let recorder = Recorder::default();
        let slot = MIDNIGHT;
        record_audio(&recorder, slot, &[0.25]);
        assert!(recorder.session.lock().unwrap().is_none());
        recorder.set_active(true, slot * SLOT_MS + 1000);
        record_audio(&recorder, slot, &[0.25]);
        assert!(recorder
            .session
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .window()
            .is_none());
        record_audio(&recorder, slot + 1, &[0.25]);
        recorder.request((slot + 2) * SLOT_MS, None);
        assert!(recorder.ready(Instant::now()).is_none());
        recorder.set_active(false, (slot + 2) * SLOT_MS);
        assert!(recorder.session.lock().unwrap().is_none());

        recorder.set_active(true, (slot + 2) * SLOT_MS);
        assert!(recorder
            .ready(Instant::now() + Duration::from_secs(11))
            .is_none());
        let session = recorder.session.lock().unwrap();
        let session = session.as_ref().unwrap();
        assert!(session.slots.is_empty());
        assert!(session.window().is_none());
    }

    #[test]
    fn rolling_window_keeps_eight_and_freezes_selection_at_save_click() {
        let recorder = Recorder::default();
        recorder.set_active(true, MIDNIGHT * SLOT_MS - 1);
        for slot in MIDNIGHT..MIDNIGHT + 9 {
            record_audio(&recorder, slot, &[0.125]);
        }
        {
            let guard = recorder.session.lock().unwrap();
            let session = guard.as_ref().unwrap();
            assert!(!session.slots.contains_key(&MIDNIGHT));
            assert_eq!(
                session
                    .slots
                    .values()
                    .filter(|slot| slot.audio.is_some())
                    .count(),
                8
            );
        }
        recorder.request((MIDNIGHT + 9) * SLOT_MS, None);
        record_audio(&recorder, MIDNIGHT + 9, &[0.25]);
        let export = recorder
            .ready(Instant::now() + Duration::from_secs(11))
            .unwrap();
        assert_eq!(export.request.window.first, MIDNIGHT + 1);
        assert_eq!(export.request.window.last, MIDNIGHT + 8);
        assert!(export.slots[&(MIDNIGHT + 1)].audio.is_some());
    }

    #[test]
    fn export_round_trips_audio_and_keeps_late_decodes_and_logs_for_their_slot() {
        let recorder = Recorder::default();
        let slot = MIDNIGHT - 1;
        let slot_ms = slot * SLOT_MS;
        let boundary_ms = MIDNIGHT * SLOT_MS;
        let directory = test_directory();
        let root = directory.path.join("debug-info");
        recorder.set_active(true, slot_ms - 1);
        let samples = [0.125, -0.75, 0.33];
        record_audio(&recorder, slot, &samples);
        recorder.log(slot_ms - 1, format_args!("before capture"));
        recorder.log(slot_ms + 1, format_args!("in slot"));
        assert!(!root.exists());

        recorder.request(boundary_ms + 1, Some("127.0.0.1:2237".into()));
        let message = "CQ K1ABC FN42";
        recorder.local(
            boundary_ms + 1100,
            &DecodeRecord {
                timestamp: slot_timestamp(slot),
                row: StreamDecodedMessage {
                    freq: 1200.0,
                    dt: 0.3,
                    snr: -17.0,
                    sync: 1.0,
                    msg: message.into(),
                    itone: [0; 79],
                },
                provenance: StreamDecodeProvenance::Experiment,
                stage: DecodeStage::Final,
            },
        );
        let udp = ExternalDecode {
            ms_since_midnight: slot_timestamp(slot).milliseconds_since_midnight(),
            snr: -19,
            dt: 0.4,
            freq: 1201,
            message: message.into(),
        };
        let peer = "127.0.0.1:2238".parse().unwrap();
        recorder.record_udp(boundary_ms + 1900, peer, &udp);
        recorder.record_udp(boundary_ms + GRACE_MS + 1, peer, &udp);
        recorder.log(boundary_ms + 2000, format_args!("last-slot worker done"));
        recorder.log(
            boundary_ms + GRACE_MS + 1,
            format_args!("outside export window"),
        );

        let export = recorder
            .ready(Instant::now() + Duration::from_secs(11))
            .unwrap();
        let saved = export.write(&root).unwrap().unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let audio_path = saved
            .join("audio")
            .join(format!("{}.wav", slot_timestamp(slot)));
        assert_eq!(
            ft8rs::input::file::infer_start_time_from_path(&audio_path),
            Some(slot_timestamp(slot))
        );
        let audio = ft8rs::input::audio::read_wav_mono_f32(&audio_path).unwrap();
        assert_eq!(audio.samples, samples);
        assert_eq!(audio.sample_rate, AUDIO_RATE);

        let read_record = |name| -> Value {
            let content = fs::read_to_string(saved.join(name)).unwrap();
            assert_eq!(content.lines().count(), 1);
            serde_json::from_str(&content).unwrap()
        };
        let local = read_record("local-decodes.jsonl");
        assert_eq!(local["message"], message);
        assert_eq!(local["provenance"], "Experiment");
        let external = read_record("udp-decodes.jsonl");
        assert_eq!(external["slot_utc"], slot_timestamp(slot).format());
        assert_eq!(external["received_at_unix_ms"], boundary_ms + 1900);
        assert_eq!(external["peer"], "127.0.0.1:2238");
        let logs = fs::read_to_string(saved.join("app.log")).unwrap();
        assert_eq!(logs, "in slot\nlast-slot worker done\n");
        let manifest: Value =
            serde_json::from_reader(File::open(saved.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["slots"][0]["audio"]["device"], "test input");
        assert_eq!(manifest["slots"][0]["audio"]["capture_xruns"], 2);
        assert_eq!(manifest["udp_listener_at_save"], "127.0.0.1:2237");
    }

    #[test]
    fn failed_export_preserves_capture_and_cancelled_export_never_publishes() {
        let directory = test_directory();
        let root = directory.path.join("debug-info");
        let recorder = Recorder::default();
        recorder.set_active(true, MIDNIGHT * SLOT_MS - 1);
        record_audio(&recorder, MIDNIGHT, &[0.125]);
        let at_ms = (MIDNIGHT + 2) * SLOT_MS;

        File::create(&root).unwrap();
        recorder.request(at_ms, None);
        assert!(recorder
            .ready(Instant::now())
            .unwrap()
            .write(&root)
            .is_err());
        fs::remove_file(&root).unwrap();
        recorder.request(at_ms, None);
        let saved = recorder
            .ready(Instant::now())
            .unwrap()
            .write(&root)
            .unwrap()
            .unwrap();
        recorder.request(at_ms, None);
        let cancelled = recorder.ready(Instant::now()).unwrap();
        recorder.set_active(false, at_ms);
        assert_eq!(cancelled.write(&root).unwrap(), None);
        assert!(saved.join("manifest.json").is_file());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn exported_recording_replays_the_same_decode_rows() {
        use ft8rs::input::file::{decode_wav_file, FileDecodeOptions};

        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../ft8rs-core/tests/ft8/210703_133430.wav");
        let original = ft8rs::input::audio::read_wav_mono_f32(&fixture).unwrap();
        assert_eq!(original.sample_rate, AUDIO_RATE);
        let recorder = Recorder::default();
        recorder.set_active(true, MIDNIGHT * SLOT_MS - 1);
        record_audio(&recorder, MIDNIGHT, &original.samples);
        recorder.request((MIDNIGHT + 2) * SLOT_MS, None);
        let directory = test_directory();
        let saved = recorder
            .ready(Instant::now())
            .unwrap()
            .write(&directory.path)
            .unwrap()
            .unwrap();
        let timestamp = slot_timestamp(MIDNIGHT);
        let exported = saved.join("audio").join(format!("{timestamp}.wav"));
        let options = FileDecodeOptions {
            start_time: timestamp,
            config: StreamDecodeConfig::default(),
        };
        let rows = |path| {
            decode_wav_file(path, options.clone())
                .unwrap()
                .into_iter()
                .map(|row| json!([row.timestamp.format(), row.decode]))
                .collect::<Vec<_>>()
        };
        let baseline = rows(fixture);
        assert!(!baseline.is_empty());
        assert_eq!(rows(exported), baseline);
    }
}
