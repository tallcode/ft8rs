//! Test-only receive DSP studies. No runtime entry point or native source edits.
use super::*;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
#[path = "../../../tests/decoder/random.rs"]
#[allow(clippy::duplicate_mod)] // Reuse a private test helper without widening its API.
mod polar_random;

// The complete decoder runner includes ignored tests. Research studies only run
// when explicitly given a destination; they must not write into a user's tree
// or require untracked research fixtures during the standard acceptance run.
fn research_path(variable: &str) -> Option<PathBuf> {
    let path = std::env::var_os(variable).map(PathBuf::from);
    if path.is_none() {
        eprintln!("skipping optional polar study: set {variable} to run it");
    }
    path
}

const METHODS: [&str; 10] = [
    "amp1",
    "amp3",
    "split80ms",
    "split40ms",
    "gp2",
    "gp5",
    "gp10",
    "gp2_diff6",
    "pilot_global",
    "pilot_local",
];

#[allow(clippy::needless_range_loop)] // Indexed triangular solves preserve the algebra's order.
fn inverse_positive(matrix: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = matrix.len();
    let mut lower = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let x = matrix[i][j] - (0..j).map(|k| lower[i][k] * lower[j][k]).sum::<f64>();
            lower[i][j] = if i == j { x.sqrt() } else { x / lower[j][j] };
            assert!(lower[i][j].is_finite());
        }
    }
    let mut inverse = vec![vec![0.0; n]; n];
    for column in 0..n {
        let mut y = vec![0.0; n];
        for i in 0..n {
            y[i] = (f64::from(i == column) - (0..i).map(|j| lower[i][j] * y[j]).sum::<f64>())
                / lower[i][i];
        }
        for i in (0..n).rev() {
            inverse[i][column] = (y[i]
                - (i + 1..n)
                    .map(|j| lower[j][i] * inverse[j][column])
                    .sum::<f64>())
                / lower[i][i];
        }
    }
    inverse
}

// Marginalize a zero-mean Gaussian channel rather than fitting arbitrary phase
// to each competing tone. C_q=D_q(I+rho R)D_q*. Equal determinants cancel.
fn gp_weights(width: f64, differential: f64, rho: f64) -> Vec<Vec<f64>> {
    let matrix: Vec<Vec<f64>> = (0..32)
        .map(|i| {
            (0..32)
                .map(|j| {
                    let lag = (i as f64 - j as f64) / 200.0;
                    let r = (-0.5 * std::f64::consts::PI.powi(2) * width.powi(2) * lag.powi(2))
                        .exp()
                        * (std::f64::consts::PI * differential * lag).cos();
                    f64::from(i == j) + rho * r
                })
                .collect()
        })
        .collect();
    let inverse = inverse_positive(&matrix);
    (0..32)
        .map(|i| (0..32).map(|j| f64::from(i == j) - inverse[i][j]).collect())
        .collect()
}

fn samples_for(band: &Band, f: f64, dt: f64, k: usize) -> [C; 32] {
    let start = ((dt + 0.5) * 200.0).round() as isize;
    std::array::from_fn(|n| {
        let j = start + (32 * k + n) as isize;
        if j < 0 || j >= band.observed_end as isize {
            return C::new(0.0, 0.0);
        }
        let t = (j - start) as f64 / 200.0;
        band.samples[j as usize] * C::from_polar(1.0, -TAU * (f - band.center) * t)
    })
}

fn pilot_rho(band: &Band, f: f64, dt: f64, noise: f64) -> f64 {
    let mut energy = 0.0;
    for base in [0, 36, 72] {
        for k in 0..7 {
            energy += samples_for(band, f, dt, base + k)
                .iter()
                .map(C::norm_sqr)
                .sum::<f64>();
        }
    }
    (energy / (21.0 * 32.0) / noise - 1.0).clamp(0.03, 10.0)
}

fn scores_to_llr(scores: &[[f64; 8]; 79]) -> Vec<f64> {
    let mut llr = Vec::with_capacity(174);
    for base in [7, 43] {
        for row in &scores[base..base + 29] {
            for bit in (0..3).rev() {
                let one: Vec<_> = (0..8)
                    .filter(|q| (q >> bit) & 1 == 1)
                    .map(|q| row[GRAY[q]])
                    .collect();
                let zero: Vec<_> = (0..8)
                    .filter(|q| (q >> bit) & 1 == 0)
                    .map(|q| row[GRAY[q]])
                    .collect();
                llr.push((lse(&one) - lse(&zero)).clamp(-20.0, 20.0));
            }
        }
    }
    llr
}

