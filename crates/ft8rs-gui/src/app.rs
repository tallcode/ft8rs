//! ft8.rs GUI — egui front-end over the live engine.
//!
//! Monitor-only (file decode stays in the CLI). The four hot fields
//! (mycall/hiscall/hisgrid/nfqso) + Monitor button sit at the bottom; everything
//! else lives in the Settings dialog (tabs). Edits apply live while monitoring
//! via `EngineCommand::ApplyState` (the engine plans L0/L1/L2 and reconfigures at
//! the next slot boundary). The decode table mirrors the CLI columns; slot
//! separators show only the time.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, RichText};

use crate::bands::CurrentBand;
use crate::clock_sync;
use crate::debug_info::{self, DebugInfo, Recorder};
use crate::fonts::{
    install as install_fonts, FontSupport, BODY_FONT_SIZE, CHINESE_UNAVAILABLE, SMALL_FONT_SIZE,
};
use crate::i18n::{Language, Locale};

use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance};
use ft8rs_engine::protocol::{
    DxContextSnapshot, EngineCommand, EngineEvent, EngineStatus, HisgridSource,
};
use ft8rs_engine::reconfig::{plan_reconfig, EngineState};
use ft8rs_engine::report::UdpConfig;
use ft8rs_engine::soundcard::{InputChannel, SoundcardDeviceInfo};
use ft8rs_engine::EngineHandle;

/// Hard cap on retained decode rows. Painting is virtualized; DecodeBatch
/// applies this limit after each insertion and compacts once per input frame.
const MAX_ROWS: usize = 20_000;
/// Cooperative input budget; final batch compaction happens once afterwards.
const EVENT_BUDGET: Duration = Duration::from_millis(2);
// High bits distinguish UTC timelines. Low bits retain the absolute UTC slot
// for the displayed date and time.
const CLOCK_SEGMENT_STRIDE: u64 = 1 << 40;
const DIALOG_FOCUS_ATTEMPTS: u8 = 3;
const DIALOG_FOCUS_INTERVAL: Duration = Duration::from_millis(100);
/// Slightly larger than WSJT-X's default 10 pt (~13.33 logical pixels at
/// Windows' 96-DPI baseline), for easier reading in the receive log.
const DECODE_FONT_SIZE: f32 = 14.0;
/// Breathing room above and below each decode line, in logical pixels.
const DECODE_ROW_PADDING_Y: f32 = 1.0;
/// Internal left padding for the decode table (header + rows go edge-to-edge).
const TABLE_PAD: f32 = 12.0;
const SETTINGS_HEADING_TOP_SPACE: f32 = 2.0;
const SETTINGS_FORM_BOTTOM_SPACE: f32 = 10.0;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum SettingsTab {
    General,
    Audio,
    Decode,
    Frequency,
    Output,
    Compare,
    Advanced,
}

const SETTINGS_TABS: [(SettingsTab, &str); 7] = [
    (SettingsTab::General, "General"),
    (SettingsTab::Audio, "Audio"),
    (SettingsTab::Decode, "Decode"),
    (SettingsTab::Frequency, "Frequency"),
    (SettingsTab::Compare, "Compare"),
    (SettingsTab::Output, "Output"),
    (SettingsTab::Advanced, "Advanced"),
];

/// Where a decode row came from, for the compare-mode coloring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DecodeSource {
    Local,
    External,
    Both,
}

#[cfg_attr(test, derive(Clone, Debug, PartialEq))]
struct Row {
    text: String,
    // Merge keys (compare mode): same real slot + freq within tolerance +
    // compatible message tokens identify the same transmission across the two
    // decoders. `gen` is the absolute UTC slot number, so adjacent slots can
    // arrive out of order and rows from different days cannot collide.
    gen: u64,
    // Label captured when the first decode for this slot entered the log.
    slot_band: Option<&'static str>,
    freq: f64,
    dt: f64,
    tokens: Vec<String>,
    source: DecodeSource,
    filterable: Option<FilterableMessage>,
}

impl Row {
    fn local_filterable(&self) -> Option<FilterableMessage> {
        if self.source == DecodeSource::Local {
            self.filterable
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
enum DisplayItem {
    Decode(usize),
    Separator(u64, Option<&'static str>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterableMessage {
    Contest,
    Telemetry,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DisplayFilters {
    hide_contest: bool,
    hide_telemetry: bool,
}

impl DisplayFilters {
    const NONE: Self = Self {
        hide_contest: false,
        hide_telemetry: false,
    };

    fn hides(self, row: &Row) -> bool {
        match row.local_filterable() {
            Some(FilterableMessage::Contest) => self.hide_contest,
            Some(FilterableMessage::Telemetry) => self.hide_telemetry,
            None => false,
        }
    }
}

pub struct Ft8rsApp {
    debug_info: DebugInfo,
    debug_result: Option<Result<std::path::PathBuf, String>>,
    engine: EngineHandle,
    clock_sync: Option<Receiver<clock_sync::Update>>,
    clock_status: Option<clock_sync::Update>,
    clock_segment: u64,
    clock_boundary: Option<(Instant, u64)>,
    activation: Arc<crate::single_instance::Activation>,
    main_focus: DialogFocus,

    // Hot fields (bottom bar).
    mycall: String,
    hiscall: String,
    hisgrid: String,
    nfqso: String,

    // Settings.
    language: Language,
    locale: Locale,
    requested_locale: Locale,
    font_support: FontSupport,
    locale_checked_at: Instant,
    profile: DecodeProfile,
    swl: bool,
    hybrid_experiment: bool,
    nfa: String,
    nfb: String,
    mygrid: String,
    udp_on: bool,
    udp_host: String,
    udp_port: String,
    hide_hash: bool,
    display_filters: DisplayFilters,

    // Compare (GUI-only): listen to WSJT-X UDP decodes and merge/dedupe them
    // with the local decoder's, coloring by source.
    udp_in_on: bool,
    udp_in_host: String,
    udp_in_port: String,
    udp_in_tol: f64,

    devices: Vec<SoundcardDeviceInfo>,
    selected_device: Option<String>, // id:<backend-qualified ID>; None = default
    input_channel: InputChannel,     // which channel of a multi-channel device to decode
    input_peak: Option<f32>,         // recent capture peak; None until audio arrives
    capture_xruns: u64,

    // Runtime.
    status: EngineStatus,
    applied: Option<EngineState>,
    rows: Vec<Row>,
    display_items: Vec<DisplayItem>,
    error: Option<String>,
    dx: Option<DxContextSnapshot>,
    pending_confirm: Option<EngineState>,

    settings_open: bool,
    settings_focus: DialogFocus,
    about_focus: DialogFocus,
    copyright_focus: DialogFocus,
    about_open: bool,
    // The (mandatory) WSJT-X copyright notice, shown in its own popup from About.
    copyright_open: bool,
    tab: SettingsTab,

    menu_events: Receiver<muda::MenuEvent>,

    // Native menu handles and last synchronized presentation state.
    native_menu: NativeMenu,
    menu_synced: (DecodeProfile, Language, Locale),
    // Last theme the style was built for; re-applied when the OS theme flips.
    styled_theme: egui::Theme,
    // Last window title pushed to the OS (only re-sent when it changes).
    last_title: String,
    // The app logo as an egui texture (None if it failed to decode).
    logo: Option<egui::TextureHandle>,
    // Persistent socket owner; retained while disabled so rebinds are serialized.
    udp_in: Option<crate::wsjtx_udp::UdpIn>,
    udp_in_dirty: bool,
    current_band: CurrentBand,
}

impl Ft8rsApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        activation: Arc<crate::single_instance::Activation>,
        recorder: Arc<Recorder>,
    ) -> Self {
        activation.attach(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let font_support = install_fonts(&cc.egui_ctx);
        let theme = cc.egui_ctx.theme();
        apply_style(&cc.egui_ctx, theme);
        let engine = EngineHandle::spawn_observed(recorder.clone());
        let _ = engine.send(EngineCommand::RefreshDevices);

        // Restore IDs; legacy names migrate after enumeration only if unique.
        // Missing or ambiguous selections are reported instead of falling back.
        let storage = cc.storage;
        let load = |key: &str, default: &str| -> String {
            storage
                .and_then(|st| st.get_string(key))
                .unwrap_or_else(|| default.to_string())
        };
        let load_bool = |key: &str, default: bool| -> bool {
            storage
                .and_then(|st| st.get_string(key))
                .map(|value| value == "1")
                .unwrap_or(default)
        };
        let language = Language::parse(&load("language", "auto"));
        let requested_locale = language.resolve_system();
        let locale = font_support.locale(requested_locale);
        let device = load("device", "");
        let profile =
            DecodeProfile::parse(&load("profile", "wsjtx")).unwrap_or(DecodeProfile::Wsjtx);
        let mycall = load("mycall", "");
        // Install the menu while eframe still keeps the root hidden. Preserve
        // its physical client size: SetMenu consumes client height, whereas
        // winit knows how to resize the outer window for a menu-bearing window.
        let menu_events = native_menu_events(&cc.egui_ctx);
        let hwnd = window_hwnd(cc);
        #[cfg(target_os = "windows")]
        let client_size = cc.winit_window().map(|window| window.inner_size());
        let native_menu = install_menu(profile, language, locale, hwnd);
        native_menu.sync_debug(locale, debug_info::eligible(&mycall), false, false);
        #[cfg(target_os = "windows")]
        if let (Some(window), Some(size)) = (cc.winit_window(), client_size) {
            if !window.is_maximized() {
                let _ = window.request_inner_size(size);
            }
        }

        // Decode the embedded logo into a texture for the About dialog.
        let logo = image::load_from_memory(crate::LOGO_PNG).ok().map(|img| {
            let img = img.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            cc.egui_ctx
                .load_texture("logo", color, egui::TextureOptions::LINEAR)
        });

        Self {
            debug_info: DebugInfo::new(recorder),
            debug_result: None,
            engine,
            clock_sync: None,
            clock_status: None,
            clock_segment: 0,
            clock_boundary: None,
            activation,
            main_focus: DialogFocus::default(),
            language,
            locale,
            requested_locale,
            font_support,
            locale_checked_at: Instant::now(),
            mycall,
            hiscall: load("hiscall", ""),
            hisgrid: load("hisgrid", ""),
            nfqso: load("nfqso", "0"),
            profile,
            swl: load_bool("swl", false),
            hybrid_experiment: load_bool("hybrid_experiment", false),
            nfa: load("nfa", "200"),
            nfb: load("nfb", "3000"),
            mygrid: load("mygrid", ""),
            udp_on: load_bool("udp_on", false),
            udp_host: load("udp_host", "127.0.0.1"),
            udp_port: load("udp_port", "2238"),
            hide_hash: load_bool("hide_hash", false),
            display_filters: DisplayFilters {
                hide_contest: load_bool("hide_contest", true),
                hide_telemetry: load_bool("hide_telemetry", true),
            },
            udp_in_on: load_bool("udp_in_on", true),
            udp_in_host: load("udp_in_host", "127.0.0.1"),
            udp_in_port: load("udp_in_port", "2237"),
            udp_in_tol: load("udp_in_tol", "7").trim().parse().unwrap_or(7.0),
            devices: Vec::new(),
            selected_device: (!device.is_empty()).then_some(device),
            // Preserve the operator's channel choice; Mono negotiates a native
            // mono stream and otherwise takes the first capture channel.
            input_channel: InputChannel::parse(&load("audio_channel", "mono")).unwrap_or_default(),
            input_peak: None,
            capture_xruns: 0,
            status: EngineStatus::Idle,
            applied: None,
            rows: Vec::new(),
            display_items: Vec::new(),
            error: None,
            dx: None,
            pending_confirm: None,
            settings_open: false,
            settings_focus: DialogFocus::default(),
            about_focus: DialogFocus::default(),
            copyright_focus: DialogFocus::default(),
            about_open: false,
            copyright_open: false,
            tab: SettingsTab::General,
            menu_events,
            native_menu,
            menu_synced: (profile, language, locale),
            styled_theme: theme,
            last_title: String::new(),
            logo,
            udp_in: None,
            udp_in_dirty: true,
            current_band: CurrentBand::default(),
        }
    }

    /// Window title: `ft8.rs - {MyCallsign} - {Profile}` (callsign omitted when
    /// unset). Pushed to the OS only when it changes.
    fn sync_title(&mut self, ctx: &egui::Context) {
        let profile = profile_label(self.profile);
        let title = match norm(&self.mycall) {
            Some(call) => format!("ft8.rs - {call} - {profile}"),
            None => format!("ft8.rs - {profile}"),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn is_monitoring(&self) -> bool {
        matches!(
            self.status,
            EngineStatus::Aligning | EngineStatus::Monitoring
        )
    }

    fn dx_needs_hiscall(&self) -> bool {
        self.profile == DecodeProfile::Dx && norm(&self.hiscall).is_none()
    }

    fn build_state(&self) -> Result<EngineState, String> {
        let mut config = StreamDecodeConfig {
            profile: self.profile,
            mycall: norm(&self.mycall),
            mygrid: norm(&self.mygrid),
            hiscall: norm(&self.hiscall),
            hisgrid: norm(&self.hisgrid),
            swl: self.swl,
            hybrid_experiment: self.hybrid_experiment,
            nagain: false,
            filter: false,
            hide_dupes: false,
            hide_hash: self.hide_hash,
            ..StreamDecodeConfig::default()
        };
        (config.nfa, config.nfb) = parse_frequency_range(&self.nfa, &self.nfb)?;
        let nfqso = self
            .nfqso
            .trim()
            .parse::<f64>()
            .map_err(|_| "RX frequency must be a number".to_owned())?;
        config.nfqso =
            ft8rs::stream::validation::normalize_rx_frequency(nfqso, config.nfa, config.nfb)?;
        Ok(EngineState {
            device: self
                .selected_device
                .as_deref()
                .map(|selector| {
                    let migrated =
                        ft8rs_engine::soundcard::migrate_device_name(selector, &self.devices)?;
                    if !migrated.starts_with("id:") {
                        return Err(format!(
                            "Saved input '{selector}' is unavailable; select an input device"
                        ));
                    }
                    Ok(migrated)
                })
                .transpose()?,
            channel: self.input_channel,
            config,
            udp: self.udp_config()?,
        })
    }

    fn udp_config(&self) -> Result<Option<UdpConfig>, String> {
        if !self.udp_on {
            return Ok(None);
        }
        if self.udp_host.trim().is_empty() {
            return Err("UDP report host is required".to_string());
        }
        Ok(Some(UdpConfig {
            host: self.udp_host.trim().to_string(),
            port: parse_port(&self.udp_port)?,
        }))
    }

    fn settings_validation_error(&self, tab: SettingsTab) -> Option<String> {
        match tab {
            SettingsTab::Frequency => parse_frequency_range(&self.nfa, &self.nfb).err(),
            SettingsTab::Output => self.udp_config().err(),
            SettingsTab::Compare if self.udp_in_on => {
                if self.udp_in_host.trim().is_empty() {
                    Some("UDP listen host is required".to_owned())
                } else {
                    parse_port(&self.udp_in_port).err()
                }
            }
            _ => None,
        }
    }

    fn invalid_monitor_settings_page(&self) -> Option<SettingsTab> {
        [SettingsTab::Frequency, SettingsTab::Output]
            .into_iter()
            .find(|&tab| self.settings_validation_error(tab).is_some())
    }

    fn toggle_monitor(&mut self) {
        if self.is_monitoring() {
            let _ = self.engine.send(EngineCommand::StopMonitor);
            self.status = EngineStatus::Idle;
            self.applied = None;
            self.pending_confirm = None;
            self.input_peak = None;
            self.capture_xruns = 0;
            return;
        }
        if self.dx_needs_hiscall() {
            self.error = Some("DX requires a target callsign.".to_string());
            return;
        }
        if let Some(tab) = self.invalid_monitor_settings_page() {
            self.select_settings_tab(tab);
            self.open_settings();
            return;
        }
        let state = match self.build_state() {
            Ok(state) => state,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.error = None;
        self.rows.clear();
        self.display_items.clear();
        self.dx = None;
        self.input_peak = None;
        self.capture_xruns = 0;
        if self
            .engine
            .send(EngineCommand::StartMonitor(state.clone()))
            .is_ok()
        {
            self.applied = Some(state);
            self.status = EngineStatus::Aligning;
        }
    }

    /// Push a live config change to the engine, if monitoring and it differs.
    /// A change that discards DX intel (target switch) is deferred to a confirm
    /// dialog instead of applied immediately.
    fn apply_if_changed(&mut self) {
        if self.invalid_monitor_settings_page().is_some() {
            return;
        }
        if self.dx_needs_hiscall() {
            self.error = Some("DX requires a target callsign.".to_string());
            return; // would be invalid; leave the running session as-is
        }
        let desired = match self.build_state() {
            Ok(state) => state,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.error = None;
        if !self.is_monitoring() {
            return;
        }
        let Some(applied) = &self.applied else {
            return;
        };
        let outcome = plan_reconfig(applied, &desired);
        if outcome.is_noop() {
            return;
        }
        if outcome.confirm_required {
            self.pending_confirm = Some(desired);
            return;
        }
        if self
            .engine
            .send(EngineCommand::ApplyState(desired.clone()))
            .is_ok()
        {
            self.applied = Some(desired);
        }
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        let clock = Instant::now();
        // Use the latest received dial status for newly seen slots in this batch.
        while clock.elapsed() < EVENT_BUDGET {
            let Some(status) = self.udp_in.as_ref().and_then(|udp| udp.try_dial_status()) else {
                break;
            };
            self.current_band.observe(status);
        }
        self.current_band.expire(Instant::now());
        let band = self.current_band.name();
        let mut batch = DecodeBatch::new(
            &mut self.rows,
            &mut self.display_items,
            self.udp_in_tol,
            self.display_filters,
        );
        // Keep the existing local-before-external consumption order, with one
        // budget for both sources rather than an unbounded drain of either.
        while clock.elapsed() < EVENT_BUDGET {
            let Some(event) = self.engine.try_recv() else {
                break;
            };
            match event {
                EngineEvent::Status(status) => {
                    if status == EngineStatus::Idle {
                        self.pending_confirm = None;
                        self.applied = None;
                        self.input_peak = None;
                        self.capture_xruns = 0;
                    }
                    self.status = status;
                }
                EngineEvent::DevicesRefreshed(devices) => {
                    if let Some(selected) = self.selected_device.as_deref() {
                        match ft8rs_engine::soundcard::migrate_device_name(selected, &devices) {
                            Ok(id) => self.selected_device = Some(id),
                            Err(err) => self.error = Some(err),
                        }
                    }
                    self.devices = devices;
                }
                EngineEvent::InputLevel(peak) => self.input_peak = Some(peak),
                EngineEvent::CaptureStarted { .. } => {
                    self.input_peak = None;
                    self.capture_xruns = 0;
                }
                EngineEvent::CaptureLevels { peak, xruns, .. } => {
                    self.input_peak = Some(peak);
                    self.capture_xruns = xruns;
                }
                EngineEvent::CaptureSelectionRejected {
                    requested_device,
                    requested_channel,
                    active_device,
                    active_channel,
                } => {
                    if self.selected_device == requested_device
                        && self.input_channel == requested_channel
                    {
                        self.selected_device = active_device.clone();
                        self.input_channel = active_channel;
                    }
                    if let Some(applied) = &mut self.applied {
                        if applied.device == requested_device
                            && applied.channel == requested_channel
                        {
                            applied.device = active_device;
                            applied.channel = active_channel;
                        }
                    }
                }
                EngineEvent::Decode(record) => {
                    let mut row = local_decode_row(record);
                    row.gen += self.clock_segment;
                    row.slot_band = band;
                    batch.insert(row);
                }
                EngineEvent::ClockDiscontinuity { at_ms, observed } => {
                    self.clock_segment += CLOCK_SEGMENT_STRIDE;
                    let first_slot = at_ms.max(0) as u64 / 15_000 + 1;
                    self.clock_boundary = Some((observed, first_slot));
                    self.dx = None;
                }
                EngineEvent::A7Checkpoint { .. }
                | EngineEvent::JtdxCheckpoint { .. }
                | EngineEvent::CaptureDiag { .. }
                | EngineEvent::SlotComplete { .. }
                | EngineEvent::Reconfigured(_) => {}
                EngineEvent::DxContext(snapshot) => self.dx = Some(snapshot),
                EngineEvent::Error(err) => self.error = Some(err),
                EngineEvent::DecodeRowsDropped(count) => {
                    let message = format!("GUI backlog full: {count} decoded rows were dropped");
                    log::warn!("{message}");
                    self.error.get_or_insert(message);
                }
            }
        }
        while clock.elapsed() < EVENT_BUDGET {
            let Some((received, decode)) =
                self.udp_in.as_ref().and_then(|udp| udp.try_recv_timed())
            else {
                break;
            };
            let mut row = external_decode_row(decode);
            if self
                .clock_boundary
                .is_some_and(|(observed, first_slot)| received < observed || row.gen < first_slot)
            {
                continue;
            }
            row.gen += self.clock_segment;
            row.slot_band = band;
            batch.insert(row);
        }
        if clock.elapsed() >= EVENT_BUDGET {
            // Leave unconsumed events in their original mailboxes and continue
            // next frame instead of waiting for the idle 150 ms refresh.
            ctx.request_repaint();
        }
    }

    /// Submit configuration to the socket owner; never bind, close or join here.
    fn sync_udp_in(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.udp_in_dirty) {
            let target = if !self.udp_in_on {
                Some(None)
            } else {
                parse_port(&self.udp_in_port)
                    .ok()
                    .filter(|_| !self.udp_in_host.trim().is_empty())
                    .map(|port| Some((self.udp_in_host.trim(), port)))
            };
            if let Some(target) = target {
                if self.udp_in.is_none() && target.is_some() {
                    let wake = ctx.clone();
                    match crate::wsjtx_udp::UdpIn::spawn_observed(
                        move || wake.request_repaint(),
                        Some(Arc::clone(&self.debug_info.recorder)),
                    ) {
                        Ok(listener) => self.udp_in = Some(listener),
                        Err(err) => {
                            self.udp_in_on = false;
                            self.error = Some(err);
                        }
                    }
                }
                if let Some(listener) = &mut self.udp_in {
                    match listener.configure(target) {
                        // Clear the previous error on submission, not on a
                        // delayed acknowledgement that could erase a newer
                        // capture/engine error.
                        Ok(true) => {
                            self.current_band.clear();
                            if self.udp_in_on {
                                self.error = None;
                            }
                        }
                        Ok(false) => {}
                        Err(err) => {
                            self.current_band.clear();
                            self.udp_in_on = false;
                            self.error = Some(err);
                        }
                    }
                }
            }
        }
        // Submit new settings before reading acknowledgements: a late failure
        // from a superseded bind must not disable a newer valid selection.
        while let Some(result) = self.udp_in.as_ref().and_then(|udp| udp.try_status()) {
            if let Err(err) = result {
                self.udp_in_on = false;
                self.udp_in_dirty = true;
                self.error = Some(err);
                ctx.request_repaint();
            }
        }
    }

    // ── UI ──────────────────────────────────────────────────────────────────

    fn open_settings(&mut self) {
        if self.tab == SettingsTab::Audio {
            let _ = self.engine.send(EngineCommand::RefreshDevices);
        }
        self.settings_open = true;
        self.settings_focus.request();
    }

    fn select_settings_tab(&mut self, tab: SettingsTab) {
        if self.tab != tab {
            self.tab = tab;
            if tab == SettingsTab::Audio {
                let _ = self.engine.send(EngineCommand::RefreshDevices);
            }
        }
    }

    fn pump_menu(&mut self) {
        let mut selected = false;
        while let Ok(event) = self.menu_events.try_recv() {
            match event.id.0.as_str() {
                // Re-arm the focus request on every click, not just the first: the
                // dialog may already be open but sitting behind the main window
                // (native menu clicks don't raise an existing viewport), so a
                // second click must still bring it to the front.
                "settings" => self.open_settings(),
                "about" => {
                    self.about_open = true;
                    self.about_focus.request();
                }
                "debug:collect" if debug_info::eligible(&self.mycall) => {
                    self.set_debug_collection(!self.debug_info.active());
                }
                "debug:save" => self.save_debug_info(),
                "debug:open" => self.open_debug_directory(),
                id => {
                    if let Some(name) = id.strip_prefix("language:") {
                        self.language = Language::parse(name);
                        selected = true;
                    } else if let Some(name) = id.strip_prefix("profile:") {
                        if let Ok(profile) = DecodeProfile::parse(name) {
                            selected = true;
                            if self.profile != profile {
                                self.profile = profile;
                                self.apply_if_changed();
                            }
                        }
                    }
                }
            }
        }
        self.sync_language_and_menu();
        // Native check items toggle even when the selected value is clicked again.
        if selected {
            self.native_menu
                .sync(self.profile, self.language, self.locale);
        }
    }

    fn set_debug_collection(&mut self, enabled: bool) {
        self.debug_info
            .set_active(enabled && debug_info::eligible(&self.mycall));
        self.debug_result = None;
    }

    fn save_debug_info(&mut self) {
        if debug_info::eligible(&self.mycall) && self.debug_info.can_save() {
            let udp_listener = self
                .udp_in_on
                .then(|| format!("{}:{}", self.udp_in_host.trim(), self.udp_in_port.trim()));
            self.debug_result = None;
            if let Err(err) = self.debug_info.save(udp_listener) {
                self.debug_save_failed(&err);
            }
        }
    }

    fn debug_save_failed(&mut self, err: &str) {
        let label = self.locale.text("Unable to save debug information");
        self.debug_result = Some(Err(format!("{label}: {err}")));
    }

    fn open_debug_directory(&mut self) {
        if debug_info::eligible(&self.mycall) {
            self.debug_result = None;
            if let Err(err) = self.debug_info.open_directory() {
                let label = self.locale.text("Unable to open debug information folder");
                self.debug_result = Some(Err(format!("{label}: {err}")));
            }
        }
    }

    fn sync_debug_menu(&mut self) {
        let eligible = debug_info::eligible(&self.mycall);
        if !eligible {
            if self.debug_info.active() {
                self.set_debug_collection(false);
            }
            self.debug_result = None;
        }
        self.native_menu.sync_debug(
            self.locale,
            eligible,
            self.debug_info.active(),
            self.debug_info.can_save(),
        );
    }

    /// Language is a presentation preference; changing it never reconfigures the engine.
    fn sync_language_and_menu(&mut self) {
        if self.language != self.menu_synced.1
            || (self.language == Language::Auto
                && self.locale_checked_at.elapsed() >= Duration::from_secs(2))
        {
            self.requested_locale = self.language.resolve_system();
            self.locale = self.font_support.locale(self.requested_locale);
            self.locale_checked_at = Instant::now();
        }
        let current = (self.profile, self.language, self.locale);
        if self.menu_synced != current {
            self.native_menu
                .sync(self.profile, self.language, self.locale);
            self.menu_synced = current;
        }
    }

    // In-window menu fallback for platforms without a native menu (Linux). macOS
    // and Windows attach a native menu bar instead.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        ui.horizontal(|ui| {
            if ui.button(tr("Settings")).clicked() {
                self.open_settings();
            }
            ui.menu_button(tr("Profile"), |ui| {
                for profile in MENU_PROFILES {
                    if ui
                        .selectable_label(self.profile == profile, profile_label(profile))
                        .clicked()
                    {
                        self.profile = profile;
                        self.apply_if_changed();
                        ui.close();
                    }
                }
            });
            ui.menu_button(tr("Language"), |ui| {
                for language in Language::ALL {
                    if ui
                        .selectable_value(
                            &mut self.language,
                            language,
                            self.font_support.language_label(language, locale),
                        )
                        .clicked()
                    {
                        ui.close();
                    }
                }
            });
            if ui.button(tr("About")).clicked() {
                self.about_open = true;
                self.about_focus.request();
            }
            if debug_info::eligible(&self.mycall) {
                ui.menu_button(tr("Developer Mode"), |ui| {
                    let mut collect = self.debug_info.active();
                    if ui
                        .checkbox(&mut collect, tr("Collect Debug Information"))
                        .changed()
                    {
                        self.set_debug_collection(collect);
                    }
                    if ui
                        .add_enabled(
                            self.debug_info.can_save(),
                            egui::Button::new(tr("Save Debug Information")),
                        )
                        .clicked()
                    {
                        self.save_debug_info();
                        ui.close();
                    }
                    if ui.button(tr("Open Debug Information Folder")).clicked() {
                        self.open_debug_directory();
                        ui.close();
                    }
                });
            }
        });
    }

    fn dx_panel(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        ui.add_space(4.0);
        ui.label(RichText::new(tr("DX information")).size(15.0).strong());
        ui.separator();
        ui.add_space(4.0);
        let Some(dx) = &self.dx else {
            ui.label(RichText::new(tr("No data yet")).weak());
            return;
        };
        let target = if dx.target.is_empty() {
            "—"
        } else {
            &dx.target
        };
        ui.label(format!("{}: {target}", tr("Target")));
        ui.add_space(6.0);

        ui.label(format!("{}:", tr("Focus frequencies (Hz)")));
        if dx.foci.is_empty() {
            ui.label(RichText::new("   —").weak());
        } else {
            for freq in &dx.foci {
                ui.label(format!("   {:.0}", freq));
            }
        }
        ui.add_space(6.0);

        let parity = match dx.tx_parity {
            Some(0) => tr("Even"),
            Some(_) => tr("Odd"),
            None => "—",
        };
        ui.label(format!("{}: {parity}", tr("Target TX slot")));

        let grid = dx.hisgrid.as_deref().unwrap_or("—");
        let source = match dx.hisgrid_source {
            Some(HisgridSource::User) => tr(" (user)"),
            Some(HisgridSource::Harvested) => tr(" (decoded)"),
            _ => "",
        };
        ui.label(format!("{}: {grid}{source}", tr("Grid")));

        let dt = dx
            .dt
            .map(|dt| format!("{dt:+.1}"))
            .unwrap_or_else(|| "—".to_string());
        ui.label(format!("DT: {dt}"));
    }

    /// Pixel height of one decode line (used to size the header panel and rows).
    fn row_height(ctx: &egui::Context) -> f32 {
        ctx.fonts_mut(|f| f.row_height(&egui::FontId::monospace(DECODE_FONT_SIZE)))
            + 2.0 * DECODE_ROW_PADDING_Y
    }

    /// The column header, drawn in its own fixed top panel: vertically-centered
    /// text plus a single divider line at the panel's bottom edge.
    fn table_header(&self, ui: &mut egui::Ui) {
        const PAD: f32 = TABLE_PAD;
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.text(
            egui::pos2(rect.left() + PAD, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!(
                "{:<6} {:>3} {:>5} {:>5}  {}",
                "UTC", "dB", "DT", "Freq", "Message"
            ),
            egui::FontId::monospace(DECODE_FONT_SIZE),
            ui.visuals().strong_text_color(),
        );
        // A subtle 1px hairline (the default widget stroke reads as too heavy).
        let line = if ui.visuals().dark_mode {
            Color32::from_gray(70)
        } else {
            Color32::from_gray(205)
        };
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, line),
        );
    }

    /// The scrolling decode rows (edge-to-edge, alternating slot background, with
    /// internal left padding). Lives in the central panel below the header.
    fn decode_rows(&mut self, ui: &mut egui::Ui) {
        let font = egui::FontId::monospace(DECODE_FONT_SIZE);
        let row_h = Self::row_height(ui.ctx());
        let dark = ui.visuals().dark_mode;
        let text_color = ui.visuals().text_color();
        let ink_center_y = cap_height_sample(ui.painter(), &font)
            .mesh_bounds
            .center()
            .y;
        let own_call = norm(&self.mycall);
        let target_call = norm(&self.hiscall);
        let target_grid = norm(&self.hisgrid);
        // Match the WSJT-X detailed separator in both themes.
        let separator_bg = Color32::from_gray(if dark { 0xa2 } else { 0xd3 });
        // Compare-mode coloring only applies while listening.
        let comparing = self.udp_in_on;
        // Row interactions, collected inside the (immutably borrowed) closure and
        // applied after it: double-click fills the bottom-bar QSO context, the
        // right-click menu clears the list.
        let mut fill: Option<(Vec<String>, f64)> = None;
        let mut clear = false;
        let rows = &self.rows;
        let display_items = &self.display_items;
        // Zero the row spacing *before* show_rows: it derives both the content
        // height and each row's position from `row_height + item_spacing.y`, so if
        // the outer spacing (8px from our style) didn't match the 0-spacing rows we
        // paint, show_rows would reserve more height than we draw — leaving the
        // last row mid-panel with a gap below it after stick-to-bottom.
        ui.spacing_mut().item_spacing.y = 0.0;
        // A plain log: rows fill from the top, new ones append at the bottom, and
        // it auto-scrolls to the newest (stick_to_bottom). show_rows virtualizes
        // so the per-frame cost stays flat up to MAX_ROWS. auto_shrink([false,
        // false]) makes the area fill the panel.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show_rows(ui, row_h, display_items.len(), |ui, range| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                let width = ui.available_width();
                for item in &display_items[range] {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::click());
                    let row = match item {
                        DisplayItem::Decode(index) => &rows[*index],
                        DisplayItem::Separator(gen, band) => {
                            paint_decode_separator(
                                ui.painter(),
                                rect,
                                separator_label(*gen, *band),
                                font.clone(),
                                Color32::BLACK,
                                separator_bg,
                            );
                            continue;
                        }
                    };
                    let color = if comparing {
                        source_color(row.source, dark, text_color)
                    } else {
                        text_color
                    };
                    let (job, highlights) = decode_highlight_job(
                        &row.text,
                        own_call.as_deref(),
                        target_call.as_deref(),
                        target_grid.as_deref(),
                        font.clone(),
                        color,
                    );
                    paint_decode_row(ui.painter(), rect, job, &highlights, color, ink_center_y);
                    if resp.double_clicked() {
                        fill = Some((row.tokens.clone(), row.freq));
                    }
                    resp.context_menu(|ui| {
                        // A button inside a context menu auto-closes it on click.
                        if ui.button("Clear list").clicked() {
                            clear = true;
                        }
                    });
                }
            });

        if clear {
            self.rows.clear();
            self.display_items.clear();
        }
        if let Some((tokens, freq)) = fill {
            self.apply_double_click_fill(&tokens, freq);
        }
    }

