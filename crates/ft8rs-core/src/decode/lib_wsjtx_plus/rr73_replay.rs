//! Full-slot diagnostic using the DX synthetic fixture and frozen native payloads.
use super::session::StreamDecodeSession;
use crate::decode::dx::DxStreamDecodeSession;
use crate::decode::{gen_ft8wave::gen_ft8wave, genft8::get_ft8_tones_from_77bits};
use crate::stream::session::StreamDecodeConfig;
use crate::{DecodeProfile, SlotTimestamp};

use super::{rr73_checkpoints, Row};
use crate::stream::session::StreamDecodeProvenance;
use std::time::{Duration, Instant};

const TARGET: &str = "K1JT BG5ATV RR73";
const TARGET_FREQUENCY: f64 = 1000.6;

fn native_codeword(label: &str) -> Vec<u8> {
    rr73_checkpoints()
        .into_iter()
        .find(|row| row.label == label)
        .expect("missing frozen codeword")
        .native_codeword
}

const MESSAGES: [&str; 8] = [
    "K1JT BG5ATV PM00",
    "K1JT BG5ATV -04",
    "K1JT BG5ATV R-07",
    TARGET,
    "K1JT BG5ATV 73",
    "CQ BG5ATV PM00",
    "K1JT BG5ATV -16",
    "K1JT BG5ATV R-12",
];
const LEVELS: [f64; 8] = [-10.0, -18.0, -20.0, -22.0, -24.0, -25.0, -26.0, -27.0];

fn corpus() -> &'static str {
    include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/decoder/data/ft8/waveforms/dx-current-payloads.tsv"
    ))
}

fn uniform(state: &mut u64) -> f64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (((state.wrapping_mul(2685821657736338717) >> 11) as f64) + 0.5) / (1u64 << 53) as f64
}
fn frozen_tones(message: &str, legacy: bool) -> [i32; 79] {
    if legacy && message == TARGET {
        return crate::decode::genft8::get_ft8_tones_from_codeword(&native_codeword(
            "legacy-ideal",
        ));
    }
    let mut row = corpus()
        .lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| line.strip_prefix(message)?.strip_prefix('\t'))
        .expect("missing frozen native fixture")
        .split('\t');
    let bits: Vec<_> = row.next().unwrap().bytes().map(|b| b - b'0').collect();
    let tones = get_ft8_tones_from_77bits(&bits);
    let expected: Vec<_> = row
        .next()
        .unwrap()
        .bytes()
        .map(|b| i32::from(b - b'0'))
        .collect();
    assert_eq!(tones.as_slice(), expected.as_slice());
    tones
}