fn local_pilot_llr(s: &Spectrum, noise: f64, rho: f64) -> Vec<f64> {
    let pilots: Vec<usize> = [0, 36, 72].into_iter().flat_map(|b| b..b + 7).collect();
    let correlation = |lag: f64| (-0.5 * std::f64::consts::PI.powi(2) * 4.0 * lag * lag).exp();
    let prior = noise * rho;
    let matrix: Vec<Vec<f64>> = pilots
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            pilots
                .iter()
                .enumerate()
                .map(|(j, &b)| {
                    prior * correlation((a as f64 - b as f64) * 0.16)
                        + f64::from(i == j) * noise / 32.0
                })
                .collect()
        })
        .collect();
    let inverse = inverse_positive(&matrix);
    let observed: Vec<C> = pilots
        .iter()
        .map(|&p| s[p][COSTAS[p % 36]] / 32.0)
        .collect();
    let scores = std::array::from_fn(|k| {
        let covariance: Vec<_> = pilots
            .iter()
            .map(|&p| prior * correlation((k as f64 - p as f64) * 0.16))
            .collect();
        let weights: Vec<f64> = (0..21)
            .map(|i| (0..21).map(|j| inverse[i][j] * covariance[j]).sum())
            .collect();
        let mean: C = weights.iter().zip(&observed).map(|(w, y)| y * w).sum();
        let uncertainty = (prior
            - weights
                .iter()
                .zip(&covariance)
                .map(|(a, b)| a * b)
                .sum::<f64>())
        .max(0.0);
        let v = 32.0 * noise;
        std::array::from_fn(|q| {
            s[k][q].norm_sqr() / v - (s[k][q] - mean * 32.0).norm_sqr() / (v + 1024.0 * uncertainty)
        })
    });
    scores_to_llr(&scores)
}

fn polar_llr(band: &Band, f: f64, dt: f64, method: &str) -> Vec<f64> {
    let spectrum = band.spectra(f, dt, 0.0);
    let stats = MetricStats::new(&spectrum);
    match method {
        "amp1" => return metrics(&spectrum, &stats, 1, "amp"),
        "amp3" => return metrics(&spectrum, &stats, 3, "amp"),
        "pilot_global" => return coherent_metrics(&spectrum),
        _ => (),
    }
    let noise = (stats.var / 32.0).max(1e-15);
    let rho = pilot_rho(band, f, dt, noise);
    if method == "pilot_local" {
        return local_pilot_llr(&spectrum, noise, rho);
    }
    let (width, differential) = match method {
        "gp2" => (2.0, 0.0),
        "gp5" => (5.0, 0.0),
        "gp10" => (10.0, 0.0),
        "gp2_diff6" => (2.0, 6.0),
        _ => (0.0, 0.0),
    };
    let weights = (width > 0.0).then(|| gp_weights(width, differential, rho));
    let scores = std::array::from_fn(|k| {
        let samples = samples_for(band, f, dt, k);
        std::array::from_fn(|q| {
            let z: [C; 32] = std::array::from_fn(|n| samples[n] * basis()[q][n]);
            if let Some(w) = &weights {
                let mut score = 0.0;
                for i in 0..32 {
                    for j in 0..32 {
                        score += (z[i].conj() * z[j]).re * w[i][j];
                    }
                }
                score / noise
            } else {
                let size = if method == "split80ms" { 16 } else { 8 };
                z.chunks(size)
                    .map(|s| s.iter().sum::<C>().norm_sqr())
                    .sum::<f64>()
                    * rho
                    / (1.0 + size as f64 * rho)
                    / noise
            }
        })
    });
    scores_to_llr(&scores)
}

fn blind_gp(audio: &[f64]) -> (Vec<Value>, usize, f64) {
    let started = Instant::now();
    let (bands, candidates) = discover(audio, 200.0, 3000.0);
    let mut rows: Vec<Value> = Vec::new();
    let mut calls = 0;
    for candidate in candidates.iter().take(16) {
        let band = &bands[candidate.band];
        for refined in refine(band, candidate) {
            for method in ["amp1", "gp2", "gp5", "gp10", "gp2_diff6"] {
                let llr = polar_llr(band, refined.f, refined.dt, method);
                calls += 1;
                if let Some((message, tones)) = decode_with_tones(&llr) {
                    if !rows.iter().any(|r| r["message"].as_str() == Some(&message)) {
                        rows.push(json!({"message":message,"tones":tones.to_vec(),"frequency":refined.f,
                            "dt":refined.dt,"method":method,"raw_crc_ms":started.elapsed().as_secs_f64()*1000.0}));
                    }
                }
            }
        }
    }
    (rows, calls, started.elapsed().as_secs_f64() * 1000.0)
}

fn tones_for(message: &str) -> [i32; 79] {
    let data = include_str!("../../../tests/decoder/data/ft8/waveforms/dx-current-payloads.tsv");
    data.lines()
        .filter(|s| !s.starts_with('#'))
        .map(|s| s.split('\t').collect::<Vec<_>>())
        .find(|r| r[0] == message)
        .unwrap()[2]
        .bytes()
        .map(|b| i32::from(b - b'0'))
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}

fn truth_bits(tones: &[i32; 79]) -> Vec<u8> {
    [7, 43]
        .into_iter()
        .flat_map(|base| {
            (base..base + 29).flat_map(|k| {
                let q = GRAY.iter().position(|&t| t == tones[k] as usize).unwrap();
                (0..3).rev().map(move |bit| ((q >> bit) & 1) as u8)
            })
        })
        .collect()
}

fn read_audio(path: &Path) -> Vec<f64> {
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<i16>()
        .map(|x| x.unwrap() as f64 / 32768.0)
        .collect()
}

#[test]
fn polar_covariance_inverse_is_consistent() {
    let a = vec![
        vec![2.0, 0.3, 0.1],
        vec![0.3, 1.5, 0.2],
        vec![0.1, 0.2, 1.2],
    ];
    let inv = inverse_positive(&a);
    for (i, _) in a.iter().enumerate() {
        for (j, _) in inv.iter().enumerate() {
            let value: f64 = (0..3).map(|k| a[i][k] * inv[k][j]).sum();
            assert!((value - f64::from(i == j)).abs() < 1e-12);
        }
    }
    for width in [0.5, 2.0, 5.0, 10.0] {
        for diff in [0.0, 6.0] {
            let w = gp_weights(width, diff, 0.4);
            assert!(w.iter().flatten().all(|x| x.is_finite()));
            assert!((0..32).all(|i| w[i][i] > 0.0 && w[i][i] < 1.0));
        }
    }
}

