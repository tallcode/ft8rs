//! Virtualized decode table and message highlighting.
use eframe::egui::{self, Color32};

use super::decode_log::{
    call_candidate, fill_target_context, is_grid4, separator_label, DecodeSource, DisplayItem,
};
use super::widgets::cap_height_sample;
use super::{norm, Ft8rsApp};

/// Slightly larger than WSJT-X's default 10 pt (~13.33 logical pixels at
/// Windows' 96-DPI baseline), for easier reading in the receive log.
pub(super) const DECODE_FONT_SIZE: f32 = 14.0;
/// Breathing room above and below each decode line, in logical pixels.
const DECODE_ROW_PADDING_Y: f32 = 1.0;
/// Internal left padding for the decode table (header + rows go edge-to-edge).
const TABLE_PAD: f32 = 12.0;

impl Ft8rsApp {
    /// Pixel height of one decode line (used to size the header panel and rows).
    pub(super) fn row_height(ctx: &egui::Context) -> f32 {
        ctx.fonts_mut(|f| f.row_height(&egui::FontId::monospace(DECODE_FONT_SIZE)))
            + 2.0 * DECODE_ROW_PADDING_Y
    }

    /// The column header, drawn in its own fixed top panel: vertically-centered
    /// text plus a single divider line at the panel's bottom edge.
    pub(super) fn table_header(&self, ui: &mut egui::Ui) {
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
    pub(super) fn decode_rows(&mut self, ui: &mut egui::Ui) {
        let font = egui::FontId::monospace(DECODE_FONT_SIZE);
        let row_h = Self::row_height(ui.ctx());
        let dark = ui.visuals().dark_mode;
        let text_color = ui.visuals().text_color();
        let ink_center_y = cap_height_sample(ui.painter(), &font)
            .mesh_bounds
            .center()
            .y;
        let own_call = norm(&self.preferences.mycall);
        let target_call = norm(&self.preferences.hiscall);
        let target_grid = norm(&self.preferences.hisgrid);
        // Match the WSJT-X detailed separator in both themes.
        let separator_bg = Color32::from_gray(if dark { 0xa2 } else { 0xd3 });
        // Compare-mode coloring only applies while listening.
        let comparing = self.preferences.udp_in_on;
        // Row interactions, collected inside the (immutably borrowed) closure and
        // applied after it: double-click fills the bottom-bar QSO context, the
        // right-click menu clears the list.
        let mut fill: Option<(Vec<String>, f64)> = None;
        let mut clear = false;
        let (rows, display_items) = self.decode_log.display();
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
                        DisplayItem::Separator(slot, band) => {
                            paint_decode_separator(
                                ui.painter(),
                                rect,
                                separator_label(slot.utc_slot, *band),
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
            self.decode_log.clear();
        }
        if let Some((tokens, freq)) = fill {
            self.apply_double_click_fill(&tokens, freq);
        }
    }

    /// Double-clicking a decode row loads its QSO context into the bottom bar:
    /// the DX callsign, its grid (when the message carries one), and the RX
    /// frequency, then applies it to a running session (like editing the fields).
    fn apply_double_click_fill(&mut self, tokens: &[String], freq: f64) {
        fill_target_context(
            &mut self.preferences.hiscall,
            &mut self.preferences.hisgrid,
            tokens,
        );
        self.preferences.nfqso = format!("{}", freq.round() as i64);
        self.apply_if_changed();
    }
}

struct DecodeHighlight {
    bytes: std::ops::Range<usize>,
    background: Color32,
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

/// Compare-mode row color: external-only = sky, local-only = rose (tailwind,
/// lighter -300 on dark themes / darker -900 on light), both = default.
pub(super) fn source_color(source: DecodeSource, dark: bool, default: Color32) -> Color32 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
