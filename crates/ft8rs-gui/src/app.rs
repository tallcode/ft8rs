//! ft8rs GUI (P3) — egui front-end over the live engine.
//!
//! Monitor-only (file decode stays in the CLI). The four hot fields
//! (mycall/hiscall/hisgrid/nfqso) + Monitor button sit at the bottom; everything
//! else lives in the Settings dialog (tabs). Edits apply live while monitoring
//! via `EngineCommand::ApplyState` (the engine plans L0/L1/L2 and reconfigures at
//! the next slot boundary). The decode table mirrors the CLI columns; slot
//! separators show only the time.

use std::time::Duration;

use eframe::egui::{self, Color32, RichText};

use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance};
use ft8rs_engine::protocol::{
    DxContextSnapshot, EngineCommand, EngineEvent, EngineStatus, HisgridSource,
};
use ft8rs_engine::reconfig::{plan_reconfig, EngineState};
use ft8rs_engine::report::UdpConfig;
use ft8rs_engine::soundcard::SoundcardDeviceInfo;
use ft8rs_engine::EngineHandle;

const MAX_ROWS: usize = 4000;
const DECODE_FONT_SIZE: f32 = 12.0;
/// Internal left padding for the decode table (header + rows go edge-to-edge).
const TABLE_PAD: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq)]
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum DecodeSource {
    Local,
    External,
    Both,
}

struct Row {
    text: String,
    parity: u8,
    // Merge keys (compare mode): same slot + freq within tolerance + compatible
    // message tokens identify the same transmission across the two decoders.
    slot_key: u32,
    freq: f64,
    tokens: Vec<String>,
    source: DecodeSource,
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

    // Runtime.
    status: EngineStatus,
    applied: Option<EngineState>,
    rows: Vec<Row>,
    error: Option<String>,
    dx: Option<DxContextSnapshot>,
    pending_confirm: Option<EngineState>,

    settings_open: bool,
    about_open: bool,
    tab: SettingsTab,

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
}