#[test]
#[ignore = "local optimized diagnostic research, explicit output directory"]
fn polar_transition_probe() {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists(), "choose fresh output directory");
    std::fs::create_dir_all(&output).unwrap();
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../experiments/2026-11-11-polar-path/audio");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(corpus.join("manifest.json")).unwrap()).unwrap();
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(output.join("diagnostic.jsonl")).unwrap());
    for case in manifest["cases"].as_array().unwrap() {
        let scenario = case["scenario"].as_str().unwrap();
        if ![
            "awgn",
            "hq",
            "spread2",
            "spread5",
            "hm",
            "differential6",
            "noise_only",
            "interferer_only",
        ]
        .contains(&scenario)
        {
            continue;
        }
        let audio = read_audio(&corpus.join(case["file"].as_str().unwrap()));
        let band = Band::new(&audio, 1022.0);
        let target = case["target"].as_str().unwrap();
        let tones = tones_for(target);
        let bits = truth_bits(&tones);
        let mut methods = Vec::new();
        for method in METHODS {
            let start = Instant::now();
            let llr = polar_llr(&band, 1000.0, 0.0, method);
            let bad_bits = bits
                .iter()
                .zip(&llr)
                .filter(|(b, l)| (**l > 0.0) != (**b == 1))
                .count();
            let decoded = decode_with_tones(&llr);
            methods.push(json!({"method":method,"hard_bit_errors":bad_bits,
                "decoded":decoded.as_ref().map(|r| &r.0),
                "correct":case["target_present"]==true && decoded.as_ref().is_some_and(|(m,t)| m==target && *t==tones),
                "wrong":decoded.as_ref().is_some_and(|(m,_)| !((case["target_present"]==true && m==target)
                    || case["interferer"].as_str()==Some(m.as_str()))),
                "work_ms":start.elapsed().as_secs_f64()*1000.0}));
        }
        let row = Row {
            msg: target.into(),
            f: 1000.0,
            dt: 0.0,
            method: "truth-diagnostic-only".into(),
            tones: Some(tones),
        };
        let fit = fit_message(&audio, &row, Instant::now() + Duration::from_secs(10)).unwrap();
        let (_bands, candidates) = discover(&audio, 200.0, 3000.0);
        let rank = candidates
            .iter()
            .position(|c| (c.f - 1000.0).abs() <= 4.0 && c.dt.abs() <= 0.12);
        writeln!(
            file,
            "{}",
            json!({"case":case,"timing_frequency":"known diagnostic only",
            "methods":methods,"truth_gate":{"score":fit.score,"data_score":fit.data_score,
                "minimum_block":fit.minimum_block,"supported":fit.supported()},
            "candidate_rank":rank,"candidate_count":candidates.len()})
        )
        .unwrap();
        file.flush().unwrap();
        eprintln!("polar {}", case["file"]);
    }
}

fn fading(seed: u64, width: f64) -> Vec<C> {
    let n = 360000;
    let mut rng = polar_random::Noise::new(seed);
    let mut power = 0.0;
    let mut bins: Vec<C> = (0..n)
        .map(|k| {
            let f = if k <= n / 2 {
                k as f64
            } else {
                k as f64 - n as f64
            } * 12000.0
                / n as f64;
            let w = (-(f / width).powi(2)).exp();
            power += w * w;
            C::new(rng.gaussian(), rng.gaussian()) * w
        })
        .collect();
    FftPlanner::new().plan_fft_inverse(n).process(&mut bins);
    bins.truncate(180000);
    for x in &mut bins {
        *x /= (2.0 * power).sqrt();
    }
    bins
}

fn fresh_audio(
    seed: u64,
    message: &str,
    width: f64,
    diff: f64,
    snr: f64,
    kind: &str,
) -> (Vec<f64>, f64, f64) {
    let mut rng = polar_random::Noise::new(seed);
    let f = 370.0 + (rng.gaussian().tanh() + 1.0) * 1080.0;
    let dt = 0.1 + 0.22 * rng.gaussian().tanh();
    let phase = rng.gaussian();
    let mut tones = tones_for(message);
    if kind == "sync_only" {
        tones[7..36].fill(0);
        tones[43..72].fill(0);
    }
    if kind == "split_payload" {
        tones[43..72].copy_from_slice(&tones_for("CQ W9XYZ EN50")[43..72]);
    }
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, f);
    let start = ((dt + 0.5) * 12000.0).round() as usize;
    let mut clean = vec![C::new(0.0, 0.0); 180000];
    for (i, (&re, &im)) in re.iter().zip(&im).enumerate() {
        if kind == "sync_only" && matches!(i/1920,7..=35|43..=71) {
            continue;
        }
        clean[start + i] = C::new(re, im) * C::from_polar(1.0, phase);
    }
    let gains = (width > 0.0).then(|| (fading(seed + 173, width), fading(seed + 379, width)));
    let mut signal = vec![0.0; 180000];
    for i in 0..180000 {
        let t = i as f64 / 12000.0;
        let z = if let Some((a, b)) = &gains {
            let delayed = if i >= 36 {
                clean[i - 36]
            } else {
                C::new(0.0, 0.0)
            };
            (clean[i] * a[i] * C::from_polar(1.0, -TAU * diff * t / 2.0)
                + delayed * b[i] * C::from_polar(1.0, TAU * diff * t / 2.0))
                / 2.0_f64.sqrt()
        } else {
            clean[i]
        };
        signal[i] = z.re;
    }
    let active = &signal[start..start + 151680];
    let power = active.iter().map(|x| x * x).sum::<f64>() / active.len() as f64;
    let gain = ((2500.0 / 6000.0) * 10f64.powf(snr / 10.0) / power).sqrt();
    let mut noise = polar_random::Noise::new(seed + 3001);
    let audio = (0..180000)
        .map(|i| {
            let t = i as f64 / 12000.0;
            let s = match kind {
                "noise_only" => 0.0,
                "carrier" => 0.6 * (TAU * (f + 18.75) * t + phase).cos(),
                "swept_carrier" => 0.6 * (TAU * ((f + 18.75) * t + 0.3 * t * t) + phase).cos(),
                _ => signal[i] * gain,
            };
            // Freeze PCM16 quantization just as in the first campaign.
            ((noise.gaussian() + s) * 256.0).round() / 32768.0
        })
        .collect();
    (audio, f, dt)
}

