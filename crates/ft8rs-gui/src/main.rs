#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;

use app::Ft8rsApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([960.0, 760.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("ft8rs"),
        ..Default::default()
    };
    eframe::run_native(
        "ft8rs",
        options,
        Box::new(|cc| Ok(Box::new(Ft8rsApp::new(cc)))),
    )
}
