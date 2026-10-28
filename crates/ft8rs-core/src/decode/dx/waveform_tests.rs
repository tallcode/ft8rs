//! Known-truth regressions generated with unchanged native packing/waveforms.
//! Optimized manual gates run locally; no WAV blobs or external probe is needed.
use super::*;
use crate::decode::{
    gen_ft8wave::gen_ft8wave, genft8::get_ft8_tones_from_77bits, packjt77::pack77,
};

const MESSAGES: [&str; 8] = [
    "K1JT BG5ATV PM00",
    "K1JT BG5ATV -04",
    "K1JT BG5ATV R-07",
    "K1JT BG5ATV RR73",
    "K1JT BG5ATV 73",
    "CQ BG5ATV PM00",
    "K1JT BG5ATV -16",
    "K1JT BG5ATV R-12",
];
const LEVELS: [f64; 8] = [-10.0, -18.0, -20.0, -22.0, -24.0, -25.0, -26.0, -27.0];

fn uniform(state: &mut u64) -> f64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (((state.wrapping_mul(2685821657736338717) >> 11) as f64) + 0.5) / (1u64 << 53) as f64
}
fn inject(audio: &mut [f64], message: &str, freq: f64, dt: f64, snr: f64, phase: f64) {
    let tones = get_ft8_tones_from_77bits(&pack77(message));
    let (re, im) = gen_ft8wave(&tones, freq);
    let power = re.iter().map(|v| v * v).sum::<f64>() / re.len() as f64;
    let amplitude = ((2500.0 / 6000.0) * 10f64.powf(snr / 10.0) / power).sqrt();
    let start = ((dt + 0.5) * 12000.0).round() as isize;
    for i in 0..re.len() {
        let dest = start + i as isize;
        if dest >= 0 && (dest as usize) < audio.len() {
            audio[dest as usize] += amplitude * (re[i] * phase.cos() - im[i] * phase.sin());
        }
    }
}
fn samples(root_seed: u64, slot: usize, pump: bool, target_present: bool) -> Vec<f32> {
    let mut state = root_seed + slot as u64 * 10007;
    let mut audio = Vec::with_capacity(180000);
    for _ in 0..90000 {
        let r = (-2.0 * uniform(&mut state).ln()).sqrt();
        let angle = std::f64::consts::TAU * uniform(&mut state);
        audio.push(r * angle.cos());
        audio.push(r * angle.sin());
    }
    if target_present && slot.is_multiple_of(2) {
        let i = slot / 2;
        let freq = 1000.0 + [0.0, 0.3, -0.4, 0.6, -0.2, 0.8, -0.6, 1.0][i];
        let dt = [0.2, 0.2, 0.4, 0.1, 0.3, 0.5, -0.1, 0.2][i];
        inject(
            &mut audio,
            MESSAGES[i],
            freq,
            dt,
            LEVELS[i],
            0.2 + i as f64 * 0.41,
        );
    }
    inject(&mut audio, "CQ K1ABC FN42", 1025.0, 0.2, -5.0, 0.7);
    inject(&mut audio, "CQ W9XYZ EN50", 970.0, 0.1, -9.0, 1.3);
    inject(&mut audio, "CQ G4AAA IO91", 1800.0, 0.4, -4.0, 2.1);
    audio
        .into_iter()
        .enumerate()
        .map(|(i, mut x)| {
            if pump && (4.0..8.0).contains(&(i as f64 / 12000.0)) {
                x *= 0.18;
            }
            (x * 0.05) as f32
        })
        .collect()
}
fn config(operator: Option<&str>) -> StreamDecodeConfig {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        nfqso: 1000.0,
        mycall: operator.map(str::to_string),
        hiscall: Some("BG5ATV".into()),
        // Canonical file search has no wall-clock-dependent start gate.
        cooperative_realtime: false,
        ..Default::default()
    }
}
fn replay(
    root: u64,
    pump: bool,
    operator: Option<&str>,
    present: bool,
    grid: bool,
) -> Vec<(usize, String)> {
    let mut config = config(operator);
    if grid {
        config.hisgrid = Some("PM00".into());
    }
    let mut session = DxStreamDecodeSession::new(config);
    let mut decoded = Vec::new();
    let start = SlotTimestamp::parse("261028_120000").unwrap();
    for slot in 0..16 {
        let rows = session.decode_slot_at(
            &start.add_seconds(slot as i64 * 15),
            &samples(root, slot, pump, present),
        );
        assert!(session.last_diagnostics().augmentation_errors.is_empty());
        for row in rows {
            assert!(
                present && slot.is_multiple_of(2),
                "phantom target in slot {slot}: {}",
                row.msg
            );
            assert_eq!(
                row.msg,
                MESSAGES[slot / 2],
                "wrong target content in slot {slot}"
            );
            decoded.push((slot, row.msg));
        }
    }
    decoded
}

#[test]
#[ignore = "local optimized known-truth DX recovery gate"]
fn controlled_target_recovery_keeps_seven_verified_gains() {
    for (seed, pump, count) in [
        (2026102801, false, 5),
        (2026102801, true, 5),
        (2026102809, false, 4),
        (2026102809, true, 4),
    ] {
        let rows = replay(seed, pump, Some("K1JT"), true, false);
        assert_eq!(rows.len(), count, "seed={seed}, pump={pump}");
    }
}
#[test]
#[ignore = "local optimized incomplete-context and negative-signal DX gate"]
fn partial_context_and_known_grid_do_not_invent_target_messages() {
    let rows = replay(2026102809, false, None, true, false);
    assert_eq!(rows.len(), 4);
    assert!(replay(2026102879, false, Some("K1JT"), false, true).is_empty());
}