impl Ft8rsApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let theme = cc.egui_ctx.theme();
        apply_style(&cc.egui_ctx, theme);
        let engine = EngineHandle::spawn();
        let _ = engine.send(EngineCommand::RefreshDevices);

        // Restore persisted settings (P5.2). Devices are keyed by name; if the
        // saved device is gone at startup it simply falls back to default.
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
        // Native menu bar: macOS via the system menu, Windows via the window's
        // HWND (Win32 menu bar), Linux falls back to the in-window menu.
        let profile_menu = install_menu(profile, window_hwnd(cc));

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
            udp_in_on: load_bool("udp_in_on", false),
            udp_in_host: load("udp_in_host", "127.0.0.1"),
            udp_in_port: load("udp_in_port", "2237"),
            udp_in_tol: load("udp_in_tol", "7").trim().parse().unwrap_or(7.0),
            devices: Vec::new(),
            selected_device: (!device.is_empty()).then_some(device),
            status: EngineStatus::Idle,
            applied: None,
            rows: Vec::new(),
            error: None,
            dx: None,
            pending_confirm: None,
            settings_open: false,
            about_open: false,
            tab: SettingsTab::Station,
            profile_menu,
            menu_profile_synced: profile,
            styled_theme: theme,
            last_title: String::new(),
            logo,
            udp_in: None,
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
        matches!(self.status, EngineStatus::Aligning | EngineStatus::Monitoring)
    }

    fn dx_needs_hiscall(&self) -> bool {
        self.profile == DecodeProfile::Dx && norm(&self.hiscall).is_none()
    }

    fn build_state(&self) -> EngineState {
        let mut config = StreamDecodeConfig {
            profile: self.profile,
            mycall: norm(&self.mycall),
            mygrid: norm(&self.mygrid),
            hiscall: norm(&self.hiscall),
            hisgrid: norm(&self.hisgrid),
            swl: self.swl,
            nagain: self.nagain,
            filter: self.filter,
            hide_dupes: self.hide_dupes,
            hide_hash: self.hide_hash,
            ..StreamDecodeConfig::default()
        };
        if let Ok(value) = self.nfa.trim().parse::<f64>() {
            config.nfa = value;
        }
        if let Ok(value) = self.nfb.trim().parse::<f64>() {
            config.nfb = value;
        }
        // nfqso only counts inside the [nfa, nfb] band; 0 or out-of-band means
        // "no QSO focus" (passed as 0) so it never perturbs the decode.
        let nfqso = self.nfqso.trim().parse::<f64>().unwrap_or(0.0);
        config.nfqso = if nfqso >= config.nfa && nfqso <= config.nfb {
            nfqso
        } else {
            0.0
        };
        EngineState {
            device: self.selected_device.clone(),
            config,
            udp: self.udp_config(),
        }
    }

    fn udp_config(&self) -> Option<UdpConfig> {
        if !self.udp_on {
            return None;
        }
        Some(UdpConfig {
            host: self.udp_host.trim().to_string(),
            port: self.udp_port.trim().parse().unwrap_or(2238),
        })
    }

    fn toggle_monitor(&mut self) {
        if self.is_monitoring() {
            let _ = self.engine.send(EngineCommand::StopMonitor);
            self.status = EngineStatus::Idle;
            self.applied = None;
            return;
        }
        if self.dx_needs_hiscall() {
            self.error = Some("profile dx requires His Call".to_string());
            return;
        }
        let state = self.build_state();
        self.error = None;
        self.rows.clear();
        self.dx = None;
        if self.engine.send(EngineCommand::StartMonitor(state.clone())).is_ok() {
            self.applied = Some(state);
            self.status = EngineStatus::Aligning;
        }
    }

    /// Push a live config change to the engine, if monitoring and it differs.
    /// A change that discards DX intel (target switch) is deferred to a confirm
    /// dialog instead of applied immediately (decision 4 / §6.1).
    fn apply_if_changed(&mut self) {
        if !self.is_monitoring() {
            return;
        }
        if self.dx_needs_hiscall() {
            return; // would be invalid; leave the running session as-is
        }
        let desired = self.build_state();
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
        if self.engine.send(EngineCommand::ApplyState(desired.clone())).is_ok() {
            self.applied = Some(desired);
        }
    }

    fn pump_events(&mut self) {
        while let Some(event) = self.engine.try_recv() {
            match event {
                EngineEvent::Status(status) => self.status = status,
                EngineEvent::DevicesRefreshed(devices) => self.devices = devices,
                EngineEvent::Decode(record) => self.push_decode(record),
                EngineEvent::SlotComplete { .. } => {}
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
            row.msg,
            provenance_tag(record.provenance),
        );
        self.ingest(slot_key, row.freq, text, norm_tokens(&row.msg), DecodeSource::Local);
    }

    fn merge_external(&mut self, decode: crate::wsjtx_udp::ExternalDecode) {
        let slot_key = decode.ms_since_midnight / 15_000;
        // No provenance column for external decodes (WSJT-X doesn't send one).
        let body = clean_message(&decode.message);
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {:<20} {}",
            format_ms_time(decode.ms_since_midnight),
            decode.snr,
            decode.dt,
            decode.freq,
            body,
            "",
        );
        self.ingest(
            slot_key,
            decode.freq as f64,
            text,
            norm_tokens(&decode.message),
            DecodeSource::External,
        );
    }

    /// Merge a decode into the row list. A match (same slot + freq within
    /// tolerance + compatible message) from the other source upgrades the
    /// existing row to `Both`; any match keeps the first row (dedupe). No match
    /// appends a new row.
    fn ingest(
        &mut self,
        slot_key: u32,
        freq: f64,
        text: String,
        tokens: Vec<String>,
        source: DecodeSource,
    ) {
        let tol = self.udp_in_tol;
        for row in self.rows.iter_mut() {
            if row.slot_key == slot_key
                && (row.freq - freq).abs() <= tol
                && tokens_compatible(&row.tokens, &tokens)
            {
                if row.source != source && row.source != DecodeSource::Both {
                    row.source = DecodeSource::Both;
                }
                return; // keep the first row
            }
        }
        let parity = (slot_key % 2) as u8;
        self.rows.push(Row {
            text,
            parity,
            slot_key,
            freq,
            tokens,
            source,
        });
        if self.rows.len() > MAX_ROWS {
            let drop = self.rows.len() - MAX_ROWS;
            self.rows.drain(..drop);
        }
    }

    /// Start/stop/restart the WSJT-X listener to match the current settings.
    /// Called every frame; cheap when nothing changed.
    fn sync_udp_in(&mut self) {
        if !self.udp_in_on {
            self.udp_in = None; // drop stops the thread
            return;
        }
        let port: u16 = self.udp_in_port.trim().parse().unwrap_or(2237);
        let host = self.udp_in_host.trim();
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
        while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "settings" => self.settings_open = true,
                "about" => self.about_open = true,
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
            }
            if ui.button("About").clicked() {
                self.about_open = true;
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
        let target = if dx.target.is_empty() { "—" } else { &dx.target };
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
        ctx.fonts(|f| f.row_height(&egui::FontId::monospace(DECODE_FONT_SIZE)))
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
            format!("{:<6} {:>3} {:>5} {:>5}  {}", "UTC", "dB", "DT", "Freq", "Message"),
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
        let row_h = ui.fonts(|f| f.row_height(&font));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                let width = ui.available_width();
                let dark = ui.visuals().dark_mode;
                let text_color = ui.visuals().text_color();
                let stripe = ui.visuals().faint_bg_color;
                // Compare-mode coloring only applies while listening; otherwise
                // every row is the default color.
                let comparing = self.udp_in_on;
                for row in &self.rows {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, row_h), egui::Sense::hover());
                    // Alternate the slot background: :00/:30 vs :15/:45. The stripe
                    // spans the full width; the text is padded inside.
                    if row.parity == 1 {
                        ui.painter().rect_filled(rect, 0.0, stripe);
                    }
                    let color = if comparing {
                        source_color(row.source, dark, text_color)
                    } else {
                        text_color
                    };
                    ui.painter().text(
                        egui::pos2(rect.left() + PAD, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        &row.text,
                        font.clone(),
                        color,
                    );
                }
            });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        // Uniform control height is the key to a stable row: every widget derives
        // its height from font(11) + 2·PAD_Y, with the interact-size floor removed
        // so nothing snaps to a different minimum. No nested layouts on the left —
        // labels, fields, the RX value and its spinner are all direct children of
        // one centered row, so they line up by construction.
        const PAD_Y: f32 = 4.0;
        ui.style_mut().override_font_id = Some(egui::FontId::monospace(11.0));
        {
            let s = ui.spacing_mut();
            s.item_spacing = egui::vec2(6.0, 0.0);
            s.button_padding = egui::vec2(8.0, PAD_Y);
            s.interact_size.y = 0.0;
        }

        ui.horizontal_centered(|ui| {
            let mut commit = false;
            ui.label("Callsign");
            // The first field's measured height is the canonical row height; every
            // other control is forced to match it exactly.
            let r = callsign_field(ui, &mut self.hiscall, 88.0);
            commit |= r.lost_focus();
            let ctrl_h = r.rect.height();
            ui.label("Grid");
            commit |= grid_field(ui, &mut self.hisgrid, 40.0).lost_focus();
            ui.label("RX");
            // A normal text field (typeable, same tailwind style as the others) +
            // a tight ▲/▼ stepper, then the unit label.
            let r = rx_text_field(ui, &mut self.nfqso, 40.0);
            commit |= r.lost_focus();
            ui.spacing_mut().item_spacing.x = 1.0;
            commit |= rx_stepper(ui, &mut self.nfqso, r.rect.height());
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label("Hz");

            // Right-aligned state button — square, height matched to the row.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, fill) = match self.status {
                    EngineStatus::Monitoring => ("■", Color32::from_rgb(0x16, 0xa3, 0x4a)),
                    EngineStatus::Aligning => ("■", Color32::from_rgb(0xd9, 0x77, 0x06)),
                    EngineStatus::Idle | EngineStatus::Error(_) => {
                        ("▶", Color32::from_rgb(0x25, 0x63, 0xeb))
                    }
                };
                let enabled = self.is_monitoring() || !self.dx_needs_hiscall();
                let button = egui::Button::new(RichText::new(icon).color(Color32::WHITE))
                    .fill(fill)
                    .min_size(egui::vec2(ctrl_h, ctrl_h));
                if ui.add_enabled(enabled, button).clicked() {
                    self.toggle_monitor();
                }
                if let Some(err) = &self.error {
                    ui.add_space(8.0);
                    ui.colored_label(Color32::from_rgb(0xf8, 0x71, 0x71), format!("⚠ {err}"));
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
        // A real OS window (own viewport), fixed size, native title bar. The tabs
        // live in a left SidePanel whose edge separator spans the full height.
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("settings"),
            egui::ViewportBuilder::default()
                .with_title("Settings")
                .with_inner_size([560.0, 400.0])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false),
            |vctx, _class| {
                egui::SidePanel::left("settings_tabs")
                    .resizable(false)
                    .exact_width(150.0)
                    .frame(
                        egui::Frame::side_top_panel(&vctx.style())
                            .inner_margin(egui::Margin::symmetric(12.0, 14.0)),
                    )
                    .show(vctx, |ui| {
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
                                    if ui.selectable_label(self.tab == tab, label).clicked() {
                                        self.tab = tab;
                                    }
                                }
                            },
                        );
                    });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&vctx.style())
                            .inner_margin(egui::Margin::symmetric(18.0, 14.0)),
                    )
                    .show(vctx, |ui| {
                        commit = self.settings_tab_ui(ui);
                    });
                if vctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        if close {
            self.settings_open = false;
        }
        if commit {
            self.apply_if_changed();
        }
    }

    fn settings_tab_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut commit = false;
        match self.tab {
            SettingsTab::Audio => {
                section_heading(ui, "Audio");
                commit |= setting_row(ui, "Input device", |ui| {
                    let before = self.selected_device.clone();
                    egui::ComboBox::from_id_salt("device")
                        .width(150.0)
                        .selected_text(
                            self.selected_device
                                .clone()
                                .unwrap_or_else(|| "Default".to_string()),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.selected_device, None, "Default");
                            for dev in &self.devices {
                                ui.selectable_value(
                                    &mut self.selected_device,
                                    Some(dev.name.clone()),
                                    &dev.name,
                                );
                            }
                        });
                    self.selected_device != before
                });
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        let _ = self.engine.send(EngineCommand::RefreshDevices);
                    }
                    if let Some(dev) = self.current_device_info() {
                        ui.label(
                            RichText::new(format!(
                                "{} · {}ch / {} Hz",
                                dev.host, dev.input.channels, dev.input.sample_rate
                            ))
                            .weak(),
                        );
                    }
                });
            }
            SettingsTab::Decode => {
                section_heading(ui, "Decode");
                commit |= setting_row(ui, "Profile", |ui| {
                    let before = self.profile;
                    egui::ComboBox::from_id_salt("profile")
                        .width(140.0)
                        .selected_text(self.profile.as_str())
                        .show_ui(ui, |ui| {
                            for profile in [
                                DecodeProfile::Wsjtx,
                                DecodeProfile::Jtdx,
                                DecodeProfile::Hybrid,
                                DecodeProfile::Dx,
                            ] {
                                ui.selectable_value(&mut self.profile, profile, profile.as_str());
                            }
                        });
                    self.profile != before
                });
                let dx = self.profile == DecodeProfile::Dx;
                commit |= setting_row(ui, "SWL", |ui| {
                    ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.swl))
                        .changed()
                });
                commit |= setting_row(ui, "nagain (deep)", |ui| {
                    ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.nagain))
                        .changed()
                });
                if dx {
                    ui.add_space(4.0);
                    ui.label(RichText::new("dx forces SWL/nagain internally").weak());
                }
            }
            SettingsTab::Frequency => {
                section_heading(ui, "Frequency");
                commit |= setting_row(ui, "Low (nfa) Hz", |ui| text_field(ui, &mut self.nfa, 140.0));
                commit |= setting_row(ui, "High (nfb) Hz", |ui| text_field(ui, &mut self.nfb, 140.0));
            }
            SettingsTab::Station => {
                section_heading(ui, "Station");
                commit |= setting_row(ui, "My Callsign", |ui| {
                    callsign_field(ui, &mut self.mycall, 140.0).lost_focus()
                });
                commit |= setting_row(ui, "My Grid", |ui| {
                    grid_field(ui, &mut self.mygrid, 140.0).lost_focus()
                });
            }
            SettingsTab::Output => {
                section_heading(ui, "Output");
                commit |= setting_row(ui, "UDP reports", |ui| {
                    ui.add(egui::Checkbox::without_text(&mut self.udp_on)).changed()
                });
                commit |= setting_row(ui, "Host", |ui| text_field(ui, &mut self.udp_host, 160.0));
                commit |= setting_row(ui, "Port", |ui| text_field(ui, &mut self.udp_port, 100.0));
            }
            SettingsTab::Compare => {
                section_heading(ui, "Compare");
                commit |= setting_row(ui, "Listen to WSJT-X (UDP)", |ui| {
                    ui.add(egui::Checkbox::without_text(&mut self.udp_in_on)).changed()
                });
                commit |= setting_row(ui, "Host", |ui| text_field(ui, &mut self.udp_in_host, 160.0));
                commit |= setting_row(ui, "Port", |ui| text_field(ui, &mut self.udp_in_port, 100.0));
                commit |= setting_row(ui, "Match ± Hz", |ui| {
                    ui.add(egui::DragValue::new(&mut self.udp_in_tol).speed(0.25).range(0.0..=10.0))
                        .changed()
                });
                ui.add_space(10.0);
                ui.label(RichText::new("Merges WSJT-X decodes with the local ones to compare").weak());
                ui.label(RichText::new("sensitivity. Color shows the source:").weak());
                ui.add_space(4.0);
                let dark = ui.visuals().dark_mode;
                let default = ui.visuals().text_color();
                ui.colored_label(source_color(DecodeSource::External, dark, default), "■ only WSJT-X");
                ui.colored_label(source_color(DecodeSource::Local, dark, default), "■ only ft8.rs");
                ui.colored_label(default, "■ both decoders");
            }
            SettingsTab::Advanced => {
                section_heading(ui, "Advanced");
                let dx = self.profile == DecodeProfile::Dx;
                commit |= setting_row(ui, "filter (narrow band)", |ui| {
                    ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.filter))
                        .changed()
                });
                commit |= setting_row(ui, "hide dupes", |ui| {
                    ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.hide_dupes))
                        .changed()
                });
                commit |= setting_row(ui, "hide <...> hash", |ui| {
                    ui.add_enabled(!dx, egui::Checkbox::without_text(&mut self.hide_hash))
                        .changed()
                });
                ui.add_space(6.0);
                ui.label(RichText::new("Kernel decode switches (rebuild session).").weak());
            }
        }
        commit
    }

    fn current_device_info(&self) -> Option<&SoundcardDeviceInfo> {
        let name = self.selected_device.as_ref()?;
        self.devices.iter().find(|dev| &dev.name == name)
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        if self.pending_confirm.is_none() {
            return;
        }
        let mut decided: Option<bool> = None;
        egui::Window::new("confirm")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
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
                        let confirm = egui::Button::new(
                            RichText::new("Switch").strong().color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(0x25, 0x63, 0xeb));
                        if ui.add(confirm).clicked() {
                            decided = Some(true);
                        }
                    });
                });
            });
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
                // Revert the edited target to the running value.
                let running = self
                    .applied
                    .as_ref()
                    .and_then(|state| state.config.hiscall.clone())
                    .unwrap_or_default();
                self.hiscall = running;
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
        // A real OS window (own viewport) with a native title bar.
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("about"),
            egui::ViewportBuilder::default()
                .with_title("About ft8.rs")
                .with_inner_size([320.0, 420.0])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false),
            |vctx, _class| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&vctx.style())
                            .inner_margin(egui::Margin::symmetric(22.0, 14.0)),
                    )
                    .show(vctx, |ui| {
                    ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_space(10.0);
                    ui.vertical_centered(|ui| {
                        if let Some(tex) = &logo {
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                tex.id(),
                                egui::vec2(80.0, 80.0),
                            )));
                            ui.add_space(6.0);
                        }
                        ui.label(RichText::new("ft8.rs").size(18.0).strong());
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("Version {}", env!("FT8RS_VERSION"))).weak(),
                        );
                        ui.label(
                            RichText::new(format!("FFT engine: {}", ft8rs::fft_engine_name()))
                                .weak(),
                        );
                        ui.label(RichText::new("License: GPL-3.0").weak());
                    });
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(RichText::new("Acknowledgements").strong());
                    ui.add_space(4.0);
                    ui.label("• WSJT-X — original FT8 decoder");
                    ui.label("• JTDX — deep-decode improvements");
                    ui.label("• RustFFT — FFT engine");
                    ui.label("• egui / eframe — user interface");
                    ui.label("• cpal — audio capture");
                    ui.add_space(6.0);
                    ui.label(RichText::new("Thanks to the amateur-radio community.").weak());
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
        storage.set_string(
            "device",
            self.selected_device.clone().unwrap_or_default(),
        );
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Follow the OS light/dark theme: re-apply the custom style when it flips.
        let theme = ctx.theme();
        if theme != self.styled_theme {
            apply_style(ctx, theme);
            self.styled_theme = theme;
        }
        self.sync_title(ctx);
        self.sync_udp_in();
        self.pump_menu();
        self.pump_events();

        // macOS and Windows use a native menu bar; only the fallback platforms
        // (Linux) get an in-window top menu panel.
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::bottom("controls")
            .exact_height(42.0)
            .show(ctx, |ui| self.bottom_bar(ui));
        if self.profile == DecodeProfile::Dx {
            egui::SidePanel::right("dx_panel")
                .resizable(false)
                .default_width(232.0)
                .show(ctx, |ui| self.dx_panel(ui));
        }
        // Fixed header in its own top panel (so it never scrolls and owns its
        // divider line), then the scrolling rows in the central panel. Both use a
        // zero inner margin so they reach the window edge; padding is internal.
        let zero = egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::same(0.0));
        egui::TopBottomPanel::top("table_header")
            .exact_height(Self::row_height(ctx) + 8.0)
            .frame(zero)
            .show_separator_line(false) // we draw our own thin divider
            .show(ctx, |ui| self.table_header(ui));
        egui::CentralPanel::default()
            .frame(zero)
            .show(ctx, |ui| self.decode_rows(ui));

        self.settings_window(ctx);
        self.about_window(ctx);
        self.confirm_window(ctx);

        // Keep pumping engine events even without user input.
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// The RX-frequency field: a normal numeric text field (digits only, clamped
/// 0..=3000) styled exactly like the other input fields — so it stays typeable
/// and visually consistent. Returns the Response.
fn rx_text_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .margin(egui::Margin::symmetric(8.0, 4.0)),
    );
    if resp.changed() {
        value.retain(|c| c.is_ascii_digit());
        if value.parse::<f64>().map_or(false, |v| v > 3000.0) {
            *value = "3000".to_string();
        }
    }
    resp
}

