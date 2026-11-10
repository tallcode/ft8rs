//! Monitor errors, DX confirmation, and About/copyright viewports.
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use ft8rs_engine::protocol::{EngineCommand, EngineStatus};

use super::Ft8rsApp;

const DIALOG_FOCUS_ATTEMPTS: u8 = 3;
const DIALOG_FOCUS_INTERVAL: Duration = Duration::from_millis(100);

impl Ft8rsApp {
    pub(super) fn monitor_stopped_dialog(&mut self, ctx: &egui::Context) {
        let EngineStatus::Error(reason) = &self.monitor.status else {
            return;
        };
        let locale = self.locale;
        let modal = egui::Modal::new(egui::Id::unique("monitor_stopped")).show(ctx, |ui| {
            ui.set_max_width(400.0);
            ui.heading(locale.text("Audio monitoring stopped"));
            ui.label(
                locale.text("Audio input was interrupted. Check the receiver or audio device, then start monitoring again."),
            );
            ui.label(reason);
            ui.button(locale.text("OK")).clicked()
        });
        if modal.inner {
            self.monitor.status = EngineStatus::Idle;
        }
    }

    pub(super) fn confirm_window(&mut self, ctx: &egui::Context) {
        let locale = self.locale;
        let tr = |text| locale.text(text);
        if self.pending_confirm.is_none() {
            return;
        }
        let mut decided: Option<bool> = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(tr("Switch DX target")).size(15.0).strong());
            ui.add_space(8.0);
            ui.label(tr("This clears the collected DX information."));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(tr("Cancel")).clicked() {
                    decided = Some(false);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let confirm = egui::Button::new(
                        RichText::new(tr("Switch")).strong().color(Color32::WHITE),
                    )
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
                        self.monitor.submitted_state = Some(desired);
                        self.dx = None;
                    }
                }
            }
            Some(false) => {
                self.pending_confirm = None;
                if let Some(submitted) = &self.monitor.submitted_state {
                    self.preferences.hiscall = submitted.config.hiscall.clone().unwrap_or_default();
                    self.preferences.hisgrid = submitted.config.hisgrid.clone().unwrap_or_default();
                    self.preferences.profile = submitted.config.profile;
                }
            }
            None => {}
        }
    }

    pub(super) fn about_window(&mut self, ctx: &egui::Context) {
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
                .with_title(self.locale.text("About ft8.rs"))
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
                                                RichText::new(crate::APP_NAME).size(18.0).strong(),
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
                                            ui.add_space(4.0);
                                            ui.hyperlink_to("ft8.rs", ft8rs::about::HOMEPAGE);
                                            ui.hyperlink_to(
                                                "github.com/tallcode/ft8rs",
                                                ft8rs::about::SOURCE_REPOSITORY,
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
    pub(super) fn copyright_window(&mut self, ctx: &egui::Context) {
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

// Immediate viewport commands are processed by eframe after drawing and
// swapping buffers. Start dialogs hidden in their builder and reveal them with
// Visible here, so an unpainted native window never appears during creation.
/// Space focus retries in wall time so fast startup frames cannot exhaust them
/// before Windows has shown/activated the dialog. Reopening a minimized dialog
/// also needs to restore it: winit ignores Focus on invisible/minimized windows.
#[derive(Default)]
pub(super) struct DialogFocus {
    remaining: u8,
    next_attempt: Option<Instant>,
}

impl DialogFocus {
    pub(super) fn request(&mut self) {
        self.remaining = DIALOG_FOCUS_ATTEMPTS;
        self.next_attempt = None;
    }

    pub(super) fn update(&mut self, ctx: &egui::Context, now: Instant) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
