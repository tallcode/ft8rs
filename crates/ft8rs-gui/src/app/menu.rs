//! Native menus, their Linux fallback, and menu event handling.
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;
use ft8rs::stream::session::DecodeProfile;

use crate::debug_info;
use crate::i18n::{Language, Locale};

use super::Ft8rsApp;

impl Ft8rsApp {
    pub(super) fn pump_menu(&mut self) {
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
                "debug:collect" if debug_info::eligible(&self.preferences.mycall) => {
                    self.set_debug_collection(!self.debug_info.active());
                }
                "debug:save" => self.save_debug_info(),
                "debug:open" => self.open_debug_directory(),
                id => {
                    if let Some(name) = id.strip_prefix("language:") {
                        self.preferences.language = Language::parse(name);
                        selected = true;
                    } else if let Some(name) = id.strip_prefix("profile:") {
                        if let Ok(profile) = DecodeProfile::parse(name) {
                            selected = true;
                            if self.preferences.profile != profile {
                                self.preferences.profile = profile;
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
            self.native_menu.sync(
                self.preferences.profile,
                self.preferences.language,
                self.locale,
            );
        }
    }

    fn set_debug_collection(&mut self, enabled: bool) {
        self.debug_info
            .set_active(enabled && debug_info::eligible(&self.preferences.mycall));
        self.debug_result = None;
    }

    fn save_debug_info(&mut self) {
        if debug_info::eligible(&self.preferences.mycall) && self.debug_info.can_save() {
            let udp_listener = self.preferences.udp_in_on.then(|| {
                format!(
                    "{}:{}",
                    self.preferences.udp_in_host.trim(),
                    self.preferences.udp_in_port.trim()
                )
            });
            self.debug_result = None;
            if let Err(err) = self.debug_info.save(udp_listener) {
                self.debug_save_failed(&err);
            }
        }
    }

    pub(super) fn debug_save_failed(&mut self, err: &str) {
        let label = self.locale.text("Unable to save debug information");
        self.debug_result = Some(Err(format!("{label}: {err}")));
    }

    fn open_debug_directory(&mut self) {
        if debug_info::eligible(&self.preferences.mycall) {
            self.debug_result = None;
            if let Err(err) = self.debug_info.open_directory() {
                let label = self.locale.text("Unable to open debug information folder");
                self.debug_result = Some(Err(format!("{label}: {err}")));
            }
        }
    }

    pub(super) fn sync_debug_menu(&mut self) {
        let eligible = debug_info::eligible(&self.preferences.mycall);
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
    pub(super) fn sync_language_and_menu(&mut self) {
        if self.preferences.language != self.menu_synced.1
            || (self.preferences.language == Language::Auto
                && self.locale_checked_at.elapsed() >= Duration::from_secs(2))
        {
            self.requested_locale = self.preferences.language.resolve_system();
            self.locale = self.font_support.locale(self.requested_locale);
            self.locale_checked_at = Instant::now();
        }
        let current = (
            self.preferences.profile,
            self.preferences.language,
            self.locale,
        );
        if self.menu_synced != current {
            self.native_menu.sync(
                self.preferences.profile,
                self.preferences.language,
                self.locale,
            );
            self.menu_synced = current;
        }
    }

    // In-window menu fallback for platforms without a native menu (Linux). macOS
    // and Windows attach a native menu bar instead.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    pub(super) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        ui.horizontal(|ui| {
            if ui.button(tr("Settings")).clicked() {
                self.open_settings();
            }
            ui.menu_button(tr("Profile"), |ui| {
                for profile in MENU_PROFILES {
                    if ui
                        .selectable_label(
                            self.preferences.profile == profile,
                            profile_label(profile),
                        )
                        .clicked()
                    {
                        self.preferences.profile = profile;
                        self.apply_if_changed();
                        ui.close();
                    }
                }
            });
            ui.menu_button(tr("Language"), |ui| {
                for language in Language::ALL {
                    if ui
                        .selectable_value(
                            &mut self.preferences.language,
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
            if debug_info::eligible(&self.preferences.mycall) {
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
}

pub(super) fn initialize(
    cc: &eframe::CreationContext<'_>,
    profile: DecodeProfile,
    language: Language,
    locale: Locale,
    debug_eligible: bool,
) -> (Receiver<muda::MenuEvent>, NativeMenu) {
    // Install the menu while eframe still keeps the root hidden. Preserve
    // its physical client size: SetMenu consumes client height, whereas
    // winit knows how to resize the outer window for a menu-bearing window.
    let menu_events = native_menu_events(&cc.egui_ctx);
    let hwnd = window_hwnd(cc);
    #[cfg(target_os = "windows")]
    let client_size = cc.winit_window().map(|window| window.inner_size());
    let native_menu = install_menu(profile, language, locale, hwnd);
    native_menu.sync_debug(locale, debug_eligible, false, false);
    #[cfg(target_os = "windows")]
    if let (Some(window), Some(size)) = (cc.winit_window(), client_size) {
        if !window.is_maximized() {
            let _ = window.request_inner_size(size);
        }
    }
    (menu_events, native_menu)
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

/// UI labels are independent of the stable lowercase storage and menu IDs.
pub(super) fn profile_label(profile: DecodeProfile) -> &'static str {
    match profile {
        DecodeProfile::Wsjtx => "WSJT-X",
        DecodeProfile::WsjtxPlus => "WSJT-X+",
        DecodeProfile::Jtdx => "JTDX",
        DecodeProfile::Hybrid => "Hybrid",
        DecodeProfile::Dx => "DX",
    }
}

/// Profile names stay the same in both interface languages.
pub(super) const MENU_PROFILES: [DecodeProfile; 5] = [
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
pub(super) struct NativeMenu {
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
                Err(err) => log::warn!(target: "ft8rs::app", "Developer menu visibility: {err}"),
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
