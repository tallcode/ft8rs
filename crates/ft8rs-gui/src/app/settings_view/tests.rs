use super::*;
use ft8rs_engine::soundcard::SoundcardFormatInfo;

use crate::fonts::install as install_fonts;

use super::super::apply_style;

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
                            egui::Shape::Text(text) if &text.galley.job.text == value => Some(text),
                            _ => None,
                        })
                        .unwrap();
                    let frame = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if rect.fill == Color32::WHITE && rect.rect.contains(text.pos) =>
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
                            control = callsign_field(ui, &mut String::new(), f32::INFINITY).rect;
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
