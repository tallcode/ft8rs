#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use app::Ft8rsApp;
use eframe::egui;

/// The app logo, embedded so it works without an external file (used both for the
/// OS window/dock/taskbar icon and the About dialog).
pub(crate) const LOGO_PNG: &[u8] = include_bytes!("../assets/ft8rs.png");

/// Decode the embedded logo into an OS window icon (RGBA). Returns None if the
/// PNG can't be decoded (keeps startup robust).
fn load_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(LOGO_PNG).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([820.0, 760.0])
        .with_min_inner_size([480.0, 440.0])
        .with_title("FT8.RS");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "ft8rs",
        options,
        Box::new(|cc| Ok(Box::new(Ft8rsApp::new(cc)))),
    )
}