fn inject(audio: &mut [f64], tones: &[i32; 79], freq: f64, dt: f64, snr: f64, phase: f64) {
    let (re, im) = gen_ft8wave(tones, freq);
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
fn slot_noise(mut state: u64) -> Vec<f64> {
    let mut audio = Vec::with_capacity(180000);
    for _ in 0..90000 {
        let r = (-2.0 * uniform(&mut state).ln()).sqrt();
        let angle = std::f64::consts::TAU * uniform(&mut state);
        audio.push(r * angle.cos());
        audio.push(r * angle.sin());
    }
    audio
}

fn samples(
    root_seed: u64,
    slot: usize,
    pump: bool,
    target_present: bool,
    legacy: bool,
) -> Vec<f32> {
    let mut audio = slot_noise(root_seed + slot as u64 * 10007);
    if target_present && slot.is_multiple_of(2) {
        let i = slot / 2;
        let freq = 1000.0 + [0.0, 0.3, -0.4, 0.6, -0.2, 0.8, -0.6, 1.0][i];
        let dt = [0.2, 0.2, 0.4, 0.1, 0.3, 0.5, -0.1, 0.2][i];
        inject(
            &mut audio,
            &frozen_tones(MESSAGES[i], legacy),
            freq,
            dt,
            LEVELS[i],
            0.2 + i as f64 * 0.41,
        );
    }
    inject(
        &mut audio,
        &frozen_tones("CQ K1ABC FN42", false),
        1025.0,
        0.2,
        -5.0,
        0.7,
    );
    inject(
        &mut audio,
        &frozen_tones("CQ W9XYZ EN50", false),
        970.0,
        0.1,
        -9.0,
        1.3,
    );
    inject(
        &mut audio,
        &frozen_tones("CQ G4AAA IO91", false),
        1800.0,
        0.4,
        -4.0,
        2.1,
    );
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

// Match the protected DX fixture recipe, keeping its decoder and fixture
// source unchanged. Only slot 6 switches to the frozen legacy codeword.
#[test]
#[ignore = "serial optimized full-slot legacy DX compatibility gate"]
fn replay_legacy_dx_focus() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let start = SlotTimestamp::parse("261028_120000").unwrap();
    let config = StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        nfqso: 1000.0,
        mycall: Some("K1JT".into()),
        hiscall: Some("BG5ATV".into()),
        cooperative_realtime: false,
        ..Default::default()
    };
    let mut dx = DxStreamDecodeSession::new(config.clone());
    let mut dx_targets = Vec::new();
    for slot in 0..16 {
        let timestamp = start.add_seconds(slot as i64 * 15);
        let audio = samples(2026102801, slot, true, true, true);
        if slot == 6 {
            let snapshot = dx.context_snapshot();
            assert_eq!(snapshot.foci, [1000.0]);
            assert_eq!(snapshot.hisgrid.as_deref(), Some("PM00"));
            let wide = StreamDecodeConfig {
                profile: DecodeProfile::WsjtxPlus,
                nfqso: snapshot.foci[0],
                hisgrid: snapshot.hisgrid,
                ..config.clone()
            };
            let decode = |mut cfg: StreamDecodeConfig, compatibility, audio: &[f32]| {
                cfg.wsjtx_plus_compatibility = compatibility;
                let mut session = StreamDecodeSession::new(cfg);
                let mut callbacks = Vec::new();
                let started = Instant::now();
                let rows = session
                    .decode_slot_streaming_tagged_at(&timestamp, audio, |row| {
                        callbacks.push((Row::from(&row.decode), row.provenance, started.elapsed()));
                        Ok(())
                    })
                    .unwrap();
                let elapsed = started.elapsed();
                assert!(elapsed <= Duration::from_secs(10));
                assert_eq!(rows.len(), callbacks.len());
                assert!(rows
                    .iter()
                    .zip(&callbacks)
                    .all(|(row, callback)| Row::from(row) == callback.0));
                (callbacks, elapsed)
            };

            let narrow = StreamDecodeConfig {
                nfa: 975.0,
                nfb: 1025.0,
                ..wide.clone()
            };
            let (narrow, _) = decode(narrow, true, &audio);
            assert!(narrow.iter().all(|row| row.0.msg != TARGET));
            assert!(narrow.iter().all(|row| !row.0.msg.contains("W9XYZ")));

            // Reopen only the lower edge. The 1800 Hz interferer remains
            // outside the scan, isolating the missing 970 Hz cancellation.
            let lower_edge = StreamDecodeConfig {
                nfa: 960.0,
                nfb: 1025.0,
                ..wide.clone()
            };
            let (lower_edge, _) = decode(lower_edge, true, &audio);
            assert!(lower_edge.iter().any(|row| row.0.msg == "CQ W9XYZ EN50"));
            assert!(lower_edge.iter().all(|row| !row.0.msg.contains("G4AAA")));
            assert!(
                lower_edge
                    .iter()
                    .any(|row| row.0.msg == TARGET
                        && row.1 == StreamDecodeProvenance::ContextRecovery)
            );

            let (base, _) = decode(wide.clone(), false, &audio);
            assert!(base.iter().any(|row| row.0.msg == "CQ W9XYZ EN50"));
            let (compatible, elapsed) = decode(wide.clone(), true, &audio);
            assert_eq!(compatible.len(), base.len() + 1);
            for (expected, actual) in base.iter().zip(&compatible) {
                assert_eq!((&expected.0, expected.1), (&actual.0, actual.1));
            }
            let (recovery, provenance, callback) = &compatible[base.len()];
            assert_eq!(recovery.msg, TARGET);
            assert_eq!(*provenance, StreamDecodeProvenance::ContextRecovery);
            let legacy_tones = frozen_tones(TARGET, true);
            assert_eq!(recovery.tones, legacy_tones);
            eprintln!(
                "Legacy wide recovery: first base callback {:?}, recovery {:?}, slot {:?}",
                compatible[0].2, callback, elapsed
            );

            let without_target = samples(2026102801, slot, true, false, true);
            let (negative, _) = decode(wide, true, &without_target);
            assert!(negative
                .iter()
                .all(|row| row.1 != StreamDecodeProvenance::ContextRecovery
                    && !row.0.msg.contains("BG5ATV")));
        }
        for row in dx.decode_slot_at(&timestamp, &audio) {
            dx_targets.push((slot, row.msg));
        }
    }
    let expected: Vec<_> = [0, 2, 4, 8]
        .into_iter()
        .map(|slot| (slot, MESSAGES[slot / 2].to_string()))
        .collect();
    assert_eq!(dx_targets, expected);
}

