//! ft8rs GUI (P3) — egui front-end over the live engine.
//!
//! Monitor-only (file decode stays in the CLI). The four hot fields
//! (mycall/hiscall/hisgrid/nfqso) + Monitor button sit at the bottom; everything
//! else lives in the Settings dialog (tabs). Edits apply live while monitoring
//! via `EngineCommand::ApplyState` (the engine plans L0/L1/L2 and reconfigures at
//! the next slot boundary). The decode table mirrors the CLI columns; slot
//! separators show only the time.

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, RichText};

use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance};
use ft8rs_engine::protocol::{
    DxContextSnapshot, EngineCommand, EngineEvent, EngineStatus, HisgridSource,
};
use ft8rs_engine::reconfig::{plan_reconfig, EngineState};
use ft8rs_engine::report::UdpConfig;
use ft8rs_engine::soundcard::{InputChannel, SoundcardDeviceInfo, SoundcardFormatInfo};
use ft8rs_engine::EngineHandle;

/// Hard cap on retained decode rows. Rendering is virtualized (see `decode_rows`)
/// so this only bounds memory and the per-decode dedupe scan, not scroll cost;
/// the oldest rows are dropped once the list grows past it (see `ingest`).
const MAX_ROWS: usize = 20_000;
const DIALOG_FOCUS_ATTEMPTS: u8 = 3;
const DIALOG_FOCUS_INTERVAL: Duration = Duration::from_millis(100);
/// Slightly larger than WSJT-X's default 10 pt (~13.33 logical pixels at
/// Windows' 96-DPI baseline), for easier reading in the receive log.
const DECODE_FONT_SIZE: f32 = 14.0;
/// Breathing room above and below each decode line, in logical pixels.
const DECODE_ROW_PADDING_Y: f32 = 1.0;
/// Internal left padding for the decode table (header + rows go edge-to-edge).
const TABLE_PAD: f32 = 12.0;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum SettingsTab {
    Audio,
    Decode,
    Frequency,
    Station,
    Output,
    Compare,
    Advanced,
}

/// Where a decode row came from, for the compare-mode coloring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DecodeSource {
    Local,
    External,
    Both,
}

struct Row {
    text: String,
    // Merge keys (compare mode): same real slot + freq within tolerance +
    // compatible message tokens identify the same transmission across the two
    // decoders. `gen` is the absolute UTC slot number, so adjacent slots can
    // arrive out of order and rows from different days cannot collide.
    gen: u64,
    freq: f64,
    tokens: Vec<String>,
    source: DecodeSource,
}

#[derive(Clone, Copy)]
enum DisplayItem {
    Decode(usize),
    Separator(u64),
}

pub struct Ft8rsApp {
    engine: EngineHandle,

    // Hot fields (bottom bar).
    mycall: String,
    hiscall: String,
    hisgrid: String,
    nfqso: String,

    // Settings.
    profile: DecodeProfile,
    swl: bool,
    hybrid_sic: bool,
    hybrid_experiment: bool,
    nagain: bool,
    nfa: String,
    nfb: String,
    mygrid: String,
    udp_on: bool,
    udp_host: String,
    udp_port: String,
    filter: bool,
    hide_dupes: bool,
    hide_hash: bool,

    // Compare (GUI-only): listen to WSJT-X UDP decodes and merge/dedupe them
    // with the local decoder's, coloring by source.
    udp_in_on: bool,
    udp_in_host: String,
    udp_in_port: String,
    udp_in_tol: f64,

    devices: Vec<SoundcardDeviceInfo>,
    selected_device: Option<String>, // device name; None = default
    input_channel: InputChannel,     // which channel of a multi-channel device to decode
    input_peak: Option<f32>,         // recent capture peak; None until audio arrives
    capture_format: Option<(String, SoundcardFormatInfo)>,
    channel_peaks: Vec<f32>,
    capture_xruns: u64,
    // Last slot's capture diagnostics (samples, blocks, avg_block, jitter_ms) and
    // decode count — an at-a-glance readout for locating capture/alignment issues.
    capture_diag: Option<(usize, u32, u32, i64, u32)>,
    last_slot_decodes: Option<usize>,

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

    // Native Profile menu checkmarks (macOS); empty elsewhere.
    profile_menu: Vec<(DecodeProfile, muda::CheckMenuItem)>,
    menu_profile_synced: DecodeProfile,
    // Last theme the style was built for; re-applied when the OS theme flips.
    styled_theme: egui::Theme,
    // Last window title pushed to the OS (only re-sent when it changes).
    last_title: String,
    // The app logo as an egui texture (None if it failed to decode).
    logo: Option<egui::TextureHandle>,
    // Running WSJT-X UDP listener (compare mode); None when disabled/stopped.
    udp_in: Option<crate::wsjtx_udp::UdpIn>,
    udp_in_dirty: bool,
}