    /// Double-clicking a decode row loads its QSO context into the bottom bar:
    /// the DX callsign, its grid (when the message carries one), and the RX
    /// frequency, then applies it to a running session (like editing the fields).
    fn apply_double_click_fill(&mut self, tokens: &[String], freq: f64) {
        fill_target_context(&mut self.hiscall, &mut self.hisgrid, tokens);
        self.nfqso = format!("{}", freq.round() as i64);
        self.apply_if_changed();
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        // Uniform control height is the key to a stable row: every widget derives
        // its height from the 14-point font + 2·PAD_Y, with the interact-size floor removed
        // so nothing snaps to a different minimum. No nested layouts on the left —
        // labels, fields, the RX value and its spinner are all direct children of
        // one centered row, so they line up by construction.
        const PAD_Y: f32 = 4.0;
        ui.style_mut().override_font_id = Some(egui::FontId::monospace(DECODE_FONT_SIZE));
        {
            let s = ui.spacing_mut();
            s.item_spacing = egui::vec2(6.0, 0.0);
            s.button_padding = egui::vec2(8.0, PAD_Y);
            s.interact_size.y = 0.0;
        }

        ui.horizontal_centered(|ui| {
            let mut commit = false;
            let call_label = ui.label(tr("Callsign"));
            // The first field's measured height is the canonical row height; every
            // other control is forced to match it exactly.
            let old_call = norm(&self.hiscall);
            let r = callsign_field(ui, &mut self.hiscall, 110.0).labelled_by(call_label.id);
            if r.changed() && norm(&self.hiscall) != old_call {
                self.hisgrid.clear();
            }
            commit |= r.lost_focus();
            let ctrl_h = r.rect.height();
            let grid_label = ui.label(tr("Grid"));
            commit |= grid_field(ui, &mut self.hisgrid, 46.0)
                .labelled_by(grid_label.id)
                .lost_focus();
            let rx_label = ui.label(tr("RX"));
            let mut frequency = self.nfqso.parse::<u32>().unwrap_or(0);
            let response = ui
                .add(
                    egui::DragValue::new(&mut frequency)
                        .range(0..=3000)
                        .speed(1),
                )
                .labelled_by(rx_label.id)
                .on_hover_text(tr(
                    "Drag to adjust the receive frequency, or click to type.",
                ));
            if response.changed() {
                self.nfqso = frequency.to_string();
                commit = true;
            }
            ui.label("Hz");

            // Right-aligned state button — square, height matched to the row.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, fill) = match self.status {
                    EngineStatus::Monitoring => (
                        egui::include_image!("../assets/stop.svg"),
                        Color32::from_rgb(0x16, 0xa3, 0x4a),
                    ),
                    EngineStatus::Aligning => (
                        egui::include_image!("../assets/stop.svg"),
                        Color32::from_rgb(0xd9, 0x77, 0x06),
                    ),
                    EngineStatus::Idle | EngineStatus::Error(_) => (
                        egui::include_image!("../assets/play.svg"),
                        Color32::from_rgb(0x25, 0x63, 0xeb),
                    ),
                };
                let enabled = self.is_monitoring() || !self.dx_needs_hiscall();
                let response = monitor_button(ui, icon, fill, ctrl_h, enabled).on_hover_text(
                    if self.is_monitoring() {
                        tr("Stop monitoring")
                    } else {
                        tr("Start monitoring")
                    },
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        enabled,
                        if self.is_monitoring() {
                            tr("Stop monitoring")
                        } else {
                            tr("Start monitoring")
                        },
                    )
                });
                if response.clicked() {
                    self.toggle_monitor();
                }
            });
            if commit {
                self.apply_if_changed();
            }
        });
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        if !self.settings_open {
            return;
        }
        let mut commit = false;
        let mut close = false;
        let viewport_id = egui::ViewportId::from_hash_of("settings");
        let min_height = settings_min_height(ctx, locale);
        let sidebar_width = settings_sidebar_width(ctx);
        // A resizable OS window (own viewport) with a native title bar. The tabs
        // live in a left SidePanel whose edge separator spans the full height.
        ctx.show_viewport_immediate(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title(tr("Settings"))
                .with_inner_size([560.0, min_height.max(480.0)])
                .with_min_inner_size([480.0, min_height])
                .with_resizable(true)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_visible(false)
                .with_active(false),
            |vctx, _class| {
                self.settings_focus.update(vctx, Instant::now());
                egui::Panel::left("settings_tabs")
                    .resizable(false)
                    .exact_size(sidebar_width)
                    .frame(
                        egui::Frame::side_top_panel(&vctx.global_style())
                            .inner_margin(egui::Margin::symmetric(12, 14)),
                    )
                    .show(vctx, |ui| {
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            for (tab, label) in SETTINGS_TABS {
                                if ui.selectable_label(self.tab == tab, tr(label)).clicked() {
                                    self.select_settings_tab(tab);
                                }
                            }
                        });
                    });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&vctx.global_style())
                            .inner_margin(egui::Margin::symmetric(18, 14)),
                    )
                    .show(vctx, |ui| {
                        settings_page(ui, self.tab, |ui| {
                            ui.add_enabled_ui(self.pending_confirm.is_none(), |ui| {
                                commit = self.settings_tab_ui(ui);
                            });
                            if let Some(message) = self.settings_validation_error(self.tab) {
                                ui.add_space(6.0);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(locale.text(&message))
                                            .color(ui.visuals().error_fg_color),
                                    )
                                    .wrap(),
                                );
                            }
                        });
                    });
                if vctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        if close {
            self.settings_open = false;
        }
        if commit || close {
            self.udp_in_dirty = true;
            self.apply_if_changed();
        }
    }

    fn settings_tab_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        let mut commit = false;
        match self.tab {
            SettingsTab::General => {
                let item_gap = ui.spacing().item_spacing.y;
                let heading_gap = item_gap + SETTINGS_HEADING_TOP_SPACE;
                let extra_gap_scale = 0.75;
                section_heading(ui, tr("My Station"));
                ui.push_id("my_station", |ui| {
                    settings_form(ui, |ui| {
                        commit |= setting_row(ui, tr("Callsign"), |ui| {
                            callsign_field(ui, &mut self.mycall, f32::INFINITY).lost_focus()
                        });
                        commit |= setting_row(ui, tr("Grid"), |ui| {
                            grid_field(ui, &mut self.mygrid, f32::INFINITY).lost_focus()
                        });
                    });
                });
                // The station form's ID scope contributes one extra item gap.
                ui.add_space(
                    (SETTINGS_FORM_BOTTOM_SPACE + item_gap + heading_gap) * extra_gap_scale,
                );
                section_heading(ui, tr("Display"));
                settings_form(ui, |ui| {
                    setting_row(ui, tr("Language"), |ui| {
                        egui::ComboBox::from_id_salt("language")
                            .width(ui.available_width())
                            .selected_text(self.font_support.language_label(self.language, locale))
                            .show_ui(ui, |ui| {
                                for language in Language::ALL {
                                    ui.selectable_value(
                                        &mut self.language,
                                        language,
                                        self.font_support.language_label(language, locale),
                                    );
                                }
                            });
                        false
                    });
                });
                if !self.font_support.chinese {
                    ui.add_space(8.0);
                    ui.label(CHINESE_UNAVAILABLE);
                } else {
                    ui.add_space(SETTINGS_FORM_BOTTOM_SPACE * extra_gap_scale);
                }
                ui.add_space(heading_gap * extra_gap_scale);
                self.clock_sync_ui(ui);
            }
            SettingsTab::Audio => {
                section_heading(ui, tr("Audio"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("Input device"), |ui| self.device_combo(ui));
                    commit |= setting_row(ui, tr("Channel"), |ui| {
                        let before = self.input_channel;
                        egui::ComboBox::from_id_salt("input_channel")
                            .width(ui.available_width())
                            .truncate()
                            .selected_text(match self.input_channel {
                                InputChannel::Mono => tr("Mono"),
                                InputChannel::Left => tr("Left"),
                                InputChannel::Right => tr("Right"),
                                InputChannel::Mix => tr("Mix (average)"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Mono,
                                    tr("Mono"),
                                );
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Left,
                                    tr("Left"),
                                );
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Right,
                                    tr("Right"),
                                );
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Mix,
                                    tr("Mix (average)"),
                                );
                            });
                        self.input_channel != before
                    });
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(tr("Input level"));
                    match self.input_peak {
                        Some(peak) if peak > 0.0 => {
                            let dbfs = 20.0 * peak.log10();
                            // Green if we're clearly getting signal; red near silence
                            // (a dead capture — e.g. a virtual driver handing back zeros).
                            let color = if dbfs >= -0.1 {
                                Color32::from_rgb(0xdc, 0x26, 0x26)
                            } else if dbfs > -60.0 {
                                Color32::from_rgb(0x16, 0xa3, 0x4a)
                            } else {
                                Color32::from_rgb(0xdc, 0x26, 0x26)
                            };
                            let clipped = if dbfs >= -0.1 {
                                tr(" (near clipping)")
                            } else {
                                ""
                            };
                            ui.label(
                                RichText::new(format!("{dbfs:.1} dBFS{clipped}")).color(color),
                            );
                        }
                        Some(_) => {
                            ui.label(
                                RichText::new(tr("silent (−inf dBFS)"))
                                    .color(Color32::from_rgb(0xdc, 0x26, 0x26)),
                            );
                        }
                        None => {
                            ui.label(RichText::new(tr("— (start monitoring)")).weak());
                        }
                    }
                });
                if self.capture_xruns > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{}: {} ({})",
                            tr("Audio discontinuity reports"),
                            self.capture_xruns,
                            tr("monitoring continues")
                        ))
                        .weak(),
                    );
                }
                if self.input_channel == InputChannel::Mono
                    || self.input_channel == InputChannel::Mix
                {
                    ui.add_space(4.0);
                    for text in [
                        tr("Mono requests one channel; if unavailable, uses channel 1."),
                        tr("Mix averages all channels and can cancel opposite-phase audio."),
                    ] {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let line_height = ui.text_style_height(&egui::TextStyle::Small);
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(8.0, line_height),
                                egui::Sense::hover(),
                            );
                            ui.painter().circle_filled(
                                rect.center(),
                                1.5,
                                ui.visuals().weak_text_color(),
                            );
                            ui.add(egui::Label::new(RichText::new(text).small().weak()).wrap());
                        });
                    }
                }
            }
            SettingsTab::Decode => {
                section_heading(ui, tr("Decode"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("Profile"), |ui| {
                        let before = self.profile;
                        egui::ComboBox::from_id_salt("profile")
                            .width(ui.available_width())
                            .truncate()
                            .selected_text(profile_label(self.profile))
                            .show_ui(ui, |ui| {
                                for profile in MENU_PROFILES {
                                    ui.selectable_value(
                                        &mut self.profile,
                                        profile,
                                        profile_label(profile),
                                    );
                                }
                            });
                        self.profile != before
                    });
                    if matches!(self.profile, DecodeProfile::Jtdx | DecodeProfile::Hybrid) {
                        commit |= setting_row(ui, "SWL", |ui| {
                            ui.add(egui::Checkbox::without_text(&mut self.swl))
                                .on_hover_text(tr("Controls both JTDX paths in Hybrid."))
                                .changed()
                        });
                    }
                    if self.profile == DecodeProfile::Hybrid {
                        commit |= setting_row(ui, tr("Exp. decoder"), |ui| {
                            ui.add(egui::Checkbox::without_text(&mut self.hybrid_experiment))
                                .on_hover_text(
                                    tr("Adds weak stable-signal and interference recovery. Results carry exp; processing may take about 10 seconds."),
                                )
                                .changed()
                        });
                    }
                });
            }
            SettingsTab::Frequency => {
                section_heading(ui, tr("Frequency"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("Low (Hz)"), |ui| text_field(ui, &mut self.nfa));
                    commit |= setting_row(ui, tr("High (Hz)"), |ui| text_field(ui, &mut self.nfb));
                });
            }
            SettingsTab::Output => {
                section_heading(ui, tr("Output"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("UDP reports"), |ui| {
                        ui.add(egui::Checkbox::without_text(&mut self.udp_on))
                            .changed()
                    });
                    commit |= setting_row(ui, tr("Host"), |ui| text_field(ui, &mut self.udp_host));
                    commit |= setting_row(ui, tr("Port"), |ui| text_field(ui, &mut self.udp_port));
                });
            }
            SettingsTab::Compare => {
                section_heading(ui, tr("Compare"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("Listen to UDP"), |ui| {
                        ui.add(egui::Checkbox::without_text(&mut self.udp_in_on))
                            .changed()
                    });
                    commit |=
                        setting_row(ui, tr("Host"), |ui| text_field(ui, &mut self.udp_in_host));
                    commit |=
                        setting_row(ui, tr("Port"), |ui| text_field(ui, &mut self.udp_in_port));
                    commit |= setting_row(ui, tr("Match ± Hz"), |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.udp_in_tol)
                                .speed(0.25)
                                .range(0.0..=10.0),
                        )
                        .changed()
                    });
                });
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(tr(
                            "Merges WSJT-X decodes with the local ones to compare sensitivity.",
                        ))
                        .small()
                        .weak(),
                    )
                    .wrap(),
                );
                ui.label(RichText::new(tr("Color shows the source:")).small().weak());
                ui.add_space(4.0);
                let dark = ui.visuals().dark_mode;
                let default = ui.visuals().text_color();
                for (source, label) in [
                    (DecodeSource::External, tr("only WSJT-X")),
                    (DecodeSource::Local, tr("only ft8.rs")),
                    (DecodeSource::Both, tr("both decoders")),
                ] {
                    let color = source_color(source, dark, default);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.add(
                            egui::Image::new(egui::include_image!("../assets/stop.svg"))
                                .fit_to_exact_size(egui::Vec2::splat(16.0))
                                .tint(color),
                        );
                        ui.colored_label(color, RichText::new(label).small());
                    });
                }
            }
            SettingsTab::Advanced => {
                section_heading(ui, tr("Advanced"));
                let previous_filters = self.display_filters;
                settings_form(ui, |ui| {
                    let dx = self.profile == DecodeProfile::Dx;
                    commit |= setting_row(ui, tr("Hide <...> calls"), |ui| {
                        ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.hide_hash))
                            .changed()
                    });
                    for (label, enabled) in [
                        (
                            "Hide contest messages",
                            &mut self.display_filters.hide_contest,
                        ),
                        ("Hide telemetry", &mut self.display_filters.hide_telemetry),
                    ] {
                        setting_row(ui, tr(label), |ui| {
                            ui.add(egui::Checkbox::without_text(enabled));
                            false
                        });
                    }
                });
                if self.display_filters != previous_filters {
                    self.display_items = display_items(&self.rows, self.display_filters);
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tr("Decoder changes take effect at the next slot."))
                        .small()
                        .weak(),
                );
            }
        }
        commit
    }

    fn clock_sync_ui(&mut self, ui: &mut egui::Ui) {
        use clock_sync::{Error, Update};

        let locale = self.locale;
        let tr = |text| locale.text(text);
        section_heading(ui, tr("Calibrate system time"));
        if ui
            .add_enabled(
                self.clock_sync.is_none(),
                egui::Button::new(tr("Calibrate now")),
            )
            .clicked()
        {
            let ctx = ui.ctx().clone();
            match clock_sync::start(self.engine.clock_adjustment_reporter(), move || {
                ctx.request_repaint()
            }) {
                Ok(rx) => {
                    self.clock_sync = Some(rx);
                    self.clock_status = Some(Update::Querying);
                }
                Err(error) => {
                    self.clock_status = Some(Update::Failed(Error::System(error.to_string())))
                }
            }
        }
        let status = match &self.clock_status {
            None => return,
            Some(Update::Querying | Update::Applying | Update::Verifying) => {
                tr("Calibrating…").to_owned()
            }
            Some(Update::WithinTolerance { offset_us }) => format!(
                "{}\n{} {:+.3} ms",
                tr("System time is within tolerance"),
                tr("Measured offset"),
                *offset_us as f64 / 1000.0
            ),
            Some(Update::Finished {
                offset_us,
                remaining_us,
            }) => {
                let remaining = remaining_us.map_or_else(
                    || tr("Verification unavailable").to_owned(),
                    |us| format!("{} {:+.1} ms", tr("Remaining offset"), us as f64 / 1000.0),
                );
                format!(
                    "{}\n{} {:+.1} ms\n{remaining}",
                    tr("System time calibrated"),
                    tr("Measured offset"),
                    *offset_us as f64 / 1000.0
                )
            }
            Some(Update::Failed(Error::NoResponse)) => tr("No valid NTP response").to_owned(),
            Some(Update::Failed(Error::Cancelled)) => tr("Time calibration cancelled").to_owned(),
            Some(Update::Failed(Error::System(error))) => {
                format!("{}\n{error}", tr("Unable to set system time"))
            }
        };
        let line_height = ui.text_style_height(&egui::TextStyle::Small) + 2.0;
        let mut label = RichText::new(status)
            .small()
            .weak()
            .line_height(Some(line_height));
        if matches!(self.clock_status, Some(Update::Failed(_))) {
            label = label.color(ui.visuals().error_fg_color);
        }
        ui.add(egui::Label::new(label).wrap());
    }

    fn pump_clock_sync(&mut self) {
        let Some(rx) = &self.clock_sync else { return };
        loop {
            match rx.try_recv() {
                Ok(status) if status.busy() => self.clock_status = Some(status),
                Ok(status) => {
                    self.clock_status = Some(status);
                    self.clock_sync = None;
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.clock_status = Some(clock_sync::Update::Failed(
                        clock_sync::Error::System("Clock calibration worker stopped".into()),
                    ));
                    self.clock_sync = None;
                    return;
                }
            }
        }
    }

    /// Standard picker: egui handles focus, keyboard input, popup sizing and
    /// truncation. Long endpoint names remain available in the hover text.
    fn device_combo(&mut self, ui: &mut egui::Ui) -> bool {
        let before = self.selected_device.clone();
        let response =
            input_device_combo(ui, &mut self.selected_device, &self.devices, self.locale);
        if response.clicked() && egui::Popup::is_id_open(ui.ctx(), response.id.with("popup")) {
            let _ = self.engine.send(EngineCommand::RefreshDevices);
        }
        self.selected_device != before
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        if self.pending_confirm.is_none() {
            return;
        }
        let mut decided: Option<bool> = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Switch DX target")).size(15.0).strong());
            ui.add_space(8.0);
            ui.label(tr("This clears the collected DX information."));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Cancel")).clicked() {
                    decided = Some(false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let confirm = egui::Button::new(
                        RichText::new(tr("Switch")).strong().color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(0x25, 0x63, 0xeb));
                    if ui.add(confirm).clicked() {
                        decided = Some(true);
                    }
                });
            });
        });
        if modal.should_close() {
            decided = Some(false);
        }
        match decided {
            Some(true) => {
                if let Some(desired) = self.pending_confirm.take() {
                    if self
                        .engine
                        .send(EngineCommand::ApplyState(desired.clone()))
                        .is_ok()
                    {
                        self.applied = Some(desired);
                        self.dx = None;
                    }
                }
            }
            Some(false) => {
                self.pending_confirm = None;
                if let Some(running) = &self.applied {
                    self.hiscall = running.config.hiscall.clone().unwrap_or_default();
                    self.hisgrid = running.config.hisgrid.clone().unwrap_or_default();
                    self.profile = running.config.profile;
                }
            }
            None => {}
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut close = false;
        let logo = self.logo.clone();
        let viewport_id = egui::ViewportId::from_hash_of("about");
        // A real OS window (own viewport) with a native title bar.
        ctx.show_viewport_immediate(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title(self.locale.text("About ft8.rs"))
                .with_inner_size([440.0, 500.0])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_visible(false)
                .with_active(false),
            |vctx, _class| {
                self.about_focus.update(vctx, Instant::now());
                egui::CentralPanel::default()
                    // No panel margin: the scroll area reaches the window edge so
                    // its scrollbar sits flush to the right. Content padding is
                    // applied inside the scroll area instead (the inner Frame).
                    .frame(egui::Frame::central_panel(&vctx.global_style()).inner_margin(0.0))
                    .show(vctx, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        ui.spacing_mut().item_spacing.y = 4.0;
                        // Scroll the credits in case fonts/translations overflow the window.
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                egui::Frame::NONE
                                    .inner_margin(egui::Margin::symmetric(22, 14))
                                    .show(ui, |ui| {
                                        ui.add_space(10.0);
                                        ui.vertical_centered(|ui| {
                                            if let Some(tex) = &logo {
                                                ui.add(egui::Image::new(
                                                    egui::load::SizedTexture::new(
                                                        tex.id(),
                                                        egui::vec2(80.0, 80.0),
                                                    ),
                                                ));
                                                ui.add_space(6.0);
                                            }
                                            ui.label(
                                                RichText::new(crate::APP_NAME).size(18.0).strong(),
                                            );
                                            ui.add_space(4.0);
                                            ui.label(
                                                RichText::new(format!(
                                                    "Version {}",
                                                    env!("FT8RS_VERSION")
                                                ))
                                                .weak(),
                                            );
                                            ui.label(
                                                RichText::new(format!(
                                                    "FFT engine: {}",
                                                    ft8rs::fft_engine_name()
                                                ))
                                                .weak(),
                                            );
                                            ui.label(RichText::new(ft8rs::about::COPYRIGHT).weak());
                                            ui.label(
                                                RichText::new(format!(
                                                    "License: {}",
                                                    ft8rs::about::LICENSE
                                                ))
                                                .weak(),
                                            );
                                            ui.add_space(4.0);
                                            ui.hyperlink_to("ft8.rs", ft8rs::about::HOMEPAGE);
                                            ui.hyperlink_to(
                                                "github.com/tallcode/ft8rs",
                                                ft8rs::about::SOURCE_REPOSITORY,
                                            );
                                        });
                                        ui.add_space(10.0);
                                        // No-warranty / redistribution notice (GPL §“interactive”).
                                        ui.label(
                                            RichText::new(ft8rs::about::WARRANTY_NOTICE)
                                                .weak()
                                                .size(11.0),
                                        );
                                        ui.horizontal_wrapped(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            ui.label(
                                                RichText::new("Full license: ").weak().size(11.0),
                                            );
                                            ui.hyperlink_to(
                                                RichText::new(ft8rs::about::LICENSE_URL).size(11.0),
                                                ft8rs::about::LICENSE_URL,
                                            );
                                        });
                                        ui.add_space(12.0);
                                        ui.separator();
                                        ui.add_space(8.0);
                                        // Derivative-work credits: the decoder is a port of these GPL works.
                                        ui.label(RichText::new("Derived from").strong());
                                        ui.add_space(4.0);
                                        for a in ft8rs::about::ATTRIBUTIONS {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.spacing_mut().item_spacing.x = 0.0;
                                                ui.hyperlink_to(
                                                    RichText::new(a.name).strong(),
                                                    a.url,
                                                );
                                                ui.label(format!(" — {}", a.detail));
                                            });
                                            ui.label(
                                                RichText::new(format!("  {}", a.copyright))
                                                    .weak()
                                                    .size(11.0),
                                            );
                                            ui.label(
                                                RichText::new(format!("  {}", a.license))
                                                    .weak()
                                                    .size(11.0),
                                            );
                                            ui.add_space(4.0);
                                        }
                                        ui.add_space(4.0);
                                        ui.label(RichText::new("Also built with").strong());
                                        ui.add_space(4.0);
                                        for (name, role) in ft8rs::about::LIBRARIES {
                                            ui.label(format!("• {name} — {role}"));
                                        }
                                        ui.add_space(6.0);
                                        ui.label(
                                            RichText::new("Thanks to the amateur-radio community.")
                                                .weak(),
                                        );
                                        // The WSJT-X notice is long; keep it behind a button that
                                        // opens it in its own popup.
                                        ui.add_space(10.0);
                                        if ui
                                            .button(
                                                RichText::new("WSJT-X copyright notice").size(11.0),
                                            )
                                            .clicked()
                                        {
                                            self.copyright_open = true;
                                            self.copyright_focus.request();
                                        }
                                    }); // content Frame
                            }); // ScrollArea
                    });
                if vctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        if close {
            self.about_open = false;
        }
    }

    /// The mandatory WSJT-X copyright notice, in its own popup window (opened
    /// from the About dialog). WSJT-X's license requires it to be shown.
    fn copyright_window(&mut self, ctx: &egui::Context) {
        if !self.copyright_open {
            return;
        }
        let mut close = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("copyright"),
            egui::ViewportBuilder::default()
                .with_title("WSJT-X Copyright Notice")
                .with_inner_size([460.0, 280.0])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_visible(false)
                .with_active(false),
            |vctx, _class| {
                self.copyright_focus.update(vctx, Instant::now());
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&vctx.global_style())
                            .inner_margin(egui::Margin::symmetric(18, 14)),
                    )
                    .show(vctx, |ui| {
                        ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                        ui.label(
                            RichText::new("ft8.rs's FT8 decoder is based on WSJT-X:")
                                .weak()
                                .size(11.0),
                        );
                        ui.add_space(8.0);
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.label(ft8rs::about::WSJTX_COPYRIGHT_NOTICE);
                            });
                    });
                if vctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        if close {
            self.copyright_open = false;
        }
    }
}

