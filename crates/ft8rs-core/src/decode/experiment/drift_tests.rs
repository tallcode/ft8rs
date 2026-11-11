use super::*;
use crate::decode::hybrid::HybridStreamDecodeSession;
use crate::stream::session::StreamDecodeProvenance;
use crate::{SlotTimestamp, StreamDecodeConfig};

struct Fixture {
    samples: Vec<f32>,
    truth: Vec<(String, f64, f64)>,
}

fn uniform(seed: &mut u64) -> f64 {
    *seed ^= *seed >> 12;
    *seed ^= *seed << 25;
    *seed ^= *seed >> 27;
    ((seed.wrapping_mul(2685821657736338717) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
}

#[allow(clippy::too_many_arguments)]
fn inject(
    audio: &mut [f32],
    message: &str,
    freq: f64,
    dt: f64,
    slope: f64,
    snr: f64,
    phase: f64,
    sync_only: bool,
) {
    let mut tones =
        crate::decode::genft8::get_ft8_tones_from_77bits(&crate::decode::packjt77::pack77(message));
    if sync_only {
        tones[7..36].fill(0);
        tones[43..72].fill(0);
    }
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, freq);
    let start = ((dt + 0.5) * FS).round() as usize;
    let amplitude = (2.0 * 2500.0 / 6000.0 * 10f64.powf(snr / 10.0)).sqrt();
    for (i, (&re, &im)) in re.iter().zip(&im).enumerate() {
        if sync_only && matches!(i / 1920, 7..=35 | 43..=71) {
            continue;
        }
        let t = (start + i) as f64 / FS - 7.5;
        let rotation = C::from_polar(amplitude, TAU * 0.5 * slope * t * t + phase);
        audio[start + i] += (C::new(re, im) * rotation).re as f32;
    }
}

// The seeds, waveform arithmetic and case order reproduce the frozen experiment.
// Fresh cases use different seeds and off-grid rates from its first campaign.
fn fixture(id: usize, kind: &str, i: usize, fresh: bool) -> Fixture {
    let mut seed = 202611100002u64 + (id as u64 + 1) * 141817;
    let mut samples = Vec::with_capacity(180000);
    for _ in 0..90000 {
        let r = (-2.0 * uniform(&mut seed).ln()).sqrt();
        let phi = TAU * uniform(&mut seed);
        samples.push((r * phi.cos()) as f32);
        samples.push((r * phi.sin()) as f32);
    }
    let message = [
        "CQ K1ABC FN42",
        "CQ W9XYZ EN50",
        "CQ EA9SQY MQ71",
        "CQ VK3WJT QF22",
        "CQ PY2ABC GG66",
        "CQ VE7ON CN89",
        "K1ABC W9XYZ -19",
        "EA9SQY K1ABC R-17",
    ][id % 8];
    let freq = 320.0 + 2470.0 * uniform(&mut seed);
    let dt = -0.22 + 0.8 * uniform(&mut seed);
    let phase = TAU * uniform(&mut seed);
    let sign = if i.is_multiple_of(2) { 1.0 } else { -1.0 };
    let snr = if fresh && kind == "mixed" {
        [-19.0, -20.0, -21.0, -22.0][(i / 2) % 4]
    } else {
        [-18.0, -20.0, -22.0][(i / 2) % 3]
    };
    let slope = match kind {
        "off_grid" if fresh => sign * [0.527, 0.561, 0.642, 0.683][(i / 2) % 4],
        "off_grid" => sign * if i < 6 { 0.54 } else { 0.66 },
        "cw" if fresh => sign * [0.60, 0.57, 0.64, 0.72][(i / 2) % 4],
        "mixed" if fresh => sign * [0.60, 0.549, 0.665][(i / 2) % 3],
        "stationary" | "noise" => 0.0,
        _ => sign * 0.60,
    };
    let mut truth = Vec::new();
    if matches!(kind, "on_grid" | "off_grid" | "mixed" | "stationary") {
        inject(&mut samples, message, freq, dt, slope, snr, phase, false);
        truth.push((message.to_string(), freq, dt));
    }
    if kind == "mixed" {
        let offset = if fresh {
            [12.0, 20.0, 30.0, 40.0][(i / 2) % 4]
        } else {
            20.0
        };
        let other = if freq > 2000.0 {
            freq - offset
        } else {
            freq + offset
        };
        inject(
            &mut samples,
            "CQ DL9ABC JO62",
            other,
            0.05,
            0.0,
            -9.0,
            phase + 0.31,
            false,
        );
        truth.push(("CQ DL9ABC JO62".into(), other, 0.05));
    }
    if kind == "sync_only" {
        inject(&mut samples, message, freq, dt, slope, -10.0, phase, true);
    }
    if kind == "cw" {
        for (sample_index, y) in samples.iter_mut().enumerate() {
            let t = sample_index as f64 / FS - 7.5;
            *y += (0.5 * (TAU * (freq * t + 0.5 * slope * t * t) + phase).cos()) as f32;
            // Preserve the frozen campaign's second line, including its onset.
            if fresh && sample_index >= 8 {
                *y += (0.42
                    * (TAU * ((freq + 19.37) * t - 0.5 * slope * t * t) + phase * 1.7).cos())
                    as f32;
            }
        }
    }
    Fixture { samples, truth }
}

fn matches(row: &StreamDecodedMessage, truth: &(String, f64, f64)) -> bool {
    row.msg == truth.0
        && (row.freq - truth.1).abs() <= 5.0
        && (row.dt - truth.2).abs() <= 0.3
        && row.itone
            == crate::decode::genft8::get_ft8_tones_from_77bits(&crate::decode::packjt77::pack77(
                &truth.0,
            ))
}

#[test]
fn parallel_search_preserves_serial_order_and_fec_budget() {
    let case = fixture(36, "mixed", 0, false);
    let audio: Vec<_> = case.samples.iter().map(|&x| f64::from(x)).collect();
    for used in [0, MAX_FEC - 12, MAX_FEC - 1] {
        let run = |workers| {
            let mut budget = Budget {
                deadline: Instant::now() + Duration::from_secs(10),
                low: 200.0,
                high: 3000.0,
                diagnostics: ExperimentDiagnostics {
                    fec_calls: used,
                    ..Default::default()
                },
            };
            let rows = search_with_workers(&audio, 200.0, 3000.0, 16, &mut budget, workers);
            assert!(budget.diagnostics.fec_calls <= MAX_FEC);
            (rows, budget.diagnostics)
        };
        let (serial, before) = run(1);
        let (parallel, after) = run(6);
        assert_eq!(before.fec_calls, after.fec_calls);
        assert_eq!(before.budget_exhausted, after.budget_exhausted);
        assert_eq!(serial.len(), parallel.len());
        for (left, right) in serial.iter().zip(parallel) {
            assert_eq!(left.msg, right.msg);
            assert_eq!(left.f.to_bits(), right.f.to_bits());
            assert_eq!(left.dt.to_bits(), right.dt.to_bits());
            assert_eq!(left.method, right.method);
            assert_eq!(left.tones, right.tones);
        }
    }
}

#[test]
fn drift_recovery_skips_invalid_partial_and_exhausted_inputs() {
    let valid = vec![1.0; 180000];
    let partial = vec![1.0; 144000];
    let nonfinite = vec![f32::NAN; 180000];
    for (samples, low, high, used, expired) in [
        (&valid, 200.0, 3000.0, MAX_FEC, false),
        (&valid, 200.0, 3000.0, usize::MAX, false),
        (&valid, 200.0, 3000.0, 0, true),
        (&valid, f64::NAN, 3000.0, 0, false),
        (&valid, 200.0, f64::INFINITY, 0, false),
        (&valid, 3000.0, 200.0, 0, false),
        (&partial, 200.0, 3000.0, 0, false),
        (&nonfinite, 200.0, 3000.0, 0, false),
    ] {
        let result = recover_drift(
            ExperimentInput {
                samples,
                baseline: &[],
                low,
                high,
                deadline: Instant::now() + Duration::from_secs(u64::from(!expired)),
            },
            &ExperimentDiagnostics {
                fec_calls: used,
                ..Default::default()
            },
            &mut |_| panic!("invalid drift input emitted a row"),
        );
        assert!(result.is_none());
    }
}

fn decode_fixture(
    case: &Fixture,
    staged: bool,
) -> (
    Vec<crate::stream::session::StreamDecodedWithProvenance>,
    ExperimentDiagnostics,
) {
    let mut session = HybridStreamDecodeSession::new(StreamDecodeConfig {
        hybrid_experiment: true,
        cooperative_realtime: staged,
        ..Default::default()
    });
    let timestamp = SlotTimestamp::parse("230208_140300").unwrap();
    let mut callbacks = Vec::new();
    let mut emit = |row: &crate::stream::session::StreamDecodedWithProvenance| {
        assert!(
            case.truth.iter().any(|truth| matches(&row.decode, truth)),
            "wrong row: {row:?}"
        );
        callbacks.push(row.clone());
        Ok(())
    };
    let rows = if staged {
        let mut state = session.start_slot();
        session
            .decode_slot_nzhsym41_tagged(&timestamp, &mut state, &case.samples[..141696], &mut emit)
            .unwrap();
        session.subtract_slot_nzhsym47(&mut state, &case.samples[..162432]);
        let mut live = case.samples.clone();
        live.extend([0.0; 256]);
        session
            .decode_slot_nzhsym50_tagged(&timestamp, state, &live, &mut emit)
            .unwrap()
    } else {
        session
            .decode_slot_streaming_tagged_at(&timestamp, &case.samples, &mut emit)
            .unwrap()
    };
    assert_eq!(rows.len(), callbacks.len());
    let report = session.last_diagnostics().experiment.clone();
    assert!(report.error.is_none(), "{report:?}");
    assert!(report.fec_calls <= MAX_FEC, "{report:?}");
    for evidence in session.last_evidence_store().rows() {
        if evidence
            .evidence
            .iter()
            .all(|item| item.provenance == crate::decode::hybrid::Provenance::Experiment)
        {
            assert!(
                !evidence.import_eligible,
                "experimental row became an AP/hash seed"
            );
        }
    }
    (callbacks, report)
}

#[test]
fn hybrid_drift_recovers_overlap_in_file_and_live_paths() {
    let case = fixture(36, "mixed", 0, false);
    let original = case.samples.clone();
    for staged in [false, true] {
        let (rows, report) = decode_fixture(&case, staged);
        assert!(
            case.truth
                .iter()
                .all(|t| rows.iter().any(|r| matches(&r.decode, t))),
            "{rows:?}"
        );
        assert!(report.drift_accepted_rows > 0, "{report:?}");
        let target = rows
            .iter()
            .find(|r| matches(&r.decode, &case.truth[0]))
            .unwrap();
        assert_eq!(target.provenance, StreamDecodeProvenance::Experiment);
        assert_eq!(case.samples, original);
    }
}

#[test]
fn drift_rejects_the_frozen_sweep_false_decode() {
    let case = fixture(70, "cw", 2, false);
    let (_, report) = decode_fixture(&case, false);
    assert_eq!(report.drift_accepted_rows, 0);
    assert_eq!(report.drift_removed_lines, 1);
}

#[test]
#[ignore = "serial frozen 148-slot known-truth and interference confirmation"]
fn hybrid_drift_frozen_truth_corpus() {
    for fresh in [false, true] {
        let mut id = if fresh { 200 } else { 0 };
        let groups = if fresh {
            [
                ("on_grid", 0, 0),
                ("off_grid", 12, 9),
                ("mixed", 24, 18),
                ("stationary", 4, 4),
                ("noise", 8, 0),
                ("sync_only", 8, 0),
                ("cw", 16, 0),
            ]
        } else {
            [
                ("on_grid", 24, 20),
                ("off_grid", 12, 10),
                ("mixed", 8, 8),
                ("stationary", 8, 8),
                ("noise", 8, 0),
                ("sync_only", 8, 0),
                ("cw", 8, 0),
            ]
        };
        for (kind, count, minimum_targets) in groups {
            let mut targets = 0;
            for i in 0..count {
                let case = fixture(id, kind, i, fresh);
                let (rows, _) = decode_fixture(&case, false);
                if let Some(target) = case.truth.first() {
                    targets += usize::from(rows.iter().any(|r| matches(&r.decode, target)));
                }
                if kind == "mixed" {
                    assert!(
                        rows.iter().any(|r| matches(&r.decode, &case.truth[1])),
                        "strong row lost: {id}"
                    );
                }
                id += 1;
            }
            eprintln!("fresh={fresh} {kind}: targets={targets}/{count}");
            assert!(
                targets >= minimum_targets,
                "fresh={fresh} {kind}: {targets} < {minimum_targets}"
            );
        }
    }
}