pub(in crate::decode::lib_wsjtx_plus) struct SoftObservation {
    pub(in crate::decode::lib_wsjtx_plus) freq: f64,
    pub(in crate::decode::lib_wsjtx_plus) time: isize,
    pub(in crate::decode::lib_wsjtx_plus) metric: usize,
    pub(in crate::decode::lib_wsjtx_plus) baseline: bool,
    pub(in crate::decode::lib_wsjtx_plus) nsync: usize,
    pub(in crate::decode::lib_wsjtx_plus) llr: Vec<f64>,
}

thread_local! {
    static SOFT: std::cell::RefCell<Option<Vec<SoftObservation>>> = const { std::cell::RefCell::new(None) };
    static TERMINAL: std::cell::RefCell<Option<Vec<f64>>> = const { std::cell::RefCell::new(None) };
}

pub(in crate::decode::lib_wsjtx_plus) fn record_terminal_residual(residual: &[f64]) {
    if SOFT.with_borrow(Option::is_some) {
        TERMINAL.with_borrow_mut(|capture| *capture = Some(residual.to_vec()));
    }
}

pub(in crate::decode::lib_wsjtx_plus) fn watch_soft(freq: f64) -> bool {
    (freq - TARGET_FREQUENCY).abs() <= 2.0 && SOFT.with_borrow(Option::is_some)
}

pub(in crate::decode::lib_wsjtx_plus) fn record_soft(row: SoftObservation) {
    SOFT.with_borrow_mut(|state| state.as_mut().expect("diagnostic active").push(row));
}

#[test]
#[ignore = "serial optimized current RR73 soft-information and FEC diagnostic"]
fn replay_current_incomplete_context() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let start = SlotTimestamp::parse("261028_120000").unwrap();
    let config = StreamDecodeConfig {
        profile: DecodeProfile::WsjtxPlus,
        nfqso: 1000.0,
        hiscall: Some("BG5ATV".into()),
        wsjtx_plus_compatibility: true,
        ..Default::default()
    };
    let mut session = StreamDecodeSession::new(config);
    let fine_recovery = FINE_RECOVERY.replace(false);
    let mut target_audio = Vec::new();
    let mut interferers = Vec::new();
    for slot in 0..=6 {
        let timestamp = start.add_seconds(slot as i64 * 15);
        let audio = samples(2026102809, slot, false, true, false);
        if slot == 6 {
            SOFT.with_borrow_mut(|state| *state = Some(Vec::new()));
        }
        let rows = session
            .decode_slot_streaming_tagged_at(&timestamp, &audio, |row| {
                let d = &row.decode;
                if slot == 6
                    && row.provenance == StreamDecodeProvenance::Regular
                    && (2.0..=40.0).contains(&(d.freq - TARGET_FREQUENCY).abs())
                    && d.snr >= -15.0
                {
                    interferers.push(d.clone());
                }
                Ok(())
            })
            .unwrap();
        if slot == 6 {
            target_audio = audio;
        }
        eprintln!(
            "Current Plus slot {slot} target rows: {:?}",
            rows.iter()
                .filter(|r| r.msg.contains("BG5ATV"))
                .map(|r| &r.msg)
                .collect::<Vec<_>>()
        );
    }
    FINE_RECOVERY.set(fine_recovery);
    let rows = SOFT.with_borrow_mut(|state| state.take().unwrap());
    let truth = native_codeword("current-ideal");
    evaluate_soft("session candidate replay and refinements", &rows, &truth);

    let regular = TERMINAL.with_borrow_mut(|capture| capture.take().expect("terminal residual"));
    // Match the final decoder's 50-symbol input support in every arm.
    let mut raw = vec![0.0; regular.len()];
    for (sample, &input) in raw.iter_mut().zip(&target_audio[..50 * 3456]) {
        *sample = f64::from(input);
    }
    assert!(interferers.iter().any(|d| (d.freq - 970.0).abs() < 2.0));
    assert!(interferers.iter().any(|d| (d.freq - 1025.0).abs() < 2.0));
    let mut targeted = raw.clone();
    for d in &interferers {
        super::super::subtractft8::subtract_ft8_refined(
            &mut targeted,
            &d.itone,
            d.freq,
            d.dt + 0.5,
            true,
        );
    }
    eprintln!(
        "Targeted subtraction: {:?}",
        interferers
            .iter()
            .map(|d| (&d.msg, d.freq, d.snr))
            .collect::<Vec<_>>()
    );
    for (label, audio) in [
        ("raw", &raw),
        ("regular residual", &regular),
        ("targeted residual", &targeted),
    ] {
        SOFT.with_borrow_mut(|capture| *capture = Some(Vec::new()));
        super::super::ft8_decode::measure_audio(audio, TARGET_FREQUENCY, 0.1);
        let observations = SOFT.with_borrow_mut(|capture| capture.take().unwrap());
        evaluate_soft(label, &observations, &truth);
    }
}