impl Ft8rsApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        install_fonts(&cc.egui_ctx);
        let theme = cc.egui_ctx.theme();
        apply_style(&cc.egui_ctx, theme);
        let engine = EngineHandle::spawn();
        let _ = engine.send(EngineCommand::RefreshDevices);

        // Restore persisted settings. Devices are keyed by name; a missing
        // saved device is reported on Start, so we never listen to the wrong rig.
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
        let device = load("device", "");
        let profile =
            DecodeProfile::parse(&load("profile", "wsjtx")).unwrap_or(DecodeProfile::Wsjtx);
        // Install the menu while eframe still keeps the root hidden. Preserve
        // its physical client size: SetMenu consumes client height, whereas
        // winit knows how to resize the outer window for a menu-bearing window.
        let menu_events = native_menu_events(&cc.egui_ctx);
        let hwnd = window_hwnd(cc);
        #[cfg(target_os = "windows")]
        let client_size = cc.winit_window().map(|window| window.inner_size());
        let profile_menu = install_menu(profile, hwnd);
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
            engine,
            mycall: load("mycall", ""),
            hiscall: load("hiscall", ""),
            hisgrid: load("hisgrid", ""),
            nfqso: load("nfqso", "0"),
            profile,
            swl: load_bool("swl", false),
            hybrid_sic: load_bool("hybrid_sic", true),
            hybrid_experiment: load_bool("hybrid_experiment", false),
            nagain: load_bool("nagain", false),
            nfa: load("nfa", "200"),
            nfb: load("nfb", "3000"),
            mygrid: load("mygrid", ""),
            udp_on: load_bool("udp_on", false),
            udp_host: load("udp_host", "127.0.0.1"),
            udp_port: load("udp_port", "2238"),
            filter: load_bool("filter", false),
            hide_dupes: load_bool("hide_dupes", false),
            hide_hash: load_bool("hide_hash", false),
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
            capture_format: None,
            channel_peaks: Vec::new(),
            capture_xruns: 0,
            capture_diag: None,
            last_slot_decodes: None,
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
            tab: SettingsTab::Station,
            menu_events,
            profile_menu,
            menu_profile_synced: profile,
            styled_theme: theme,
            last_title: String::new(),
            logo,
            udp_in: None,
            udp_in_dirty: true,
        }
    }

    /// Window title: `FT8.RS - {MyCallsign} - {Profile}` (callsign omitted when
    /// unset). Pushed to the OS only when it changes.
    fn sync_title(&mut self, ctx: &egui::Context) {
        let profile = self.profile.as_str().to_ascii_uppercase();
        let title = match norm(&self.mycall) {
            Some(call) => format!("FT8.RS - {call} - {profile}"),
            None => format!("FT8.RS - {profile}"),
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
            hybrid_sic: self.hybrid_sic,
            hybrid_experiment: self.hybrid_experiment,
            nagain: self.nagain,
            filter: self.filter,
            hide_dupes: self.hide_dupes,
            hide_hash: self.hide_hash,
            ..StreamDecodeConfig::default()
        };
        (config.nfa, config.nfb) = parse_frequency_range(&self.nfa, &self.nfb)?;
        // nfqso only counts inside the [nfa, nfb] band; 0 or out-of-band means
        // "no QSO focus" (passed as 0) so it never perturbs the decode.
        let nfqso = self.nfqso.trim().parse::<f64>().unwrap_or(0.0);
        config.nfqso = if nfqso >= config.nfa && nfqso <= config.nfb {
            nfqso
        } else {
            0.0
        };
        Ok(EngineState {
            device: self.selected_device.clone(),
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

    fn toggle_monitor(&mut self) {
        if self.is_monitoring() {
            let _ = self.engine.send(EngineCommand::StopMonitor);
            self.status = EngineStatus::Idle;
            self.applied = None;
            self.pending_confirm = None;
            self.input_peak = None;
            self.capture_format = None;
            self.channel_peaks.clear();
            self.capture_diag = None;
            self.capture_xruns = 0;
            return;
        }
        if self.dx_needs_hiscall() {
            self.error = Some("profile dx requires His Call".to_string());
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
        self.capture_format = None;
        self.capture_diag = None;
        self.channel_peaks.clear();
        self.last_slot_decodes = None;
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
    /// dialog instead of applied immediately (decision 4 / §6.1).
    fn apply_if_changed(&mut self) {
        if self.dx_needs_hiscall() {
            self.error = Some("profile dx requires His Call".to_string());
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

    fn pump_events(&mut self) {
        while let Some(event) = self.engine.try_recv() {
            match event {
                EngineEvent::Status(status) => {
                    if status == EngineStatus::Idle {
                        self.pending_confirm = None;
                        self.applied = None;
                        self.input_peak = None;
                        self.capture_format = None;
                        self.channel_peaks.clear();
                        self.capture_diag = None;
                        self.capture_xruns = 0;
                    }
                    self.status = status;
                }
                EngineEvent::DevicesRefreshed(devices) => self.devices = devices,
                EngineEvent::InputLevel(peak) => self.input_peak = Some(peak),
                EngineEvent::CaptureStarted {
                    device_name,
                    format,
                } => {
                    self.capture_format = Some((device_name, format));
                    self.input_peak = None;
                    self.channel_peaks.clear();
                    self.capture_diag = None;
                    self.last_slot_decodes = None;
                    self.capture_xruns = 0;
                }
                EngineEvent::CaptureLevels {
                    channel_peaks,
                    peak,
                    xruns,
                } => {
                    self.channel_peaks = channel_peaks;
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
                EngineEvent::CaptureDiag {
                    samples,
                    blocks,
                    avg_block,
                    jitter_ms,
                    sample_rate,
                } => self.capture_diag = Some((samples, blocks, avg_block, jitter_ms, sample_rate)),
                EngineEvent::Decode(record) => self.push_decode(record),
                EngineEvent::SlotComplete { count, .. } => self.last_slot_decodes = Some(count),
                EngineEvent::Reconfigured(_) => {}
                EngineEvent::DxContext(snapshot) => self.dx = Some(snapshot),
                EngineEvent::Error(err) => self.error = Some(err),
            }
        }
        // Compare mode: fold in external (WSJT-X) decodes.
        while let Some(decode) = self.udp_in.as_ref().and_then(|udp| udp.try_recv()) {
            self.merge_external(decode);
        }
    }

    fn push_decode(&mut self, record: ft8rs_engine::protocol::DecodeRecord) {
        let ts = record.timestamp;
        let slot_key = ts.milliseconds_since_midnight() / 15_000;
        let row = record.row;
        // Message in a fixed-width column so the provenance tag forms its own
        // trailing column (slot boundaries are shown by the alternating bg).
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
            ts.format_time(),
            row.snr.round() as i32,
            row.dt,
            row.freq.round() as i64,
            clean_message(&row.msg),
            provenance_tag(record.provenance),
        );
        self.ingest(
            slot_key,
            row.freq,
            text,
            norm_tokens(&row.msg),
            DecodeSource::Local,
        );
    }

    fn merge_external(&mut self, decode: crate::wsjtx_udp::ExternalDecode) {
        let slot_key = decode.ms_since_midnight / 15_000;
        let text = external_decode_text(&decode);
        self.ingest(
            slot_key,
            decode.freq as f64,
            text,
            norm_tokens(&decode.message),
            DecodeSource::External,
        );
    }

    /// Merge a decode into the row list. In compare mode a local and an external
    /// decode of the same signal (same slot + freq within tolerance + compatible
    /// message) collapse to one `Both` row. **Same-source rows are never folded:**
    /// each source is already deduplicated upstream (the engine merges hybrid's
    /// WSJT-X + JTDX output; WSJT-X UDP sends each decode once), so folding
    /// same-source would drop genuinely distinct rows — e.g. a jtdx-unique hybrid
    /// decode that differs from a WSJT-X row only in dt (the fold key has no dt
    /// term). No match appends a new row.
    ///
    /// UTC day resolution prevents a recurring transmission from merging with
    /// yesterday's retained rows, while tolerating different source latencies.
    fn ingest(
        &mut self,
        slot_key: u32,
        freq: f64,
        text: String,
        tokens: Vec<String>,
        source: DecodeSource,
    ) {
        // The two sources can deliver adjacent slots out of order. Resolve the
        // time-of-day against today's nearest UTC slot, including midnight.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let gen = absolute_slot(slot_key, now);

        if fold_into_cross_source(&mut self.rows, gen, freq, self.udp_in_tol, &tokens, source) {
            return;
        }
        if self.rows.last().is_none_or(|row| row.gen != gen) {
            self.display_items.push(DisplayItem::Separator(gen));
        }
        self.display_items
            .push(DisplayItem::Decode(self.rows.len()));
        self.rows.push(Row {
            text,
            gen,
            freq,
            tokens,
            source,
        });
        if self.rows.len() > MAX_ROWS {
            let drop = self.rows.len() - MAX_ROWS;
            self.rows.drain(..drop);
            self.display_items = display_items(&self.rows);
        }
    }

    /// Start/stop/restart the WSJT-X listener to match the current settings.
    /// Called every frame; cheap when nothing changed.
    fn sync_udp_in(&mut self) {
        if !std::mem::take(&mut self.udp_in_dirty) {
            return;
        }
        if !self.udp_in_on {
            self.udp_in = None; // drop stops the thread
            return;
        }
        let port = match parse_port(&self.udp_in_port) {
            Ok(port) => port,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        let host = self.udp_in_host.trim();
        if host.is_empty() {
            self.error = Some("UDP listen host is required".to_string());
            return;
        }
        let needs_restart = match &self.udp_in {
            Some(udp) => udp.host != host || udp.port != port,
            None => true,
        };
        if !needs_restart {
            return;
        }
        self.udp_in = None;
        match crate::wsjtx_udp::UdpIn::spawn(host, port) {
            Ok(udp) => {
                self.udp_in = Some(udp);
                self.error = None;
            }
            Err(err) => {
                // Disable to avoid a per-frame rebind loop; surface the error.
                self.udp_in_on = false;
                self.error = Some(err);
            }
        }
    }

    // ── UI ──────────────────────────────────────────────────────────────────

    fn pump_menu(&mut self) {
        while let Ok(event) = self.menu_events.try_recv() {
            match event.id.0.as_str() {
                // Re-arm the focus request on every click, not just the first: the
                // dialog may already be open but sitting behind the main window
                // (native menu clicks don't raise an existing viewport), so a
                // second click must still bring it to the front.
                "settings" => {
                    self.settings_open = true;
                    self.settings_focus.request();
                }
                "about" => {
                    self.about_open = true;
                    self.about_focus.request();
                }
                id => {
                    if let Some(name) = id.strip_prefix("profile:") {
                        if let Ok(profile) = DecodeProfile::parse(name) {
                            if self.profile != profile {
                                self.profile = profile;
                                self.apply_if_changed();
                            }
                        }
                    }
                }
            }
        }
        self.sync_profile_menu();
    }

    /// Keep the native Profile menu checkmarks in sync with the current profile
    /// (it can change from the menu, the settings combo, or a confirm revert).
    fn sync_profile_menu(&mut self) {
        if self.menu_profile_synced == self.profile {
            return;
        }
        for (profile, item) in &self.profile_menu {
            item.set_checked(*profile == self.profile);
        }
        self.menu_profile_synced = self.profile;
    }

    // In-window menu fallback for platforms without a native menu (Linux). macOS
    // and Windows attach a native menu bar instead.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Settings").clicked() {
                self.settings_open = true;
                self.settings_focus.request();
            }
            if ui.button("About").clicked() {
                self.about_open = true;
                self.about_focus.request();
            }
        });
    }

    fn dx_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.label(RichText::new("DX Intel").size(15.0).strong());
        ui.separator();
        ui.add_space(4.0);
        let Some(dx) = &self.dx else {
            ui.label(RichText::new("no data yet").weak());
            return;
        };
        let target = if dx.target.is_empty() {
            "—"
        } else {
            &dx.target
        };
        ui.label(format!("Target: {target}"));
        ui.add_space(6.0);

        ui.label("Foci (Hz):");
        if dx.foci.is_empty() {
            ui.label(RichText::new("   —").weak());
        } else {
            for freq in &dx.foci {
                ui.label(format!("   {:.0}", freq));
            }
        }
        ui.add_space(6.0);

        let parity = match dx.tx_parity {
            Some(0) => "even",
            Some(_) => "odd",
            None => "—",
        };
        ui.label(format!("TX slot: {parity}"));

        let grid = dx.hisgrid.as_deref().unwrap_or("—");
        let source = match dx.hisgrid_source {
            Some(HisgridSource::User) => " (user)",
            Some(HisgridSource::Harvested) => " (decoded)",
            _ => "",
        };
        ui.label(format!("Grid: {grid}{source}"));

        let dt = dx
            .dt
            .map(|dt| format!("{dt:+.1}"))
            .unwrap_or_else(|| "—".to_string());
        ui.label(format!("dt: {dt}"));
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
        const PAD: f32 = TABLE_PAD;
        let font = egui::FontId::monospace(DECODE_FONT_SIZE);
        let row_h = Self::row_height(ui.ctx());
        let dark = ui.visuals().dark_mode;
        let text_color = ui.visuals().text_color();
        let own_call = norm(&self.mycall);
        let target_call = norm(&self.hiscall);
        let target_grid = norm(&self.hisgrid);
        let separator_bg = if dark {
            Color32::from_gray(65)
        } else {
            Color32::from_gray(210)
        };
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
                        DisplayItem::Separator(gen) => {
                            ui.painter().rect_filled(rect, 0.0, separator_bg);
                            ui.painter().text(
                                egui::pos2(rect.left() + PAD, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                separator_label(*gen),
                                font.clone(),
                                text_color,
                            );
                            continue;
                        }
                    };
                    let color = if comparing {
                        source_color(row.source, dark, text_color)
                    } else {
                        text_color
                    };
                    let job = decode_highlight_job(
                        &row.text,
                        own_call.as_deref(),
                        target_call.as_deref(),
                        target_grid.as_deref(),
                        font.clone(),
                        color,
                    );
                    let galley = ui.painter().layout_job(job);
                    ui.painter().galley(
                        egui::pos2(rect.left() + PAD, rect.center().y - galley.size().y / 2.0),
                        galley,
                        color,
                    );
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
            let call_label = ui.label("Callsign");
            // The first field's measured height is the canonical row height; every
            // other control is forced to match it exactly.
            let old_call = norm(&self.hiscall);
            let r = callsign_field(ui, &mut self.hiscall, 110.0).labelled_by(call_label.id);
            if r.changed() && norm(&self.hiscall) != old_call {
                self.hisgrid.clear();
            }
            commit |= r.lost_focus();
            let ctrl_h = r.rect.height();
            let grid_label = ui.label("Grid");
            commit |= grid_field(ui, &mut self.hisgrid, 46.0)
                .labelled_by(grid_label.id)
                .lost_focus();
            let rx_label = ui.label("RX");
            let mut frequency = self.nfqso.parse::<u32>().unwrap_or(0);
            let response = ui
                .add(
                    egui::DragValue::new(&mut frequency)
                        .range(0..=3000)
                        .speed(1),
                )
                .labelled_by(rx_label.id)
                .on_hover_text("Drag to adjust the receive frequency, or click to type.");
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
                        "Stop monitoring"
                    } else {
                        "Start monitoring"
                    },
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        enabled,
                        if self.is_monitoring() {
                            "Stop monitoring"
                        } else {
                            "Start monitoring"
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
        if !self.settings_open {
            return;
        }
        let mut commit = false;
        let mut close = false;
        let viewport_id = egui::ViewportId::from_hash_of("settings");
        // A resizable OS window (own viewport) with a native title bar. The tabs
        // live in a left SidePanel whose edge separator spans the full height.
        ctx.show_viewport_immediate(
            viewport_id,
            egui::ViewportBuilder::default()
                .with_title("Settings")
                .with_inner_size([560.0, 400.0])
                .with_min_inner_size([480.0, 320.0])
                .with_resizable(true)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_visible(false)
                .with_active(false),
            |vctx, _class| {
                self.settings_focus.update(vctx, Instant::now());
                egui::Panel::left("settings_tabs")
                    .resizable(true)
                    .default_size(150.0)
                    .min_size(112.0)
                    .max_size(200.0)
                    .frame(
                        egui::Frame::side_top_panel(&vctx.global_style())
                            .inner_margin(egui::Margin::symmetric(12, 14)),
                    )
                    .show(vctx, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("settings_navigation")
                            .show(ui, |ui| {
                                ui.with_layout(
                                    egui::Layout::top_down_justified(egui::Align::LEFT),
                                    |ui| {
                                        for (tab, label) in [
                                            (SettingsTab::Station, "Station"),
                                            (SettingsTab::Audio, "Audio"),
                                            (SettingsTab::Decode, "Decode"),
                                            (SettingsTab::Frequency, "Frequency"),
                                            (SettingsTab::Output, "Output"),
                                            (SettingsTab::Compare, "Compare"),
                                            (SettingsTab::Advanced, "Advanced"),
                                        ] {
                                            if ui.selectable_label(self.tab == tab, label).clicked()
                                            {
                                                self.tab = tab;
                                            }
                                        }
                                    },
                                );
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
                            if let Some(err) = &self.error {
                                ui.add_space(6.0);
                                ui.colored_label(ui.visuals().error_fg_color, err);
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
        let mut commit = false;
        match self.tab {
            SettingsTab::Audio => {
                section_heading(ui, "Audio");
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, "Input device", |ui| self.device_combo(ui));
                    commit |= setting_row(ui, "Channel", |ui| {
                        let before = self.input_channel;
                        egui::ComboBox::from_id_salt("input_channel")
                            .width(ui.available_width())
                            .truncate()
                            .selected_text(match self.input_channel {
                                InputChannel::Mono => "Mono",
                                InputChannel::Left => "Left",
                                InputChannel::Right => "Right",
                                InputChannel::Mix => "Mix (average)",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Mono,
                                    "Mono",
                                );
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Left,
                                    "Left",
                                );
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Right,
                                    "Right",
                                );
                                ui.selectable_value(
                                    &mut self.input_channel,
                                    InputChannel::Mix,
                                    "Mix (average)",
                                );
                            });
                        self.input_channel != before
                    });
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        let _ = self.engine.send(EngineCommand::RefreshDevices);
                    }
                    if let Some((name, format)) = &self.capture_format {
                        ui.label(
                            RichText::new(format!(
                                "Active: {name} · {}ch / {} Hz / {}",
                                format.channels, format.sample_rate, format.sample_format
                            ))
                            .weak(),
                        );
                    } else if let Some(dev) = self.current_device_info() {
                        ui.label(
                            RichText::new(format!(
                                "Default format: {} · {}ch / {} Hz / {}",
                                dev.host,
                                dev.input.channels,
                                dev.input.sample_rate,
                                dev.input.sample_format
                            ))
                            .weak(),
                        );
                    }
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label("Input level");
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
                            let clipped = if dbfs >= -0.1 { " (near clipping)" } else { "" };
                            ui.label(
                                RichText::new(format!("{dbfs:.1} dBFS{clipped}")).color(color),
                            );
                        }
                        Some(_) => {
                            ui.label(
                                RichText::new("silent (−inf dBFS)")
                                    .color(Color32::from_rgb(0xdc, 0x26, 0x26)),
                            );
                        }
                        None => {
                            ui.label(RichText::new("— (start monitoring)").weak());
                        }
                    }
                });
                if !self.channel_peaks.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (i, &peak) in self.channel_peaks.iter().enumerate() {
                            ui.label(
                                RichText::new(format!("Ch {}: {} dBFS", i + 1, format_dbfs(peak)))
                                    .weak(),
                            );
                        }
                    });
                }
                if self.capture_xruns > 0 {
                    ui.label(
                        RichText::new(format!(
                            "Audio discontinuity reports: {} (monitoring continues)",
                            self.capture_xruns
                        ))
                        .weak(),
                    );
                }
                // Per-slot capture diagnostics: window length (should be ~14–15 s),
                // driver block size/count, arrival jitter (large ⇒ unreliable capture
                // timestamps), and decodes in the last slot. One live run locates most
                // capture/alignment issues.
                if let Some((samples, blocks, avg_block, jitter_ms, sample_rate)) =
                    self.capture_diag
                {
                    ui.add_space(2.0);
                    let secs = samples as f64 / f64::from(sample_rate);
                    let decodes = self
                        .last_slot_decodes
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "—".to_string());
                    // Flag jitter that would smear a timestamp-precise window.
                    let jitter_color = if jitter_ms >= 100 {
                        Color32::from_rgb(0xdc, 0x26, 0x26)
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "slot: {secs:.1}s / {samples} smp · {blocks} blk (~{avg_block} smp) · "
                            ))
                            .weak(),
                        );
                        ui.label(
                            RichText::new(format!("jitter {jitter_ms} ms")).color(jitter_color),
                        );
                        ui.label(RichText::new(format!(" · {decodes} dec")).weak());
                    });
                }
                if self.input_channel == InputChannel::Mono
                    || self.input_channel == InputChannel::Mix
                {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "Mono requests one channel; if unavailable, uses channel 1. \
                             Mix averages all channels and can cancel opposite-phase audio.",
                        )
                        .weak(),
                    );
                }
            }
            SettingsTab::Decode => {
                section_heading(ui, "Decode");
                let dx = self.profile == DecodeProfile::Dx;
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, "Profile", |ui| {
                        let before = self.profile;
                        egui::ComboBox::from_id_salt("profile")
                            .width(ui.available_width())
                            .truncate()
                            .selected_text(self.profile.as_str())
                            .show_ui(ui, |ui| {
                                for profile in [
                                    DecodeProfile::Wsjtx,
                                    DecodeProfile::Jtdx,
                                    DecodeProfile::Hybrid,
                                    DecodeProfile::Dx,
                                ] {
                                    ui.selectable_value(
                                        &mut self.profile,
                                        profile,
                                        profile.as_str(),
                                    );
                                }
                            });
                        self.profile != before
                    });
                    if self.profile == DecodeProfile::Hybrid {
                        commit |= setting_row(ui, "Cross cancellation", |ui| {
                            ui.add(egui::Checkbox::without_text(&mut self.hybrid_sic))
                                .changed()
                        });
                    }
                    commit |= setting_row(ui, "SWL", |ui| {
                        ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.swl))
                            .on_hover_text(
                                "Controls both JTDX paths in Hybrid. DX manages SWL internally.",
                            )
                            .changed()
                    });
                    commit |= setting_row(ui, "Deep decode", |ui| {
                        ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.nagain))
                            .on_hover_text("Enable JTDX focused deep decoding (nagain).")
                            .changed()
                    });
                    if self.profile == DecodeProfile::Hybrid {
                        commit |= setting_row(ui, "Experimental decoder", |ui| {
                            ui.add(egui::Checkbox::without_text(&mut self.hybrid_experiment))
                                .on_hover_text(
                                    "Adds weak stable-signal and interference recovery. \
                                     Results carry exp; processing may take about 10 seconds.",
                                )
                                .changed()
                        });
                    }
                });
                if dx {
                    ui.add_space(4.0);
                    ui.label(RichText::new("dx forces SWL/nagain internally").weak());
                }
            }
            SettingsTab::Frequency => {
                section_heading(ui, "Frequency");
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, "Low (Hz)", |ui| text_field(ui, &mut self.nfa));
                    commit |= setting_row(ui, "High (Hz)", |ui| text_field(ui, &mut self.nfb));
                });
            }
            SettingsTab::Station => {
                section_heading(ui, "Station");
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, "My Callsign", |ui| {
                        callsign_field(ui, &mut self.mycall, f32::INFINITY).lost_focus()
                    });
                    commit |= setting_row(ui, "My Grid", |ui| {
                        grid_field(ui, &mut self.mygrid, f32::INFINITY).lost_focus()
                    });
                });
            }
            SettingsTab::Output => {
                section_heading(ui, "Output");
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, "UDP reports", |ui| {
                        ui.add(egui::Checkbox::without_text(&mut self.udp_on))
                            .changed()
                    });
                    commit |= setting_row(ui, "Host", |ui| text_field(ui, &mut self.udp_host));
                    commit |= setting_row(ui, "Port", |ui| text_field(ui, &mut self.udp_port));
                });
            }
            SettingsTab::Compare => {
                section_heading(ui, "Compare");
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, "Listen to WSJT-X (UDP)", |ui| {
                        ui.add(egui::Checkbox::without_text(&mut self.udp_in_on))
                            .changed()
                    });
                    commit |= setting_row(ui, "Host", |ui| text_field(ui, &mut self.udp_in_host));
                    commit |= setting_row(ui, "Port", |ui| text_field(ui, &mut self.udp_in_port));
                    commit |= setting_row(ui, "Match ± Hz", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.udp_in_tol)
                                .speed(0.25)
                                .range(0.0..=10.0),
                        )
                        .changed()
                    });
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Merges WSJT-X decodes with the local ones to compare").weak(),
                );
                ui.label(RichText::new("sensitivity. Color shows the source:").weak());
                ui.add_space(4.0);
                let dark = ui.visuals().dark_mode;
                let default = ui.visuals().text_color();
                ui.colored_label(
                    source_color(DecodeSource::External, dark, default),
                    "■ only WSJT-X",
                );
                ui.colored_label(
                    source_color(DecodeSource::Local, dark, default),
                    "■ only ft8.rs",
                );
                ui.colored_label(default, "■ both decoders");
            }
            SettingsTab::Advanced => {
                section_heading(ui, "Advanced");
                settings_form(ui, |ui| {
                    let dx = self.profile == DecodeProfile::Dx;
                    commit |= setting_row(ui, "Narrow-band filter", |ui| {
                        ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.filter))
                            .changed()
                    });
                    commit |= setting_row(ui, "Hide duplicates", |ui| {
                        ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.hide_dupes))
                            .changed()
                    });
                    commit |= setting_row(ui, "Hide <...> calls", |ui| {
                        ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.hide_hash))
                            .changed()
                    });
                });
                ui.add_space(6.0);
                ui.label(RichText::new("Changes take effect at the next slot.").weak());
            }
        }
        commit
    }

    /// Standard picker: egui handles focus, keyboard input, popup sizing and
    /// truncation. Long endpoint names remain available in the hover text.
    fn device_combo(&mut self, ui: &mut egui::Ui) -> bool {
        let before = self.selected_device.clone();
        input_device_combo(ui, &mut self.selected_device, &self.devices);
        self.selected_device != before
    }

    fn current_device_info(&self) -> Option<&SoundcardDeviceInfo> {
        match &self.selected_device {
            Some(name) => self.devices.iter().find(|dev| &dev.name == name),
            None => self.devices.iter().find(|dev| dev.is_default_input),
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        if self.pending_confirm.is_none() {
            return;
        }
        let mut decided: Option<bool> = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("Switch DX target").size(15.0).strong());
            ui.add_space(8.0);
            ui.label("This discards the collected DX intel.");
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let confirm =
                        egui::Button::new(RichText::new("Switch").strong().color(Color32::WHITE))
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
                .with_title("About ft8.rs")
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
                                                RichText::new(ft8rs::about::NAME)
                                                    .size(18.0)
                                                    .strong(),
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
        storage.set_string("mycall", self.mycall.clone());
        storage.set_string("hiscall", self.hiscall.clone());
        storage.set_string("hisgrid", self.hisgrid.clone());
        storage.set_string("nfqso", self.nfqso.clone());
        storage.set_string("mygrid", self.mygrid.clone());
        storage.set_string("profile", self.profile.as_str().to_string());
        storage.set_string("swl", bs(self.swl));
        storage.set_string("hybrid_sic", bs(self.hybrid_sic));
        storage.set_string("hybrid_experiment", bs(self.hybrid_experiment));
        storage.set_string("nagain", bs(self.nagain));
        storage.set_string("nfa", self.nfa.clone());
        storage.set_string("nfb", self.nfb.clone());
        storage.set_string("udp_on", bs(self.udp_on));
        storage.set_string("udp_host", self.udp_host.clone());
        storage.set_string("udp_port", self.udp_port.clone());
        storage.set_string("filter", bs(self.filter));
        storage.set_string("hide_dupes", bs(self.hide_dupes));
        storage.set_string("hide_hash", bs(self.hide_hash));
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
        // Follow the OS light/dark theme: re-apply the custom style when it flips.
        let theme = ctx.theme();
        if theme != self.styled_theme {
            apply_style(&ctx, theme);
            self.styled_theme = theme;
        }
        self.sync_title(&ctx);
        self.sync_udp_in();
        self.pump_menu();
        self.pump_events();

        // macOS and Windows use a native menu bar; only the fallback platforms
        // (Linux) get an in-window top menu panel.
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("controls")
            .exact_size(46.0)
            .show(ui, |ui| self.bottom_bar(ui));
        if let Some(err) = &self.error {
            egui::Panel::bottom("error_status").show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("⚠ {err}")).color(ui.visuals().error_fg_color),
                    )
                    .wrap(),
                );
            });
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
        self.about_window(&ctx);
        self.copyright_window(&ctx);
        self.confirm_window(&ctx);

        // Keep pumping engine events even without user input.
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// A callsign field: forces ASCII uppercase live. Returns the Response (use
/// `.lost_focus()` for the commit signal, `.rect.height()` for sizing).
fn callsign_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .margin(egui::Margin::symmetric(8, 4)),
    );
    if resp.changed() && value.chars().any(|c| c.is_ascii_lowercase()) {
        *value = value.to_ascii_uppercase();
    }
    resp
}

