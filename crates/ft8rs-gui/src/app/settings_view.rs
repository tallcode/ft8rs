//! Settings viewport, device picker, and clock calibration controls.
use std::sync::mpsc;
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use ft8rs::stream::session::DecodeProfile;
use ft8rs_engine::protocol::EngineCommand;
use ft8rs_engine::soundcard::{InputChannel, SoundcardDeviceInfo};

use crate::clock_sync;
use crate::fonts::CHINESE_UNAVAILABLE;
use crate::i18n::{Language, Locale};

use super::decode_log::DecodeSource;
use super::decode_table::source_color;
use super::menu::{profile_label, MENU_PROFILES};
use super::preferences::SettingsTab;
use super::widgets::{callsign_field, centered_text_edit, grid_field};
use super::Ft8rsApp;

const SETTINGS_HEADING_TOP_SPACE: f32 = 2.0;
const SETTINGS_FORM_BOTTOM_SPACE: f32 = 10.0;

const SETTINGS_TABS: [(SettingsTab, &str); 7] = [
    (SettingsTab::General, "General"),
    (SettingsTab::Audio, "Audio"),
    (SettingsTab::Decode, "Decode"),
    (SettingsTab::Frequency, "Frequency"),
    (SettingsTab::Compare, "Compare"),
    (SettingsTab::Output, "Output"),
    (SettingsTab::Advanced, "Advanced"),
];

impl Ft8rsApp {
    pub(super) fn open_settings(&mut self) {
        if self.tab == SettingsTab::Audio {
            let _ = self.engine.send(EngineCommand::RefreshDevices);
        }
        self.settings_open = true;
        self.settings_focus.request();
    }

