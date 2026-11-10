//! Main-window controls and shared input widgets.
use eframe::egui::{self, Color32, RichText};
use ft8rs_engine::protocol::{EngineStatus, HisgridSource};

use super::decode_table::DECODE_FONT_SIZE;
use super::widgets::{callsign_field, grid_field};
use super::{norm, Ft8rsApp};

impl Ft8rsApp {
    pub(super) fn dx_panel(&mut self, ui: &mut egui::Ui) {
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

    pub(super) fn bottom_bar(&mut self, ui: &mut egui::Ui) {
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
            let old_call = norm(&self.preferences.hiscall);
            let r =
                callsign_field(ui, &mut self.preferences.hiscall, 110.0).labelled_by(call_label.id);
            if r.changed() && norm(&self.preferences.hiscall) != old_call {
                self.preferences.hisgrid.clear();
            }
            commit |= r.lost_focus();
            let ctrl_h = r.rect.height();
            let grid_label = ui.label(tr("Grid"));
            commit |= grid_field(ui, &mut self.preferences.hisgrid, 46.0)
                .labelled_by(grid_label.id)
                .lost_focus();
            let rx_label = ui.label(tr("RX"));
            let mut frequency = self.preferences.nfqso.parse::<u32>().unwrap_or(0);
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
                self.preferences.nfqso = frequency.to_string();
                commit = true;
            }
            ui.label("Hz");

            // Right-aligned state button — square, height matched to the row.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (icon, fill) = match self.monitor.status {
                    EngineStatus::Monitoring => (
                        egui::include_image!("../../assets/stop.svg"),
                        Color32::from_rgb(0x16, 0xa3, 0x4a),
                    ),
                    EngineStatus::Aligning => (
                        egui::include_image!("../../assets/stop.svg"),
                        Color32::from_rgb(0xd9, 0x77, 0x06),
                    ),
                    EngineStatus::Idle | EngineStatus::Error(_) => (
                        egui::include_image!("../../assets/play.svg"),
                        Color32::from_rgb(0x25, 0x63, 0xeb),
                    ),
                };
                let enabled = !matches!(self.monitor.status, EngineStatus::Error(_))
                    && (self.is_monitoring() || !self.dx_needs_hiscall());
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
}

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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_monitor_icons_have_centered_visual_weight_at_multiple_resolutions() {
        for bytes in [
            include_bytes!("../../assets/play.svg").as_slice(),
            include_bytes!("../../assets/stop.svg").as_slice(),
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
                    egui::include_image!("../../assets/play.svg"),
                    egui::include_image!("../../assets/stop.svg"),
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