impl eframe::App for Ft8rsApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let bs = |value: bool| if value { "1" } else { "0" }.to_string();
        storage.set_string("language", self.language.as_str().to_owned());
        storage.set_string("mycall", self.mycall.clone());
        storage.set_string("hiscall", self.hiscall.clone());
        storage.set_string("hisgrid", self.hisgrid.clone());
        storage.set_string("nfqso", self.nfqso.clone());
        storage.set_string("mygrid", self.mygrid.clone());
        storage.set_string("profile", self.profile.as_str().to_string());
        storage.set_string("swl", bs(self.swl));
        storage.set_string("hybrid_experiment", bs(self.hybrid_experiment));
        storage.set_string("nfa", self.nfa.clone());
        storage.set_string("nfb", self.nfb.clone());
        storage.set_string("udp_on", bs(self.udp_on));
        storage.set_string("udp_host", self.udp_host.clone());
        storage.set_string("udp_port", self.udp_port.clone());
        storage.set_string("hide_hash", bs(self.hide_hash));
        storage.set_string("hide_contest", bs(self.display_filters.hide_contest));
        storage.set_string("hide_telemetry", bs(self.display_filters.hide_telemetry));
        storage.set_string("udp_in_on", bs(self.udp_in_on));
        storage.set_string("udp_in_host", self.udp_in_host.clone());
        storage.set_string("udp_in_port", self.udp_in_port.clone());
        storage.set_string("udp_in_tol", self.udp_in_tol.to_string());
        storage.set_string("device", self.selected_device.clone().unwrap_or_default());
        storage.set_string("audio_channel", self.input_channel.as_str().to_string());
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        // eframe defaults to translucent near-black. Use the opaque panel
        // background so uncovered surface pixels match the UI during resizing.
        let [r, g, b, _] = visuals.panel_fill.to_array();
        egui::Color32::from_rgb(r, g, b).to_normalized_gamma_f32()
    }

    // eframe 0.35: the root entry point is `ui` (a `&mut Ui` with no margin),
    // not `update(ctx)`. Panels now `.show(ui, …)` into that ui; the Context is
    // reached via `ui.ctx()`.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.activation.take_request() {
            self.main_focus.request();
        }
        self.main_focus.update(&ctx, Instant::now());
        // Follow the OS light/dark theme: re-apply the custom style when it flips.
        let theme = ctx.theme();
        if theme != self.styled_theme {
            apply_style(&ctx, theme);
            self.styled_theme = theme;
        }
        self.sync_title(&ctx);
        self.sync_udp_in(&ctx);
        self.pump_menu();
        self.pump_events(&ctx);
        self.pump_clock_sync();
        match self.debug_info.poll() {
            Ok(Some(path)) if self.debug_info.active() => self.debug_result = Some(Ok(path)),
            Ok(_) => {}
            Err(err) if self.debug_info.active() => self.debug_save_failed(&err),
            Err(_) => {}
        }

        // macOS and Windows use a native menu bar; only the fallback platforms
        // (Linux) get an in-window top menu panel.
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("controls")
            .exact_size(46.0)
            .show(ui, |ui| self.bottom_bar(ui));
        if self.requested_locale == Locale::Chinese && !self.font_support.chinese {
            egui::Panel::bottom("font_status").show(ui, |ui| {
                ui.add(egui::Label::new(CHINESE_UNAVAILABLE).wrap());
            });
        }
        if let Some(err) = &self.error {
            egui::Panel::bottom("error_status").show(ui, |ui| {
                let color = ui.visuals().error_fg_color;
                ui.horizontal_top(|ui| {
                    ui.add(
                        egui::Image::new(egui::include_image!("../assets/warning.svg"))
                            .fit_to_exact_size(egui::Vec2::splat(16.0))
                            .tint(color),
                    );
                    ui.add(
                        egui::Label::new(RichText::new(self.locale.text(err)).color(color)).wrap(),
                    );
                });
            });
        }
        if let Some(result) = &self.debug_result {
            let mut dismiss = false;
            egui::Panel::bottom("debug_result").show(ui, |ui| {
                ui.horizontal(|ui| {
                    match result {
                        Ok(path) => {
                            ui.label(self.locale.text("Debug information saved"))
                                .on_hover_text(path.display().to_string());
                        }
                        Err(error) => {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                    }
                    dismiss = ui.small_button("×").clicked();
                });
            });
            if dismiss {
                self.debug_result = None;
            }
        }
        if self.profile == DecodeProfile::Dx {
            egui::Panel::right("dx_panel")
                .resizable(false)
                .default_size(232.0)
                .show(ui, |ui| self.dx_panel(ui));
        }
        // Fixed header in its own top panel (so it never scrolls and owns its
        // divider line), then the scrolling rows in the central panel. Both use a
        // zero inner margin so they reach the window edge; padding is internal.
        let zero =
            egui::Frame::central_panel(&ctx.global_style()).inner_margin(egui::Margin::same(0));
        egui::Panel::top("table_header")
            .exact_size(Self::row_height(&ctx) + 8.0)
            .frame(zero)
            .show_separator_line(false) // we draw our own thin divider
            .show(ui, |ui| self.table_header(ui));
        egui::CentralPanel::default()
            .frame(zero)
            .show(ui, |ui| self.decode_rows(ui));

        self.settings_window(&ctx);
        self.sync_language_and_menu();
        self.sync_debug_menu();
        self.about_window(&ctx);
        self.copyright_window(&ctx);
        self.confirm_window(&ctx);

        // Keep pumping engine events even without user input.
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn centered_text_edit(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let font = egui::FontSelection::Default.resolve(ui.style());
    let sample = cap_height_sample(ui.painter(), &font);
    // TextEdit centers the line box. Balance its native margins around the
    // visible caps instead so the editor also moves its cursor and selection.
    const PAD_Y: i8 = 4;
    let shift = (sample.rect.center().y - sample.mesh_bounds.center().y)
        .round()
        .clamp(-f32::from(PAD_Y), f32::from(PAD_Y)) as i8;
    ui.add(
        egui::TextEdit::singleline(value)
            .font(font)
            .align(egui::Align2::LEFT_CENTER)
            .desired_width(width)
            .margin(egui::Margin {
                top: PAD_Y + shift,
                bottom: PAD_Y - shift,
                ..egui::Margin::symmetric(8, PAD_Y)
            }),
    )
}

/// A callsign field: forces ASCII uppercase live. Returns the Response (use
/// `.lost_focus()` for the commit signal, `.rect.height()` for sizing).
fn callsign_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let resp = centered_text_edit(ui, value, width);
    if resp.changed() && value.chars().any(|c| c.is_ascii_lowercase()) {
        *value = value.to_ascii_uppercase();
    }
    resp
}

