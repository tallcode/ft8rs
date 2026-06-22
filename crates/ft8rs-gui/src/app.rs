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

use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};
use ft8rs::SlotTimestamp;
use ft8rs_engine::protocol::{EngineCommand, EngineEvent, EngineStatus};
use ft8rs_engine::reconfig::{plan_reconfig, EngineState};
use ft8rs_engine::report::UdpConfig;
use ft8rs_engine::soundcard::SoundcardDeviceInfo;
use ft8rs_engine::EngineHandle;

const MAX_ROWS: usize = 4000;
const CQ_COLOR: Color32 = Color32::from_rgb(140, 230, 140);
const MYCALL_COLOR: Color32 = Color32::from_rgb(120, 200, 255);
const SEP_COLOR: Color32 = Color32::from_rgb(150, 150, 150);

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Audio,
    Decode,
    Frequency,
    Station,
    Output,
    Advanced,
}

enum Row {
    Separator(String),
    Decode { text: String, color: Option<Color32> },
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

    devices: Vec<SoundcardDeviceInfo>,
    selected_device: Option<String>, // device name; None = default

    // Runtime.
    status: EngineStatus,
    applied: Option<EngineState>,
    rows: Vec<Row>,
    last_slot: Option<SlotTimestamp>,
    error: Option<String>,

    settings_open: bool,
    about_open: bool,
    tab: SettingsTab,
}