fn evaluate_soft(label: &str, rows: &[SoftObservation], truth: &[u8]) {
    let mut baseline_errors = [usize::MAX; 5];
    let mut alternative_errors = [usize::MAX; 10];
    let mut best_weighted_error = f64::INFINITY;
    let mut best_sync = 0;
    let mut distinct = std::collections::HashSet::new();
    let mut hits = Vec::new();
    let mut unexpected = Vec::new();
    for row in rows {
        let (mut errors, mut wrong_weight, mut total_weight) = (0, 0.0, 0.0);
        for (&soft, &bit) in row.llr.iter().zip(truth) {
            let weight = soft.abs();
            total_weight += weight;
            if u8::from(soft >= 0.0) != bit {
                errors += 1;
                wrong_weight += weight;
            }
        }
        if total_weight > 0.0 {
            best_weighted_error = best_weighted_error.min(wrong_weight / total_weight);
        }
        best_sync = best_sync.max(row.nsync);
        let stats = if row.baseline {
            &mut baseline_errors[..]
        } else {
            &mut alternative_errors[..]
        };
        stats[row.metric - 1] = stats[row.metric - 1].min(errors);
        if !distinct.insert(row.llr.iter().map(|v| v.to_bits()).collect::<Vec<_>>()) {
            continue;
        }
        let llr32: Vec<_> = row.llr.iter().map(|&v| v as f32).collect();
        let decodes = [
            (
                "BP/OSD",
                super::super::decode174_91::decode174_91(&row.llr, &[0; 174], 3),
            ),
            (
                "OSD3",
                super::super::osd174_91::osd_decode174_91(&llr32, &[0; 174], 3),
            ),
            (
                "OSD4",
                super::super::osd174_91::osd_decode174_91(&llr32, &[0; 174], 4),
            ),
        ];
        for (method, result) in decodes {
            if let Some(decoded) = result {
                let details = (method, row.freq, row.time, row.metric, errors);
                if decoded.cw == truth {
                    hits.push(details);
                } else {
                    let msg = crate::decode::packjt77::unpack77(&decoded.message91[..77], None);
                    unexpected.push((details, msg));
                }
            }
        }
    }
    assert!(baseline_errors.iter().all(|&errors| errors < 174));
    assert!(alternative_errors.iter().all(|&errors| errors < 174));
    eprintln!("{label}: base hard errors: {baseline_errors:?}");
    eprintln!("{label}: alternative hard errors: {alternative_errors:?}");
    eprintln!(
        "{label}: best Costas tones {best_sync}/21, minimum wrong-bit LLR weight {:.3}%",
        100.0 * best_weighted_error
    );
    eprintln!(
        "{label}: {} observations, {} distinct, correct={hits:?}, other CRC={unexpected:?}",
        rows.len(),
        distinct.len()
    );
}