/// A Maidenhead grid field constrained to AA00: positions 0–1 uppercase letters,
/// 2–3 digits, max 4 chars. Returns the Response.
fn grid_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    // TextEdit's width includes its margins. Measure all four characters with
    // the active font and leave room for both padding and the end cursor.
    let font = egui::FontSelection::Default.resolve(ui.style());
    let min_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap("RR99".to_owned(), font, Color32::WHITE)
            .size()
            .x
            .ceil()
    }) + 20.0;
    let resp = centered_text_edit(ui, value, width.max(min_width));
    if resp.changed() {
        let cleaned = sanitize_grid(value);
        if cleaned != *value {
            *value = cleaned;
        }
    }
    resp
}

/// Keep only a valid Maidenhead 4-char prefix: two letters (uppercased) then two
/// digits; anything that doesn't fit its slot is dropped.
fn sanitize_grid(input: &str) -> String {
    let mut out = String::with_capacity(4);
    for ch in input.chars() {
        match out.len() {
            0 | 1 if ch.is_ascii_alphabetic() => out.push(ch.to_ascii_uppercase()),
            2 | 3 if ch.is_ascii_digit() => out.push(ch),
            _ => {}
        }
        if out.len() == 4 {
            break;
        }
    }
    out
}

/// Use image geometry instead of font baselines and side bearings. Symmetric
/// padding keeps the hit target square even with the bottom bar's text padding.
fn monitor_button(
    ui: &mut egui::Ui,
    icon: egui::ImageSource<'_>,
    fill: Color32,
    side: f32,
    enabled: bool,
) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::Vec2::ZERO;
        let image = egui::Image::new(icon).fit_to_exact_size(egui::Vec2::splat(side * 0.68));
        // Button::new(Image) preserves the requested size; Button::image would
        // clamp it to the text font height.
        ui.add_enabled_ui(enabled, |ui| {
            ui.add_sized(egui::Vec2::splat(side), egui::Button::new(image).fill(fill))
        })
        .inner
    })
    .inner
}

/// One fixed sidebar width for both languages, including label/button/panel
/// padding. Exact panel sizing also overrides a width saved by older versions.
fn settings_sidebar_width(ctx: &egui::Context) -> f32 {
    let style = ctx.global_style();
    let font = egui::TextStyle::Button.resolve(&style);
    let label_width = ctx.fonts_mut(|fonts| {
        [Locale::English, Locale::Chinese]
            .into_iter()
            .flat_map(|locale| {
                SETTINGS_TABS
                    .iter()
                    .map(move |(_, label)| locale.text(label))
            })
            .map(|label| {
                fonts
                    .layout_no_wrap(label.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
            .fold(0.0f32, f32::max)
    });
    (label_width + 2.0 * style.spacing.button_padding.x + 24.0 + 4.0)
        .ceil()
        .max(150.0)
}

/// Reserve enough client height for every navigation row at the current font metrics.
fn settings_min_height(ctx: &egui::Context, locale: Locale) -> f32 {
    let style = ctx.global_style();
    let font = egui::TextStyle::Button.resolve(&style);
    let text_height = ctx.fonts_mut(|fonts| {
        SETTINGS_TABS
            .iter()
            .map(|(_, label)| {
                fonts
                    .layout_no_wrap(locale.text(label).to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .y
            })
            .fold(0.0f32, f32::max)
    });
    let row_height =
        (text_height + 2.0 * style.spacing.button_padding.y).max(style.spacing.interact_size.y);
    let rows = SETTINGS_TABS.len() as f32;
    // Side-panel padding (14 top + bottom), row gaps and a little breathing room.
    (28.0 + rows * row_height + (rows - 1.0) * style.spacing.item_spacing.y + 16.0)
        .ceil()
        .max(440.0)
}

/// A bold section heading for the settings content pane.
fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(SETTINGS_HEADING_TOP_SPACE);
    ui.label(RichText::new(text).size(14.0).strong());
    ui.add_space(10.0);
}

/// Standard Button + Popup controls allow the menu to size independently of
/// its trigger, including shrinking an open menu when the window gets smaller.
fn input_device_label(device: &SoundcardDeviceInfo, devices: &[SoundcardDeviceInfo]) -> String {
    if devices
        .iter()
        .filter(|other| other.name == device.name)
        .count()
        > 1
    {
        format!("{} ({}, #{})", device.name, device.host, device.index)
    } else {
        device.name.clone()
    }
}

fn input_device_combo(
    ui: &mut egui::Ui,
    selected_device: &mut Option<String>,
    devices: &[SoundcardDeviceInfo],
    locale: Locale,
) -> egui::Response {
    let selected = selected_device
        .as_deref()
        .map(|selector| {
            devices
                .iter()
                .find(|device| selector == format!("id:{}", device.id))
                .map(|device| input_device_label(device, devices))
                .unwrap_or_else(|| selector.to_owned())
        })
        .unwrap_or_else(|| locale.text("Default").to_owned());
    let field_width = ui.available_width();
    let popup_margin = egui::Frame::popup(ui.style()).total_margin().sum().x;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_width = std::iter::once(locale.text("Default").to_owned())
        .chain(
            devices
                .iter()
                .map(|device| input_device_label(device, devices)),
        )
        .map(|name| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(name.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0f32, f32::max);
    // The open menu can be wider than its trigger. Keep one line per endpoint,
    // bounded by this viewport; only exceptionally long names need an ellipsis.
    let menu_width = (text_width + 2.0 * ui.spacing().button_padding.x)
        .max((field_width - popup_margin).max(0.0))
        .min((ui.ctx().content_rect().width() - popup_margin - 16.0).max(0.0));
    let response = ui
        .push_id("input_device", |ui| {
            ui.add(
                egui::Button::new(&selected)
                    .right_text(egui::Atom::paint(
                        egui::Vec2::splat(ui.spacing().icon_width),
                        |ui, args| {
                            // Same vector triangle and proportions as egui::ComboBox.
                            let rect = egui::Rect::from_center_size(
                                args.rect.center(),
                                egui::vec2(args.rect.width() * 0.7, args.rect.height() * 0.45),
                            );
                            ui.painter().add(egui::Shape::rotated_triangle(
                                rect,
                                0.0,
                                args.fallback_text_color,
                            ));
                        },
                    ))
                    .truncate()
                    .min_size(egui::vec2(field_width, ui.spacing().interact_size.y)),
            )
        })
        .inner;
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(
            egui::Role::ComboBox,
            ui.is_enabled(),
            locale.text("Input device"),
        );
        info.current_text_value = Some(selected.clone());
        info
    });
    egui::Popup::menu(&response)
        .align(egui::RectAlign::BOTTOM_END)
        .width(menu_width + popup_margin)
        .show(|ui| {
            ui.set_width(menu_width);
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            egui::ScrollArea::vertical()
                .max_height(ui.spacing().combo_height)
                .show(ui, |ui| {
                    ui.selectable_value(selected_device, None, locale.text("Default"));
                    for device in devices {
                        let label = input_device_label(device, devices);
                        ui.selectable_value(
                            selected_device,
                            Some(format!("id:{}", device.id)),
                            label,
                        )
                        .on_hover_text(format!("{} / {}", device.host, device.id));
                    }
                });
        });
    response.on_hover_text(selected)
}

/// Each page owns its grid measurements, widget IDs and scroll position.
/// Filling the available area keeps content width independent of page contents.
fn settings_page<R>(
    ui: &mut egui::Ui,
    tab: SettingsTab,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.push_id(tab, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("settings_content")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                add(ui)
            })
            .inner
    })
    .inner
}

/// Let egui measure labels as one column and give controls the remaining width.
/// The surrounding ScrollArea handles vertical overflow at smaller heights.
fn settings_form(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.spacing_mut().button_padding.y = 4.0;
    egui::Grid::new("settings_form")
        .num_columns(2)
        .spacing(egui::vec2(16.0, 10.0))
        .min_col_width(0.0)
        .show(ui, add);
    ui.add_space(SETTINGS_FORM_BOTTOM_SPACE);
}

/// One row in the standard settings Grid, returning the control's commit signal.
fn setting_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    ui.add(egui::Label::new(label).extend());
    let commit = add(ui);
    ui.end_row();
    commit
}

/// A single-line settings field. Returns true on edit commit (focus left / Enter).
fn text_field(ui: &mut egui::Ui, value: &mut String) -> bool {
    centered_text_edit(ui, value, f32::INFINITY).lost_focus()
}

fn norm(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_uppercase())
    }
}

struct DecodeHighlight {
    bytes: std::ops::Range<usize>,
    background: Color32,
}

fn cap_height_sample(painter: &egui::Painter, font: &egui::FontId) -> std::sync::Arc<egui::Galley> {
    // A stable reference for callsigns, grid locators, and numeric fields.
    painter.layout_no_wrap("H0".into(), font.clone(), Color32::PLACEHOLDER)
}

fn decode_text_origin(row: egui::Rect, ink_center_y: f32, pixels_per_point: f32) -> egui::Pos2 {
    use egui::emath::GuiRounding as _;

    egui::pos2(row.left() + TABLE_PAD, row.center().y - ink_center_y)
        .round_to_pixels(pixels_per_point)
}

fn paint_decode_separator(
    painter: &egui::Painter,
    row: egui::Rect,
    label: String,
    font: egui::FontId,
    color: Color32,
    background: Color32,
) {
    use egui::emath::GuiRounding as _;

    let pixels_per_point = painter.pixels_per_point();
    let row = row.round_to_pixels(pixels_per_point);
    let galley = painter.layout_no_wrap(label, font, color);
    let pos = decode_text_origin(row, galley.mesh_bounds.center().y, pixels_per_point);
    painter.rect_filled(row, 0.0, background);
    painter.galley(pos, galley, color);
}

fn paint_decode_row(
    painter: &egui::Painter,
    row: egui::Rect,
    job: egui::text::LayoutJob,
    highlights: &[DecodeHighlight],
    color: Color32,
    ink_center_y: f32,
) {
    use egui::emath::GuiRounding as _;
    use egui::text::CCursor;

    let galley = painter.layout_job(job);
    let pixels_per_point = painter.pixels_per_point();
    let row = row.round_to_pixels(pixels_per_point);
    let pos = decode_text_origin(row, ink_center_y, pixels_per_point);
    let padding = DECODE_ROW_PADDING_Y.round_to_pixels(pixels_per_point);
    let inset = row.shrink2(egui::vec2(0.0, padding));
    for highlight in highlights {
        let x = |byte| {
            let cursor = CCursor::end_of_str(&galley.job.text[..byte]);
            pos.x + galley.pos_from_cursor(cursor).left()
        };
        // Keep the width tied to the token and the height tied to the table row.
        let background = egui::Rect::from_min_max(
            egui::pos2(x(highlight.bytes.start) - 1.0, inset.top()),
            egui::pos2(x(highlight.bytes.end) + 1.0, inset.bottom()),
        )
        .round_to_pixels(pixels_per_point);
        painter.rect_filled(background, 0.0, highlight.background);
    }
    painter.galley(pos, galley, color);
}

/// Highlight complete callsign and target-grid tokens in the message column, leaving the time,
/// signal and provenance columns in their existing compare-mode color.
fn decode_highlight_job(
    text: &str,
    own_call: Option<&str>,
    target_call: Option<&str>,
    target_grid: Option<&str>,
    font_id: egui::FontId,
    color: Color32,
) -> (egui::text::LayoutJob, Vec<DecodeHighlight>) {
    const MESSAGE_START: usize = 24; // six UTC + dB/DT/frequency fields and separators
    let mut job = egui::text::LayoutJob::default();
    let mut highlights = Vec::new();
    let plain = egui::TextFormat {
        font_id,
        color,
        ..Default::default()
    };
    let mut cursor = 0;
    if let Some(body) = text.get(MESSAGE_START..) {
        let mut body_cursor = 0;
        for token in body.split_whitespace() {
            let start = MESSAGE_START
                + body_cursor
                + body[body_cursor..]
                    .find(token)
                    .expect("token comes from the remaining message");
            if start > cursor {
                job.append(&text[cursor..start], 0.0, plain.clone());
            }
            let normalized = token.trim_matches(['<', '>']).to_ascii_uppercase();
            let background =
                if own_call == Some(normalized.as_str()) && call_candidate(&normalized).is_some() {
                    Some((Color32::YELLOW, Color32::BLACK))
                } else if target_call == Some(normalized.as_str())
                    && call_candidate(&normalized).is_some()
                {
                    Some((Color32::from_rgb(0xe0, 0x22, 0x22), Color32::WHITE))
                } else if target_grid == Some(normalized.as_str()) && is_grid4(&normalized) {
                    Some((Color32::from_rgb(0x18, 0x45, 0xa8), Color32::WHITE))
                } else {
                    None
                };
            let format = if let Some((background, foreground)) = background {
                highlights.push(DecodeHighlight {
                    bytes: start..start + token.len(),
                    background,
                });
                egui::TextFormat {
                    color: foreground,
                    ..plain.clone()
                }
            } else {
                plain.clone()
            };
            job.append(token, 0.0, format);
            cursor = start + token.len();
            body_cursor = cursor - MESSAGE_START;
        }
    }
    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, plain);
    }
    (job, highlights)
}

fn parse_frequency_range(low: &str, high: &str) -> Result<(f64, f64), String> {
    let parse = |value: &str| {
        value
            .trim()
            .parse::<f64>()
            .map_err(|_| "Frequency range must contain numbers".to_owned())
    };
    let (low, high) = (parse(low)?, parse(high)?);
    ft8rs::stream::validation::validate_frequency_range(low, high)?;
    Ok((low, high))
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|&port| port != 0)
        .ok_or_else(|| "UDP port must be an integer from 1 to 65535".to_string())
}

fn absolute_slot(slot_key: u32, now_seconds: u64) -> u64 {
    const SLOTS_PER_DAY: u64 = 5760;
    let current = now_seconds / 15;
    let today = current / SLOTS_PER_DAY * SLOTS_PER_DAY + u64::from(slot_key);
    [
        today.saturating_sub(SLOTS_PER_DAY),
        today,
        today + SLOTS_PER_DAY,
    ]
    .into_iter()
    .min_by_key(|candidate| candidate.abs_diff(current))
    .unwrap()
}

/// Build a divider for each slot with at least one visible decode.
fn display_items(rows: &[Row], filters: DisplayFilters) -> Vec<DisplayItem> {
    let mut items = Vec::with_capacity(rows.len() * 2);
    for (index, row) in rows.iter().enumerate() {
        if !filters.hides(row) {
            push_display_row(&mut items, rows, index);
        }
    }
    items
}

fn push_display_row(items: &mut Vec<DisplayItem>, rows: &[Row], index: usize) {
    let gen = rows[index].gen;
    if !matches!(items.last(), Some(DisplayItem::Decode(last)) if rows[*last].gen == gen) {
        items.push(DisplayItem::Separator(gen, rows[index].slot_band));
    }
    items.push(DisplayItem::Decode(index));
}

fn ft8_payload(tones: &[i32; 79]) -> Option<[u8; 77]> {
    const COSTAS: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
    if tones[..7] != COSTAS || tones[36..43] != COSTAS || tones[72..] != COSTAS {
        return None;
    }

    // All 77 payload bits are systematic and fit in the first data block.
    const UNGRAY: [u8; 8] = [0, 1, 3, 2, 6, 4, 5, 7];
    let mut payload = [0; 77];
    for (bits, &tone) in payload.chunks_mut(3).zip(&tones[7..33]) {
        let symbol = *UNGRAY.get(tone as usize)?;
        for (bit, shift) in bits.iter_mut().zip((0..3).rev()) {
            *bit = (symbol >> shift) & 1;
        }
    }
    Some(payload)
}

fn filterable_message(payload: Option<&[u8; 77]>, message: &str) -> Option<FilterableMessage> {
    let message_type = payload.map(|bits| {
        let subtype = (bits[71] << 2) | (bits[72] << 1) | bits[73];
        let kind = (bits[74] << 2) | (bits[75] << 1) | bits[76];
        (kind, subtype)
    });
    match message_type {
        Some((0, 0)) => None, // Free text can contain either pattern literally.
        Some((0, 5)) => Some(FilterableMessage::Telemetry),
        Some((0, 3 | 4) | (3 | 5, _)) => Some(FilterableMessage::Contest),
        _ if matches_contest_filter(message) => Some(FilterableMessage::Contest),
        _ => None,
    }
}

fn matches_contest_filter(message: &str) -> bool {
    let mut words = message_body_tokens(message).peekable();
    while let Some(word) = words.next() {
        let call = word.trim_matches(['<', '>']);
        if (call.len() > 2 && call.ends_with("/R"))
            || (word == "R" && words.peek().is_some_and(|grid| is_grid4(grid)))
        {
            return true;
        }
    }
    false
}

fn separator_label(gen: u64, band: Option<&str>) -> String {
    let seconds = (gen % CLOCK_SEGMENT_STRIDE) * 15;
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    // Civil date from Unix days (Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let band = band.map_or_else(String::new, |band| format!("{band} - "));
    format!(
        "------ {year:04}-{month:02}-{day:02} - {:02}:{:02}:{:02} UTC - {band}FT8 ------",
        time / 3_600,
        time / 60 % 60,
        time % 60,
    )
}

/// Provenance tag shown as a trailing column after the message
/// (`a7`/`ap`/`a8`/deep/imported). Unknown provenance stays unknown internally,
/// but has no visible tag: it is not a low-confidence message annotation.
fn provenance_tag(provenance: StreamDecodeProvenance) -> &'static str {
    match provenance {
        StreamDecodeProvenance::Unknown => "",
        StreamDecodeProvenance::CooperativeRecovery => "sic",
        StreamDecodeProvenance::ContextRecovery => "ctx",
        StreamDecodeProvenance::PlusCompatibility => "Pc",
        StreamDecodeProvenance::CandidateRetry => "Pr",
        StreamDecodeProvenance::Experiment => "exp",
        StreamDecodeProvenance::A7Memory => "a7",
        StreamDecodeProvenance::ApMask => "ap",
        StreamDecodeProvenance::WsjtxAp(ap_type) => ap_type.annotation(),
        StreamDecodeProvenance::A8List => "a8",
        StreamDecodeProvenance::JtdxDeep => "dp",
        StreamDecodeProvenance::ImportedMemory => "im",
        StreamDecodeProvenance::Regular => "",
    }
}