fn evaluate_fresh(case: &Value, audio: &[f64], f: f64, dt: f64, heldout: bool) -> Value {
    let target = case["target"].as_str().unwrap();
    let present = case["target_present"] == true;
    let tones = tones_for(target);
    let bits = truth_bits(&tones);
    let band = Band::new(audio, f + 22.0);
    let methods:Vec<_>=METHODS.into_iter().map(|method| {
        let started=Instant::now();
        let llr=polar_llr(&band,f,dt,method);
        let bad_bits=bits.iter().zip(&llr).filter(|(b,l)|(**l>0.0)!=(**b==1)).count();
        let decoded=decode_with_tones(&llr);
        json!({"method":method,"hard_bit_errors":bad_bits,"decoded":decoded.as_ref().map(|r|&r.0),
            "correct":present && decoded.as_ref().is_some_and(|(m,t)|m==target && *t==tones),
            "wrong":decoded.as_ref().is_some_and(|(m,_)|!(present && m==target)),
            "work_ms":started.elapsed().as_secs_f64()*1000.0})
    }).collect();
    let baseline = if heldout {
        use crate::stream::ProfileStreamDecodeSession;
        use crate::{DecodeProfile, SlotTimestamp, StreamDecodeConfig};
        let mut session = ProfileStreamDecodeSession::new(StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            hybrid_experiment: true,
            nfqso: 1500.0,
            ..Default::default()
        });
        let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
        let now = Instant::now();
        let mut callbacks = Vec::new();
        let rows = session
            .decode_slot_streaming_at(
                &SlotTimestamp::parse("261111_120000").unwrap(),
                &samples,
                &mut |r: &StreamDecodedMessage| {
                    callbacks.push(json!({"row":r,"ms":now.elapsed().as_secs_f64()*1000.0}));
                    Ok(())
                },
            )
            .unwrap();
        json!({"profile":"hybrid-exp","rows":rows,"callbacks":callbacks,"finish_ms":now.elapsed().as_secs_f64()*1000.0})
    } else {
        Value::Null
    };
    let blind = if heldout {
        let (rows, calls, work) = blind_gp(audio);
        json!({"raw_crc_candidates":rows,"fec_calls":calls,"finish_ms":work,
            "status":"no production admission; no published messages"})
    } else {
        Value::Null
    };
    json!({"case":case,"known_frequency_hz":f,"known_dt_s":dt,"oracle_metrics":methods,
        "baseline":baseline,"blind":blind})
}

#[test]
#[ignore = "local optimized dense channel study; explicit fresh output"]
fn polar_dense_validation() {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists(), "choose fresh output");
    std::fs::create_dir_all(output.join("audio")).unwrap();
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(output.join("observations.jsonl")).unwrap());
    let mut id = 0;
    for seed_index in 0..8 {
        let target = [
            "CQ K1ABC FN42",
            "CQ BG5ATV PM00",
            "K1JT BG5ATV -04",
            "CQ G4AAA IO91",
        ][seed_index % 4];
        for (wi, width) in [0.0, 0.5, 2.0, 3.0, 4.0, 5.0, 7.5, 10.0]
            .into_iter()
            .enumerate()
        {
            let differences: &[f64] = if width < 2.0 {
                &[0.0]
            } else {
                &[
                    0.0,
                    if seed_index < 4 {
                        6.0
                    } else if seed_index % 2 == 0 {
                        4.5
                    } else {
                        7.5
                    },
                ]
            };
            for (di, &diff) in differences.iter().enumerate() {
                let seed =
                    202611120001 + seed_index as u64 * 100003 + wi as u64 * 1009 + di as u64 * 71;
                for snr in [-15.0, -12.0, -9.0] {
                    let (audio, f, dt) = fresh_audio(seed, target, width, diff, snr, "target");
                    let case = json!({"id":id,"seed":seed,"seed_index":seed_index,"fold":if seed_index<4 {"development"} else {"heldout"},
                        "target":target,"target_present":true,"kind":"target","spread_hz":width,"differential_hz":diff,"snr_db":snr});
                    save_case(&output, &case, &audio);
                    let result = evaluate_fresh(&case, &audio, f, dt, seed_index >= 4);
                    writeln!(file, "{result}").unwrap();
                    file.flush().unwrap();
                    eprintln!("dense id={id} seed={seed_index} B={width} diff={diff} snr={snr}");
                    id += 1;
                }
            }
        }
    }
    for seed_index in 0..16 {
        for (ki, kind) in [
            "noise_only",
            "sync_only",
            "carrier",
            "swept_carrier",
            "split_payload",
            "sync_only_broad",
        ]
        .into_iter()
        .enumerate()
        {
            let seed = 202611190001 + seed_index as u64 * 100003 + ki as u64 * 1009;
            let broad = kind == "sync_only_broad";
            let (audio, f, dt) = fresh_audio(
                seed,
                "CQ K1ABC FN42",
                if broad { 10.0 } else { 2.0 },
                0.0,
                -6.0,
                if broad { "sync_only" } else { kind },
            );
            let case = json!({"id":id,"seed":seed,"seed_index":seed_index,"fold":if seed_index<8 {"development"} else {"heldout"},
                "target":"CQ K1ABC FN42","target_present":false,"kind":kind,"spread_hz":if broad{10.0}else{2.0},"differential_hz":0.0,"snr_db":-6.0});
            save_case(&output, &case, &audio);
            let result = evaluate_fresh(&case, &audio, f, dt, seed_index >= 8);
            writeln!(file, "{result}").unwrap();
            file.flush().unwrap();
            eprintln!("dense negative id={id} {kind}");
            id += 1;
        }
    }
}