thread_local! {
    static FINE_RECOVERY: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

pub(in crate::decode::lib_wsjtx_plus) fn fine_recovery_enabled() -> bool {
    FINE_RECOVERY.get()
}

pub(super) fn set_fine_recovery(enabled: bool) {
    FINE_RECOVERY.set(enabled);
}

type FineCallback = (Row, StreamDecodeProvenance, Duration);

fn fine_replay(
    session: &mut StreamDecodeSession,
    enabled: bool,
    timestamp: &SlotTimestamp,
    audio: &[f32],
) -> (Vec<FineCallback>, Duration) {
    let previous = FINE_RECOVERY.replace(enabled);
    let started = Instant::now();
    let mut callbacks = Vec::new();
    let rows = session.decode_slot_streaming_tagged_at(timestamp, audio, |row| {
        callbacks.push((Row::from(&row.decode), row.provenance, started.elapsed()));
        Ok(())
    });
    let elapsed = started.elapsed();
    FINE_RECOVERY.set(previous);
    let rows = rows.unwrap();
    assert_eq!(rows.len(), callbacks.len());
    for (row, callback) in rows.iter().zip(&callbacks) {
        assert_eq!(Row::from(row), callback.0);
    }
    (callbacks, elapsed)
}

fn assert_same_fine_prefix(expected: &[FineCallback], actual: &[FineCallback]) {
    assert!(actual.len() >= expected.len());
    for (a, b) in expected.iter().zip(actual) {
        assert_eq!((&a.0, a.1), (&b.0, b.1));
    }
}

#[test]
#[ignore = "serial optimized Plus fine RR73 prototype"]
fn replay_fine_rr73_prototype() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let config = StreamDecodeConfig {
        profile: DecodeProfile::WsjtxPlus,
        nfqso: 1000.0,
        hiscall: Some("BG5ATV".into()),
        wsjtx_plus_compatibility: true,
        ..Default::default()
    };
    let start = SlotTimestamp::parse("261028_120000").unwrap();
    let mut native =
        crate::stream::session::StreamDecodeSession::new(config.clone_for_profile_wsjt_x());
    let mut baseline = StreamDecodeSession::new(config.clone());
    let mut candidate = StreamDecodeSession::new(config);
    let mut target_slots: [Vec<usize>; 3] = Default::default();
    let mut cumulative = [Duration::ZERO; 3];
    for slot in 0..16 {
        let timestamp = start.add_seconds(slot as i64 * 15);
        let audio = samples(2026102809, slot, false, true, false);
        let native_started = Instant::now();
        let native_rows = native.decode_slot_at(&timestamp, &audio);
        let native_elapsed = native_started.elapsed();
        let (original, old_elapsed) = fine_replay(&mut baseline, false, &timestamp, &audio);
        let (replayed, new_elapsed) = fine_replay(&mut candidate, true, &timestamp, &audio);
        for (i, hit) in [
            native_rows.iter().any(|row| row.msg == TARGET),
            original.iter().any(|row| row.0.msg == TARGET),
            replayed.iter().any(|row| row.0.msg == TARGET),
        ]
        .into_iter()
        .enumerate()
        {
            if hit {
                target_slots[i].push(slot);
            }
        }
        for (sum, elapsed) in cumulative
            .iter_mut()
            .zip([native_elapsed, old_elapsed, new_elapsed])
        {
            *sum += elapsed;
        }
        assert!(new_elapsed <= Duration::from_secs(10));
        assert_eq!(replayed.len(), original.len() + usize::from(slot == 6));
        assert_same_fine_prefix(&original, &replayed);
        assert_eq!(
            candidate.export_regular_hash_calls(),
            baseline.export_regular_hash_calls()
        );
        if slot == 6 {
            let (row, provenance, callback) = replayed.last().unwrap();
            assert_eq!(row.msg, TARGET);
            assert_eq!(row.tones, frozen_tones(TARGET, false));
            assert_eq!(*provenance, StreamDecodeProvenance::ContextRecovery);
            eprintln!(
                "fine RR73 {} Hz DT {}, callback {:?}, slot {:?} -> {:?}",
                f64::from_bits(row.freq),
                f64::from_bits(row.dt),
                callback,
                old_elapsed,
                new_elapsed
            );
        }
    }
    assert_eq!(target_slots, [vec![], vec![], vec![6]]);
    eprintln!("original / Plus control / Plus fine target slots {target_slots:?}, 16-slot total {cumulative:?}");
}

#[test]
#[ignore = "serial optimized synthetic fine-RR73 negative controls"]
fn fine_rr73_independent_negatives() {
    use super::super::ft8_decode::recover_rr73;

    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let controls = [
        ("noise", None, "BG5ATV"),
        ("wrong call", Some(frozen_tones(TARGET, false)), "G4AAA"),
        (
            "wrong message",
            Some(frozen_tones("K1JT BG5ATV 73", false)),
            "BG5ATV",
        ),
        (
            "legacy codeword",
            Some(frozen_tones(TARGET, true)),
            "BG5ATV",
        ),
    ];
    for (kind, (label, tones, target)) in controls.into_iter().enumerate() {
        let mut trials = 0usize;
        let mut active_slots = 0usize;
        let mut worst = Duration::ZERO;
        for slot in 0..75 {
            let mut audio = slot_noise(2026110601 + (kind * 75 + slot) as u64 * 10007);
            if let Some(tones) = &tones {
                let freq = 1000.6 + 0.0625 * (slot % 5) as f64;
                let dt = 0.1 + 0.005 * (slot % 7) as f64;
                inject(&mut audio, tones, freq, dt, -16.0, 0.17 * slot as f64);
            }
            audio[50 * 3456..].fill(0.0);
            for sample in &mut audio {
                *sample = f64::from((*sample * 0.05) as f32);
            }
            let mut count = 0;
            let started = Instant::now();
            let result = recover_rr73(&audio, 1000.0, 200.0..=3000.0, target, || count += 1);
            worst = worst.max(started.elapsed());
            assert!(result.is_none(), "{label} slot {slot}: {result:?}");
            assert!(count <= 80);
            trials += count;
            active_slots += usize::from(count > 0);
        }
        if tones.is_some() {
            assert!(active_slots >= 60, "{label} did not reach FEC often enough");
        }
        eprintln!("fine RR73 {label}: 75 slots, {active_slots} reached FEC, {trials} trials, worst {worst:?}");
    }
}