/// A Maidenhead grid field constrained to AA00: positions 0–1 uppercase letters,
/// 2–3 digits, max 4 chars. Returns the Response.
fn grid_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .margin(egui::Margin::symmetric(8, 4)),
    );
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

/// A bold section heading for the settings content pane.
fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(14.0).strong());
    ui.add_space(10.0);
}

/// Standard Button + Popup controls allow the menu to size independently of
/// its trigger, including shrinking an open menu when the window gets smaller.
fn input_device_combo(
    ui: &mut egui::Ui,
    selected_device: &mut Option<String>,
    devices: &[SoundcardDeviceInfo],
) -> egui::Response {
    let selected = selected_device.as_deref().unwrap_or("Default").to_string();
    let field_width = ui.available_width();
    let popup_margin = egui::Frame::popup(ui.style()).total_margin().sum().x;
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_width = std::iter::once("Default")
        .chain(devices.iter().map(|device| device.name.as_str()))
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
                    .right_text("▾")
                    .truncate()
                    .min_size(egui::vec2(field_width, ui.spacing().interact_size.y)),
            )
        })
        .inner;
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), "Input device");
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
                    ui.selectable_value(selected_device, None, "Default");
                    for device in devices {
                        ui.selectable_value(
                            selected_device,
                            Some(device.name.clone()),
                            &device.name,
                        )
                        .on_hover_text(&device.name);
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
    ui.add_space(10.0);
}