/// A tight ▲/▼ stepper sitting next to the RX field: two equal clickable halves
/// (combined height == `height`), painted to match. Steps the parsed value 1 Hz
/// within 0..=3000. Returns true when a click changed it.
fn rx_stepper(ui: &mut egui::Ui, value: &mut String, height: f32) -> bool {
    use egui::{pos2, vec2, Pos2, Rect, Sense, Shape, Stroke};
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, height), Sense::hover());
    let mid = rect.center().y;
    let up_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, mid - 0.5));
    let dn_rect = Rect::from_min_max(pos2(rect.min.x, mid + 0.5), rect.max);
    let up = ui.interact(up_rect, ui.id().with("rx_up"), Sense::click());
    let dn = ui.interact(dn_rect, ui.id().with("rx_dn"), Sense::click());

    let (fg, hover_bg, rounding) = {
        let v = ui.visuals();
        (
            v.text_color(),
            v.widgets.hovered.weak_bg_fill,
            v.widgets.inactive.rounding,
        )
    };
    let p = ui.painter();
    if up.hovered() {
        p.rect_filled(up_rect, rounding, hover_bg);
    }
    if dn.hovered() {
        p.rect_filled(dn_rect, rounding, hover_bg);
    }
    let tri = |c: Pos2, pointing_up: bool| {
        let (hw, hh) = (3.0, 2.0);
        if pointing_up {
            vec![pos2(c.x - hw, c.y + hh), pos2(c.x + hw, c.y + hh), pos2(c.x, c.y - hh)]
        } else {
            vec![pos2(c.x - hw, c.y - hh), pos2(c.x + hw, c.y - hh), pos2(c.x, c.y + hh)]
        }
    };
    p.add(Shape::convex_polygon(tri(up_rect.center(), true), fg, Stroke::NONE));
    p.add(Shape::convex_polygon(tri(dn_rect.center(), false), fg, Stroke::NONE));

    let step = |value: &mut String, delta: f64| {
        let v = (value.trim().parse::<f64>().unwrap_or(0.0) + delta).clamp(0.0, 3000.0);
        *value = (v as i64).to_string();
    };
    let mut changed = false;
    if up.clicked() {
        step(value, 1.0);
        changed = true;
    }
    if dn.clicked() {
        step(value, -1.0);
        changed = true;
    }
    changed
}