#[test]
#[ignore = "serial optimized real-recording fine-RR73 A/B gate"]
fn fine_rr73_real_recordings() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    for (recording, target, focus, nfa) in [
        ("210703_133430", "F5BZB", 2571.0, 100.0),
        ("230208_140300", "OY1DZ", 1541.25, 200.0),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/ft8/{recording}.wav"));
        let wav = crate::input::audio::read_wav_mono_f32(&path).unwrap();
        assert_eq!(wav.sample_rate, 12000);
        let config = StreamDecodeConfig {
            profile: DecodeProfile::WsjtxPlus,
            hiscall: Some(target.into()),
            nfqso: focus,
            nfa,
            wsjtx_plus_compatibility: true,
            ..Default::default()
        };
        let mut control = StreamDecodeSession::new(config.clone());
        let mut experimental = StreamDecodeSession::new(config);
        let start = SlotTimestamp::parse(recording).unwrap();
        let mut slots = 0;
        let mut count = 0;
        let mut total = [Duration::ZERO; 2];
        let mut max_slot = [Duration::ZERO; 2];
        let mut max_callback = [Duration::ZERO; 2];
        for (slot, audio) in wav.samples.chunks(180000).enumerate() {
            let timestamp = start.add_seconds(slot as i64 * 15);
            let (old, new) = if slot % 2 == 0 {
                let old = fine_replay(&mut control, false, &timestamp, audio);
                let new = fine_replay(&mut experimental, true, &timestamp, audio);
                (old, new)
            } else {
                let new = fine_replay(&mut experimental, true, &timestamp, audio);
                let old = fine_replay(&mut control, false, &timestamp, audio);
                (old, new)
            };
            assert!(new.1 <= Duration::from_secs(10));
            assert_eq!(old.0.len(), new.0.len(), "{recording} slot {slot}");
            assert_same_fine_prefix(&old.0, &new.0);
            assert_eq!(
                control.export_regular_hash_calls(),
                experimental.export_regular_hash_calls()
            );
            count += old.0.len();
            slots += 1;
            for (i, (rows, elapsed)) in [old, new].into_iter().enumerate() {
                total[i] += elapsed;
                max_slot[i] = max_slot[i].max(elapsed);
                if let Some(row) = rows.last() {
                    max_callback[i] = max_callback[i].max(row.2);
                }
            }
        }
        eprintln!("{recording}: {slots} slots, {count} exact rows, total {total:?}, max slot {max_slot:?}, max callback {max_callback:?}");
    }
}

#[test]
#[ignore = "serial optimized fine-RR73 held-out synthetic replay"]
fn fine_rr73_held_out_positives() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let config = StreamDecodeConfig {
        profile: DecodeProfile::WsjtxPlus,
        hiscall: Some("BG5ATV".into()),
        nfqso: 1000.0,
        wsjtx_plus_compatibility: true,
        ..Default::default()
    };
    let timestamp = SlotTimestamp::parse("261028_120130").unwrap();
    let tones = frozen_tones(TARGET, false);
    let mut baseline_hits = 0;
    let mut added_hits = 0;
    let mut max_slot = Duration::ZERO;
    let mut max_recovery_callback = Duration::ZERO;
    for i in 0..24 {
        let audio = samples(2026110801 + i * 10007, 6, i % 3 == 0, true, false);
        let (old, _) = fine_replay(
            &mut StreamDecodeSession::new(config.clone()),
            false,
            &timestamp,
            &audio,
        );
        let (new, elapsed) = fine_replay(
            &mut StreamDecodeSession::new(config.clone()),
            true,
            &timestamp,
            &audio,
        );
        assert!(elapsed <= Duration::from_secs(10));
        assert_same_fine_prefix(&old, &new);
        assert!(new.len() <= old.len() + 1);
        baseline_hits += usize::from(old.iter().any(|row| row.0.msg == TARGET));
        if let Some((row, provenance, callback)) = new.get(old.len()) {
            assert_eq!(row.msg, TARGET, "held-out seed {i}");
            assert_eq!(row.tones, tones);
            assert_eq!(*provenance, StreamDecodeProvenance::ContextRecovery);
            added_hits += 1;
            max_recovery_callback = max_recovery_callback.max(*callback);
        }
        max_slot = max_slot.max(elapsed);
    }
    eprintln!("fine RR73 held-out: 24 slots, {baseline_hits} base, {added_hits} verified additions, worst slot {max_slot:?}, worst recovery callback {max_recovery_callback:?}");
}

