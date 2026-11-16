//! Exercise the real App::logic hook with no UI passes, as eframe does for
//! minimized/occluded windows. No audio device, socket, or native menu is opened.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::App;

use super::super::*;
use crate::wsjtx_udp::{ExternalDecode, UdpIn};

fn headless_app() -> Ft8rsApp {
    let mut preferences = Preferences::load(None);
    preferences.language = Language::English;
    let menu_synced = (preferences.profile, preferences.language, Locale::English);
    Ft8rsApp {
        debug_info: DebugInfo::new(Arc::new(Recorder::default())),
        debug_result: None,
        engine: EngineHandle::spawn(),
        clock_sync: None,
        clock_status: None,
        clock_segment: 0,
        clock_boundary: None,
        activation: Arc::default(),
        main_focus: DialogFocus::default(),
        preferences,
        locale: Locale::English,
        requested_locale: Locale::English,
        font_support: FontSupport { chinese: false },
        locale_checked_at: Instant::now(),
        devices: Vec::new(),
        input_peak: None,
        capture_xruns: 0,
        monitor: MonitorRun {
            id: 0,
            status: EngineStatus::Idle,
            submitted_state: None,
        },
        decode_log: DecodeLog::default(),
        error: None,
        dx: None,
        pending_confirm: None,
        settings_open: false,
        settings_focus: DialogFocus::default(),
        about_focus: DialogFocus::default(),
        copyright_focus: DialogFocus::default(),
        about_open: false,
        copyright_open: false,
        tab: SettingsTab::General,
        menu_events: mpsc::channel().1,
        native_menu: NativeMenu::default(),
        menu_synced,
        styled_theme: egui::Theme::Dark,
        last_title: String::new(),
        logo: None,
        udp_in: None,
        udp_in_dirty: false,
        current_band: CurrentBand::default(),
    }
}

#[test]
fn hidden_window_keeps_receiving_and_scheduling_without_ui_passes() {
    let ctx = egui::Context::default();
    let scheduled = Arc::new(AtomicBool::new(false));
    ctx.set_request_repaint_callback({
        let scheduled = Arc::clone(&scheduled);
        move |info| {
            if info.delay <= Duration::from_millis(150) {
                scheduled.store(true, Ordering::Relaxed);
            }
        }
    });
    let mut frame = eframe::Frame::_new_kittest();
    let mut app = headless_app();
    let (udp, sender) = UdpIn::test_mailbox();
    app.udp_in = Some(udp);

    // More events than the screenshot's 256,676 lost rows, over 2,577 ticks.
    // All timestamps are within the nearest-day UDP resolution window.
    const TICKS: u64 = 2577;
    const PER_TICK: usize = 100;
    const RETAINED: usize = 20_000;
    let current_slot = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 15;
    for tick in 0..TICKS {
        let slot = current_slot - TICKS + tick;
        for index in 0..PER_TICK {
            sender
                .try_send((
                    Instant::now(),
                    ExternalDecode {
                        ms_since_midnight: (slot % 5760 * 15_000) as u32,
                        snr: -10,
                        dt: 0.0,
                        freq: 1000 + index as u32 * 10,
                        message: "CQ K1ABC FN42".into(),
                    },
                ))
                .expect("hidden-window reception must not fill the mailbox");
        }
        let expected = ((tick as usize + 1) * PER_TICK).min(RETAINED);
        let mut input = egui::RawInput::default();
        let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
        viewport.minimized = Some(tick < TICKS / 2);
        viewport.occluded = Some(tick >= TICKS / 2);
        // The 2 ms cooperative budget may need more than one tick on a loaded
        // machine. Every logic-only tick must still request its successor.
        for attempt in 0..100 {
            scheduled.store(false, Ordering::Relaxed);
            let _ = ctx.run_logic(&input, |ctx| app.logic(ctx, &mut frame));
            assert!(scheduled.load(Ordering::Relaxed), "background wakeup lost");
            let rows = app.decode_log.display().0;
            if rows.len() == expected
                && rows
                    .last()
                    .is_some_and(|row| row.slot.utc_slot == slot && row.freq == 1990.0)
            {
                break;
            }
            assert!(attempt < 99, "decode mailbox did not drain while hidden");
        }
        assert!(app.error.is_none());
    }
    assert!(app.udp_in.as_ref().unwrap().try_recv_timed().is_none());
    let rows = app.decode_log.display().0;
    assert_eq!(rows.len(), RETAINED);
    assert_eq!(rows.first().unwrap().slot.utc_slot, current_slot - 200);
    assert_eq!(rows.last().unwrap().slot.utc_slot, current_slot - 1);

    // Restoring the window paints the existing log without replaying the feed.
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
        app.logic(ui.ctx(), &mut frame);
        app.ui(ui, &mut frame);
    });
    assert_eq!(app.decode_log.display().0.len(), RETAINED);
    assert!(app.error.is_none());
}

#[test]
fn hidden_window_processes_engine_and_clock_worker_updates() {
    let ctx = egui::Context::default();
    let mut app = headless_app();
    let mut frame = eframe::Frame::_new_kittest();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .get_mut(&egui::ViewportId::ROOT)
        .unwrap()
        .minimized = Some(true);
    let (updates, receiver) = mpsc::channel();
    app.clock_sync = Some(receiver);
    updates
        .send(clock_sync::Update::Failed(clock_sync::Error::System(
            "test clock worker completed".into(),
        )))
        .unwrap();
    // Exercise the real engine mailbox without opening a capture device.
    let now = Instant::now();
    app.engine
        .send(EngineCommand::ClockAdjusted {
            started: now,
            completed: now,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let _ = ctx.run_logic(&input, |ctx| app.logic(ctx, &mut frame));
        if app.clock_segment == 1 && app.clock_sync.is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "background workers not consumed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(
        app.clock_status,
        Some(clock_sync::Update::Failed(clock_sync::Error::System(ref error)))
            if error == "test clock worker completed"
    ));
    assert!(app.clock_boundary.is_some());
}