impl Ft8rsApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_cjk_font(&cc.egui_ctx);
        apply_style(&cc.egui_ctx);
        install_menu();
        let engine = EngineHandle::spawn();
        let _ = engine.send(EngineCommand::RefreshDevices);
        Self {
            engine,
            mycall: String::new(),
            hiscall: String::new(),
            hisgrid: String::new(),
            nfqso: String::new(),
            profile: DecodeProfile::Wsjtx,
            swl: false,
            nagain: false,
            nfa: "200".to_string(),
            nfb: "3000".to_string(),
            mygrid: String::new(),
            udp_on: false,
            udp_host: "127.0.0.1".to_string(),
            udp_port: "2238".to_string(),
            filter: false,
            hide_dupes: false,
            hide_hash: false,
            devices: Vec::new(),
            selected_device: None,
            status: EngineStatus::Idle,
            applied: None,
            rows: Vec::new(),
            last_slot: None,
            error: None,
            settings_open: false,
            about_open: false,
            tab: SettingsTab::Audio,
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
        config.nfqso = self.nfqso.trim().parse::<f64>().unwrap_or(0.0);
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
        self.last_slot = None;
        if self.engine.send(EngineCommand::StartMonitor(state.clone())).is_ok() {
            self.applied = Some(state);
            self.status = EngineStatus::Aligning;
        }
    }

    /// Push a live config change to the engine, if monitoring and it differs.
    fn apply_if_changed(&mut self) {
        if !self.is_monitoring() {
            return;
        }
        if self.dx_needs_hiscall() {
            return; // would be invalid; leave the running session as-is
        }
        let desired = self.build_state();
        let changed = match &self.applied {
            Some(applied) => !plan_reconfig(applied, &desired).is_noop(),
            None => true,
        };
        if changed && self.engine.send(EngineCommand::ApplyState(desired.clone())).is_ok() {
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
                EngineEvent::DxContext(_) => {}
                EngineEvent::Error(err) => self.error = Some(err),
            }
        }
    }

    fn push_decode(&mut self, record: ft8rs_engine::protocol::DecodeRecord) {
        let ts = record.timestamp;
        if self.last_slot.as_ref() != Some(&ts) {
            self.rows.push(Row::Separator(format!(
                "────────  {}  UTC  ────────",
                pretty_time(&ts)
            )));
            self.last_slot = Some(ts.clone());
        }
        let row = record.row;
        let text = format!(
            "{:<6} {:>3} {:>5.1} {:>5}  {}",
            ts.format_time(),
            row.snr.round() as i32,
            row.dt,
            row.freq.round() as i64,
            row.msg
        );
        let color = row_color(&row.msg, norm(&self.mycall).as_deref());
        self.rows.push(Row::Decode { text, color });
        if self.rows.len() > MAX_ROWS {
            let drop = self.rows.len() - MAX_ROWS;
            self.rows.drain(..drop);
        }
    }

    // ── UI ──────────────────────────────────────────────────────────────────

    fn pump_menu(&mut self) {
        while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
            if event.id == muda::MenuId::new("settings") {
                self.settings_open = true;
            } else if event.id == muda::MenuId::new("about") {
                self.about_open = true;
            }
        }
    }

    // Non-macOS in-window menu (macOS uses the native system menu bar).
    #[cfg(not(target_os = "macos"))]
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("设置 Settings").clicked() {
                self.settings_open = true;
            }
            if ui.button("关于 About").clicked() {
                self.about_open = true;
            }
        });
    }

    fn decode_table(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "{:<6} {:>3} {:>5} {:>5}  {}",
                    "UTC", "dB", "DT", "Freq", "信息 Message"
                ))
                .monospace()
                .strong(),
            )
            .wrap_mode(egui::TextWrapMode::Extend),
        );
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for row in &self.rows {
                    match row {
                        Row::Separator(text) => {
                            ui.add(
                                egui::Label::new(RichText::new(text).monospace().color(SEP_COLOR))
                                    .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                        Row::Decode { text, color } => {
                            let mut rich = RichText::new(text).monospace();
                            if let Some(color) = color {
                                rich = rich.color(*color);
                            }
                            ui.add(
                                egui::Label::new(rich).wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                    }
                }
            });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let mut commit = false;
            commit |= labeled_field(ui, "His Call", &mut self.hiscall, 150.0);
            commit |= labeled_field(ui, "His Grid", &mut self.hisgrid, 100.0);
            commit |= labeled_field(ui, "nfqso", &mut self.nfqso, 90.0);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let monitoring = self.is_monitoring();
                let label = if monitoring { "■  Stop" } else { "▶  Monitor" };
                let fill = if monitoring {
                    Color32::from_rgb(0xdc, 0x26, 0x26) // red-600
                } else {
                    Color32::from_rgb(0x25, 0x63, 0xeb) // blue-600
                };
                let enabled = monitoring || !self.dx_needs_hiscall();
                let button = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                    .fill(fill)
                    .min_size(egui::vec2(124.0, 32.0));
                if ui.add_enabled(enabled, button).clicked() {
                    self.toggle_monitor();
                }
                // Status to the left of the button — nothing shown when idle.
                ui.add_space(8.0);
                if let Some(err) = &self.error {
                    ui.colored_label(Color32::from_rgb(0xf8, 0x71, 0x71), format!("⚠ {err}"));
                } else {
                    match &self.status {
                        EngineStatus::Monitoring => {
                            ui.colored_label(Color32::from_rgb(0x4a, 0xde, 0x80), "● Monitoring");
                        }
                        EngineStatus::Aligning => {
                            ui.colored_label(Color32::from_rgb(0xfa, 0xcc, 0x15), "○ Aligning…");
                        }
                        EngineStatus::Idle | EngineStatus::Error(_) => {}
                    }
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
        let mut keep_open = true;
        egui::Window::new("settings")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size(egui::vec2(640.0, 470.0))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                // Own header (no OS-style title bar): close button, top-right.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("设置 Settings").size(15.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if close_button(ui).clicked() {
                            keep_open = false;
                        }
                    });
                });
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(168.0);
                        ui.label(RichText::new("SETTINGS").small().weak());
                        ui.add_space(6.0);
                        // Full-width, left-aligned tab rows.
                        ui.with_layout(
                            egui::Layout::top_down_justified(egui::Align::LEFT),
                            |ui| {
                                for (tab, label) in [
                                    (SettingsTab::Audio, "音频 Audio"),
                                    (SettingsTab::Decode, "解码 Decode"),
                                    (SettingsTab::Frequency, "频率 Frequency"),
                                    (SettingsTab::Station, "台站 Station"),
                                    (SettingsTab::Output, "输出 Output"),
                                    (SettingsTab::Advanced, "高级 Advanced"),
                                ] {
                                    if ui.selectable_label(self.tab == tab, label).clicked() {
                                        self.tab = tab;
                                    }
                                }
                            },
                        );
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        ui.set_min_width(380.0);
                        commit = self.settings_tab_ui(ui);
                    });
                });
            });
        if !keep_open {
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
                section_heading(ui, "音频 Audio");
                commit |= setting_row(ui, "Input device", |ui| {
                    let before = self.selected_device.clone();
                    egui::ComboBox::from_id_salt("device")
                        .width(200.0)
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
                    if ui.button("刷新 Refresh").clicked() {
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
                section_heading(ui, "解码 Decode");
                commit |= setting_row(ui, "Profile", |ui| {
                    let before = self.profile;
                    egui::ComboBox::from_id_salt("profile")
                        .width(160.0)
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
                section_heading(ui, "频率 Frequency");
                commit |= setting_row(ui, "Low (nfa) Hz", |ui| text_field(ui, &mut self.nfa, 140.0));
                commit |= setting_row(ui, "High (nfb) Hz", |ui| text_field(ui, &mut self.nfb, 140.0));
            }
            SettingsTab::Station => {
                section_heading(ui, "台站 Station");
                commit |= setting_row(ui, "My Call", |ui| text_field(ui, &mut self.mycall, 160.0));
                commit |= setting_row(ui, "My Grid", |ui| text_field(ui, &mut self.mygrid, 160.0));
            }
            SettingsTab::Output => {
                section_heading(ui, "输出 Output");
                commit |= setting_row(ui, "UDP reports", |ui| {
                    ui.add(egui::Checkbox::without_text(&mut self.udp_on)).changed()
                });
                commit |= setting_row(ui, "Host", |ui| text_field(ui, &mut self.udp_host, 160.0));
                commit |= setting_row(ui, "Port", |ui| text_field(ui, &mut self.udp_port, 100.0));
            }
            SettingsTab::Advanced => {
                section_heading(ui, "高级 Advanced");
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

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let mut keep_open = true;
        egui::Window::new("about")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("关于 About").size(15.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if close_button(ui).clicked() {
                            keep_open = false;
                        }
                    });
                });
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(8.0);
                ui.label(RichText::new("ft8rs").strong());
                ui.label(format!("Version: {}", env!("FT8RS_VERSION")));
                ui.label(format!("FFT engine: {}", ft8rs::fft_engine_name()));
                ui.label("License: GPL-3.0");
            });
        if !keep_open {
            self.about_open = false;
        }
    }
}