/// Format a UTC time-of-day given in milliseconds since midnight as `HHMMSS`
/// (matching `SlotTimestamp::format_time`), for external decode rows.
fn format_ms_time(ms: u32) -> String {
    let s = ms / 1000;
    format!("{:02}{:02}{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// JTDX can remove brackets around resolved hashes. Unpack without a callbook
/// to find their positions, then keep the calls already resolved by the decoder.
fn local_display_message(message: &str, payload: Option<&[u8; 77]>) -> String {
    use ft8rs::decode::lib_jtdx::ft8v2::packjt77::unpack77;

    let body = clean_message(message);
    let Some(template) = payload.and_then(|bits| unpack77(bits, None)) else {
        return body;
    };
    if !template.contains("<...>") {
        return body;
    }

    let words: Vec<_> = body.split_whitespace().collect();
    let template: Vec<_> = template.split_whitespace().collect();
    if let Some(restored) = restore_hash_brackets(&words, &template) {
        return restored;
    }

    // JTDX can emit the two legs of a compound transmission as separate rows.
    let restored = match template.as_slice() {
        [first, "RR73;", second, hash, report] => {
            restore_hash_brackets(&words, &[*first, *hash, "RR73"])
                .or_else(|| restore_hash_brackets(&words, &[*second, *hash, *report]))
        }
        ["TU;", _, second, ..] => restore_hash_brackets(&words, &["DE", *second, "TU"])
            .or_else(|| restore_hash_brackets(&words, &template[1..])),
        _ => None,
    };
    restored.unwrap_or(body)
}

fn restore_hash_brackets(words: &[&str], template: &[&str]) -> Option<String> {
    if words.len() != template.len() {
        return None;
    }
    words
        .iter()
        .zip(template)
        .map(|(&word, &field)| match field {
            "<...>" if word.starts_with('<') && word.ends_with('>') => Some(word.into()),
            "<...>"
                if word.len() >= 3
                    && word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/') =>
            {
                Some(format!("<{word}>"))
            }
            _ if word == field => Some(word.into()),
            _ => None,
        })
        .collect::<Option<Vec<String>>>()
        .map(|tokens| tokens.join(" "))
}

/// Keep all annotations supplied locally, with provenance in the trailing column.
fn local_decode_text(
    record: &ft8rs_engine::protocol::DecodeRecord,
    payload: Option<&[u8; 77]>,
) -> String {
    let row = &record.row;
    let mut annotations: Vec<_> = row
        .msg
        .split_whitespace()
        .filter(|t| is_annotation(t))
        .collect();
    let provenance = provenance_tag(record.provenance);
    if !provenance.is_empty() && !annotations.contains(&provenance) {
        annotations.push(provenance);
    }
    format!(
        "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
        record.timestamp.format_time(),
        row.snr.round() as i32,
        row.dt,
        row.freq.round() as i64,
        local_display_message(&row.msg, payload),
        annotations.join(" "),
    )
}

/// External rows display only the message body, without decoder annotations.
fn external_decode_text(decode: &crate::wsjtx_udp::ExternalDecode) -> String {
    format!(
        "{:<6} {:>3} {:>5.1} {:>5}  {:<20} ",
        format_ms_time(decode.ms_since_midnight),
        decode.snr,
        decode.dt,
        decode.freq,
        clean_message(&decode.message),
    )
}

/// One frame of input. The logical prefix preserves per-insertion eviction:
/// removed rows cannot participate in later folds, even before physical drain.
struct DecodeBatch<'a> {
    rows: &'a mut Vec<Row>,
    display: &'a mut Vec<DisplayItem>,
    prefix: usize,
    rebuild: bool,
    tolerance: f64,
    filters: DisplayFilters,
}

impl<'a> DecodeBatch<'a> {
    fn new(
        rows: &'a mut Vec<Row>,
        display: &'a mut Vec<DisplayItem>,
        tolerance: f64,
        filters: DisplayFilters,
    ) -> Self {
        Self {
            rows,
            display,
            prefix: 0,
            rebuild: false,
            tolerance,
            filters,
        }
    }

    fn insert(&mut self, mut incoming: Row) {
        let live = &mut self.rows[self.prefix..];
        let low = live.partition_point(|row| row.gen < incoming.gen);
        let high = live.partition_point(|row| row.gen <= incoming.gen);
        let slot_rows = &mut live[low..high];
        let hidden = self.filters.hides(&incoming);
        if let Some(filterable_changed) = fold_decode_row(slot_rows, &incoming, self.tolerance) {
            self.rebuild |= filterable_changed && self.filters != DisplayFilters::NONE;
            return;
        }
        if let Some(first) = slot_rows.first() {
            incoming.slot_band = first.slot_band;
        }
        let index = self.prefix + high;
        self.rebuild |= index != self.rows.len();
        self.rows.insert(index, incoming);
        if !self.rebuild && !hidden {
            push_display_row(self.display, self.rows, index);
        }
        let prefix = self.rows.len().saturating_sub(MAX_ROWS);
        if prefix > self.prefix {
            self.prefix = prefix;
            self.rebuild = true;
        }
    }
}

impl Drop for DecodeBatch<'_> {
    fn drop(&mut self) {
        if self.prefix != 0 {
            self.rows.drain(..self.prefix);
        }
        if self.rebuild {
            *self.display = display_items(self.rows, self.filters);
        }
    }
}

/// Map time-of-day to its nearest real UTC slot, including late midnight rows.
fn decode_row(
    slot_key: u32,
    freq: f64,
    dt: f64,
    text: String,
    tokens: Vec<String>,
    source: DecodeSource,
    filterable: Option<FilterableMessage>,
) -> Row {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Row {
        text,
        gen: absolute_slot(slot_key, now),
        slot_band: None,
        freq,
        dt,
        tokens,
        source,
        filterable,
    }
}

fn local_decode_row(record: ft8rs_engine::protocol::DecodeRecord) -> Row {
    let payload = ft8_payload(&record.row.itone);
    let mut row = decode_row(
        record.timestamp.milliseconds_since_midnight() / 15_000,
        record.row.freq,
        record.row.dt,
        local_decode_text(&record, payload.as_ref()),
        norm_tokens(&record.row.msg),
        DecodeSource::Local,
        filterable_message(payload.as_ref(), &record.row.msg),
    );
    if let Some(seconds) = record
        .timestamp
        .unix_seconds_utc()
        .filter(|&seconds| seconds >= 0)
    {
        row.gen = seconds as u64 / 15;
    }
    row
}

fn external_decode_row(decode: crate::wsjtx_udp::ExternalDecode) -> Row {
    decode_row(
        decode.ms_since_midnight / 15_000,
        decode.freq as f64,
        decode.dt,
        external_decode_text(&decode),
        norm_tokens(&decode.message),
        DecodeSource::External,
        None,
    )
}

/// Merge repeated observations of the same transmission. SNR and annotation
/// changes do not create a new signal. DT keeps distinct same-message signals
/// apart; cross-source matching retains the configured frequency tolerance.
/// A match reports whether its display-filter category changed.
fn fold_decode_row(rows: &mut [Row], incoming: &Row, tolerance: f64) -> Option<bool> {
    for row in rows {
        let same_source = row.source == incoming.source;
        let tolerance = if same_source {
            tolerance.min(5.0)
        } else {
            tolerance
        };
        if row.gen == incoming.gen
            && (row.freq - incoming.freq).abs() <= tolerance
            && (row.dt - incoming.dt).abs() <= 0.3
            && tokens_compatible(&row.tokens, &incoming.tokens)
        {
            let previous_filterable = row.local_filterable();
            if incoming.source == DecodeSource::Local && row.source == DecodeSource::External {
                let resolved = row.tokens.clone();
                row.text.clone_from(&incoming.text);
                row.freq = incoming.freq;
                row.dt = incoming.dt;
                row.tokens.clone_from(&incoming.tokens);
                retain_resolved_hash_calls(row, &resolved);
            } else {
                retain_resolved_hash_calls(row, &incoming.tokens);
                if incoming.source == DecodeSource::Local {
                    for annotation in incoming
                        .text
                        .split_whitespace()
                        .filter(|t| is_annotation(t))
                    {
                        if !row.text.split_whitespace().any(|token| token == annotation) {
                            row.text.push(' ');
                            row.text.push_str(annotation);
                        }
                    }
                }
            }
            if row.source != incoming.source {
                row.source = DecodeSource::Both;
            }
            row.filterable = row.filterable.or(incoming.filterable);
            return Some(row.local_filterable() != previous_filterable);
        }
    }
    None
}

/// Enrich only the displayed row, keeping its measurements and provenance.
/// Calls learned from UDP never feed back into a native decoder session.
fn retain_resolved_hash_calls(row: &mut Row, known: &[String]) {
    let mut placeholders = row.text.match_indices("<...>");
    let mut replacements = Vec::new();
    for (token, known) in row.tokens.iter_mut().zip(known) {
        if is_hash_token(token) {
            let position = placeholders.next().map(|(position, _)| position);
            if !is_hash_token(known) {
                token.clone_from(known);
                if let Some(position) = position {
                    replacements.push((position, format!("<{known}>")));
                }
            }
        }
    }
    // Replace from the end so longer calls do not shift earlier positions.
    for (position, call) in replacements.into_iter().rev() {
        row.text.replace_range(position..position + 5, &call);
    }
}

/// Normalize a decode message for merge matching: drop WSJT-X annotations, then
/// uppercase the remaining tokens (the actual message body).
fn norm_tokens(msg: &str) -> Vec<String> {
    message_body_tokens(msg)
        .map(|t| {
            if t == "<...>" {
                t.to_string()
            } else {
                t.trim_matches(['<', '>']).to_ascii_uppercase()
            }
        })
        .collect()
}

/// A 4-character Maidenhead grid locator (`AA00`..`RR99`).
fn is_grid4(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 4
        && (b'A'..=b'R').contains(&b[0])
        && (b'A'..=b'R').contains(&b[1])
        && b[2].is_ascii_digit()
        && b[3].is_ascii_digit()
}

/// A report/marker token that is never a callsign: signal reports (`-08`, `R+00`),
/// acks (`RR73`/`RRR`/`73`/`R`), CQ directional/marker words, or a pure-digit
/// directed-CQ frequency like `CQ 072 ...`.
fn is_report_or_marker(s: &str) -> bool {
    matches!(
        s,
        "RR73" | "RRR" | "73" | "R" | "RR" | "TU" | "DX" | "QRZ" | "TEST" | "POTA" | "SOTA"
    ) || s.starts_with(['+', '-'])
        || (s.starts_with('R') && s.get(1..2).is_some_and(|c| c == "+" || c == "-"))
        || s.bytes().all(|b| b.is_ascii_digit())
}

/// Extract a callsign from a token (stripping `<>` hash braces), or `None` if it
/// is a grid, a report/marker, or too short to be a call.
fn call_candidate(tok: &str) -> Option<String> {
    let t = tok.trim_matches(['<', '>']);
    if t.len() < 3 || t == "..." || is_grid4(t) || is_report_or_marker(t) {
        return None;
    }
    let has_digit = t.bytes().any(|b| b.is_ascii_digit());
    let has_alpha = t.bytes().any(|b| b.is_ascii_alphabetic());
    (has_digit && has_alpha).then(|| t.to_string())
}

/// The DX callsign and grid to load from a decode's tokens on double-click. For a
/// `CQ` message it is the first real callsign after `CQ` (skipping `DX`/directional
/// markers) and the grid following it; otherwise the message is directed
/// (`TO FROM ...`) and the DX is the sender — the second token — with a grid only
/// if the third token is a grid4.
fn dx_call_and_grid(tokens: &[String]) -> (Option<String>, Option<String>) {
    if tokens.is_empty() {
        return (None, None);
    }
    if tokens[0] == "CQ" {
        for i in 1..tokens.len() {
            if let Some(call) = call_candidate(&tokens[i]) {
                return (Some(call), grid_after(tokens, i + 1));
            }
        }
        (None, None)
    } else {
        let call = tokens.get(1).and_then(|t| call_candidate(t));
        (call, grid_after(tokens, 2))
    }
}

fn fill_target_context(call: &mut String, grid: &mut String, tokens: &[String]) {
    let (new_call, new_grid) = dx_call_and_grid(tokens);
    if let Some(new_call) = new_call {
        if norm(call).as_ref() != Some(&new_call) {
            grid.clear();
        }
        *call = new_call;
        if let Some(new_grid) = new_grid {
            *grid = new_grid;
        }
    }
}

/// The token at `idx` as a grid, or `None`. `is_grid4` is purely structural, so
/// `RR73` (the ack) matches it — exclude report/marker tokens here.
fn grid_after(tokens: &[String], idx: usize) -> Option<String> {
    tokens
        .get(idx)
        .filter(|g| is_grid4(g) && !is_report_or_marker(g))
        .map(|g| g.to_string())
}

/// Use the same message body for local/external display and compare matching.
fn clean_message(msg: &str) -> String {
    message_body_tokens(msg).collect::<Vec<_>>().join(" ")
}

fn message_body_tokens(msg: &str) -> impl Iterator<Item = &str> {
    msg.split_whitespace().filter(|t| !is_annotation(t))
}

/// Decoder annotations are separate from the uppercase FT8 payload. Accept a
/// confidence marker adjoining an AP tag as well as whitespace-separated tags.
/// Local provenance normally lives in its own column; also ignore it if a
/// sender includes it in the message tail.
fn is_annotation(token: &str) -> bool {
    let tag = token.trim_matches('?');
    token == "?"
        || matches!(
            tag,
            "sic" | "ctx" | "exp" | "ap" | "dp" | "im" | "p" | "Pc" | "Pr" | "retry"
        )
        || (tag.len() >= 2 && tag.starts_with('a') && tag[1..].bytes().all(|b| b.is_ascii_digit()))
}

/// Whether two normalized messages describe the same transmission. Only an
/// unresolved `<...>` is a wildcard; two different resolved calls never match.
fn tokens_compatible(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x == y || is_hash_token(x) || is_hash_token(y))
}

fn is_hash_token(token: &str) -> bool {
    token == "<...>"
}

/// Deliver native menu events and wake the root UI immediately. Muda's default
/// receiver alone does not wake winit, so polling it waits for an unrelated frame.
fn native_menu_events(ctx: &egui::Context) -> Receiver<muda::MenuEvent> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    muda::MenuEvent::set_event_handler(Some(move |event| {
        if tx.send(event).is_ok() {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }));
    rx
}

// Immediate viewport commands are processed by eframe after drawing and
// swapping buffers. Start dialogs hidden in their builder and reveal them with
// Visible here, so an unpainted native window never appears during creation.
/// Space focus retries in wall time so fast startup frames cannot exhaust them
/// before Windows has shown/activated the dialog. Reopening a minimized dialog
/// also needs to restore it: winit ignores Focus on invisible/minimized windows.
#[derive(Default)]
struct DialogFocus {
    remaining: u8,
    next_attempt: Option<Instant>,
}

impl DialogFocus {
    fn request(&mut self) {
        self.remaining = DIALOG_FOCUS_ATTEMPTS;
        self.next_attempt = None;
    }

    fn update(&mut self, ctx: &egui::Context, now: Instant) {
        if self.remaining == 0 {
            return;
        }
        if self.next_attempt.is_none_or(|next| now >= next) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.remaining -= 1;
            self.next_attempt = Some(now + DIALOG_FOCUS_INTERVAL);
        }
        if self.remaining > 0 {
            let delay = self.next_attempt.unwrap().saturating_duration_since(now);
            ctx.request_repaint_after(delay);
            // Immediate viewports are drawn by their parent, so wake it too.
            ctx.request_repaint_after_for(delay, egui::ViewportId::ROOT);
        }
    }
}

/// Compare-mode row color: external-only = sky, local-only = rose (tailwind,
/// lighter -300 on dark themes / darker -900 on light), both = default.
fn source_color(source: DecodeSource, dark: bool, default: Color32) -> Color32 {
    match source {
        DecodeSource::Both => default,
        DecodeSource::External => {
            if dark {
                Color32::from_rgb(0x7d, 0xd3, 0xfc) // sky-300
            } else {
                Color32::from_rgb(0x0c, 0x4a, 0x6e) // sky-900
            }
        }
        DecodeSource::Local => {
            if dark {
                Color32::from_rgb(0xfd, 0xa4, 0xaf) // rose-300
            } else {
                Color32::from_rgb(0x88, 0x13, 0x37) // rose-900
            }
        }
    }
}

/// UI labels are independent of the stable lowercase storage and menu IDs.
fn profile_label(profile: DecodeProfile) -> &'static str {
    match profile {
        DecodeProfile::Wsjtx => "WSJT-X",
        DecodeProfile::WsjtxPlus => "WSJT-X+",
        DecodeProfile::Jtdx => "JTDX",
        DecodeProfile::Hybrid => "Hybrid",
        DecodeProfile::Dx => "DX",
    }
}

/// Profile names stay the same in both interface languages.
const MENU_PROFILES: [DecodeProfile; 5] = [
    DecodeProfile::Wsjtx,
    DecodeProfile::WsjtxPlus,
    DecodeProfile::Jtdx,
    DecodeProfile::Hybrid,
    DecodeProfile::Dx,
];