/// One row in the standard settings Grid, returning the control's commit signal.
fn setting_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    ui.add(egui::Label::new(label).extend());
    let commit = add(ui);
    ui.end_row();
    commit
}

/// A bare single-line field. Symmetric margin gives it a uniform height with the
/// text vertically centered. Returns true on edit commit (focus left / Enter).
fn text_field(ui: &mut egui::Ui, value: &mut String) -> bool {
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 4)),
    )
    .lost_focus()
}

fn norm(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_uppercase())
    }
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
) -> egui::text::LayoutJob {
    const MESSAGE_START: usize = 24; // six UTC + dB/DT/frequency fields and separators
    let mut job = egui::text::LayoutJob::default();
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
                egui::TextFormat {
                    color: foreground,
                    background,
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
    job
}

fn parse_frequency_range(low: &str, high: &str) -> Result<(f64, f64), String> {
    let parse = |value: &str| {
        value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && (0.0..=5000.0).contains(v))
    };
    match (parse(low), parse(high)) {
        (Some(low), Some(high)) if low < high => Ok((low, high)),
        _ => Err("Frequency range must satisfy 0 ≤ Low < High ≤ 5000 Hz".to_string()),
    }
}

fn parse_port(value: &str) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|&port| port != 0)
        .ok_or_else(|| "UDP port must be an integer from 1 to 65535".to_string())
}