#[test]
#[ignore = "serial optimized long known-truth fine-RR73 acceptance"]
fn fine_rr73_long_truth_recordings() {
    use crate::decode::{genft8::get_ft8_tones_from_77bits, packjt77::pack77};

    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let start = SlotTimestamp::parse("261106_120000").unwrap();
    let mut total_added = 0;
    let mut incorrect = 0;
    for (recording, (receiver, sender)) in [
        ("K1JT", "BG5ATV"),
        ("N1JFU", "EA6EE"),
        ("W1FC", "F5BZB"),
        ("K1BZM", "EA3GP"),
    ]
    .into_iter()
    .enumerate()
    {
        let expected = format!("{receiver} {sender} RR73");
        let bits = pack77(&expected);
        assert_eq!(
            bits[59..74]
                .iter()
                .fold(0u16, |a, &b| (a << 1) | u16::from(b)),
            32403
        );
        let tones = get_ft8_tones_from_77bits(&bits);
        let wrong_message = get_ft8_tones_from_77bits(&pack77(&format!("{receiver} {sender} 73")));
        let wrong_call = frozen_tones(TARGET, false);
        let config = StreamDecodeConfig {
            profile: DecodeProfile::WsjtxPlus,
            hiscall: Some(sender.into()),
            nfqso: 1000.0,
            nfa: 925.0,
            nfb: 1075.0,
            wsjtx_plus_compatibility: true,
            ..Default::default()
        };
        let mut base = StreamDecodeSession::new(config.clone());
        let mut candidate = StreamDecodeSession::new(config);
        let (mut base_hits, mut added_hits, mut errors, mut positives) = (0, 0, 0, 0);
        let (mut max_slot, mut max_callback) = (Duration::ZERO, Duration::ZERO);
        for slot in 0..120 {
            let index = recording * 120 + slot;
            let mut noise = slot_noise(2026110901 + index as u64 * 10007);
            let freq = 1000.0 + [0.46, -0.71, 1.11, -1.35, 0.72][index % 5];
            let dt = [0.075, 0.12, 0.165, 0.21, 0.255][index / 5 % 5];
            let phase = index as f64 * 0.37;
            let positive = slot % 5 < 3;
            if positive {
                let snr = [-17.0, -19.0, -21.0, -22.0, -23.0][index / 3 % 5];
                inject(&mut noise, &tones, freq, dt, snr, phase);
            } else if slot % 5 == 3 {
                inject(&mut noise, &wrong_message, freq, dt, -17.0, phase);
            } else if sender != "BG5ATV" && slot % 10 == 4 {
                inject(&mut noise, &wrong_call, freq, dt, -17.0, phase);
            }
            for (msg, f, s) in [
                ("CQ K1ABC FN42", 1025.0, -5.0),
                ("CQ W9XYZ EN50", 970.0, -9.0),
            ] {
                inject(&mut noise, &frozen_tones(msg, false), f, 0.1, s, phase + f);
            }
            let audio: Vec<_> = noise
                .into_iter()
                .enumerate()
                .map(|(i, x)| {
                    let gain = if slot % 7 == 0 && (48000..96000).contains(&i) {
                        0.3
                    } else {
                        1.0
                    };
                    (x * gain * 0.05) as f32
                })
                .collect();
            let stamp = start.add_seconds(index as i64 * 15);
            let (old, _) = fine_replay(&mut base, false, &stamp, &audio);
            let (new, elapsed) = fine_replay(&mut candidate, true, &stamp, &audio);
            assert_same_fine_prefix(&old, &new);
            assert!(elapsed <= Duration::from_secs(10));
            max_slot = max_slot.max(elapsed);
            positives += usize::from(positive);
            base_hits += usize::from(positive && old.iter().any(|row| row.0.msg == expected));
            for (row, provenance, callback) in &new[old.len()..] {
                assert_eq!(*provenance, StreamDecodeProvenance::ContextRecovery);
                max_callback = max_callback.max(*callback);
                if positive && row.msg == expected && row.tones == tones {
                    added_hits += 1;
                } else {
                    errors += 1;
                    eprintln!(
                        "unexpected fine recovery, recording={recording} slot={slot}: {}",
                        row.msg
                    );
                }
            }
        }
        total_added += added_hits;
        incorrect += errors;
        eprintln!("long truth {sender}: 120 slots, {positives} positives, {base_hits} base, {added_hits} correct additions, {errors} incorrect, max slot {max_slot:?}, max recovery callback {max_callback:?}");
    }
    let additions = total_added + incorrect;
    eprintln!(
        "long truth aggregate: 480 slots, {total_added} correct additions, {incorrect} incorrect"
    );
    assert!(additions > 0);
    assert!(
        incorrect * 100 < additions,
        "new-message errors {incorrect}/{additions}"
    );
}

