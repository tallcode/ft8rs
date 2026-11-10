//! ft8.rs GUI state and coordination between the engine and egui views.
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use ft8rs::stream::session::DecodeProfile;
use ft8rs_engine::protocol::{DxContextSnapshot, EngineCommand, EngineEvent, EngineStatus};
use ft8rs_engine::reconfig::{plan_reconfig, EngineState};
use ft8rs_engine::soundcard::SoundcardDeviceInfo;
use ft8rs_engine::EngineHandle;

use crate::bands::CurrentBand;
use crate::clock_sync;
use crate::debug_info::{self, DebugInfo, Recorder};
use crate::fonts::{
    install as install_fonts, FontSupport, BODY_FONT_SIZE, CHINESE_UNAVAILABLE, SMALL_FONT_SIZE,
};
use crate::i18n::{Language, Locale};

mod controls;
mod decode_log;
mod decode_table;
mod dialogs;
mod menu;
mod preferences;
mod settings_view;
mod widgets;

use decode_log::{external_decode_row, local_decode_row, DecodeLog};
use dialogs::DialogFocus;
use menu::{profile_label, NativeMenu};
use preferences::{parse_port, Preferences, SettingsTab};

/// Cooperative input budget; final batch compaction happens once afterwards.
const EVENT_BUDGET: Duration = Duration::from_millis(2);

struct MonitorRun {
    id: u64,
    status: EngineStatus,
    submitted_state: Option<EngineState>,
}

impl MonitorRun {
    /// Returns whether the current run ended and its UI telemetry should be reset.
    fn update_status(&mut self, id: u64, status: EngineStatus) -> bool {
        if id != self.id {
            return false;
        }
        let ended = matches!(status, EngineStatus::Idle | EngineStatus::Error(_));
        self.status = status;
        if ended {
            self.submitted_state = None;
        }
        ended
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

    preferences: Preferences,
    locale: Locale,
    requested_locale: Locale,
    font_support: FontSupport,
    locale_checked_at: Instant,
    devices: Vec<SoundcardDeviceInfo>,
    input_peak: Option<f32>, // recent capture peak; None until audio arrives
    capture_xruns: u64,

    // Runtime.
    monitor: MonitorRun,
    decode_log: DecodeLog,
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

        let preferences = Preferences::load(cc.storage);
        let requested_locale = preferences.language.resolve_system();
        let locale = font_support.locale(requested_locale);
        let (menu_events, native_menu) = menu::initialize(
            cc,
            preferences.profile,
            preferences.language,
            locale,
            debug_info::eligible(&preferences.mycall),
        );
        let menu_synced = (preferences.profile, preferences.language, locale);

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
            preferences,
            locale,
            requested_locale,
            font_support,
            locale_checked_at: Instant::now(),
            devices: Vec::new(),
            input_peak: None,
            capture_xruns: 0,
            monitor: MonitorRun {
                id: 0,
                status: EngineStatus::Idle,
                submitted_state: None,
            },
            decode_log: DecodeLog::default(),
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
            menu_synced,
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
        let profile = profile_label(self.preferences.profile);
        let title = match norm(&self.preferences.mycall) {
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
            self.monitor.status,
            EngineStatus::Aligning | EngineStatus::Monitoring
        )
    }

    fn dx_needs_hiscall(&self) -> bool {
        self.preferences.profile == DecodeProfile::Dx && norm(&self.preferences.hiscall).is_none()
    }

    fn toggle_monitor(&mut self) {
        if self.is_monitoring() {
            let _ = self.engine.send(EngineCommand::StopMonitor);
            self.monitor.id += 1;
            self.monitor.status = EngineStatus::Idle;
            self.monitor.submitted_state = None;
            self.pending_confirm = None;
            self.input_peak = None;
            self.capture_xruns = 0;
            return;
        }
        if self.dx_needs_hiscall() {
            self.error = Some("DX requires a target callsign.".to_string());
            return;
        }
        if let Some(tab) = self.preferences.invalid_monitor_settings_page() {
            self.select_settings_tab(tab);
            self.open_settings();
            return;
        }
        let state = match self.preferences.build_state(&self.devices) {
            Ok(state) => state,
            Err(err) => {
                self.error = Some(err);
                return;
            }
        };
        self.error = None;
        self.dx = None;
        self.input_peak = None;
        self.capture_xruns = 0;
        let id = self.monitor.id + 1;
        match self.engine.send(EngineCommand::StartMonitor {
            id,
            state: state.clone(),
        }) {
            Ok(()) => {
                self.monitor.id = id;
                self.monitor.submitted_state = Some(state);
                self.monitor.status = EngineStatus::Aligning;
            }
            Err(error) => self.monitor.status = EngineStatus::Error(error),
        }
    }