/// A callsign field: forces ASCII uppercase live. Returns the Response (use
/// `.lost_focus()` for the commit signal, `.rect.height()` for sizing).
fn callsign_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let resp = ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .margin(egui::Margin::symmetric(8.0, 4.0)),
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
            .margin(egui::Margin::symmetric(8.0, 4.0)),
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

/// A bold section heading for the settings content pane.
fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(14.0).strong());
    ui.add_space(10.0);
}

/// A settings row: label on the left, control right-aligned. Returns the
/// control's commit signal.
fn setting_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> bool) -> bool {
    let mut commit = false;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            commit = add(ui);
        });
    });
    ui.add_space(6.0);
    commit
}

/// A bare single-line field. Symmetric margin gives it a uniform height with the
/// text vertically centered. Returns true on edit commit (focus left / Enter).
fn text_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> bool {
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(width)
            .margin(egui::Margin::symmetric(8.0, 4.0)),
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

/// Provenance tag shown as a trailing column after the message
/// (`a7`/`ap`/`a8`/deep/imported), blank for ordinary decodes.
fn provenance_tag(provenance: StreamDecodeProvenance) -> &'static str {
    match provenance {
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

/// Normalize a decode message for merge matching: drop WSJT-X annotations, then
/// uppercase the remaining tokens (the actual message body).
fn norm_tokens(msg: &str) -> Vec<String> {
    msg.split_whitespace()
        .filter(|t| !is_annotation(t))
        .map(|t| t.to_ascii_uppercase())
        .collect()
}

/// The message body with WSJT-X trailing annotations removed (for display of
/// external decodes, so they read like the local ones).
fn clean_message(msg: &str) -> String {
    msg.split_whitespace()
        .filter(|t| !is_annotation(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A WSJT-X decode annotation, not part of the message body: `a` + 1-2 digits
/// (AP type + optional averaging count, lowercase per the decoder) or the `?`
/// low-confidence marker. FT8 message text is uppercase, so this never matches a
/// real token (see wsjtx/lib/decoder.f90 and mainwindow.cpp:6907).
fn is_annotation(token: &str) -> bool {
    token == "?"
        || (token.len() >= 2
            && token.starts_with('a')
            && token[1..].bytes().all(|b| b.is_ascii_digit()))
}

/// Whether two normalized messages describe the same transmission, lenient on
/// hashed-call format: equal token counts and, per position, equal tokens or
/// either side being a `<...>` hash token (so `<...>` ↔ `<K1ABC>` ↔ a resolved
/// call all match).
fn tokens_compatible(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x == y || is_hash_token(x) || is_hash_token(y))
}

fn is_hash_token(token: &str) -> bool {
    token.starts_with('<') && token.ends_with('>')
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

/// Build the shared muda menu (works on every platform muda supports): an app
/// menu (About / Settings… / Quit) plus a Profile menu with a checked item per
/// profile. Returns the menu and the profile items (for checkmark syncing).
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_menu(
    initial: DecodeProfile,
) -> (muda::Menu, Vec<(DecodeProfile, muda::CheckMenuItem)>) {
    use muda::accelerator::Accelerator;
    use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

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
    let _ = menu.append(&profile_menu);
    (menu, items)
}

#[cfg(target_os = "macos")]
fn install_menu(
    initial: DecodeProfile,
    _hwnd: Option<isize>,
) -> Vec<(DecodeProfile, muda::CheckMenuItem)> {
    let (menu, items) = build_menu(initial);
    menu.init_for_nsapp();
    std::mem::forget(menu);
    items
}

#[cfg(target_os = "windows")]
fn install_menu(
    initial: DecodeProfile,
    hwnd: Option<isize>,
) -> Vec<(DecodeProfile, muda::CheckMenuItem)> {
    let Some(hwnd) = hwnd else {
        return Vec::new();
    };
    let (menu, items) = build_menu(initial);
    // SAFETY: `hwnd` is the live main-window handle from eframe; muda subclasses
    // it to host the menu bar and forward WM_COMMAND to MenuEvent::receiver().
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
/// only customize rounding, spacing, a blue accent, and the monospace fonts.
fn apply_style(ctx: &egui::Context, theme: egui::Theme) {
    use egui::{FontFamily::Monospace, FontId, Margin, Rounding, Stroke, TextStyle};

    let dark = theme == egui::Theme::Dark;
    let accent = if dark {
        Color32::from_rgb(0x3b, 0x82, 0xf6) // blue-500
    } else {
        Color32::from_rgb(0x25, 0x63, 0xeb) // blue-600
    };

    let mut style = (*ctx.style()).clone();
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    let rounding = Rounding::same(2.0);
    v.widgets.noninteractive.rounding = rounding;
    v.widgets.inactive.rounding = rounding;
    v.widgets.hovered.rounding = rounding;
    v.widgets.active.rounding = rounding;
    v.widgets.open.rounding = rounding;

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
    v.window_rounding = Rounding::same(4.0);
    v.menu_rounding = Rounding::same(4.0);

    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = Margin::same(12.0);
    style.spacing.menu_margin = Margin::same(8.0);
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

    ctx.set_style(style);
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
        fonts
            .font_data
            .insert("mono".to_string(), egui::FontData::from_owned(bytes));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .insert(0, "mono".to_string());
        }
    }
    if let Some(bytes) = read_first(CJK) {
        fonts
            .font_data
            .insert("cjk".to_string(), egui::FontData::from_owned(bytes));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push("cjk".to_string());
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_detection() {
        for t in ["a1", "a7", "a9", "a72", "a31", "?"] {
            assert!(is_annotation(t), "{t} should be an annotation");
        }
        for t in ["CQ", "A7", "FN42", "K1ABC", "RR73", "73", "R-12", "<...>", "<K1ABC>"] {
            assert!(!is_annotation(t), "{t} should not be an annotation");
        }
    }

    #[test]
    fn clean_message_drops_annotations() {
        assert_eq!(clean_message("CQ K1ABC FN42 a7"), "CQ K1ABC FN42");
        assert_eq!(clean_message("K1ABC W1AW R-12 ? a31"), "K1ABC W1AW R-12");
        assert_eq!(clean_message("CQ BG5ATV PM00"), "CQ BG5ATV PM00");
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
    }
}
