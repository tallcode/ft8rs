//! Shared callsign and grid input fields.
use eframe::egui::{self, Color32};

pub(super) fn cap_height_sample(
    painter: &egui::Painter,
    font: &egui::FontId,
) -> std::sync::Arc<egui::Galley> {
    // A stable reference for callsigns, grid locators, and numeric fields.
    painter.layout_no_wrap("H0".into(), font.clone(), Color32::PLACEHOLDER)
}

pub(super) fn centered_text_edit(
    ui: &mut egui::Ui,
    value: &mut String,
    width: f32,
) -> egui::Response {
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
pub(super) fn callsign_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
    let resp = centered_text_edit(ui, value, width);
    if resp.changed() && value.chars().any(|c| c.is_ascii_lowercase()) {
        *value = value.to_ascii_uppercase();
    }
    resp
}

/// A Maidenhead grid field constrained to AA00: positions 0–1 uppercase letters,
/// 2–3 digits, max 4 chars. Returns the Response.
pub(super) fn grid_field(ui: &mut egui::Ui, value: &mut String, width: f32) -> egui::Response {
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
#[cfg(test)]
mod tests {
    use super::*;

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
}
