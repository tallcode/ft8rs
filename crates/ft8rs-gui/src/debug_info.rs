//! Session-only diagnostics for the live GUI.
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs;
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

use crate::wsjtx_udp::ExternalDecode;

mod export;
#[cfg(test)]
mod tests;

use export::Export;

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
    fn len(self) -> i64 {
        self.last - self.first + 1
    }

    fn retains(self, slot: i64) -> bool {
        // Post-boundary diagnostics can land in the following slot.
        (self.first..=self.last + 1).contains(&slot)
    }

    fn cutoff_ms(self) -> i64 {
        (self.last + 1) * SLOT_MS + GRACE_MS
    }
}

struct Request {
    window: Option<Window>,
    segments: Vec<Segment>,
    requested_ms: i64,
    ready_at: Instant,
    udp_listener: Option<String>,
}

#[derive(Clone)]
struct Segment {
    id: u64,
    window: Window,
    slots: BTreeMap<i64, Arc<Slot>>,
}

struct Session {
    first: i64,
    slots: BTreeMap<i64, Arc<Slot>>,
    latest_audio: Option<i64>,
    pending: Option<Request>,
    segment: u64,
    sealed: VecDeque<Segment>,
    cancelled: Arc<Mutex<bool>>,
}

impl Session {
    fn snapshot(&self, window: Window) -> Segment {
        Segment {
            id: self.segment,
            window,
            slots: self
                .slots
                .range(window.first..=window.last + 1)
                .map(|(&slot, data)| (slot, Arc::clone(data)))
                .collect(),
        }
    }

    fn trim_sealed(&mut self) {
        let old: i64 = self.sealed.iter().map(|segment| segment.window.len()).sum();
        let mut excess = old + self.window().map_or(0, Window::len) - KEEP_SLOTS;
        while excess > 0 {
            let segment = self.sealed.front_mut().unwrap();
            let count = segment.window.len();
            if count <= excess {
                self.sealed.pop_front();
                excess -= count;
            } else {
                segment.window.first += excess;
                segment.slots = segment.slots.split_off(&segment.window.first);
                break;
            }
        }
    }

    fn clock_changed(&mut self, at_ms: i64) {
        if let Some(mut request) = self.pending.take() {
            if let Some(window) = request.window.take() {
                request.segments.push(self.snapshot(window));
            }
            request.ready_at = Instant::now();
            self.pending = Some(request);
        }
        if let Some(window) = self.window() {
            self.sealed.push_back(self.snapshot(window));
        }
        self.segment += 1;
        self.first = at_ms.div_euclid(SLOT_MS) + 1;
        self.slots.clear();
        self.latest_audio = None;
        self.trim_sealed();
    }

    fn window(&self) -> Option<Window> {
        self.latest_audio.map(|last| Window {
            first: self.first.max(last - KEEP_SLOTS + 1),
            last,
        })
    }

    fn prune(&mut self, at_ms: i64) {
        let current = at_ms.div_euclid(SLOT_MS);
        let recent = self.window();
        let pending = self.pending.as_ref().and_then(|req| req.window);
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
                    segment: 0,
                    sealed: VecDeque::new(),
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
            session.trim_sealed();
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
            let window = session.window();
            if window.is_some() || !session.sealed.is_empty() {
                let wait_ms = window
                    .map_or(0, |window| (window.cutoff_ms() - at_ms).clamp(0, GRACE_MS))
                    as u64;
                session.pending = Some(Request {
                    window,
                    segments: session.sealed.iter().cloned().collect(),
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
        let mut request = session.pending.take()?;
        if let Some(window) = request.window.take() {
            request.segments.push(session.snapshot(window));
        }
        Some(Export {
            request,
            cancelled: Arc::clone(&session.cancelled),
        })
    }
}

impl MonitorObserver for Recorder {
    fn slot(&self, captured: CapturedSlot<'_>) {
        if self.active.load(Ordering::Acquire) {
            if let Some(seconds) = captured.timestamp.unix_seconds_utc() {
                self.audio(seconds.div_euclid(SLOT_MS / 1000), now_ms(), captured);
            }
        }
    }

    fn event(&self, event: &EngineEvent) {
        if self.active.load(Ordering::Acquire) {
            match event {
                EngineEvent::Decode(decode) => self.local(now_ms(), decode),
                EngineEvent::ClockDiscontinuity { at_ms, .. } => {
                    self.record(|session| session.clock_changed(*at_ms));
                }
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
                .is_some_and(|session| {
                    (session.latest_audio.is_some() || !session.sealed.is_empty())
                        && session.pending.is_none()
                })
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