fn save_case(output: &Path, case: &Value, audio: &[f64]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 12000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut wav = hound::WavWriter::create(
        output
            .join("audio")
            .join(format!("{:04}.wav", case["id"].as_u64().unwrap())),
        spec,
    )
    .unwrap();
    for &x in audio {
        let x = (x * 32768.0).round();
        assert!(x.abs() < 32767.0);
        wav.write_sample(x as i16).unwrap();
    }
    wav.finalize().unwrap();
}

#[test]
#[ignore = "explicit saved-waveform baseline error attribution"]
fn polar_false_positive_audit() {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
    use crate::stream::ProfileStreamDecodeSession;
    use crate::{DecodeProfile, SlotTimestamp, StreamDecodeConfig};
    let Some(study) = research_path("FT8RS_POLAR_STUDY") else {
        return;
    };
    let output = study.join("baseline-error-audit.json");
    assert!(!output.exists());
    let mut reports = Vec::new();
    for id in [311, 394, 398] {
        let audio = read_audio(&study.join("dense-v2/audio").join(format!("{id:04}.wav")));
        let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
        for profile in ["wsjtx", "wsjtx-plus", "jtdx", "hybrid", "hybrid-exp"] {
            let config = StreamDecodeConfig {
                profile: DecodeProfile::parse(if profile == "hybrid-exp" {
                    "hybrid"
                } else {
                    profile
                })
                .unwrap(),
                hybrid_experiment: profile == "hybrid-exp",
                nfqso: 1500.0,
                ..Default::default()
            };
            let mut session = ProfileStreamDecodeSession::new(config);
            let rows = session
                .decode_slot_streaming_at(
                    &SlotTimestamp::parse("261111_120000").unwrap(),
                    &samples,
                    &mut |_: &StreamDecodedMessage| Ok(()),
                )
                .unwrap();
            reports
                .push(json!({"id":id,"profile":profile,"rows":rows,"internal_source":"unknown"}));
        }
    }
    std::fs::write(output, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
}

#[test]
#[ignore = "serial full Hybrid before/after snapshot with fixed external fixtures"]
fn polar_hybrid_snapshot() {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
    use crate::decode::hybrid::HybridStreamDecodeSession;
    use crate::{SlotTimestamp, StreamDecodeConfig};
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists());
    std::fs::create_dir_all(&output).unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let study = root.join("../../experiments/2026-11-11-polar-optimization");
    let records: Vec<Value> = std::fs::read_to_string(study.join("dense-v2/observations.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(output.join("snapshots.jsonl")).unwrap());
    let replay = |id: String,
                  truth: Value,
                  audio: Vec<f32>,
                  time: SlotTimestamp,
                  session: &mut HybridStreamDecodeSession,
                  file: &mut std::io::BufWriter<std::fs::File>| {
        let clock = Instant::now();
        let mut callbacks = Vec::new();
        let rows = session
            .decode_slot_streaming_tagged_at(&time, &audio, |row| {
                callbacks.push(json!({"row":row,"ms":clock.elapsed().as_secs_f64()*1000.0}));
                Ok(())
            })
            .unwrap();
        writeln!(file,"{}",json!({"id":id,"truth":truth,"rows":rows,"callbacks":callbacks,
            "finished_ms":clock.elapsed().as_secs_f64()*1000.0,"diagnostics":format!("{:?}",session.last_diagnostics())})).unwrap();
        file.flush().unwrap();
        eprintln!("snapshot {id}");
    };
    for r in records.iter().filter(|r| r["case"]["fold"] == "heldout") {
        let id = r["case"]["id"].as_u64().unwrap();
        let audio = read_audio(&study.join("dense-v2/audio").join(format!("{id:04}.wav")))
            .into_iter()
            .map(|x| x as f32)
            .collect();
        let mut session = HybridStreamDecodeSession::new(StreamDecodeConfig {
            hybrid_experiment: true,
            nfqso: 1500.0,
            ..Default::default()
        });
        replay(
            format!("dense-{id:04}"),
            r["case"].clone(),
            audio,
            SlotTimestamp::parse("261111_120000").unwrap(),
            &mut session,
            &mut file,
        );
    }
    for (name, low) in [("210703_133430", 100.0), ("230208_140300", 200.0)] {
        let audio = read_audio(&root.join("tests/ft8").join(format!("{name}.wav")));
        let mut session = HybridStreamDecodeSession::new(StreamDecodeConfig {
            hybrid_experiment: true,
            nfqso: 1500.0,
            nfa: low,
            ..Default::default()
        });
        let start = SlotTimestamp::parse(name).unwrap();
        for (slot, samples) in audio.chunks(180000).enumerate() {
            replay(
                format!("recording-{name}-{slot:02}"),
                Value::Null,
                samples.iter().map(|&x| x as f32).collect(),
                start.add_seconds(slot as i64 * 15),
                &mut session,
                &mut file,
            );
        }
    }
}

#[test]
#[ignore = "development-only fast-channel gate calibration"]
fn polar_gate_calibration() {
    if cfg!(debug_assertions) {
        panic!("use optimized build");
    }
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists());
    std::fs::create_dir_all(&output).unwrap();
    let study = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../experiments/2026-11-11-polar-optimization");
    let records: Vec<Value> = std::fs::read_to_string(study.join("dense-v2/observations.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(output.join("gate.jsonl")).unwrap());
    for case in records
        .iter()
        .filter(|r| r["case"]["fold"] == "development")
    {
        let id = case["case"]["id"].as_u64().unwrap();
        let audio = read_audio(&study.join("dense-v2/audio").join(format!("{id:04}.wav")));
        let f = case["known_frequency_hz"].as_f64().unwrap();
        let dt = case["known_dt_s"].as_f64().unwrap();
        let target = case["case"]["target"].as_str().unwrap();
        let fields: Vec<_> = target.split_whitespace().collect();
        let context = polar::ApContext {
            enabled: true,
            mycall: Some("K1JT"),
            hiscall: Some(fields[1]),
            tails: true,
            frequency: f,
            width: 50.0,
        };
        let band = Band::new(&audio, f + 22.0);
        let hints = polar::hints(context, f);
        for (mi, model) in polar::MODELS.into_iter().enumerate() {
            let llr = polar::likelihoods(&band, f, dt, model);
            for prior in std::iter::once(None).chain(hints.iter().map(Some)) {
                if let Some((msg, tones)) = polar::decode_hint(&llr, prior) {
                    let row = Row {
                        msg: msg.clone(),
                        f,
                        dt,
                        method: "gate-calibration".into(),
                        tones: Some(tones),
                    };
                    let fit = polar::support(&audio, &row, model, &llr).unwrap();
                    writeln!(file,"{}",json!({"id":id,"case":case["case"],"model":mi,"ap":prior.map_or("none",|p|p.name),
                        "message":msg,"correct":case["case"]["target_present"]==true && msg==target,
                        "score":fit.data_score,"groups":fit.groups,"soft":fit.soft_score,"accepted":fit.accepted()})).unwrap();
                }
            }
        }
        file.flush().unwrap();
    }
}

#[test]
#[ignore = "controlled AP/FEC comparison; correct and deliberately wrong context"]
fn polar_ap_comparison() {
    if cfg!(debug_assertions) {
        panic!("use optimized build");
    }
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists());
    std::fs::create_dir_all(&output).unwrap();
    let mut file = std::io::BufWriter::new(std::fs::File::create(output.join("ap.jsonl")).unwrap());
    for seed_index in 0..4 {
        for (message_index, message) in ["CQ BG5ATV PM00", "K1JT BG5ATV -04", "K1JT BG5ATV RR73"]
            .into_iter()
            .enumerate()
        {
            for (wi, width) in [2.0, 5.0, 10.0].into_iter().enumerate() {
                let seed = 202611220001
                    + seed_index * 100003
                    + message_index as u64 * 1009
                    + wi as u64 * 71;
                for snr in [-24.0, -21.0, -18.0, -15.0, -12.0] {
                    let (audio, f, dt) = fresh_audio(seed, message, width, 0.0, snr, "target");
                    let band = Band::new(&audio, f + 22.0);
                    for metric in ["amp1", "gp10"] {
                        let model = polar::MODELS[2];
                        let llr = if metric == "amp1" {
                            polar_llr(&band, f, dt, "amp1")
                        } else {
                            polar::likelihoods(&band, f, dt, model)
                        };
                        for (wrong, his) in [(false, "BG5ATV"), (true, "W9XYZ")] {
                            let ctx = polar::ApContext {
                                enabled: true,
                                mycall: Some("K1JT"),
                                hiscall: Some(his),
                                tails: true,
                                frequency: f,
                                width: 50.0,
                            };
                            let hints = polar::hints(ctx, f);
                            let mut rows = Vec::new();
                            for prior in std::iter::once(None).chain(hints.iter().map(Some)) {
                                let decoded = polar::decode_hint(&llr, prior);
                                let fit = decoded.as_ref().and_then(|(msg, tones)| {
                                    polar::support(
                                        &audio,
                                        &Row {
                                            msg: msg.clone(),
                                            f,
                                            dt,
                                            method: "ap-study".into(),
                                            tones: Some(*tones),
                                        },
                                        model,
                                        &llr,
                                    )
                                });
                                rows.push(json!({"ap":prior.map_or("none",|p|p.name),"known_bits":prior.map_or(0,|p|p.mask.iter().filter(|&&x|x==1).count()),
                                    "decoded":decoded.as_ref().map(|r|&r.0),"correct":decoded.as_ref().is_some_and(|(m,t)|m==message && *t==tones_for(message)),
                                    "score":fit.map(|x|x.data_score),"groups":fit.map(|x|x.groups),"soft":fit.map(|x|x.soft_score),
                                    "accepted":fit.is_some_and(|x|x.accepted())}));
                            }
                            writeln!(file,"{}",json!({"seed":seed,"message":message,"width":width,"snr":snr,"metric":metric,"wrong_context":wrong,"rows":rows})).unwrap();
                        }
                    }
                    file.flush().unwrap();
                }
            }
        }
    }
}

#[test]
#[ignore = "1024 fresh negative slots with wrong target/calls/tail AP"]
fn polar_negative_validation() {
    if cfg!(debug_assertions) {
        panic!("use optimized build");
    }
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists());
    std::fs::create_dir_all(&output).unwrap();
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(output.join("negative.jsonl")).unwrap());
    for i in 0..1024 {
        let kind = [
            "noise_only",
            "sync_only",
            "carrier",
            "swept_carrier",
            "sync_only_broad",
            "split_payload",
            "blocker_only",
            "colored_noise",
        ][i % 8];
        let seed = 202611270001 + i as u64 * 100003;
        let message = if kind == "blocker_only" {
            "CQ W9XYZ EN50"
        } else {
            "CQ K1ABC FN42"
        };
        let broad = kind == "sync_only_broad" || kind == "blocker_only";
        let actual_kind = if kind == "sync_only_broad" {
            "sync_only"
        } else if kind == "blocker_only" {
            "target"
        } else {
            kind
        };
        let (mut audio, f, dt) = fresh_audio(
            seed,
            message,
            if broad { 10.0 } else { 2.0 },
            0.0,
            -6.0,
            actual_kind,
        );
        if kind == "colored_noise" {
            let mut rng = polar_random::Noise::new(seed + 907);
            let colored = fading(seed + 701, 10.0);
            audio = (0..180000)
                .map(|j| {
                    ((rng.gaussian()
                        + 0.6
                            * (colored[j]
                                * C::from_polar(1.0, TAU * (f + 22.0) * j as f64 / 12000.0))
                            .re)
                        * 256.0)
                        .round()
                        / 32768.0
                })
                .collect();
        }
        let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
        let clock = Instant::now();
        let mut callbacks = Vec::new();
        let report = polar::recover(
            ExperimentInput {
                samples: &samples,
                baseline: &[],
                low: 200.0,
                high: 3000.0,
                deadline: clock + Duration::from_secs(2),
            },
            polar::ApContext {
                enabled: true,
                mycall: Some("K1JT"),
                hiscall: Some("BG5ATV"),
                tails: true,
                frequency: f,
                width: 50.0,
            },
            &ExperimentDiagnostics::default(),
            &mut |r| callbacks.push(json!({"row":r,"ms":clock.elapsed().as_secs_f64()*1000.0})),
        );
        let wrong: Vec<_> = callbacks
            .iter()
            .filter(|r| {
                !(kind == "blocker_only" && r["row"]["msg"] == message)
                    && !(kind == "split_payload"
                        && ["CQ K1ABC FN42", "CQ W9XYZ EN50"]
                            .contains(&r["row"]["msg"].as_str().unwrap()))
            })
            .cloned()
            .collect();
        writeln!(file,"{}",json!({"id":i,"fold":if i<512 {"calibration"}else{"heldout"},"seed":seed,"kind":kind,"frequency":f,"dt":dt,
            "callbacks":callbacks,"wrong":wrong,"finish_ms":clock.elapsed().as_secs_f64()*1000.0,"diagnostics":format!("{report:?}")})).unwrap();
        file.flush().unwrap();
        if i % 32 == 0 {
            eprintln!("negative {i}/1024");
        }
    }
}