/// The native main-window handle (HWND) on Windows, used to attach the menu bar.
/// None on other platforms (and if eframe can't supply a Win32 handle).
#[cfg(target_os = "windows")]
fn window_hwnd(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn window_hwnd(_cc: &eframe::CreationContext<'_>) -> Option<isize> {
    None
}

/// Retain the native menu and the handles needed for live labels and checkmarks.
#[derive(Default)]
struct NativeMenu {
    _menu: Option<muda::Menu>,
    settings: Option<muda::MenuItem>,
    about: Option<muda::MenuItem>,
    profile: Option<muda::Submenu>,
    language: Option<muda::Submenu>,
    quit: Option<muda::PredefinedMenuItem>,
    profile_items: Vec<(DecodeProfile, muda::CheckMenuItem)>,
    language_items: Vec<(Language, muda::CheckMenuItem)>,
    debug: Option<NativeDebugMenu>,
}

struct NativeDebugMenu {
    submenu: muda::Submenu,
    collect: muda::CheckMenuItem,
    save: muda::MenuItem,
    open: muda::MenuItem,
    synced: std::cell::Cell<Option<(Locale, bool, bool)>>,
    visible: std::cell::Cell<bool>,
}

impl NativeMenu {
    fn sync_debug(&self, locale: Locale, visible: bool, active: bool, can_save: bool) {
        let (Some(menu), Some(debug)) = (&self._menu, &self.debug) else {
            return;
        };
        let state = (locale, active, can_save);
        if debug.synced.get() != Some(state) {
            debug.submenu.set_text(locale.text("Developer Mode"));
            debug
                .collect
                .set_text(locale.text("Collect Debug Information"));
            debug.save.set_text(locale.text("Save Debug Information"));
            debug
                .open
                .set_text(locale.text("Open Debug Information Folder"));
            debug.collect.set_checked(active);
            debug.save.set_enabled(can_save);
            debug.synced.set(Some(state));
        }
        if visible != debug.visible.get() {
            let result = if visible {
                menu.append(&debug.submenu)
            } else {
                menu.remove(&debug.submenu)
            };
            match result {
                Ok(()) => debug.visible.set(visible),
                Err(err) => log::warn!("Developer menu visibility: {err}"),
            }
        }
    }

    fn sync(&self, profile: DecodeProfile, language: Language, locale: Locale) {
        if let Some(item) = &self.settings {
            item.set_text(locale.text(if cfg!(target_os = "macos") {
                "Settings…"
            } else {
                "Settings"
            }));
        }
        if let Some(item) = &self.about {
            item.set_text(locale.text("About"));
        }
        if let Some(item) = &self.profile {
            item.set_text(locale.text("Profile"));
        }
        if let Some(item) = &self.language {
            item.set_text(locale.text("Language"));
        }
        if let Some(item) = &self.quit {
            item.set_text(locale.text("Quit"));
        }
        for (value, item) in &self.profile_items {
            item.set_checked(*value == profile);
        }
        for (value, item) in &self.language_items {
            item.set_text(value.label(locale));
            item.set_checked(*value == language);
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn install_menu(
    initial: DecodeProfile,
    language: Language,
    locale: Locale,
    _hwnd: Option<isize>,
) -> NativeMenu {
    use muda::{CheckMenuItem, Menu, MenuItem, Submenu};

    #[cfg(target_os = "windows")]
    let Some(hwnd) = _hwnd
    else {
        return NativeMenu::default();
    };
    let menu = Menu::new();
    let settings = MenuItem::with_id(
        "settings",
        "Settings",
        true,
        if cfg!(target_os = "macos") {
            "CmdOrCtrl+,".parse().ok()
        } else {
            None
        },
    );
    let about = MenuItem::with_id("about", "About", true, None);
    let profile_menu = Submenu::new("Profile", true);
    let mut profile_items = Vec::new();
    for profile in MENU_PROFILES {
        let item = CheckMenuItem::with_id(
            format!("profile:{}", profile.as_str()),
            profile_label(profile),
            true,
            profile == initial,
            None,
        );
        let _ = profile_menu.append(&item);
        profile_items.push((profile, item));
    }
    let language_menu = Submenu::new("Language", true);
    let mut language_items = Vec::new();
    for value in Language::ALL {
        let item = CheckMenuItem::with_id(
            format!("language:{}", value.as_str()),
            value.label(locale),
            true,
            value == language,
            None,
        );
        let _ = language_menu.append(&item);
        language_items.push((value, item));
    }
    #[cfg(target_os = "macos")]
    let quit = Some(muda::PredefinedMenuItem::quit(Some("Quit")));
    #[cfg(not(target_os = "macos"))]
    let quit = None;
    let debug = NativeDebugMenu {
        submenu: Submenu::new("Developer Mode", true),
        collect: CheckMenuItem::with_id(
            "debug:collect",
            "Collect Debug Information",
            true,
            false,
            None,
        ),
        save: MenuItem::with_id("debug:save", "Save Debug Information", false, None),
        open: MenuItem::with_id("debug:open", "Open Debug Information Folder", true, None),
        synced: Default::default(),
        visible: Default::default(),
    };
    let _ = debug
        .submenu
        .append_items(&[&debug.collect, &debug.save, &debug.open]);
    let native = NativeMenu {
        quit,
        _menu: Some(menu.clone()),
        settings: Some(settings.clone()),
        about: Some(about.clone()),
        profile: Some(profile_menu.clone()),
        language: Some(language_menu.clone()),
        profile_items,
        language_items,
        debug: Some(debug),
    };
    #[cfg(target_os = "macos")]
    {
        use muda::PredefinedMenuItem;
        let app_menu = Submenu::new(crate::APP_NAME, true);
        let quit = native.quit.as_ref().expect("macOS menu has a Quit item");
        let _ = app_menu.append_items(&[
            &about,
            &PredefinedMenuItem::separator(),
            &settings,
            &PredefinedMenuItem::separator(),
            quit,
        ]);
        let _ = menu.append_items(&[&app_menu, &profile_menu, &language_menu]);
    }
    #[cfg(target_os = "windows")]
    let _ = menu.append_items(&[&settings, &profile_menu, &language_menu, &about]);

    native.sync(initial, language, locale);
    #[cfg(target_os = "macos")]
    menu.init_for_nsapp();
    // SAFETY: hwnd is the live main-window handle supplied by eframe.
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = menu.init_for_hwnd(hwnd);
    }
    native
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn install_menu(
    _initial: DecodeProfile,
    _language: Language,
    _locale: Locale,
    _hwnd: Option<isize>,
) -> NativeMenu {
    NativeMenu::default()
}

/// Apply the custom style for the given OS theme. Colors come from egui's
/// adaptive light/dark visuals (so the app follows the system appearance); we
/// only customize corner_radius, spacing, a blue accent, and the monospace fonts.
fn apply_style(ctx: &egui::Context, theme: egui::Theme) {
    use egui::{CornerRadius, FontFamily::Monospace, FontId, Margin, Stroke, TextStyle};

    let dark = theme == egui::Theme::Dark;
    let accent = if dark {
        Color32::from_rgb(0x3b, 0x82, 0xf6) // blue-500
    } else {
        Color32::from_rgb(0x25, 0x63, 0xeb) // blue-600
    };

    let mut style = (*ctx.global_style()).clone();
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    let corner_radius = CornerRadius::same(2);
    v.widgets.noninteractive.corner_radius = corner_radius;
    v.widgets.inactive.corner_radius = corner_radius;
    v.widgets.hovered.corner_radius = corner_radius;
    v.widgets.active.corner_radius = corner_radius;
    v.widgets.open.corner_radius = corner_radius;

    // No outline and no size change on interaction — hover/press only shift the
    // fill brightness (tailwind gray ramp, with a clear hover step).
    let (fill_inactive, fill_hovered, fill_active) = if dark {
        (
            Color32::from_rgb(0x37, 0x41, 0x51), // gray-700
            Color32::from_rgb(0x57, 0x62, 0x73), // ~gray-550 (clearly brighter)
            Color32::from_rgb(0x6b, 0x72, 0x80), // gray-500
        )
    } else {
        (
            Color32::from_rgb(0xf3, 0xf4, 0xf6), // gray-100
            Color32::from_rgb(0xdc, 0xdf, 0xe4), // ~gray-250 (clearly darker)
            Color32::from_rgb(0xcb, 0xd0, 0xd7), // ~gray-350
        )
    };
    v.widgets.inactive.weak_bg_fill = fill_inactive;
    v.widgets.hovered.weak_bg_fill = fill_hovered;
    v.widgets.active.weak_bg_fill = fill_active;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.bg_stroke = Stroke::NONE;
    // Don't let widgets expand on hover/press (default adds ~1px all round).
    v.widgets.inactive.expansion = 0.0;
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.expansion = 0.0;
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);

    // Glyph rasterization is anti-aliased by egui. Keep hinting explicitly on
    // in both themes; per-font CJK tweaks choose sharper horizontal grid fitting.
    v.text_options.font_hinting = true;
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = Margin::same(12);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.interact_size.y = 24.0;

    // Monospace everywhere (matching the decode table).
    style.text_styles = [
        (TextStyle::Heading, FontId::new(17.0, Monospace)),
        (TextStyle::Body, FontId::new(BODY_FONT_SIZE, Monospace)),
        (TextStyle::Button, FontId::new(13.0, Monospace)),
        (TextStyle::Small, FontId::new(SMALL_FONT_SIZE, Monospace)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into();

    // eframe 0.35: Context::set_style is gone; set the same style for all themes.
    ctx.all_styles_mut(move |s| *s = style.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ft8rs_engine::soundcard::SoundcardFormatInfo;

    fn insert_decode_row(
        rows: &mut Vec<Row>,
        display: &mut Vec<DisplayItem>,
        incoming: Row,
        tolerance: f64,
    ) {
        DecodeBatch::new(rows, display, tolerance, DisplayFilters::NONE).insert(incoming);
    }

    // Frozen sequential UI operation, before batching. This is intentionally
    // independent of DecodeBatch's indexing/eviction bookkeeping.
    fn insert_decode_row_reference(
        rows: &mut Vec<Row>,
        display: &mut Vec<DisplayItem>,
        mut incoming: Row,
        tolerance: f64,
    ) {
        if fold_decode_row(rows, &incoming, tolerance).is_some() {
            return;
        }
        let gen = incoming.gen;
        let index = rows.partition_point(|row| row.gen <= gen);
        if let Some(previous) = rows[..index].last().filter(|row| row.gen == gen) {
            incoming.slot_band = previous.slot_band;
        }
        let append = index == rows.len();
        if append {
            if rows.last().is_none_or(|row| row.gen != gen) {
                display.push(DisplayItem::Separator(gen, incoming.slot_band));
            }
            display.push(DisplayItem::Decode(index));
        }
        rows.insert(index, incoming);
        if !append {
            *display = display_items(rows, DisplayFilters::NONE);
        }
        if rows.len() > MAX_ROWS {
            rows.drain(..rows.len() - MAX_ROWS);
            *display = display_items(rows, DisplayFilters::NONE);
        }
    }

    #[test]
    fn batch_matches_sequential_rows_across_capacity_late_and_hash_updates() {
        let row = |gen, id| {
            let message = format!("CQ K{id}ABC FN42");
            Row {
                text: message.clone(),
                gen,
                slot_band: None,
                freq: 200.0 + (id % 2800) as f64,
                dt: 0.0,
                tokens: norm_tokens(&message),
                filterable: None,
                source: DecodeSource::Local,
            }
        };
        for initial in [0, MAX_ROWS - 10, MAX_ROWS] {
            let initial_rows: Vec<_> = (0..initial).map(|i| row((i / 40) as u64, i)).collect();
            let mut seed = 739u64;
            let mut input = Vec::new();
            for i in 0..1536 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let mut incoming = if seed.is_multiple_of(3) && !initial_rows.is_empty() {
                    initial_rows[(seed as usize / 3) % initial_rows.len()].clone()
                } else {
                    row(498 + seed % 8, i + seed as usize % 10000)
                };
                if seed.is_multiple_of(5) {
                    incoming.tokens[1] = "<...>".into();
                    incoming.text = "CQ <...> FN42".into();
                }
                if seed.is_multiple_of(7) {
                    incoming.source = DecodeSource::External;
                }
                if seed.is_multiple_of(11) {
                    incoming.text.push_str(" exp");
                }
                input.push(incoming);
            }
            for chunk_size in [1, 17, 128, 1536] {
                let mut expected = initial_rows.clone();
                let mut expected_display = display_items(&expected, DisplayFilters::NONE);
                let mut actual = expected.clone();
                let mut actual_display = expected_display.clone();
                for chunk in input.chunks(chunk_size) {
                    for incoming in chunk {
                        insert_decode_row_reference(
                            &mut expected,
                            &mut expected_display,
                            incoming.clone(),
                            7.0,
                        );
                    }
                    {
                        let mut batch = DecodeBatch::new(
                            &mut actual,
                            &mut actual_display,
                            7.0,
                            DisplayFilters::NONE,
                        );
                        for incoming in chunk {
                            batch.insert(incoming.clone());
                        }
                    }
                    assert_eq!(
                        actual, expected,
                        "initial={initial}, chunk_size={chunk_size}"
                    );
                    assert_eq!(actual_display, expected_display);
                }
            }
        }
    }

    #[test]
    fn repeated_udp_observations_and_late_rows_stay_in_one_slot_section() {
        let row = |gen, source, dt, snr| Row {
            text: format!("{snr} JA8IIK/P SV9GPV -18"),
            gen,
            slot_band: None,
            freq: 1137.0,
            dt,
            tokens: norm_tokens("JA8IIK/P SV9GPV -18"),
            filterable: None,
            source,
        };
        let mut rows = Vec::new();
        let mut display = Vec::new();
        insert_decode_row(
            &mut rows,
            &mut display,
            row(1, DecodeSource::External, 2.4, -6),
            7.0,
        );
        insert_decode_row(
            &mut rows,
            &mut display,
            row(1, DecodeSource::External, 2.4, -8),
            7.0,
        );
        assert_eq!(rows.len(), 1);
        insert_decode_row(
            &mut rows,
            &mut display,
            row(1, DecodeSource::Local, 2.4, -7),
            7.0,
        );
        insert_decode_row(
            &mut rows,
            &mut display,
            row(1, DecodeSource::External, 2.4, -9),
            7.0,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, DecodeSource::Both);
        assert!(rows[0].text.starts_with("-7 "));
        // Same payload at a distinct DT is a separate transmission.
        insert_decode_row(
            &mut rows,
            &mut display,
            row(1, DecodeSource::Local, 0.1, -20),
            7.0,
        );
        assert_eq!(rows.len(), 2);
        insert_decode_row(
            &mut rows,
            &mut display,
            row(3, DecodeSource::External, 0.1, -20),
            7.0,
        );
        insert_decode_row(
            &mut rows,
            &mut display,
            row(2, DecodeSource::Local, 0.1, -20),
            7.0,
        );
        assert_eq!(rows.iter().map(|r| r.gen).collect::<Vec<_>>(), [1, 1, 2, 3]);
        let sections: Vec<_> = display
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Separator(gen, _) => Some(*gen),
                _ => None,
            })
            .collect();
        assert_eq!(sections, [1, 2, 3]);
    }

    #[test]
    fn slot_separators_capture_the_current_band_only_on_first_insert() {
        let row = |gen, slot_band, freq, filterable| Row {
            text: "CQ K1ABC FN42".into(),
            gen,
            slot_band,
            freq,
            dt: 0.0,
            tokens: norm_tokens("CQ K1ABC FN42"),
            filterable,
            source: DecodeSource::Local,
        };
        let filters = DisplayFilters {
            hide_contest: false,
            hide_telemetry: true,
        };
        let mut rows = Vec::new();
        let mut display = Vec::new();
        {
            let mut batch = DecodeBatch::new(&mut rows, &mut display, 7.0, filters);
            batch.insert(row(
                10,
                Some("20m"),
                1000.0,
                Some(FilterableMessage::Telemetry),
            ));
            batch.insert(row(12, None, 1000.0, None));
            batch.insert(row(10, Some("40m"), 1100.0, None));
            batch.insert(row(11, Some("40m"), 1000.0, None));
            batch.insert(row(12, Some("40m"), 1100.0, None));
        }
        assert_eq!(
            display,
            [
                DisplayItem::Separator(10, Some("20m")),
                DisplayItem::Decode(1),
                DisplayItem::Separator(11, Some("40m")),
                DisplayItem::Decode(2),
                DisplayItem::Separator(12, None),
                DisplayItem::Decode(3),
                DisplayItem::Decode(4),
            ]
        );
        assert_eq!(
            display_items(&rows[1..], filters)[0],
            DisplayItem::Separator(10, Some("20m"))
        );
    }

    #[test]
    fn display_filters_retain_local_rows_and_leave_udp_visible() {
        let row = |gen, text: &str, source, filterable| Row {
            text: text.into(),
            gen,
            slot_band: None,
            freq: 1386.0,
            dt: 0.0,
            tokens: norm_tokens(text),
            filterable,
            source,
        };
        let filters = DisplayFilters {
            hide_contest: true,
            hide_telemetry: true,
        };
        let rover_cq = "CQ K1ABC/R FN42";
        let mut rows = Vec::new();
        let mut display = Vec::new();
        {
            let mut batch = DecodeBatch::new(&mut rows, &mut display, 7.0, filters);
            batch.insert(row(
                10,
                "7",
                DecodeSource::Local,
                Some(FilterableMessage::Telemetry),
            ));
            batch.insert(row(
                11,
                rover_cq,
                DecodeSource::Local,
                Some(FilterableMessage::Contest),
            ));
            batch.insert(row(12, "17E65738A547413FF0", DecodeSource::External, None));
        }
        assert_eq!(
            display,
            [DisplayItem::Separator(12, None), DisplayItem::Decode(2)]
        );
        assert_eq!(display_items(&rows, DisplayFilters::NONE).len(), 6);
        for (filters, visible_slot) in [
            (
                DisplayFilters {
                    hide_contest: false,
                    ..filters
                },
                11,
            ),
            (
                DisplayFilters {
                    hide_telemetry: false,
                    ..filters
                },
                10,
            ),
        ] {
            assert_eq!(
                display_items(&rows, filters)[0],
                DisplayItem::Separator(visible_slot, None)
            );
        }
        DecodeBatch::new(&mut rows, &mut display, 7.0, filters).insert(row(
            11,
            rover_cq,
            DecodeSource::External,
            None,
        ));
        DecodeBatch::new(&mut rows, &mut display, 7.0, filters).insert(row(
            12,
            "17E65738A547413FF0",
            DecodeSource::Local,
            Some(FilterableMessage::Telemetry),
        ));
        assert_eq!(
            display,
            [
                DisplayItem::Separator(11, None),
                DisplayItem::Decode(1),
                DisplayItem::Separator(12, None),
                DisplayItem::Decode(2),
            ]
        );
        assert_eq!(rows[1].source, DecodeSource::Both);
        assert_eq!(rows[2].source, DecodeSource::Both);
        assert_eq!(display_items(&rows, filters), display);
    }

    #[test]
    fn reads_filter_types_from_the_ft8_symbols() {
        use FilterableMessage::{Contest, Telemetry};

        // Symbols 30..33 from frames encoded by the FT8 encoder.
        for (symbols, expected) in [
            ([4, 0, 0], None),            // 0.0, free text
            ([3, 3, 1], None),            // 0.1, DXpedition
            ([0, 4, 0], Some(Contest)),   // 0.3, Field Day
            ([7, 0, 1], Some(Contest)),   // 0.4, Field Day
            ([7, 3, 1], Some(Telemetry)), // 0.5, telemetry
            ([4, 0, 3], None),            // 1, standard
            ([6, 5, 5], None),            // 2, /P
            ([6, 3, 4], Some(Contest)),   // 3, RTTY Roundup
            ([6, 1, 0], None),            // 4, nonstandard call
            ([3, 2, 2], Some(Contest)),   // 5, EU VHF exchange
        ] {
            let mut tones = [0; 79];
            for sync in [0, 36, 72] {
                tones[sync..sync + 7].copy_from_slice(&[3, 1, 4, 0, 6, 5, 2]);
            }
            tones[30..33].copy_from_slice(&symbols);
            assert_eq!(
                filterable_message(ft8_payload(&tones).as_ref(), ""),
                expected
            );
        }
        assert_eq!(ft8_payload(&[0; 79]), None);
    }

    fn local_fixture_row(wire: &str, decoded: &str) -> Row {
        use ft8rs::decode::lib_jtdx::ft8v2::packjt77::pack77;

        const COSTAS: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
        const GRAY: [i32; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
        let bits = pack77(wire);
        let mut tones = [0; 79];
        for sync in [0, 36, 72] {
            tones[sync..sync + 7].copy_from_slice(&COSTAS);
        }
        for (tone, bits) in tones[7..33].iter_mut().zip(bits.chunks(3)) {
            let symbol = (bits[0] << 2) | (bits[1] << 1) | bits.get(2).copied().unwrap_or(0);
            *tone = GRAY[symbol as usize];
        }
        assert_eq!(ft8_payload(&tones).unwrap().as_slice(), bits);
        local_decode_row(ft8rs_engine::protocol::DecodeRecord {
            timestamp: ft8rs::SlotTimestamp::parse("120000").unwrap(),
            row: ft8rs::stream::StreamDecodedMessage {
                freq: 1002.0,
                dt: 0.1,
                snr: -20.0,
                msg: decoded.into(),
                sync: 0.0,
                itone: tones,
            },
            provenance: StreamDecodeProvenance::Experiment,
            stage: ft8rs_engine::protocol::DecodeStage::Final,
        })
    }

    #[test]
    fn hide_contest_matches_payload_types_rover_calls_and_acknowledged_grids() {
        let filters = DisplayFilters {
            hide_contest: true,
            ..DisplayFilters::NONE
        };
        for (message, hidden) in [
            ("CQ WU8JCF/R EN81", true),
            ("K1ABC/R W9XYZ RR73", true),
            ("<K1ABC/R> W9XYZ RR73", true),
            ("BG5ATV WA7QSU R MO76", true),
            ("W9XYZ K1ABC R RR99", true),
            ("G4ABC/P PA9XYZ JO22", false),
            ("PA9XYZ G4ABC/P R-10", false),
            ("CQ G4ABC/P IO91", false),
            ("PA9XYZ G4ABC/P R IO91", true),
            ("K1ABC W9XYZ R 6A WI", true),
            ("K1ABC W9XYZ R 32A EMA", true),
            ("K1ABC W9XYZ R 579 MA", true),
            ("<G4ABC/P> <PA9XYZ> R 580071 JO22DB", true),
            ("CQ ZW5STAY/R", true),
            ("CQ TEST K1ABC FN42", false),
            ("CQ FD K1ABC FN42", false),
            ("CQ RU K1ABC FN42", false),
            ("CQ WW K1ABC FN42", false),
            ("CQ TEST K1ABC/R FN42", true),
            ("CQ FD K1ABC/R FN42", true),
            ("CQ RU K1ABC/R FN42", true),
            ("CQ WW K1ABC/R FN42", true),
            ("CQ RU G4ABC/P IO91", false),
            ("BG5ATV WA7QSU MO76", false),
            ("BG5ATV WA7QSU R-10", false),
            ("BG5ATV WA7QSU R+10", false),
            ("BG5ATV WA7QSU RR73", false),
            ("CQ POTA K1ABC FN42", false),
            ("CQ 6W/IK2HKT", false),
            ("TEST /R", false),
            ("R MO76", false),
        ] {
            let row = local_fixture_row(message, message);
            let mut rows = Vec::new();
            let mut display = Vec::new();
            DecodeBatch::new(&mut rows, &mut display, 7.0, filters).insert(row);
            assert_eq!(display.is_empty(), hidden, "{message}");
            assert_eq!(display, display_items(&rows, filters));
            assert_eq!(display_items(&rows, DisplayFilters::NONE).len(), 2);
        }
        assert_eq!(
            filterable_message(None, "BG5ATV WA7QSU R MO76 a7"),
            Some(FilterableMessage::Contest)
        );
    }

    #[test]
    fn local_rows_restore_hash_brackets_from_the_received_payload() {
        for (wire, decoded, expected) in [
            (
                "<6W/IK2HKT> JA5ARW PM64",
                "6W/IK2HKT JA5ARW PM64",
                "<6W/IK2HKT> JA5ARW PM64",
            ),
            (
                "<6W/IK2HKT> JA5ARW PM64",
                "<...> JA5ARW PM64",
                "<...> JA5ARW PM64",
            ),
            ("CQ 6W/IK2HKT", "CQ 6W/IK2HKT", "CQ 6W/IK2HKT"),
            (
                "<JA5ARW> 6W/IK2HKT RR73",
                "JA5ARW 6W/IK2HKT RR73",
                "<JA5ARW> 6W/IK2HKT RR73",
            ),
            (
                "JA5ARW <6W/IK2HKT> -12",
                "JA5ARW 6W/IK2HKT -12",
                "JA5ARW <6W/IK2HKT> -12",
            ),
            (
                "K1ABC RR73; W9XYZ <KH1/KH7Z> -08",
                "K1ABC KH1/KH7Z RR73",
                "K1ABC <KH1/KH7Z> RR73",
            ),
            (
                "K1ABC RR73; W9XYZ <KH1/KH7Z> -08",
                "W9XYZ KH1/KH7Z -08",
                "W9XYZ <KH1/KH7Z> -08",
            ),
            (
                "<G4ABC/P> <PA9XYZ> R 580071 JO22DB",
                "G4ABC/P <PA9XYZ> R 580071 JO22DB",
                "<G4ABC/P> <PA9XYZ> R 580071 JO22DB",
            ),
            (
                "<6W/IK2HKT> JA5ARW PM64",
                "6W/IK2HKT W9XYZ PM64",
                "6W/IK2HKT W9XYZ PM64",
            ),
            ("CQ W9XYZ/R EN37", "CQ W9XYZ/R EN37", "CQ W9XYZ/R EN37"),
            (
                "W9XYZ K1ABC R FN42",
                "W9XYZ K1ABC R FN42",
                "W9XYZ K1ABC R FN42",
            ),
        ] {
            let row = local_fixture_row(wire, decoded);
            assert_eq!(clean_message(&row.text[24..]), expected, "{wire}");
            assert_eq!(row.tokens, norm_tokens(decoded));
            assert!(row.text.ends_with(" exp"));
        }
    }

    #[test]
    fn separators_mark_first_and_subsequent_nonempty_slots() {
        let row = |gen| Row {
            text: String::new(),
            gen,
            slot_band: None,
            freq: 1000.0,
            dt: 0.0,
            tokens: Vec::new(),
            filterable: None,
            source: DecodeSource::Local,
        };
        let rows = [row(10), row(10), row(13), row(13), row(15)];
        let items = display_items(&rows, DisplayFilters::NONE);
        assert_eq!(items.len(), 8);
        assert!(matches!(items[0], DisplayItem::Separator(10, None)));
        assert!(matches!(items[1], DisplayItem::Decode(0)));
        assert!(matches!(items[2], DisplayItem::Decode(1)));
        assert!(matches!(items[3], DisplayItem::Separator(13, None)));
        assert!(matches!(items[6], DisplayItem::Separator(15, None)));
        assert!(matches!(
            display_items(&rows[1..], DisplayFilters::NONE)[0],
            DisplayItem::Separator(10, None)
        ));
        assert!(display_items(&[], DisplayFilters::NONE).is_empty());
        // Unix epoch plus one skipped slot: no divider for the missing slot.
        assert_eq!(
            separator_label(2, None),
            "------ 1970-01-01 - 00:00:30 UTC - FT8 ------"
        );
        assert_eq!(
            separator_label(2, Some("20m")),
            "------ 1970-01-01 - 00:00:30 UTC - 20m - FT8 ------"
        );
    }

    #[test]
    fn target_highlight_only_marks_complete_message_tokens() {
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
            "020045", -25, -0.4, 1256, "5R8IC ZY7H HI06", "a7"
        );
        let (job, highlights) = decode_highlight_job(
            &text,
            None,
            Some("ZY7H"),
            Some("HI06"),
            egui::FontId::monospace(14.0),
            Color32::BLACK,
        );
        let marked: Vec<_> = highlights
            .iter()
            .map(|highlight| &job.text[highlight.bytes.clone()])
            .collect();
        assert_eq!(marked, ["ZY7H", "HI06"]);
        assert_eq!(job.text, text);

        let similar = text.replace("ZY7H", "ZY7H/P").replace("HI06", "HI06AA");
        let (_, highlights) = decode_highlight_job(
            &similar,
            None,
            Some("ZY7H"),
            Some("HI06"),
            egui::FontId::monospace(14.0),
            Color32::BLACK,
        );
        assert!(highlights.is_empty());
    }

    #[test]
    fn own_callsign_is_yellow_but_own_grid_is_unmarked() {
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
            "020045", -18, -0.4, 1108, "CQ BG5ATV PM00", ""
        );
        let (job, highlights) = decode_highlight_job(
            &text,
            Some("BG5ATV"),
            None,
            None,
            egui::FontId::monospace(14.0),
            Color32::WHITE,
        );
        assert_eq!(highlights.len(), 1);
        assert_eq!(&job.text[highlights[0].bytes.clone()], "BG5ATV");
        assert_eq!(highlights[0].background, Color32::YELLOW);
        let marked = job
            .sections
            .iter()
            .find(|section| section.byte_range.start.0 == highlights[0].bytes.start)
            .unwrap();
        assert_eq!(marked.format.color, Color32::BLACK);
        assert_eq!(job.text, text);
    }

    #[test]
    fn decode_highlight_and_separator_center_rendered_glyphs() {
        let red = Color32::from_rgb(0xe0, 0x22, 0x22);
        let gray = Color32::from_gray(210);
        let separator_text = Color32::from_gray(40);
        for system_font in [false, true] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let ctx = egui::Context::default();
                if system_font {
                    crate::fonts::install(&ctx);
                }
                let mut input = egui::RawInput::default();
                input
                    .viewports
                    .get_mut(&input.viewport_id)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                let output = ctx.run_ui(input, |ui| {
                    assert_eq!(ui.pixels_per_point(), scale);
                    let size = egui::vec2(450.0, Ft8rsApp::row_height(ui.ctx()));
                    let separator = egui::Rect::from_min_size(egui::pos2(12.0, 30.25), size);
                    let font = egui::FontId::monospace(DECODE_FONT_SIZE);
                    paint_decode_separator(
                        ui.painter(),
                        separator,
                        "------ 2026-11-08 - 03:07:30 UTC - FT8 ------".into(),
                        font.clone(),
                        separator_text,
                        gray,
                    );

                    let (job, highlights) = decode_highlight_job(
                        "030730   0  -0.1  1838  UA0FF ZZ5UNO -11",
                        None,
                        Some("ZZ5UNO"),
                        None,
                        font.clone(),
                        Color32::BLACK,
                    );
                    assert_eq!(&job.text[highlights[0].bytes.clone()], "ZZ5UNO");
                    paint_decode_row(
                        ui.painter(),
                        separator.translate(egui::vec2(0.0, size.y)),
                        job,
                        &highlights,
                        Color32::BLACK,
                        cap_height_sample(ui.painter(), &font)
                            .mesh_bounds
                            .center()
                            .y,
                    );
                });
                let meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
                let bounds = |color| {
                    let mut bounds = egui::Rect::NOTHING;
                    for primitive in &meshes {
                        if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                            for vertex in
                                mesh.vertices.iter().filter(|vertex| vertex.color == color)
                            {
                                bounds.extend_with(vertex.pos);
                            }
                        }
                    }
                    assert!(bounds.is_positive(), "missing rendered color {color:?}");
                    bounds
                };
                for (text, fill, tolerance) in
                    [(Color32::WHITE, red, 1.0), (separator_text, gray, 0.5)]
                {
                    let text = bounds(text);
                    let fill = bounds(fill);
                    let offset = (text.center().y - fill.center().y).abs() * scale;
                    assert!(
                        offset <= tolerance + 1e-4,
                        "ink shifted {offset} physical pixels at {scale}x (system font: {system_font})",
                    );
                }
            }
        }
    }

    #[test]
    fn dialog_focus_restores_before_focusing_and_spaces_retries() {
        let ctx = egui::Context::default();
        let mut focus = DialogFocus::default();
        let now = Instant::now();
        focus.request();
        for (millis, expect_focus) in [
            (0, true),
            (1, false),
            (99, false),
            (100, true),
            (200, true),
            (300, false),
        ] {
            let output = ctx.run_ui(egui::RawInput::default(), |_ui| {
                focus.update(&ctx, now + Duration::from_millis(millis));
            });
            let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
            let has_focus = commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus));
            assert_eq!(has_focus, expect_focus, "at {millis} ms: {commands:?}");
            if expect_focus {
                // egui may append unrelated viewport commands of its own.
                let visible = commands
                    .iter()
                    .position(|cmd| matches!(cmd, egui::ViewportCommand::Visible(true)));
                let restored = commands
                    .iter()
                    .position(|cmd| matches!(cmd, egui::ViewportCommand::Minimized(false)));
                let focused = commands
                    .iter()
                    .position(|cmd| matches!(cmd, egui::ViewportCommand::Focus));
                assert!(
                    matches!((visible, restored, focused), (Some(v), Some(r), Some(f)) if v < r && r < f),
                    "at {millis} ms: {commands:?}"
                );
            }
        }
        // Every menu click re-arms focus, including for an existing dialog.
        focus.request();
        let output = ctx.run_ui(egui::RawInput::default(), |_ui| focus.update(&ctx, now));
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|cmd| matches!(cmd, egui::ViewportCommand::Focus)));
    }

    #[test]
    fn compare_keeps_local_provenance_in_both_arrival_orders() {
        let message = "CQ K1ABC FN42";
        assert_eq!(provenance_tag(StreamDecodeProvenance::CandidateRetry), "Pr");
        assert_eq!(
            provenance_tag(StreamDecodeProvenance::PlusCompatibility),
            "Pc"
        );
        assert_eq!(
            provenance_tag(StreamDecodeProvenance::ContextRecovery),
            "ctx"
        );
        for provenance in [
            StreamDecodeProvenance::Experiment,
            StreamDecodeProvenance::CooperativeRecovery,
            StreamDecodeProvenance::ContextRecovery,
            StreamDecodeProvenance::PlusCompatibility,
            StreamDecodeProvenance::CandidateRetry,
            StreamDecodeProvenance::ApMask,
            StreamDecodeProvenance::WsjtxAp(ft8rs::stream::session::WsjtxApType::Cq),
            StreamDecodeProvenance::A8List,
        ] {
            let local_text = format!(
                "120000 -20   0.1  1002  {message} {}",
                provenance_tag(provenance)
            );
            let external_text = format!("120000 -19   0.2  1000  {message}");
            for first in [DecodeSource::Local, DecodeSource::External] {
                let local_first = first == DecodeSource::Local;
                let mut rows = vec![Row {
                    text: if local_first {
                        local_text.clone()
                    } else {
                        external_text.clone()
                    },
                    gen: 1,
                    slot_band: None,
                    freq: if local_first { 1002.0 } else { 1000.0 },
                    dt: 0.0,
                    tokens: norm_tokens(message),
                    filterable: None,
                    source: first,
                }];
                assert!(fold_decode_row(
                    &mut rows,
                    &Row {
                        gen: 1,
                        slot_band: None,
                        freq: if local_first { 1000.0 } else { 1002.0 },
                        dt: 0.0,
                        tokens: norm_tokens(message),
                        filterable: None,
                        source: if local_first {
                            DecodeSource::External
                        } else {
                            DecodeSource::Local
                        },
                        text: (if local_first {
                            &external_text
                        } else {
                            &local_text
                        })
                        .to_string()
                    },
                    7.0
                )
                .is_some());
                assert_eq!(rows[0].source, DecodeSource::Both);
                assert_eq!(rows[0].text, local_text);
                assert_eq!(rows[0].freq, 1002.0);
            }
        }
    }

    #[test]
    fn compare_retains_resolved_hash_calls_in_both_arrival_orders() {
        for (local_message, external_message, expected_message) in [
            ("<...> W1AW RR73", "<PJ4X> W1AW RR73", "<PJ4X> W1AW RR73"),
            ("PJ4X <...> -16", "PJ4X <W1AW> -16", "PJ4X <W1AW> -16"),
            ("<PJ4X> W1AW RR73", "<...> W1AW RR73", "<PJ4X> W1AW RR73"),
            (
                "<...> <W1AW> RR73",
                "<PJ4X> <...> RR73",
                "<PJ4X> <W1AW> RR73",
            ),
            (
                "<...> <...> RR73",
                "<PJ4X> <W1AW> RR73",
                "<PJ4X> <W1AW> RR73",
            ),
            ("<...> <...> RR73", "<...> <W1AW> RR73", "<...> <W1AW> RR73"),
        ] {
            let local = || Row {
                text: format!("120000 -20   0.1  1002  {local_message:<20} exp"),
                gen: 1,
                slot_band: None,
                freq: 1002.0,
                dt: 0.1,
                tokens: norm_tokens(local_message),
                filterable: None,
                source: DecodeSource::Local,
            };
            let external = || Row {
                text: external_decode_text(&crate::wsjtx_udp::ExternalDecode {
                    ms_since_midnight: 43_200_000,
                    snr: -19,
                    dt: 0.2,
                    freq: 1000,
                    message: external_message.into(),
                }),
                gen: 1,
                slot_band: None,
                freq: 1000.0,
                dt: 0.2,
                tokens: norm_tokens(external_message),
                filterable: None,
                source: DecodeSource::External,
            };
            for local_first in [true, false] {
                let mut rows = Vec::new();
                let mut display = Vec::new();
                let observations = if local_first {
                    [local(), external()]
                } else {
                    [external(), local()]
                };
                for observation in observations {
                    insert_decode_row(&mut rows, &mut display, observation, 7.0);
                }
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].source, DecodeSource::Both);
                assert_eq!(rows[0].tokens, norm_tokens(expected_message));
                assert!(rows[0].text.starts_with("120000 -20   0.1  1002  "));
                assert_eq!(clean_message(&rows[0].text[24..]), expected_message);
                assert!(rows[0].text.ends_with(" exp"));
                assert_eq!(rows[0].freq, 1002.0);
                assert_eq!(rows[0].dt, 0.1);

                // Neither source's later unresolved observation may erase a call.
                let text = rows[0].text.clone();
                for source in [DecodeSource::Local, DecodeSource::External] {
                    let report = rows[0].tokens.last().unwrap();
                    let unresolved = Row {
                        tokens: norm_tokens(&format!("<...> <...> {report}")),
                        filterable: None,
                        source,
                        ..local()
                    };
                    assert!(fold_decode_row(&mut rows, &unresolved, 7.0).is_some());
                    assert_eq!(rows[0].text, text);
                    assert_eq!(rows[0].tokens, norm_tokens(expected_message));
                }
                // A resolved call must also narrow subsequent merge matching.
                let mut conflicting = external();
                conflicting.tokens = rows[0].tokens.clone();
                let index = conflicting.tokens[..2]
                    .iter()
                    .position(|token| !is_hash_token(token))
                    .unwrap();
                conflicting.tokens[index] = "W9XYZ".into();
                assert!(fold_decode_row(&mut rows, &conflicting, 7.0).is_none());
                assert_eq!(rows[0].text, text);
            }
        }
    }

    #[test]
    fn compare_ignores_udp_annotations_and_preserves_all_local_annotations() {
        let local = |message: &str, provenance| {
            let record = ft8rs_engine::protocol::DecodeRecord {
                timestamp: ft8rs::SlotTimestamp::parse("120000").unwrap(),
                row: ft8rs::stream::StreamDecodedMessage {
                    freq: 1002.0,
                    dt: 0.1,
                    snr: -20.0,
                    msg: message.into(),
                    sync: 0.0,
                    itone: [0; 79],
                },
                provenance,
                stage: ft8rs_engine::protocol::DecodeStage::Final,
            };
            Row {
                text: local_decode_text(&record, None),
                gen: 1,
                slot_band: None,
                freq: record.row.freq,
                dt: record.row.dt,
                tokens: norm_tokens(message),
                filterable: None,
                source: DecodeSource::Local,
            }
        };
        for local_first in [true, false] {
            let native = local(
                "<...> W1AW RR73 ? a7 exp",
                StreamDecodeProvenance::Experiment,
            );
            let decode = crate::wsjtx_udp::ExternalDecode {
                ms_since_midnight: 43_200_000,
                snr: -19,
                dt: 0.2,
                freq: 1000,
                message: "<PJ4X> W1AW RR73 ?a31 dp im".into(),
            };
            let external = Row {
                text: external_decode_text(&decode),
                gen: 1,
                slot_band: None,
                freq: decode.freq as f64,
                dt: decode.dt,
                tokens: norm_tokens(&decode.message),
                filterable: None,
                source: DecodeSource::External,
            };
            let (first, second) = if local_first {
                (native, external)
            } else {
                (external, native)
            };
            let mut rows = vec![first];
            assert!(fold_decode_row(&mut rows, &second, 7.0).is_some());
            assert_eq!(rows[0].source, DecodeSource::Both);
            assert!(rows[0].text.contains("<PJ4X> W1AW RR73"));
            let annotations = |row: &Row| {
                row.text
                    .split_whitespace()
                    .filter(|token| is_annotation(token))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            };
            assert_eq!(annotations(&rows[0]), ["?", "a7", "exp"]);
            let repeated = local(
                "<...> W1AW RR73 sic",
                StreamDecodeProvenance::CooperativeRecovery,
            );
            for _ in 0..2 {
                assert!(fold_decode_row(&mut rows, &repeated, 7.0).is_some());
                assert_eq!(annotations(&rows[0]), ["?", "a7", "exp", "sic"]);
                assert_eq!(rows[0].tokens, norm_tokens("<PJ4X> W1AW RR73"));
            }
        }
    }

    #[test]
    fn unknown_provenance_does_not_prevent_compare_matching() {
        let message = "CQ BA7SAY OL53";
        for (first, second) in [
            (DecodeSource::Local, DecodeSource::External),
            (DecodeSource::External, DecodeSource::Local),
        ] {
            let mut rows = vec![Row {
                text: format!(
                    "{message} {}",
                    provenance_tag(StreamDecodeProvenance::Unknown)
                ),
                gen: 3362,
                slot_band: None,
                freq: 2367.0,
                dt: 0.0,
                tokens: norm_tokens(message),
                filterable: None,
                source: first,
            }];
            assert!(fold_decode_row(
                &mut rows,
                &Row {
                    gen: 3362,
                    slot_band: None,
                    freq: 2369.0,
                    dt: 0.0,
                    tokens: norm_tokens("CQ BA7SAY OL53 ? a2"),
                    filterable: None,
                    source: second,
                    text: message.to_string()
                },
                7.0
            )
            .is_some());
            assert_eq!(rows[0].source, DecodeSource::Both);
        }
    }

    #[test]
    fn low_confidence_zl2agy_matches_in_either_arrival_order() {
        let external = crate::wsjtx_udp::ExternalDecode {
            ms_since_midnight: 40_530_000,
            snr: -23,
            dt: -1.1,
            freq: 1118,
            message: format!("{:<36}? a1", "CQ ZL2AGY RF70"),
        };
        let local_text = "111530 -21  -1.1  1118  CQ ZL2AGY RF70";
        let local_tokens = norm_tokens("CQ ZL2AGY RF70");
        let external_text = external_decode_text(&external);
        let external_tokens = norm_tokens(&external.message);
        assert_eq!(external_tokens, local_tokens);
        assert_eq!(external_text.split_whitespace().last(), Some("RF70"));
        assert!(!external_text.contains('?'));
        assert!(!external_text.contains("a1"));

        for first in [DecodeSource::Local, DecodeSource::External] {
            let (text, tokens, incoming, source) = if first == DecodeSource::Local {
                (
                    local_text,
                    &local_tokens,
                    &external_tokens,
                    DecodeSource::External,
                )
            } else {
                (
                    external_text.as_str(),
                    &external_tokens,
                    &local_tokens,
                    DecodeSource::Local,
                )
            };
            let mut rows = vec![Row {
                text: text.to_string(),
                gen: u64::from(external.ms_since_midnight / 15_000),
                slot_band: None,
                freq: 1118.0,
                dt: 0.0,
                tokens: tokens.clone(),
                filterable: None,
                source: first,
            }];
            assert!(fold_decode_row(
                &mut rows,
                &Row {
                    gen: u64::from(external.ms_since_midnight / 15_000),
                    slot_band: None,
                    freq: 1118.0,
                    dt: 0.0,
                    tokens: incoming.to_vec(),
                    filterable: None,
                    source,
                    text: local_text.to_string()
                },
                7.0
            )
            .is_some());
            assert_eq!(rows[0].source, DecodeSource::Both);
            assert_eq!(rows[0].text, local_text);
        }

        // Removing annotations must give the same display as a plain payload.
        let without_text_marker = crate::wsjtx_udp::ExternalDecode {
            message: "CQ ZL2AGY RF70".to_string(),
            ..external
        };
        assert_eq!(external_decode_text(&without_text_marker), external_text);
    }

    #[test]
    fn annotation_detection() {
        for t in [
            "a1", "a7", "a9", "a72", "a31", "?", "?a1", "a1?", "sic", "ctx", "exp", "ap", "dp",
            "im", "p", "Pc", "Pr", "retry",
        ] {
            assert!(is_annotation(t), "{t} should be an annotation");
        }
        for t in [
            "CQ", "A7", "FN42", "K1ABC", "RR73", "73", "R-12", "<...>", "<K1ABC>",
        ] {
            assert!(!is_annotation(t), "{t} should not be an annotation");
        }
    }

    #[test]
    fn clean_message_drops_annotations() {
        assert_eq!(clean_message("CQ K1ABC FN42 a7"), "CQ K1ABC FN42");
        assert_eq!(clean_message("K1ABC W1AW R-12 ? a31"), "K1ABC W1AW R-12");
        assert_eq!(clean_message("CQ BG5ATV PM00"), "CQ BG5ATV PM00");
        let body = "CQ ZL2AGY RF70";
        for annotations in ["? a1", "?a1", "a1?", "a7 a8 ap dp im sic ctx exp"] {
            let annotated = format!("{body} {annotations}");
            assert_eq!(clean_message(&annotated), body);
            assert_eq!(norm_tokens(&annotated), norm_tokens(body));
            assert!(tokens_compatible(
                &norm_tokens(&annotated),
                &norm_tokens("CQ ZL2AGY RF70 ? a2"),
            ));
        }
        // Keep uppercase payload words, including callsigns and free text.
        assert_eq!(clean_message("A7 TEST EXP AP"), "A7 TEST EXP AP");
    }

    #[test]
    fn external_with_annotations_matches_local_body() {
        let local = norm_tokens("CQ K1ABC FN42");
        let external = norm_tokens("CQ K1ABC FN42 a7"); // WSJT-X AP marker
        assert!(tokens_compatible(&local, &external));
    }

    #[test]
    fn hashed_calls_match_leniently() {
        let resolved = norm_tokens("PJ4X K1ABC RR73");
        let hashed = norm_tokens("<...> K1ABC RR73 a2");
        assert!(tokens_compatible(&resolved, &hashed));
    }

    #[test]
    fn different_messages_do_not_match() {
        assert!(!tokens_compatible(
            &norm_tokens("CQ K1ABC FN42"),
            &norm_tokens("CQ W1AW EM73"),
        ));
        assert!(!tokens_compatible(
            &norm_tokens("<PJ4X> K1ABC RR73"),
            &norm_tokens("<W1AW> K1ABC RR73"),
        ));
        assert!(tokens_compatible(
            &norm_tokens("<PJ4X> K1ABC RR73"),
            &norm_tokens("PJ4X K1ABC RR73"),
        ));
    }

    #[test]
    fn repeated_rows_fold_and_distinct_dt_signals_are_preserved() {
        let mk = |source, freq, msg: &str| Row {
            text: String::new(),
            gen: 1,
            slot_band: None,
            freq,
            dt: 0.0,
            tokens: norm_tokens(msg),
            filterable: None,
            source,
        };
        let mut rows = vec![mk(DecodeSource::Local, 1000.0, "CQ K1ABC FN42")];

        // An identical local observation is a duplicate, regardless of SNR.
        assert!(fold_decode_row(
            &mut rows,
            &Row {
                gen: 1,
                slot_band: None,
                freq: 1002.0,
                dt: 0.0,
                tokens: norm_tokens("CQ K1ABC FN42"),
                filterable: None,
                source: DecodeSource::Local,
                text: String::new()
            },
            7.0
        )
        .is_some());
        assert_eq!(rows[0].source, DecodeSource::Local);

        // An EXTERNAL decode of the same signal folds into the local row → Both.
        assert!(fold_decode_row(
            &mut rows,
            &Row {
                gen: 1,
                slot_band: None,
                freq: 1001.0,
                dt: 0.0,
                tokens: norm_tokens("CQ K1ABC FN42"),
                filterable: None,
                source: DecodeSource::External,
                text: String::new()
            },
            7.0
        )
        .is_some());
        assert_eq!(rows[0].source, DecodeSource::Both);
        // A Both row continues to absorb repeat observations from either source.
        for source in [DecodeSource::Local, DecodeSource::External] {
            assert!(fold_decode_row(
                &mut rows,
                &Row {
                    gen: 1,
                    slot_band: None,
                    freq: 1001.0,
                    dt: 0.0,
                    tokens: norm_tokens("CQ K1ABC FN42"),
                    filterable: None,
                    source,
                    text: String::new()
                },
                7.0
            )
            .is_some());
        }

        // A different gen (different slot) never folds.
        assert!(fold_decode_row(
            &mut rows,
            &Row {
                gen: 2,
                slot_band: None,
                freq: 1000.0,
                dt: 0.0,
                tokens: norm_tokens("CQ K1ABC FN42"),
                filterable: None,
                source: DecodeSource::External,
                text: String::new()
            },
            7.0
        )
        .is_none());
    }

    #[test]
    fn compare_frequency_tolerance_uses_the_supplied_value_inclusively() {
        for first in [DecodeSource::Local, DecodeSource::External] {
            let incoming = if first == DecodeSource::Local {
                DecodeSource::External
            } else {
                DecodeSource::Local
            };
            for (tolerance, offset, expected) in [
                (0.0, 0.0, true),
                (0.0, 0.25, false),
                (0.25, 0.25, true),
                (0.25, -0.25, true),
                (0.25, 0.5, false),
                (2.0, 2.0, true),
                (2.0, -2.0, true),
                (2.0, 2.25, false),
                (5.0, 5.0, true),
                (5.0, 5.25, false),
                (10.0, 10.0, true),
                (10.0, 10.25, false),
            ] {
                let mut rows = vec![Row {
                    text: String::new(),
                    gen: 2702,
                    slot_band: None,
                    freq: 1118.0,
                    dt: 0.0,
                    tokens: norm_tokens("CQ ZL2AGY RF70 a7"),
                    filterable: None,
                    source: first,
                }];
                assert_eq!(
                    fold_decode_row(
                        &mut rows,
                        &Row {
                            gen: 2702,
                            slot_band: None,
                            freq: 1118.0 + offset,
                            dt: 0.0,
                            tokens: norm_tokens("CQ ZL2AGY RF70 ? a1"),
                            filterable: None,
                            source: incoming,
                            text: String::new()
                        },
                        tolerance
                    )
                    .is_some(),
                    expected,
                    "source={first:?}, tolerance={tolerance}, offset={offset}",
                );
                assert_eq!(rows[0].source == DecodeSource::Both, expected);
            }
        }
    }

    fn dx(msg: &str) -> (Option<String>, Option<String>) {
        dx_call_and_grid(&norm_tokens(msg))
    }

    #[test]
    fn double_click_extracts_cq_call_and_grid() {
        let both = (Some("F5BZB".to_string()), Some("IN94".to_string()));
        assert_eq!(dx("CQ F5BZB IN94"), both);
        // Directional CQ: skip DX / the pure-digit frequency, take the callsign.
        assert_eq!(dx("CQ DX F5BZB IN94"), both);
        assert_eq!(dx("CQ 072 F5BZB IN94"), both);
        // No grid present.
        assert_eq!(dx("CQ F5BZB"), (Some("F5BZB".to_string()), None));
    }

    #[test]
    fn double_click_extracts_sender_from_directed_message() {
        // "TO FROM ..." — the DX is the sender (second token).
        let both = (Some("F5BZB".to_string()), Some("IN94".to_string()));
        assert_eq!(dx("W1FC F5BZB IN94"), both);
        assert_eq!(dx("W1FC F5BZB -08"), (Some("F5BZB".to_string()), None));
        assert_eq!(dx("W1FC F5BZB RR73"), (Some("F5BZB".to_string()), None));
    }

    #[test]
    fn double_click_resolves_hash_braces_and_skips_unresolved() {
        assert_eq!(dx("K1ABC <F5BZB> RR73"), (Some("F5BZB".to_string()), None));
        // Unresolved hash: no callsign to load (freq still fills at the call site).
        assert_eq!(dx("K1ABC <...> RR73"), (None, None));
    }

    #[test]
    fn grid4_is_structural_but_extraction_excludes_the_rr73_ack() {
        assert!(is_grid4("IN94"));
        assert!(is_grid4("RR99"));
        // Structurally not grid4.
        for t in ["-08", "R-12", "73", "F5BZB", "IN9", "IS94"] {
            assert!(!is_grid4(t), "{t} is not a grid4");
        }
        // RR73 is structurally grid4, but the ack must never load as a grid.
        assert!(is_grid4("RR73"));
        let rr73 = ["X".to_string(), "RR73".to_string()];
        let in94 = ["X".to_string(), "IN94".to_string()];
        assert_eq!(grid_after(&rr73, 1), None);
        assert_eq!(grid_after(&in94, 1), Some("IN94".to_string()));
    }

    #[test]
    fn validates_numeric_settings_without_silent_defaults() {
        assert_eq!(parse_frequency_range("200", "3000"), Ok((200.0, 3000.0)));
        for (low, high) in [
            ("NaN", "3000"),
            ("200", "inf"),
            ("bad", "3000"),
            ("3000", "200"),
            ("200", "200"),
            ("-1", "3000"),
            ("200", "5001"),
        ] {
            assert!(parse_frequency_range(low, high).is_err());
        }
        assert_eq!(parse_port(" 2237 "), Ok(2237));
        for value in ["", "0", "65536", "-1", "abc"] {
            assert!(parse_port(value).is_err());
        }
    }

    #[test]
    fn new_target_does_not_inherit_the_previous_grid() {
        let mut call = "W1AW".to_string();
        let mut grid = "FN31".to_string();
        fill_target_context(&mut call, &mut grid, &norm_tokens("K1ABC W1AW RR73"));
        assert_eq!(grid, "FN31");
        fill_target_context(&mut call, &mut grid, &norm_tokens("K1ABC F5BZB RR73"));
        assert_eq!(call, "F5BZB");
        assert_eq!(grid, "");
        fill_target_context(&mut call, &mut grid, &norm_tokens("CQ F5BZB IN94"));
        assert_eq!(grid, "IN94");
        fill_target_context(&mut call, &mut grid, &norm_tokens("K1ABC <...> FN42"));
        assert_eq!((call.as_str(), grid.as_str()), ("F5BZB", "IN94"));
    }

    #[test]
    fn slot_identity_survives_source_delay_midnight_and_next_day() {
        let day = 20_000 * 86_400;
        let previous = absolute_slot(5759, day - 1);
        assert_eq!(previous, absolute_slot(5759, day + 2));
        assert_eq!(absolute_slot(0, day - 1), absolute_slot(0, day + 2));
        assert_ne!(absolute_slot(0, day), absolute_slot(0, day + 86_400));
        assert_eq!(
            absolute_slot(120, day + 1801),
            absolute_slot(120, day + 1817)
        );
    }

    #[test]
    fn settings_text_fields_center_visible_text() {
        use egui::emath::GuiRounding as _;

        for (system_font, ink_offset) in [(false, 0.0), (true, 0.0), (false, -2.0)] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let ctx = egui::Context::default();
                if system_font {
                    install_fonts(&ctx);
                } else if ink_offset != 0.0 {
                    // Reproduce high glyphs even on machines whose installed
                    // fonts already have well-balanced vertical metrics.
                    let mut fonts = egui::FontDefinitions::default();
                    let primary = &fonts.families[&egui::FontFamily::Monospace][0];
                    let font = std::sync::Arc::make_mut(fonts.font_data.get_mut(primary).unwrap());
                    font.tweak.y_offset += ink_offset;
                    ctx.set_fonts(fonts);
                }
                apply_style(&ctx, egui::Theme::Light);
                let mut call = "BG5ATV".to_owned();
                let mut grid = "PM00".to_owned();
                let mut host = "127.0.0.1".to_owned();
                let mut port = "2238".to_owned();
                let mut input = egui::RawInput::default();
                input
                    .viewports
                    .get_mut(&input.viewport_id)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                for min_height in [24.0, 30.0] {
                    let output = ctx.run_ui(input.clone(), |ui| {
                        assert_eq!(ui.pixels_per_point(), scale);
                        ui.set_max_width(440.0);
                        ui.spacing_mut().interact_size.y = min_height;
                        settings_form(ui, |ui| {
                            setting_row(ui, "Callsign", |ui| {
                                callsign_field(ui, &mut call, f32::INFINITY).lost_focus()
                            });
                            setting_row(ui, "Grid", |ui| {
                                grid_field(ui, &mut grid, f32::INFINITY).lost_focus()
                            });
                            setting_row(ui, "Host", |ui| text_field(ui, &mut host));
                            setting_row(ui, "Port", |ui| text_field(ui, &mut port));
                        });
                    });
                    for value in [&call, &grid, &host, &port] {
                        let text = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Text(text) if &text.galley.job.text == value => {
                                    Some(text)
                                }
                                _ => None,
                            })
                            .unwrap();
                        let frame = output
                            .shapes
                            .iter()
                            .find_map(|shape| match &shape.shape {
                                egui::Shape::Rect(rect)
                                    if rect.fill == Color32::WHITE
                                        && rect.rect.contains(text.pos) =>
                                {
                                    Some(rect.rect.round_to_pixels(scale))
                                }
                                _ => None,
                            })
                            .unwrap();
                        let ink_y =
                            text.pos.y.round_to_pixels(scale) + text.galley.mesh_bounds.center().y;
                        let error_px = (ink_y - frame.center().y).abs() * scale;
                        assert!(
                            error_px <= 1.01,
                            "{value} at {scale}x and {min_height} pt (system font: {system_font}, ink offset: {ink_offset}) is off center by {error_px} px"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn centered_callsign_field_replaces_the_native_selection() {
        let ctx = egui::Context::default();
        let mut call = "BG5ATV".to_owned();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let response = callsign_field(ui, &mut call, 110.0);
            response.request_focus();
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), response.id).unwrap();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(3),
                    egui::text::CCursor::new(6),
                )));
            state.store(ui.ctx(), response.id);
        });
        let input = egui::RawInput {
            events: vec![egui::Event::Text("xyz".to_owned())],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let response = callsign_field(ui, &mut call, 110.0);
            assert!(response.has_focus());
            assert!(response.changed());
            let state = egui::text_edit::TextEditState::load(ui.ctx(), response.id).unwrap();
            let range = state.cursor.char_range().unwrap();
            assert!(range.is_empty());
            assert_eq!(range.primary, egui::text::CCursor::new(6));
        });
        assert_eq!(call, "BG5XYZ");
    }

    #[test]
    fn standard_form_reflows_after_window_width_changes() {
        for locale in [Locale::English, Locale::Chinese] {
            let ctx = egui::Context::default();
            install_fonts(&ctx);
            apply_style(&ctx, egui::Theme::Light);
            let mut value = String::new();
            let mut widths = Vec::new();
            for width in [360.0, 240.0, 640.0, 280.0] {
                let mut fields = [egui::Rect::NOTHING; 2];
                for _ in 0..4 {
                    let _ = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 400.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                            settings_form(ui, |ui| {
                                setting_row(ui, locale.text("Callsign"), |ui| {
                                    fields[0] = callsign_field(ui, &mut value, f32::INFINITY).rect;
                                    false
                                });
                                setting_row(ui, locale.text("Input device"), |ui| {
                                    fields[1] = egui::ComboBox::from_id_salt("test_device")
                                        .width(ui.available_width())
                                        .truncate()
                                        .selected_text(
                                            "DAX RX 1 (a deliberately long endpoint name)",
                                        )
                                        .show_ui(ui, |_| {})
                                        .response
                                        .rect;
                                    false
                                });
                            });
                        },
                    );
                }
                for field in fields {
                    assert!(
                        field.right() <= width + 1.0,
                        "control overflow at {width}: {field:?}"
                    );
                    assert!(field.width() > 30.0);
                }
                assert!((fields[1].left() - fields[0].left()).abs() < 1.0);
                assert!(
                    fields[1].top() - fields[0].top() < 50.0,
                    "labels must stay on one line"
                );
                widths.push(fields[0].width());
            }
            assert!(widths[1] < widths[0] && widths[2] > widths[0] && widths[3] < widths[2]);
        }
    }

    #[test]
    fn long_device_names_keep_both_field_and_popup_inside_window() {
        let ctx = egui::Context::default();
        apply_style(&ctx, egui::Theme::Light);
        let devices = [
            "DAX RX 1 (FlexRadio DAX) ".repeat(12),
            "A".repeat(240),
            "很长的设备名称".repeat(30),
        ]
        .map(|name| SoundcardDeviceInfo {
            id: name.clone(),
            index: 0,
            host: "Test".to_string(),
            name,
            is_default_input: false,
            input: SoundcardFormatInfo {
                channels: 2,
                sample_rate: 48_000,
                sample_format: "f32".to_string(),
            },
        });
        let mut selected = Some(devices[0].name.clone());
        for width in [240.0, 640.0, 280.0] {
            let mut response = None;
            for frame in 0..8 {
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        settings_form(ui, |ui| {
                            setting_row(ui, "Input device", |ui| {
                                response = Some(input_device_combo(
                                    ui,
                                    &mut selected,
                                    &devices,
                                    Locale::English,
                                ));
                                false
                            });
                        });
                    },
                );
                let response = response.as_ref().unwrap();
                // Use the standard ComboBox popup ID to open the actual menu.
                let popup_id = response.id.with("popup");
                if frame >= 3 {
                    egui::Popup::open_id(&ctx, popup_id);
                }
                if frame == 7 {
                    assert!(response.rect.right() <= width + 1.0);
                    let menu = ctx
                        .memory(|memory| memory.area_rect(popup_id))
                        .expect("open menu");
                    assert!(
                        menu.width() <= width + 1.0,
                        "menu overflow at {width}: {menu:?}"
                    );
                    assert!(menu.left() >= -1.0 && menu.right() <= width + 1.0);
                    assert!(menu.height() <= 400.0, "long items must scroll vertically");
                    for shape in &output.shapes {
                        assert_single_line_device_text(&shape.shape, &devices);
                    }
                }
            }
        }
        assert_eq!(selected.as_deref(), Some(devices[0].name.as_str()));
    }

    fn assert_single_line_device_text(shape: &egui::Shape, devices: &[SoundcardDeviceInfo]) {
        match shape {
            egui::Shape::Text(text)
                if devices
                    .iter()
                    .any(|device| device.name == text.galley.job.text) =>
            {
                assert_eq!(text.galley.rows.len(), 1, "device names must never wrap");
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    assert_single_line_device_text(shape, devices);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn device_popup_can_be_wider_than_trigger_and_show_complete_name() {
        let ctx = egui::Context::default();
        apply_style(&ctx, egui::Theme::Light);
        let devices = [SoundcardDeviceInfo {
            id: "Test:input".into(),
            index: 0,
            host: "Test".to_owned(),
            name: "DAX Audio RX 1 (FlexRadio Systems DAX Audio)".to_owned(),
            is_default_input: false,
            input: SoundcardFormatInfo {
                channels: 2,
                sample_rate: 48_000,
                sample_format: "f32".to_owned(),
            },
        }];
        let mut selected = None;
        for frame in 0..8 {
            let mut response = None;
            let output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.allocate_ui(egui::vec2(140.0, 30.0), |ui| {
                        response = Some(input_device_combo(
                            ui,
                            &mut selected,
                            &devices,
                            Locale::English,
                        ));
                    });
                },
            );
            let response = response.unwrap();
            let popup_id = response.id.with("popup");
            if frame >= 3 {
                egui::Popup::open_id(&ctx, popup_id);
            }
            if frame == 7 {
                let menu = ctx.memory(|memory| memory.area_rect(popup_id)).unwrap();
                assert!(menu.width() > response.rect.width() + 50.0);
                let device_text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.job.text == devices[0].name => {
                            Some(text)
                        }
                        _ => None,
                    })
                    .expect("the open menu must render the device name");
                assert_eq!(device_text.galley.rows.len(), 1);
                assert!(
                    !device_text.galley.elided,
                    "the name fits the viewport and should be complete"
                );
            }
        }
    }

    #[test]
    fn switching_settings_pages_keeps_the_first_presented_frame_stable() {
        let ctx = egui::Context::default();
        apply_style(&ctx, egui::Theme::Light);
        let render = |tab: SettingsTab| {
            let mut control = egui::Rect::NOTHING;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(360.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    settings_page(ui, tab, |ui| {
                        let label = match tab {
                            SettingsTab::Audio => "Input device",
                            SettingsTab::Compare => "Listen to UDP",
                            SettingsTab::Frequency => "Low (Hz)",
                            _ => "Callsign",
                        };
                        settings_form(ui, |ui| {
                            setting_row(ui, label, |ui| {
                                control =
                                    callsign_field(ui, &mut String::new(), f32::INFINITY).rect;
                                false
                            });
                        });
                    });
                },
            );
            control
        };
        for tab in [
            SettingsTab::Audio,
            SettingsTab::Compare,
            SettingsTab::Frequency,
            SettingsTab::General,
            SettingsTab::Compare,
            SettingsTab::Audio,
            SettingsTab::General,
            SettingsTab::Frequency,
        ] {
            let first = render(tab);
            let second = render(tab);
            assert_eq!(
                first, second,
                "page {tab:?} shifts after its first displayed frame"
            );
        }
    }

    #[test]
    fn svg_monitor_icons_have_centered_visual_weight_at_multiple_resolutions() {
        for bytes in [
            include_bytes!("../assets/play.svg").as_slice(),
            include_bytes!("../assets/stop.svg").as_slice(),
        ] {
            for pixels in [12, 15, 18, 24, 36, 48] {
                let image = egui_extras::image::load_svg_bytes_with_size(
                    bytes,
                    egui::SizeHint::Width(pixels),
                    &Default::default(),
                )
                .unwrap();
                let mut mass = 0.0;
                let mut center = egui::Vec2::ZERO;
                for (i, color) in image.pixels.iter().enumerate() {
                    let alpha = f32::from(color.a());
                    mass += alpha;
                    center += egui::vec2(
                        (i % image.size[0]) as f32 + 0.5,
                        (i / image.size[0]) as f32 + 0.5,
                    ) * alpha;
                }
                assert!(mass > 0.0);
                center /= mass;
                let expected = egui::vec2(image.size[0] as f32, image.size[1] as f32) / 2.0;
                assert!(
                    (center - expected).length() < 0.5,
                    "icon drifts from its center at {pixels}px: {center:?}"
                );
            }
        }
    }

    #[test]
    fn svg_monitor_button_stays_square_and_centered_across_fonts_and_dpi() {
        fn image_bounds(shape: &egui::Shape) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect),
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.calc_bounds())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(image_bounds),
                _ => None,
            }
        }
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            ctx.set_pixels_per_point(scale);
            for font_size in [9.0, 11.0, 18.0] {
                for icon in [
                    egui::include_image!("../assets/play.svg"),
                    egui::include_image!("../assets/stop.svg"),
                ] {
                    let mut button_rect = egui::Rect::NOTHING;
                    let output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(400.0, 200.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            ui.style_mut().override_font_id =
                                Some(egui::FontId::monospace(font_size));
                            ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                            ui.spacing_mut().interact_size.y = 0.0;
                            let field = callsign_field(ui, &mut String::new(), 80.0);
                            button_rect = monitor_button(
                                ui,
                                icon.clone(),
                                Color32::BLUE,
                                field.rect.height(),
                                true,
                            )
                            .rect;
                        },
                    );
                    assert!((button_rect.width() - button_rect.height()).abs() < 0.01);
                    let image_rect = output
                        .shapes
                        .iter()
                        .find_map(|shape| image_bounds(&shape.shape))
                        .expect("SVG must load and render as an image");
                    assert!((image_rect.center() - button_rect.center()).length() < 0.6 / scale,
                        "icon is not centered at {scale}x / {font_size}pt: {image_rect:?} vs {button_rect:?}");
                }
            }
        }
    }
}