fn format_dbfs(peak: f32) -> String {
    if peak > 0.0 {
        format!("{:.1}", 20.0 * peak.log10())
    } else {
        "−inf".to_string()
    }
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

/// Build one divider before the first decoded slot and each subsequent slot.
/// Missing slots produce no item, so the log never contains an empty section.
fn display_items(rows: &[Row]) -> Vec<DisplayItem> {
    let mut items = Vec::with_capacity(rows.len() * 2);
    let mut previous = None;
    for (index, row) in rows.iter().enumerate() {
        if previous != Some(row.gen) {
            items.push(DisplayItem::Separator(row.gen));
        }
        items.push(DisplayItem::Decode(index));
        previous = Some(row.gen);
    }
    items
}

fn separator_label(gen: u64) -> String {
    let seconds = gen * 15;
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
    format!(
        "------ {year:04}-{month:02}-{day:02} - {:02}:{:02}:{:02} UTC - FT8 ------",
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
        StreamDecodeProvenance::Experiment => "exp",
        StreamDecodeProvenance::A7Memory => "a7",
        StreamDecodeProvenance::ApMask => "ap",
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

/// Fold `(freq, tokens)` into an existing **cross-source** row of the current
/// batch (`gen`), upgrading it to `Both`; returns true if folded. Same-source
/// rows are never folded: each source is already deduplicated upstream (the
/// engine merges hybrid's WSJT-X + JTDX output), so folding same-source would
/// drop genuinely distinct rows (e.g. a jtdx-unique decode that differs from a
/// WSJT-X row only in dt, which this fold key does not consider).
fn fold_into_cross_source(
    rows: &mut [Row],
    gen: u64,
    freq: f64,
    tol: f64,
    tokens: &[String],
    source: DecodeSource,
) -> bool {
    for row in rows.iter_mut() {
        if matches!(
            (row.source, source),
            (DecodeSource::Local, DecodeSource::External)
                | (DecodeSource::External, DecodeSource::Local)
        ) && row.gen == gen
            && (row.freq - freq).abs() <= tol
            && tokens_compatible(&row.tokens, tokens)
        {
            row.source = DecodeSource::Both;
            return true;
        }
    }
    false
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
        || matches!(tag, "sic" | "ctx" | "exp" | "ap" | "dp" | "im")
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

/// Profiles shown in the native Profile menu (macOS + Windows). The menu is
/// leaked so it lives for the app's lifetime; Linux uses the in-window menu.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const MENU_PROFILES: [DecodeProfile; 4] = [
    DecodeProfile::Wsjtx,
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

/// The "Profile" submenu with a checked item per profile. Shared by both menu
/// layouts; returns the submenu plus the items for checkmark syncing.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_profile_submenu(
    initial: DecodeProfile,
) -> (muda::Submenu, Vec<(DecodeProfile, muda::CheckMenuItem)>) {
    use muda::{CheckMenuItem, Submenu};
    let profile_menu = Submenu::new("Profile", true);
    let mut items = Vec::new();
    for profile in MENU_PROFILES {
        let item = CheckMenuItem::with_id(
            format!("profile:{}", profile.as_str()),
            profile.as_str(),
            true,
            profile == initial,
            None,
        );
        let _ = profile_menu.append(&item);
        items.push((profile, item));
    }
    (profile_menu, items)
}

/// macOS layout: an app menu ("ft8.rs" → About / Settings… / Quit) plus Profile.
#[cfg(target_os = "macos")]
fn install_menu(
    initial: DecodeProfile,
    _hwnd: Option<isize>,
) -> Vec<(DecodeProfile, muda::CheckMenuItem)> {
    use muda::accelerator::Accelerator;
    use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let menu = Menu::new();
    let app_menu = Submenu::new("ft8.rs", true);
    let about = MenuItem::with_id("about", "About", true, None);
    let settings = MenuItem::with_id(
        "settings",
        "Settings…",
        true,
        "CmdOrCtrl+,".parse::<Accelerator>().ok(),
    );
    let _ = app_menu.append_items(&[
        &about,
        &PredefinedMenuItem::separator(),
        &settings,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(Some("Quit")),
    ]);
    let _ = menu.append(&app_menu);

    let (profile_menu, items) = build_profile_submenu(initial);
    let _ = menu.append(&profile_menu);

    menu.init_for_nsapp();
    std::mem::forget(menu);
    items
}

/// Windows layout: three top-level items side by side — Settings, Profile, About
/// (Windows has no app menu concept; Quit is the title-bar close button).
#[cfg(target_os = "windows")]
fn install_menu(
    initial: DecodeProfile,
    hwnd: Option<isize>,
) -> Vec<(DecodeProfile, muda::CheckMenuItem)> {
    use muda::{Menu, MenuItem};

    let Some(hwnd) = hwnd else {
        return Vec::new();
    };
    let menu = Menu::new();
    let settings = MenuItem::with_id("settings", "Settings", true, None);
    let about = MenuItem::with_id("about", "About", true, None);
    let (profile_menu, items) = build_profile_submenu(initial);
    let _ = menu.append(&settings);
    let _ = menu.append(&profile_menu);
    let _ = menu.append(&about);

    // SAFETY: `hwnd` is the live main-window handle from eframe; muda subclasses
    // it to host the menu bar and forward WM_COMMAND to our MenuEvent handler.
    unsafe {
        let _ = menu.init_for_hwnd(hwnd);
    }
    std::mem::forget(menu);
    items
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn install_menu(
    _initial: DecodeProfile,
    _hwnd: Option<isize>,
) -> Vec<(DecodeProfile, muda::CheckMenuItem)> {
    Vec::new()
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

    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = Margin::same(12);
    style.spacing.menu_margin = Margin::same(8);
    style.spacing.interact_size.y = 24.0;

    // Monospace everywhere (matching the decode table).
    style.text_styles = [
        (TextStyle::Heading, FontId::new(17.0, Monospace)),
        (TextStyle::Body, FontId::new(13.0, Monospace)),
        (TextStyle::Button, FontId::new(13.0, Monospace)),
        (TextStyle::Small, FontId::new(11.0, Monospace)),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into();

    // eframe 0.35: Context::set_style is gone; set the same style for all themes.
    ctx.all_styles_mut(move |s| *s = style.clone());
}

/// Install a single monospace font (Monaco-like) as the primary face for BOTH
/// families so the whole UI is uniformly monospaced, then append a CJK font as
/// fallback for the Chinese labels. Falls back to egui's defaults if a candidate
/// is missing (e.g. headless CI).
fn install_fonts(ctx: &egui::Context) {
    const MONO: &[&str] = &[
        "/System/Library/Fonts/Monaco.ttf",
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/SFNSMono.ttf",
        "C:/Windows/Fonts/consola.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    ];
    const CJK: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    let read_first = |paths: &[&str]| paths.iter().find_map(|path| std::fs::read(path).ok());

    let mut fonts = egui::FontDefinitions::default();

    if let Some(bytes) = read_first(MONO) {
        // Compute the centering tweak from the bytes before they move into FontData.
        let data = egui::FontData::from_owned(bytes.clone()).tweak(center_tweak(&bytes));
        fonts
            .font_data
            .insert("mono".to_string(), std::sync::Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "mono".to_string());
        }
    }
    if let Some(bytes) = read_first(CJK) {
        let data = egui::FontData::from_owned(bytes.clone()).tweak(center_tweak(&bytes));
        fonts
            .font_data
            .insert("cjk".to_string(), std::sync::Arc::new(data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("cjk".to_string());
        }
    }
    ctx.set_fonts(fonts);
}

/// Vertical-centering tweak for a font face. egui sizes each text row from the
/// primary font's `ascent - descent + line_gap` and draws the baseline at
/// `ascent` from the row top, so the line gap becomes dead space at the *bottom*
/// of the row and the ink sits high — text looks shifted up (notably Consolas on
/// Windows; Monaco/macOS has ~no line gap, so this is a no-op there). We read the
/// gap from the font and push glyphs down half of it, centering the ink in the
/// row box without changing the row height. Returns the default tweak if the font
/// can't be parsed (keeps startup robust).
fn center_tweak(bytes: &[u8]) -> egui::FontTweak {
    use ab_glyph::Font as _;
    // A small constant downward bias: centering the typographic block leaves text
    // looking a hair high (ascenders/descenders are rarely all used), so nudge it
    // down ~1px for optical centering. Applies to every face and platform.
    let mut tweak = egui::FontTweak {
        y_offset: 1.0,
        ..Default::default()
    };
    // index 0 handles both single faces (.ttf/.otf) and the first face of a
    // collection (.ttc), e.g. Menlo / PingFang / YaHei.
    if let Ok(font) = ab_glyph::FontRef::try_from_slice_and_index(bytes, 0) {
        if let Some(upem) = font.units_per_em() {
            if upem > 0.0 {
                let gap_fraction = font.line_gap_unscaled() / upem;
                tweak.y_offset_factor = gap_fraction / 2.0;
            }
        }
    }
    tweak
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_mark_first_and_subsequent_nonempty_slots() {
        let row = |gen| Row {
            text: String::new(),
            gen,
            freq: 1000.0,
            tokens: Vec::new(),
            source: DecodeSource::Local,
        };
        let rows = [row(10), row(10), row(13), row(13), row(15)];
        let items = display_items(&rows);
        assert_eq!(items.len(), 8);
        assert!(matches!(items[0], DisplayItem::Separator(10)));
        assert!(matches!(items[1], DisplayItem::Decode(0)));
        assert!(matches!(items[2], DisplayItem::Decode(1)));
        assert!(matches!(items[3], DisplayItem::Separator(13)));
        assert!(matches!(items[6], DisplayItem::Separator(15)));
        assert!(matches!(
            display_items(&rows[1..])[0],
            DisplayItem::Separator(10)
        ));
        assert!(display_items(&[]).is_empty());
        // Unix epoch plus one skipped slot: no divider for the missing slot.
        assert_eq!(
            separator_label(2),
            "------ 1970-01-01 - 00:00:30 UTC - FT8 ------"
        );
    }

    #[test]
    fn target_highlight_only_marks_complete_message_tokens() {
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
            "020045", -25, -0.4, 1256, "5R8IC ZY7H HI06", "a7"
        );
        let job = decode_highlight_job(
            &text,
            None,
            Some("ZY7H"),
            Some("HI06"),
            egui::FontId::monospace(14.0),
            Color32::BLACK,
        );
        let marked: Vec<_> = job
            .sections
            .iter()
            .filter(|section| section.format.background != Color32::TRANSPARENT)
            .map(|section| &job.text[section.byte_range.start.0..section.byte_range.end.0])
            .collect();
        assert_eq!(marked, ["ZY7H", "HI06"]);
        assert_eq!(job.text, text);

        let similar = text.replace("ZY7H", "ZY7H/P").replace("HI06", "HI06AA");
        let job = decode_highlight_job(
            &similar,
            None,
            Some("ZY7H"),
            Some("HI06"),
            egui::FontId::monospace(14.0),
            Color32::BLACK,
        );
        assert!(job
            .sections
            .iter()
            .all(|section| section.format.background == Color32::TRANSPARENT));
    }

    #[test]
    fn own_callsign_is_yellow_but_own_grid_is_unmarked() {
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
            "020045", -18, -0.4, 1108, "CQ BG5ATV PM00", ""
        );
        let job = decode_highlight_job(
            &text,
            Some("BG5ATV"),
            None,
            None,
            egui::FontId::monospace(14.0),
            Color32::BLACK,
        );
        let marked: Vec<_> = job
            .sections
            .iter()
            .filter(|section| section.format.background != Color32::TRANSPARENT)
            .map(|section| {
                (
                    &job.text[section.byte_range.start.0..section.byte_range.end.0],
                    section.format.background,
                    section.format.color,
                )
            })
            .collect();
        assert_eq!(marked, [("BG5ATV", Color32::YELLOW, Color32::BLACK)]);
        assert_eq!(job.text, text);
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
                freq: 2367.0,
                tokens: norm_tokens(message),
                source: first,
            }];
            assert!(fold_into_cross_source(
                &mut rows,
                3362,
                2369.0,
                7.0,
                &norm_tokens("CQ BA7SAY OL53 ? a2"),
                second
            ));
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
                freq: 1118.0,
                tokens: tokens.clone(),
                source: first,
            }];
            assert!(fold_into_cross_source(
                &mut rows,
                u64::from(external.ms_since_midnight / 15_000),
                1118.0,
                7.0,
                incoming,
                source,
            ));
            assert_eq!(rows[0].source, DecodeSource::Both);
            assert_eq!(rows[0].text, text);
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
            "im",
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
    fn same_source_rows_never_fold_but_cross_source_merges_to_both() {
        let mk = |source, freq, msg: &str| Row {
            text: String::new(),
            gen: 1,
            freq,
            tokens: norm_tokens(msg),
            source,
        };
        let mut rows = vec![mk(DecodeSource::Local, 1000.0, "CQ K1ABC FN42")];

        // A second LOCAL decode of the "same" signal (within tol) must NOT fold —
        // the engine already deduplicated local output, so this is a genuinely
        // distinct row (e.g. jtdx-unique differing only in dt). It appends.
        assert!(!fold_into_cross_source(
            &mut rows,
            1,
            1002.0,
            7.0,
            &norm_tokens("CQ K1ABC FN42"),
            DecodeSource::Local,
        ));
        assert_eq!(rows[0].source, DecodeSource::Local);

        // An EXTERNAL decode of the same signal folds into the local row → Both.
        assert!(fold_into_cross_source(
            &mut rows,
            1,
            1001.0,
            7.0,
            &norm_tokens("CQ K1ABC FN42"),
            DecodeSource::External,
        ));
        assert_eq!(rows[0].source, DecodeSource::Both);
        // The matched pair is complete: neither source can consume it again.
        for source in [DecodeSource::Local, DecodeSource::External] {
            assert!(!fold_into_cross_source(
                &mut rows,
                1,
                1001.0,
                7.0,
                &norm_tokens("CQ K1ABC FN42"),
                source
            ));
        }

        // A different gen (different slot) never folds.
        assert!(!fold_into_cross_source(
            &mut rows,
            2,
            1000.0,
            7.0,
            &norm_tokens("CQ K1ABC FN42"),
            DecodeSource::External,
        ));
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
                    freq: 1118.0,
                    tokens: norm_tokens("CQ ZL2AGY RF70 a7"),
                    source: first,
                }];
                assert_eq!(
                    fold_into_cross_source(
                        &mut rows,
                        2702,
                        1118.0 + offset,
                        tolerance,
                        &norm_tokens("CQ ZL2AGY RF70 ? a1"),
                        incoming,
                    ),
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
    fn standard_form_reflows_after_window_width_changes() {
        let ctx = egui::Context::default();
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
                            setting_row(ui, "My Callsign", |ui| {
                                fields[0] = callsign_field(ui, &mut value, f32::INFINITY).rect;
                                false
                            });
                            setting_row(ui, "Input device", |ui| {
                                fields[1] = egui::ComboBox::from_id_salt("test_device")
                                    .width(ui.available_width())
                                    .truncate()
                                    .selected_text("DAX RX 1 (a deliberately long endpoint name)")
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
                                response = Some(input_device_combo(ui, &mut selected, &devices));
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
                        response = Some(input_device_combo(ui, &mut selected, &devices));
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
                            SettingsTab::Compare => "Listen to WSJT-X (UDP)",
                            SettingsTab::Frequency => "Low (Hz)",
                            _ => "My Callsign",
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
            SettingsTab::Station,
            SettingsTab::Compare,
            SettingsTab::Audio,
            SettingsTab::Station,
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