impl eframe::App for Ft8rsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_menu();
        self.pump_events();

        // macOS uses the native system menu bar and shows status in the bottom
        // bar, so no in-window top panel there.
        #[cfg(not(target_os = "macos"))]
        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::bottom("controls")
            .exact_height(56.0)
            .show(ctx, |ui| self.bottom_bar(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.decode_table(ui));

        self.settings_window(ctx);
        self.about_window(ctx);

        // Keep pumping engine events even without user input.
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// A labeled single-line field. Returns true when its edit is committed
/// (focus left / Enter), the signal to push a live config change.
fn labeled_field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32) -> bool {
    ui.label(label);
    text_field(ui, value, width)
}

/// A frameless close button using a glyph present in the default fonts (the
/// fancy ✕ renders as tofu in the monospace/CJK fallback). Kept compact so the
/// dialog header stays short.
fn close_button(ui: &mut egui::Ui) -> egui::Response {
    ui.add(egui::Button::new(RichText::new("×").size(20.0)).frame(false))
}

/// A bold section heading for the settings content pane.
fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(16.0).strong());
    ui.add_space(12.0);
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
            .margin(egui::Margin::symmetric(8.0, 7.0)),
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

fn row_color(msg: &str, mycall: Option<&str>) -> Option<Color32> {
    if msg.starts_with("CQ") {
        return Some(CQ_COLOR);
    }
    if let Some(mycall) = mycall {
        if msg
            .split_whitespace()
            .any(|word| word.eq_ignore_ascii_case(mycall))
        {
            return Some(MYCALL_COLOR);
        }
    }
    None
}

fn pretty_time(ts: &SlotTimestamp) -> String {
    let t = ts.format_time(); // HHMMSS
    if t.len() == 6 {
        format!("{}:{}:{}", &t[0..2], &t[2..4], &t[4..6])
    } else {
        t
    }
}

/// Install the native macOS system menu bar (app menu with About / Settings… /
/// Quit). The menu is leaked so it lives for the app's lifetime. No-op on other
/// platforms, which use the in-window buttons instead. Shows best when run as a
/// `.app` bundle; a bare terminal-launched binary may not display the system bar.
#[cfg(target_os = "macos")]
fn install_menu() {
    use muda::accelerator::Accelerator;
    use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let menu = Menu::new();
    let app_menu = Submenu::new("ft8rs", true);
    let about = MenuItem::with_id("about", "关于 About", true, None);
    let settings = MenuItem::with_id(
        "settings",
        "设置 Settings…",
        true,
        "CmdOrCtrl+,".parse::<Accelerator>().ok(),
    );
    let _ = app_menu.append_items(&[
        &about,
        &PredefinedMenuItem::separator(),
        &settings,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(None),
    ]);
    let _ = menu.append(&app_menu);
    menu.init_for_nsapp();
    std::mem::forget(menu);
}

