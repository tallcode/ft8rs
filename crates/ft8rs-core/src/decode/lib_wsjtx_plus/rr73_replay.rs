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
fn samples(
    root_seed: u64,
    slot: usize,
    pump: bool,
    target_present: bool,
    legacy: bool,
) -> Vec<f32> {
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
    pub(in crate::decode::lib_wsjtx_plus) llr: Vec<f64>,
}

thread_local! {
    static SOFT: std::cell::RefCell<Option<Vec<SoftObservation>>> = const { std::cell::RefCell::new(None) };
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
    for slot in 0..=6 {
        let timestamp = start.add_seconds(slot as i64 * 15);
        let audio = samples(2026102809, slot, false, true, false);
        if slot == 6 {
            SOFT.with_borrow_mut(|state| *state = Some(Vec::new()));
        }
        let rows = session
            .decode_slot_streaming_tagged_at(&timestamp, &audio, |_| Ok(()))
            .unwrap();
        eprintln!(
            "Current Plus slot {slot} target rows: {:?}",
            rows.iter()
                .filter(|r| r.msg.contains("BG5ATV"))
                .map(|r| &r.msg)
                .collect::<Vec<_>>()
        );
    }
    let rows = SOFT.with_borrow_mut(|state| state.take().unwrap());
    let truth = native_codeword("current-ideal");
    let hard_errors = |llr: &[f64]| {
        llr.iter()
            .zip(&truth)
            .filter(|&(&soft, &bit)| u8::from(soft >= 0.0) != bit)
            .count()
    };
    let mut baseline_errors = [usize::MAX; 5];
    let mut alternative_errors = [usize::MAX; 10];
    let mut distinct = std::collections::HashSet::new();
    let mut hits = Vec::new();
    let mut unexpected = Vec::new();
    for row in &rows {
        let errors = hard_errors(&row.llr);
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
    eprintln!("Base hard errors: {baseline_errors:?}");
    eprintln!("Alternative hard errors: {alternative_errors:?}");
    eprintln!(
        "{} observations, {} distinct, correct={hits:?}, other CRC={unexpected:?}",
        rows.len(),
        distinct.len()
    );
}