    /// Push a live config change to the engine, if monitoring and it differs.
    /// A change that discards DX intel (target switch) is deferred to a confirm
    /// dialog instead of applied immediately.
    fn apply_if_changed(&mut self) {
        if self.preferences.invalid_monitor_settings_page().is_some() {
            return;
        }
        if self.dx_needs_hiscall() {
            self.error = Some("DX requires a target callsign.".to_string());
            return; // would be invalid; leave the running session as-is
        }
        let desired = match self.preferences.build_state(&self.devices) {
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
        let Some(submitted) = &self.monitor.submitted_state else {
            return;
        };
        let outcome = plan_reconfig(submitted, &desired);
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
            self.monitor.submitted_state = Some(desired);
        }
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        let clock = Instant::now();
        while clock.elapsed() < EVENT_BUDGET {
            let Some(status) = self.udp_in.as_ref().and_then(|udp| udp.try_dial_status()) else {
                break;
            };
            self.current_band.observe(status);
        }
        let mut batch = self.decode_log.batch(
            self.preferences.udp_in_tol,
            self.preferences.display_filters,
        );
        // Keep the existing local-before-external consumption order, with one
        // budget for both sources rather than an unbounded drain of either.
        while clock.elapsed() < EVENT_BUDGET {
            let Some(event) = self.engine.try_recv() else {
                break;
            };
            match event {
                EngineEvent::Status { id, status } => {
                    if self.monitor.update_status(id, status) {
                        self.pending_confirm = None;
                        self.input_peak = None;
                        self.capture_xruns = 0;
                    }
                }
                EngineEvent::DevicesRefreshed(devices) => {
                    if let Some(selected) = self.preferences.selected_device.as_deref() {
                        match ft8rs_engine::soundcard::migrate_device_name(selected, &devices) {
                            Ok(id) => self.preferences.selected_device = Some(id),
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
                    id,
                    requested_device,
                    requested_channel,
                    active_device,
                    active_channel,
                } if id == self.monitor.id => {
                    if self.preferences.selected_device == requested_device
                        && self.preferences.input_channel == requested_channel
                    {
                        self.preferences.selected_device = active_device.clone();
                        self.preferences.input_channel = active_channel;
                    }
                    if let Some(submitted) = &mut self.monitor.submitted_state {
                        if submitted.device == requested_device
                            && submitted.channel == requested_channel
                        {
                            submitted.device = active_device;
                            submitted.channel = active_channel;
                        }
                    }
                }
                EngineEvent::CaptureSelectionRejected { .. } => {}
                EngineEvent::Decode(record) => {
                    let mut row = local_decode_row(record);
                    row.slot_band = self.current_band.name_at_slot(
                        row.slot.utc_slot,
                        self.clock_boundary.map(|(observed, _)| observed),
                    );
                    row.slot.clock_segment = self.clock_segment;
                    batch.insert(row);
                }
                EngineEvent::ClockDiscontinuity { at_ms, observed } => {
                    self.clock_segment += 1;
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
            if self.clock_boundary.is_some_and(|(observed, first_slot)| {
                received < observed || row.slot.utc_slot < first_slot
            }) {
                continue;
            }
            row.slot_band = self.current_band.name_at_slot(
                row.slot.utc_slot,
                self.clock_boundary.map(|(observed, _)| observed),
            );
            row.slot.clock_segment = self.clock_segment;
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
            let target = if !self.preferences.udp_in_on {
                Some(None)
            } else {
                parse_port(&self.preferences.udp_in_port)
                    .ok()
                    .filter(|_| !self.preferences.udp_in_host.trim().is_empty())
                    .map(|port| Some((self.preferences.udp_in_host.trim(), port)))
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
                            self.preferences.udp_in_on = false;
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
                            if self.preferences.udp_in_on {
                                self.error = None;
                            }
                        }
                        Ok(false) => {}
                        Err(err) => {
                            self.current_band.clear();
                            self.preferences.udp_in_on = false;
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
                self.preferences.udp_in_on = false;
                self.udp_in_dirty = true;
                self.error = Some(err);
                ctx.request_repaint();
            }
        }
    }
}

impl eframe::App for Ft8rsApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.preferences.save(storage);
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
        if self.preferences.profile == DecodeProfile::Dx {
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
        self.monitor_stopped_dialog(&ctx);

        // Keep pumping engine events even without user input.
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

fn norm(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_uppercase())
    }
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
    use ft8rs::stream::session::StreamDecodeConfig;
    use ft8rs_engine::soundcard::InputChannel;

    #[test]
    fn previous_monitor_run_cannot_clear_the_current_settings_or_show_its_error() {
        let mut run = MonitorRun {
            id: 2,
            status: EngineStatus::Aligning,
            submitted_state: Some(EngineState {
                device: None,
                channel: InputChannel::Mono,
                config: StreamDecodeConfig::default(),
                udp: None,
            }),
        };
        assert!(!run.update_status(1, EngineStatus::Idle));
        assert!(!run.update_status(1, EngineStatus::Error("previous input disconnected".into())));
        assert_eq!(run.status, EngineStatus::Aligning);
        assert!(run.submitted_state.is_some());
        assert!(!run.update_status(2, EngineStatus::Monitoring));
        assert!(run.submitted_state.is_some());
        assert!(run.update_status(2, EngineStatus::Error("current input disconnected".into())));
        assert!(matches!(run.status, EngineStatus::Error(_)));
        assert!(run.submitted_state.is_none());
    }
}