#[cfg(not(target_os = "macos"))]
fn install_menu() {}

/// Apply a neutral dark theme in the style of the Claude desktop app: near-black
/// surfaces, subtle elevated panels, rounded fields/buttons, a quiet neutral
/// selection, and a blue focus accent. Fonts stay monospace.
fn apply_style(ctx: &egui::Context) {
    use egui::{FontFamily::Monospace, FontId, Margin, Rounding, Stroke, TextStyle};

    let app_bg = Color32::from_rgb(0x0e, 0x0e, 0x0e); // neutral-950
    let elevated = Color32::from_rgb(0x1b, 0x1b, 0x1b); // dialog / panel
    let field_bg = Color32::from_rgb(0x12, 0x12, 0x12);
    let widget = Color32::from_rgb(0x26, 0x26, 0x26); // neutral-800
    let widget_hover = Color32::from_rgb(0x30, 0x30, 0x30);
    let selected = Color32::from_rgb(0x2e, 0x2e, 0x2e); // sidebar selection
    let accent = Color32::from_rgb(0x3b, 0x82, 0xf6); // blue-500 focus
    let text = Color32::from_rgb(0xed, 0xed, 0xed);
    let faint = Color32::from_rgb(0x8a, 0x8a, 0x8a);
    let border = Color32::from_rgb(0x2a, 0x2a, 0x2a);

    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(text);
    v.panel_fill = app_bg;
    v.window_fill = elevated;
    v.extreme_bg_color = field_bg;
    v.faint_bg_color = elevated;
    v.window_rounding = Rounding::same(14.0);
    v.menu_rounding = Rounding::same(10.0);
    v.window_stroke = Stroke::new(1.0, border);
    v.selection.bg_fill = selected;
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;

    let rounding = Rounding::same(8.0);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = elevated;
    w.noninteractive.weak_bg_fill = elevated;
    w.noninteractive.bg_stroke = Stroke::new(1.0, border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, faint);
    w.noninteractive.rounding = rounding;

    w.inactive.bg_fill = widget;
    w.inactive.weak_bg_fill = widget;
    w.inactive.bg_stroke = Stroke::new(1.0, border);
    w.inactive.fg_stroke = Stroke::new(1.0, text);
    w.inactive.rounding = rounding;

    w.hovered.bg_fill = widget_hover;
    w.hovered.weak_bg_fill = widget_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x3a));
    w.hovered.fg_stroke = Stroke::new(1.0, text);
    w.hovered.rounding = rounding;

    w.active.bg_fill = widget_hover;
    w.active.weak_bg_fill = widget_hover;
    w.active.bg_stroke = Stroke::new(1.0, accent);
    w.active.fg_stroke = Stroke::new(1.0, text);
    w.active.rounding = rounding;

    w.open.bg_fill = widget;
    w.open.bg_stroke = Stroke::new(1.0, border);
    w.open.rounding = rounding;

    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = Margin::same(18.0);
    style.spacing.menu_margin = Margin::same(8.0);
    style.spacing.interact_size.y = 30.0;

    // Everything in the monospace family (matching the decode table header).
    style.text_styles = [
        (TextStyle::Heading, FontId::new(18.0, Monospace)),
        (TextStyle::Body, FontId::new(14.0, Monospace)),
        (TextStyle::Button, FontId::new(14.0, Monospace)),
        (TextStyle::Small, FontId::new(12.0, Monospace)),
        (TextStyle::Monospace, FontId::new(14.0, Monospace)),
    ]
    .into();

    ctx.set_style(style);
}

/// Load a system CJK font so the Chinese labels render. Falls back silently to
/// the default font if none of the candidate paths exist (e.g. headless CI).
fn install_cjk_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    let Some(bytes) = CANDIDATES
        .iter()
        .find_map(|path| std::fs::read(path).ok())
    else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".to_string(), egui::FontData::from_owned(bytes));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".to_string());
    }
    ctx.set_fonts(fonts);
}