#[test]
#[ignore = "paired full-Hybrid AP ablation on 192 fresh known-truth slots"]
fn polar_ap_hybrid_ablation() {
    if cfg!(debug_assertions) {
        panic!("use optimized build");
    }
    use crate::decode::hybrid::HybridStreamDecodeSession;
    use crate::{SlotTimestamp, StreamDecodeConfig};
    let Some(output) = research_path("FT8RS_POLAR_OUTPUT") else {
        return;
    };
    assert!(!output.exists());
    std::fs::create_dir_all(output.join("audio")).unwrap();
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(output.join("paired-ap.jsonl")).unwrap());
    let mut id = 0;
    for seed_index in 0..8 {
        for (mi, message) in ["K1JT BG5ATV -04", "K1JT BG5ATV RR73"]
            .into_iter()
            .enumerate()
        {
            for (wi, width) in [7.5, 10.0].into_iter().enumerate() {
                for (di, difference) in [0.0, 6.0].into_iter().enumerate() {
                    let seed = 202611290001
                        + seed_index * 100003
                        + mi as u64 * 1009
                        + wi as u64 * 71
                        + di as u64 * 19;
                    for snr in [-21.0, -18.0, -15.0] {
                        let (audio, f, dt) =
                            fresh_audio(seed, message, width, difference, snr, "target");
                        let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
                        let case = json!({"id":id,"seed":seed,"seed_index":seed_index,"message":message,
                            "width":width,"difference":difference,"snr":snr,"frequency":f,"dt":dt});
                        save_case(&output, &case, &audio);
                        for enabled in if id % 2 == 0 {
                            [false, true]
                        } else {
                            [true, false]
                        } {
                            let config = StreamDecodeConfig {
                                hybrid_experiment: true,
                                mycall: Some("K1JT".into()),
                                hiscall: Some("BG5ATV".into()),
                                nfqso: f,
                                nQSOProgress: if mi == 0 { 2 } else { 3 },
                                ..Default::default()
                            };
                            let config_json = serde_json::to_value(&config).unwrap();
                            let mut session = HybridStreamDecodeSession::new(config);
                            let clock = Instant::now();
                            let mut callbacks = Vec::new();
                            let run = || {
                                session.decode_slot_streaming_tagged_at(&SlotTimestamp::parse("261111_120000").unwrap(),&samples,
                                |r| {callbacks.push(json!({"row":r,"ms":clock.elapsed().as_secs_f64()*1000.0}));Ok(())}).unwrap()
                            };
                            let rows = if enabled {
                                let mut run = run;
                                run()
                            } else {
                                polar::without_polar(run)
                            };
                            writeln!(file,"{}",json!({"case":case,"polar_enabled":enabled,"config":config_json,
                                "rows":rows,"callbacks":callbacks,"finished_ms":clock.elapsed().as_secs_f64()*1000.0,
                                "diagnostics":format!("{:?}",session.last_diagnostics())})).unwrap();
                            file.flush().unwrap();
                        }
                        if id % 12 == 0 {
                            eprintln!("paired AP {id}/192");
                        }
                        id += 1;
                    }
                }
            }
        }
    }
}

