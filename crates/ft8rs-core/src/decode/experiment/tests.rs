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
            &samples,
            200.0,
            3000.0,
            Instant::now() + Duration::from_secs(1),
            &mut |_| rows += 1,
        );
        assert_eq!(rows, 0);
        assert_eq!(result.fec_calls, 0);
    }
    let result = run(
        &vec![1.0; 180000],
        200.0,
        3000.0,
        Instant::now(),
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