#[test]
#[ignore = "serial optimized known-signal overlays on the unchanged long recording"]
fn fine_rr73_real_background_truth() {
    if cfg!(debug_assertions) {
        panic!("run with --profile fast or --release");
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ft8/230208_140300.wav");
    let wav = crate::input::audio::read_wav_mono_f32(&path).unwrap();
    assert_eq!(wav.sample_rate, 12000);
    let config = StreamDecodeConfig {
        profile: DecodeProfile::WsjtxPlus,
        hiscall: Some("BG5ATV".into()),
        nfqso: 1000.0,
        nfa: 925.0,
        nfb: 1075.0,
        wsjtx_plus_compatibility: true,
        ..Default::default()
    };
    let stamp = SlotTimestamp::parse("230208_140300").unwrap();
    let tones = frozen_tones(TARGET, false);
    let mut counts = [0; 5];
    let mut max_slot = Duration::ZERO;
    let mut max_recovery_callback = Duration::ZERO;
    for (level, snr) in [
        -17.0, -19.0, -21.0, -23.0, -18.0, -20.0, -22.0, -24.0, -25.0, -26.0,
    ]
    .into_iter()
    .enumerate()
    {
        let mut control = StreamDecodeSession::new(config.clone());
        let mut candidate = StreamDecodeSession::new(config.clone());
        for (slot, real_audio) in wav.samples.chunks(180000).enumerate() {
            let power = real_audio
                .iter()
                .map(|&x| f64::from(x).powi(2))
                .sum::<f64>()
                / real_audio.len() as f64;
            let rms = power.sqrt();
            assert!(rms.is_finite() && rms > 0.0);
            let mut overlay = vec![0.0; 180000];
            for (sample, &value) in overlay.iter_mut().zip(real_audio) {
                *sample = f64::from(value) / rms;
            }
            let signal_present = slot % 5 != 4;
            if signal_present {
                let f = 1000.0 + [0.55, -0.81, 1.09, -1.21][(level + slot) % 4];
                let dt = [0.085, 0.13, 0.205, 0.27][slot % 4];
                inject(&mut overlay, &tones, f, dt, snr, 0.59 * slot as f64);
            }
            let audio: Vec<_> = overlay.into_iter().map(|x| (x * rms) as f32).collect();
            let timestamp = stamp.add_seconds(slot as i64 * 15);
            let (old, _) = fine_replay(&mut control, false, &timestamp, &audio);
            let (new, elapsed) = fine_replay(&mut candidate, true, &timestamp, &audio);
            assert!(elapsed <= Duration::from_secs(10));
            max_slot = max_slot.max(elapsed);
            assert_same_fine_prefix(&old, &new);
            counts[0] += 1;
            counts[1] += usize::from(signal_present);
            counts[2] += usize::from(signal_present && old.iter().any(|row| row.0.msg == TARGET));
            for (row, source, callback) in &new[old.len()..] {
                assert_eq!(*source, StreamDecodeProvenance::ContextRecovery);
                max_recovery_callback = max_recovery_callback.max(*callback);
                let correct = signal_present && row.msg == TARGET && row.tones == tones;
                counts[if correct { 3 } else { 4 }] += 1;
            }
        }
    }
    let [slots, positives, base_hits, added, errors] = counts;
    eprintln!("real background truth: {slots} slots, {positives} positives, {base_hits} base, {added} correct additions, {errors} incorrect, max slot {max_slot:?}, max recovery callback {max_recovery_callback:?}");
    assert!(errors == 0 || errors * 100 < added + errors);
}
