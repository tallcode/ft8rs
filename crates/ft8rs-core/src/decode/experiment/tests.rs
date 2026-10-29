use super::*;
#[test]
fn partial_input_is_not_observed_zero_padding() {
    let band = Band::new(&vec![0.0; 144000], 1000.0);
    let spectra = band.spectra(1000.0, 0.0, 0.0);
    assert_eq!(spectra.observed[0], 32);
    assert_eq!(spectra.observed[78], 0);
    let full = Band::new(&vec![0.0; 180000], 1000.0);
    assert_eq!(full.spectra(1000.0, 0.0, 0.0).observed[78], 32);
}
#[test]
fn expired_budget_silence_and_invalid_input_never_emit() {
    for samples in [vec![0.0; 180000], vec![f32::NAN; 180000], vec![0.0; 100]] {
        let mut rows = 0;
        let result = run(
            ExperimentInput {
                samples: &samples,
                baseline: &[],
                low: 200.0,
                high: 3000.0,
                deadline: Instant::now() + Duration::from_secs(1),
            },
            &mut |_| rows += 1,
        );
        assert_eq!(rows, 0);
        assert_eq!(result.fec_calls, 0);
    }
    let result = run(
        ExperimentInput {
            samples: &vec![1.0; 180000],
            baseline: &[],
            low: 200.0,
            high: 3000.0,
            deadline: Instant::now(),
        },
        &mut |_| panic!("expired budget"),
    );
    assert!(result.budget_exhausted);
}
#[test]
fn hybrid_cw_experiment_adds_truth_without_replacing_baseline() {
    use crate::decode::hybrid::HybridStreamDecodeSession;
    use crate::{SlotTimestamp, StreamDecodeConfig};
    let bits = crate::decode::packjt77::pack77("CQ K1ABC FN42");
    let tones = crate::decode::genft8::get_ft8_tones_from_77bits(&bits);
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, 1000.37);
    let mut state = 2026106991u64;
    let mut uniform = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        ((state.wrapping_mul(2685821657736338717) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let mut audio = Vec::with_capacity(180000);
    for _ in 0..90000 {
        let r = (-2.0 * uniform().ln()).sqrt();
        let p = TAU * uniform();
        audio.push(r * p.cos());
        audio.push(r * p.sin());
    }
    let amplitude = (2.0 * 2500.0 / 6000.0 * 10f64.powf(-2.0)).sqrt();
    let start = ((0.143 + 0.5) * FS).round() as usize;
    for i in 0..re.len().min(audio.len() - start) {
        audio[start + i] += amplitude * (re[i] * 0.27f64.cos() - im[i] * 0.27f64.sin());
    }
    for (i, y) in audio.iter_mut().enumerate() {
        *y += 0.45 * (TAU * 1012.9 * i as f64 / FS + 0.4).cos();
    }
    let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
    let timestamp = SlotTimestamp::parse("261029_120000").unwrap();
    let mut results = Vec::new();
    for enabled in [false, true] {
        let mut session = HybridStreamDecodeSession::new(StreamDecodeConfig {
            hybrid_experiment: enabled,
            ..Default::default()
        });
        let mut callbacks = Vec::new();
        let rows = session
            .decode_slot_streaming_at(&timestamp, &samples, |row| {
                callbacks.push(row.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(callbacks.len(), rows.len());
        results.push(rows);
    }
    for row in &results[0] {
        assert!(results[1]
            .iter()
            .any(|other| crate::decode::cooperative::same_signal(row, other)));
    }
    assert!(!results[0].iter().any(|r| r.msg == "CQ K1ABC FN42"));
    assert!(results[1].iter().any(|r| r.msg == "CQ K1ABC FN42"));
    assert!(results[1].iter().all(|r| r.msg == "CQ K1ABC FN42"));
}

fn waveform_row(message: &str, f: f64, dt: f64) -> (Row, Vec<f64>) {
    let bits = crate::decode::packjt77::pack77(message);
    let tones = crate::decode::genft8::get_ft8_tones_from_77bits(&bits);
    let (re, _) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, f);
    let mut audio = vec![0.0; 180000];
    let start = ((dt + 0.5) * FS).round() as usize;
    audio[start..start + re.len()].copy_from_slice(&re);
    (
        Row {
            msg: message.into(),
            f,
            dt,
            method: "fixture".into(),
            tones: Some(tones),
        },
        audio,
    )
}

#[test]
fn data_gate_rejects_common_sync_without_a_payload() {
    let (row, mut audio) = waveform_row("CQ K1ABC FN42", 1000.37, 0.143);
    let start = ((row.dt + 0.5) * FS).round() as usize;
    for (j, x) in audio.iter_mut().enumerate() {
        let symbol = j.checked_sub(start).map(|i| i / 1920);
        if !symbol.is_some_and(|k| k < 7 || (36..43).contains(&k) || (72..79).contains(&k)) {
            *x = 0.0;
        }
        // Deterministic broadband noise prevents a zero-noise special case.
        let n = (j as u64)
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *x += ((n >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 0.02;
    }
    let fit = fit_message(&audio, &row, Instant::now() + Duration::from_secs(2)).unwrap();
    assert!(
        fit.score > MIN_SCORE,
        "sync-only fixture must challenge the full-frame gate"
    );
    assert!(
        !fit.data_supported(),
        "public sync is not evidence of a payload"
    );
}

#[test]
fn native_models_use_original_tones_and_keep_private_audio() {
    let (row, mut audio) = waveform_row("CQ K1ABC FN42", 1000.37, 0.143);
    let (_, weak) = waveform_row("CQ W9XYZ EN50", 975.74, 0.236);
    for (x, y) in audio.iter_mut().zip(&weak) {
        *x += 0.1 * y;
    }
    let original = audio.clone();
    let seed = StreamDecodedMessage {
        freq: row.f,
        dt: row.dt,
        snr: -5.0,
        sync: 2.0,
        // Display placeholders must not be re-encoded to make a waveform.
        msg: "<...> K1ABC FN42".into(),
        itone: row.tones.unwrap(),
    };
    let mut budget = Budget {
        deadline: Instant::now() + Duration::from_secs(3),
        low: 200.0,
        high: 3000.0,
        diagnostics: Default::default(),
    };
    let models = native_models(&audio, std::slice::from_ref(&seed), &mut budget);
    assert_eq!(models.len(), 1);
    assert_eq!(audio, original);
    let residual: Vec<f64> = audio
        .iter()
        .zip(&models[0].wave)
        .map(|(x, y)| x - y)
        .collect();
    let weak_power: f64 = weak.iter().map(|x| x * x).sum();
    let weak_gain: f64 = residual.iter().zip(&weak).map(|(x, y)| x * y).sum::<f64>() / weak_power;
    assert!(
        (0.08..0.12).contains(&weak_gain),
        "weak waveform lost: {weak_gain}"
    );
    let before: f64 = audio.iter().map(|x| x * x).sum();
    let after: f64 = residual.iter().map(|x| x * x).sum();
    assert!(after < before * 0.03, "strong waveform not removed");
    let mut invalid = seed;
    invalid.itone = [0; 79];
    assert!(native_models(&audio, &[invalid], &mut budget).is_empty());
}