    pub(super) fn select_settings_tab(&mut self, tab: SettingsTab) {
        if self.tab != tab {
            self.tab = tab;
            if tab == SettingsTab::Audio {
                let _ = self.engine.send(EngineCommand::RefreshDevices);
            }
        }
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
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
                            if let Some(message) =
                                self.preferences.settings_validation_error(self.tab)
                            {
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
                            callsign_field(ui, &mut self.preferences.mycall, f32::INFINITY)
                                .lost_focus()
                        });
                        commit |= setting_row(ui, tr("Grid"), |ui| {
                            grid_field(ui, &mut self.preferences.mygrid, f32::INFINITY).lost_focus()
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
                            .selected_text(
                                self.font_support
                                    .language_label(self.preferences.language, locale),
                            )
                            .show_ui(ui, |ui| {
                                for language in Language::ALL {
                                    ui.selectable_value(
                                        &mut self.preferences.language,
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
                        let before = self.preferences.input_channel;
                        egui::ComboBox::from_id_salt("input_channel")
                            .width(ui.available_width())
                            .truncate()
                            .selected_text(match self.preferences.input_channel {
                                InputChannel::Mono => tr("Mono"),
                                InputChannel::Left => tr("Left"),
                                InputChannel::Right => tr("Right"),
                                InputChannel::Mix => tr("Mix (average)"),
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.preferences.input_channel,
                                    InputChannel::Mono,
                                    tr("Mono"),
                                );
                                ui.selectable_value(
                                    &mut self.preferences.input_channel,
                                    InputChannel::Left,
                                    tr("Left"),
                                );
                                ui.selectable_value(
                                    &mut self.preferences.input_channel,
                                    InputChannel::Right,
                                    tr("Right"),
                                );
                                ui.selectable_value(
                                    &mut self.preferences.input_channel,
                                    InputChannel::Mix,
                                    tr("Mix (average)"),
                                );
                            });
                        self.preferences.input_channel != before
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
                if self.preferences.input_channel == InputChannel::Mono
                    || self.preferences.input_channel == InputChannel::Mix
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
                        let before = self.preferences.profile;
                        egui::ComboBox::from_id_salt("profile")
                            .width(ui.available_width())
                            .truncate()
                            .selected_text(profile_label(self.preferences.profile))
                            .show_ui(ui, |ui| {
                                for profile in MENU_PROFILES {
                                    ui.selectable_value(
                                        &mut self.preferences.profile,
                                        profile,
                                        profile_label(profile),
                                    );
                                }
                            });
                        self.preferences.profile != before
                    });
                    if matches!(
                        self.preferences.profile,
                        DecodeProfile::Jtdx | DecodeProfile::Hybrid
                    ) {
                        commit |= setting_row(ui, "SWL", |ui| {
                            ui.add(egui::Checkbox::without_text(&mut self.preferences.swl))
                                .on_hover_text(tr("Controls both JTDX paths in Hybrid."))
                                .changed()
                        });
                    }
                    if self.preferences.profile == DecodeProfile::Hybrid {
                        commit |= setting_row(ui, tr("Exp. decoder"), |ui| {
                            ui.add(egui::Checkbox::without_text(&mut self.preferences.hybrid_experiment))
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
                    commit |= setting_row(ui, tr("Low (Hz)"), |ui| {
                        text_field(ui, &mut self.preferences.nfa)
                    });
                    commit |= setting_row(ui, tr("High (Hz)"), |ui| {
                        text_field(ui, &mut self.preferences.nfb)
                    });
                });
            }
            SettingsTab::Output => {
                section_heading(ui, tr("Output"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("UDP reports"), |ui| {
                        ui.add(egui::Checkbox::without_text(&mut self.preferences.udp_on))
                            .changed()
                    });
                    commit |= setting_row(ui, tr("Host"), |ui| {
                        text_field(ui, &mut self.preferences.udp_host)
                    });
                    commit |= setting_row(ui, tr("Port"), |ui| {
                        text_field(ui, &mut self.preferences.udp_port)
                    });
                });
            }
            SettingsTab::Compare => {
                section_heading(ui, tr("Compare"));
                settings_form(ui, |ui| {
                    commit |= setting_row(ui, tr("Listen to UDP"), |ui| {
                        ui.add(egui::Checkbox::without_text(
                            &mut self.preferences.udp_in_on,
                        ))
                        .changed()
                    });
                    commit |= setting_row(ui, tr("Host"), |ui| {
                        text_field(ui, &mut self.preferences.udp_in_host)
                    });
                    commit |= setting_row(ui, tr("Port"), |ui| {
                        text_field(ui, &mut self.preferences.udp_in_port)
                    });
                    commit |= setting_row(ui, tr("Match ± Hz"), |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.preferences.udp_in_tol)
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
                            egui::Image::new(egui::include_image!("../../assets/stop.svg"))
                                .fit_to_exact_size(egui::Vec2::splat(16.0))
                                .tint(color),
                        );
                        ui.colored_label(color, RichText::new(label).small());
                    });
                }
            }
            SettingsTab::Advanced => {
                section_heading(ui, tr("Advanced"));
                let previous_filters = self.preferences.display_filters;
                settings_form(ui, |ui| {
                    let dx = self.preferences.profile == DecodeProfile::Dx;
                    commit |= setting_row(ui, tr("Hide <...> calls"), |ui| {
                        ui.add_enabled(
                            !dx,
                            egui::Checkbox::without_text(&mut self.preferences.hide_hash),
                        )
                        .changed()
                    });
                    for (label, enabled) in [
                        (
                            "Hide contest messages",
                            &mut self.preferences.display_filters.hide_contest,
                        ),
                        (
                            "Hide telemetry",
                            &mut self.preferences.display_filters.hide_telemetry,
                        ),
                    ] {
                        setting_row(ui, tr(label), |ui| {
                            ui.add(egui::Checkbox::without_text(enabled));
                            false
                        });
                    }
                });
                if self.preferences.display_filters != previous_filters {
                    self.decode_log.refilter(self.preferences.display_filters);
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

    pub(super) fn pump_clock_sync(&mut self) {
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
        let before = self.preferences.selected_device.clone();
        let response = input_device_combo(
            ui,
            &mut self.preferences.selected_device,
            &self.devices,
            self.locale,
        );
        if response.clicked() && egui::Popup::is_id_open(ui.ctx(), response.id.with("popup")) {
            let _ = self.engine.send(EngineCommand::RefreshDevices);
        }
        self.preferences.selected_device != before
    }
}

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

#[cfg(test)]
mod tests;