#[test]
fn polar_prior_masks_are_bounded_and_context_is_optional() {
    assert!(polar::hints(polar::ApContext::default(), 1000.0).is_empty());
    let context = polar::ApContext {
        enabled: true,
        mycall: Some("K1JT"),
        hiscall: Some("BG5ATV"),
        tails: true,
        frequency: 1000.0,
        width: 50.0,
    };
    let hints = polar::hints(context, 1000.0);
    assert_eq!(
        hints
            .iter()
            .map(|h| h.mask.iter().filter(|&&x| x == 1).count())
            .collect::<Vec<_>>(),
        [32, 32, 61, 61, 77, 77, 77]
    );
    assert!(hints.iter().all(|h| h.mask[77..].iter().all(|&x| x == 0)));
    assert_eq!(polar::hints(context, 1200.0).len(), 1);
    assert_eq!(
        polar::hints(
            polar::ApContext {
                hiscall: Some("<...>"),
                ..context
            },
            1000.0
        )
        .len(),
        1
    );
}

#[test]
fn polar_ap_increment_preserves_full_hybrid_callbacks() {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
    use crate::decode::hybrid::HybridStreamDecodeSession;
    use crate::stream::session::StreamDecodeProvenance;
    use crate::{DecodeProfile, SlotTimestamp, StreamDecodeConfig};
    // Frozen after the independently generated eight-seed paired confirmation.
    // Includes a report under known calls and an end message under maximum AP.
    for (seed, message, width, difference, snr, progress) in [
        (202611290020, "K1JT BG5ATV -04", 7.5, 6.0, -15.0, 2),
        (202611291010, "K1JT BG5ATV RR73", 7.5, 0.0, -18.0, 3),
    ] {
        let (audio, f, _dt) = fresh_audio(seed, message, width, difference, snr, "target");
        let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
        let config = StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            hybrid_experiment: true,
            mycall: Some("K1JT".into()),
            hiscall: Some("BG5ATV".into()),
            nfqso: f,
            nQSOProgress: progress,
            ..Default::default()
        };
        let run = || {
            let mut session = HybridStreamDecodeSession::new(config.clone());
            let mut callbacks = Vec::new();
            session
                .decode_slot_streaming_tagged_at(
                    &SlotTimestamp::parse("261111_120000").unwrap(),
                    &samples,
                    |r| {
                        callbacks.push(r.clone());
                        Ok(())
                    },
                )
                .unwrap();
            callbacks
        };
        let before = polar::without_polar(run);
        let after = run();
        assert!(!before.iter().any(|r| r.decode.msg == message));
        assert_eq!(
            serde_json::to_value(&after[..before.len()]).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        let recovered = after
            .iter()
            .find(|r| r.decode.msg == message)
            .expect("lost polar + AP increment");
        assert_eq!(recovered.decode.itone, tones_for(message));
        assert_eq!(recovered.provenance, StreamDecodeProvenance::Experiment);
        assert!(!recovered.provenance.is_wsjtx_history_seed());
    }
}

#[test]
fn polar_skips_partial_invalid_or_exhausted_inputs() {
    let mut samples = vec![0.001f32; 180000];
    let run = |audio: &[f32], previous: &ExperimentDiagnostics| {
        polar::recover(
            ExperimentInput {
                samples: audio,
                baseline: &[],
                low: 200.0,
                high: 3000.0,
                deadline: Instant::now() + Duration::from_secs(1),
            },
            polar::ApContext::default(),
            previous,
            &mut |_| panic!("invalid input emitted"),
        )
    };
    assert!(run(&samples[..179999], &ExperimentDiagnostics::default()).is_none());
    assert!(run(
        &samples,
        &ExperimentDiagnostics {
            fec_calls: MAX_FEC,
            ..Default::default()
        }
    )
    .is_none());
    samples[100] = f32::NAN;
    assert!(run(&samples, &ExperimentDiagnostics::default()).is_none());
}
