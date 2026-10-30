#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod diagnostics;
mod wsjtx_udp;

use app::Ft8rsApp;
use eframe::egui;
use std::cell::Cell;
use std::rc::Rc;

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
    if ft8rs_engine::run_decode_worker_if_requested() {
        return Ok(());
    }
    let log_path = diagnostics::init();
    let renderer = match std::env::var("FT8RS_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        Ok("wgpu") | Err(_) => eframe::Renderer::Wgpu,
        Ok(other) => {
            log::warn!("Unknown FT8RS_RENDERER={other:?}; using wgpu");
            eframe::Renderer::Wgpu
        }
    };
    log::info!(
        "Starting FT8.RS {} on {} / {} (renderer: {renderer:?})",
        env!("FT8RS_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    let result = run_with_fallback(renderer, run_gui);
    if let Err(error) = &result {
        diagnostics::report_failure(&error.to_string(), log_path.as_deref());
    } else {
        log::info!("Application closed normally");
    }
    result
}

fn run_with_fallback(
    renderer: eframe::Renderer,
    mut run: impl FnMut(eframe::Renderer, Rc<Cell<bool>>) -> eframe::Result<()>,
) -> eframe::Result<()> {
    let app_creation_started = Rc::new(Cell::new(false));
    let mut result = run(renderer, Rc::clone(&app_creation_started));
    // Retry only a failed graphics/window initialization, before the app or its
    // engine has started. A normal close or runtime error must not reopen it.
    if renderer == eframe::Renderer::Wgpu && !app_creation_started.get() {
        if let Err(error) = &result {
            log::error!("wgpu initialization failed: {error}; retrying with OpenGL");
            result = run(eframe::Renderer::Glow, app_creation_started);
        }
    }
    result
}

fn run_gui(renderer: eframe::Renderer, app_creation_started: Rc<Cell<bool>>) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([820.0, 760.0])
        .with_min_inner_size([480.0, 440.0])
        .with_title("FT8.RS");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let mut options = eframe::NativeOptions {
        viewport,
        renderer,
        ..Default::default()
    };
    if renderer == eframe::Renderer::Wgpu {
        configure_wgpu(
            &mut options,
            cfg!(windows),
            eframe::wgpu::Backends::from_env(),
        );
    }
    log::info!("Initializing native window and graphics adapter: {renderer:?}");
    eframe::run_native(
        "ft8rs",
        options,
        Box::new(move |cc| {
            log::info!("Graphics initialized; creating application");
            app_creation_started.set(true);
            let app = Ft8rsApp::new(cc);
            log::info!("Application initialized");
            Ok(Box::new(app))
        }),
    )
}

fn configure_wgpu(
    options: &mut eframe::NativeOptions,
    windows: bool,
    backend_override: Option<eframe::wgpu::Backends>,
) {
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        // Enumerating every backend can enter a broken Vulkan driver before
        // wgpu selects DX12. A Windows 11 / i5-9600T launch was restored by
        // WGPU_BACKEND=dx12. Restrict enumeration itself, while honoring users'
        // explicit backend overrides and other platforms' existing defaults.
        setup.instance_descriptor.backends = backend_override.unwrap_or(if windows {
            eframe::wgpu::Backends::DX12
        } else {
            setup.instance_descriptor.backends
        });
        log::info!("wgpu backends: {:?}", setup.instance_descriptor.backends);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_defaults_to_dx12_and_honors_explicit_backend_overrides() {
        for (windows, explicit) in [
            (true, None),
            (true, Some(eframe::wgpu::Backends::VULKAN)),
            (false, None),
        ] {
            let mut options = eframe::NativeOptions::default();
            let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_options.wgpu_setup
            else {
                panic!("native options must create a new graphics instance");
            };
            let original = setup.instance_descriptor.backends;
            configure_wgpu(&mut options, windows, explicit);
            let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &options.wgpu_options.wgpu_setup
            else {
                unreachable!();
            };
            assert_eq!(
                setup.instance_descriptor.backends,
                explicit.unwrap_or(if windows {
                    eframe::wgpu::Backends::DX12
                } else {
                    original
                })
            );
        }
    }

    #[test]
    fn fallback_retries_only_before_app_creation() {
        for (initial, started, succeeds, expected) in [
            (eframe::Renderer::Wgpu, false, false, 2),
            (eframe::Renderer::Wgpu, true, false, 1),
            (eframe::Renderer::Wgpu, true, true, 1),
            (eframe::Renderer::Glow, false, false, 1),
        ] {
            let mut attempts = Vec::new();
            let result = run_with_fallback(initial, |renderer, app_started| {
                attempts.push(renderer);
                app_started.set(started);
                if succeeds || attempts.len() == 2 {
                    Ok(())
                } else {
                    Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                        "simulated startup failure",
                    ))))
                }
            });
            assert_eq!(attempts.len(), expected);
            assert_eq!(result.is_ok(), succeeds || expected == 2);
            if expected == 2 {
                assert_eq!(attempts, [eframe::Renderer::Wgpu, eframe::Renderer::Glow]);
            }
        }
    }
}
