//! Fine-frequency RR73 recovery and its full-slot measurement probes.
#[cfg(test)]
use super::build_bit_metrics;
use super::{
    compute_nsync, extract_soft_symbols, ft8_downsample, normalize_bmet, DecodeWorkspace, COSTAS,
    GRAY_MAP, NN, N_LDPC,
};
#[cfg(test)]
use crate::decode::lib_wsjtx_plus::validation::rr73_replay::{
    record_soft, watch_soft, SoftObservation,
};

/// Evaluate each signal treatment on the same fixed synthetic timing and
/// frequency grid. The truth bits never enter metric construction or FEC.
#[cfg(test)]
pub(in crate::decode::lib_wsjtx_plus) fn measure_audio(audio: &[f64], freq: f64, dt: f64) {
    use super::super::{create_decode_workspace, DT2, NFFT1_LONG};
    use crate::decode::lib_wsjtx_plus::fft::four2a_r2c;
    let mut cx_re = vec![0.0; NFFT1_LONG];
    let mut cx_im = vec![0.0; NFFT1_LONG];
    cx_re[..audio.len()].copy_from_slice(audio);
    four2a_r2c(&mut cx_re, &mut cx_im);
    let time = ((dt + 0.5) / DT2).round() as isize + 1;
    let mut workspace = create_decode_workspace();
    ft8_downsample(&cx_re, &cx_im, freq, &mut workspace);
    extract_soft_symbols(time, &mut workspace);
    for imetric in [1, 2] {
        build_bit_metrics(&mut workspace, imetric);
        capture_grid(
            &cx_re,
            &cx_im,
            freq,
            time,
            imetric,
            &workspace,
            Grid::Fixture,
        );
    }
}

#[cfg(test)]
fn scaled(metric: &[f64]) -> Vec<f64> {
    metric
        .iter()
        .map(|value| (2.83f32 * *value as f32) as f64)
        .collect()
}

#[cfg(test)]
pub(super) fn capture(
    cx_re: &[f64],
    cx_im: &[f64],
    freq: f64,
    time: isize,
    imetric: usize,
    original: &DecodeWorkspace,
) {
    if watch_soft(freq) {
        capture_grid(
            cx_re,
            cx_im,
            freq,
            time,
            imetric,
            original,
            Grid::DecoderCandidate,
        );
    }
}

#[cfg(test)]
enum Grid {
    Fixture,
    DecoderCandidate,
}

#[cfg(test)]
fn capture_grid(
    cx_re: &[f64],
    cx_im: &[f64],
    freq: f64,
    time: isize,
    imetric: usize,
    original: &DecodeWorkspace,
    grid: Grid,
) {
    let (offsets, min_sync): (&[f64], usize) = match grid {
        Grid::Fixture => (&[-0.50, -0.25, 0.0, 0.25, 0.50], 0),
        // Include neighboring bins of the 192000-point input spectrum.
        Grid::DecoderCandidate => (
            &[
                -0.50, -0.25, -0.125, -0.0625, 0.0, 0.0625, 0.125, 0.25, 0.50,
            ],
            8,
        ),
    };
    let original_sync = compute_nsync(&original.s8);
    for (i, llr) in [
        &original.bmeta,
        &original.bmetb,
        &original.bmetc,
        &original.bmetd,
        &original.bmete,
    ]
    .into_iter()
    .enumerate()
    {
        record_soft(SoftObservation {
            freq,
            time,
            metric: i + 1,
            baseline: true,
            nsync: original_sync,
            llr: scaled(llr),
        });
    }
    let mut probe = super::super::create_decode_workspace();
    for &df in offsets {
        let f = freq + df;
        ft8_downsample(cx_re, cx_im, f, &mut probe);
        for dt in [-3, -1, 0, 1, 3] {
            let t = time + dt;
            extract_soft_symbols(t, &mut probe);
            let nsync = compute_nsync(&probe.s8);
            if nsync < min_sync {
                continue;
            }
            build_bit_metrics(&mut probe, imetric);
            for (i, llr) in [
                &probe.bmeta,
                &probe.bmetb,
                &probe.bmetc,
                &probe.bmetd,
                &probe.bmete,
            ]
            .into_iter()
            .enumerate()
            {
                record_soft(SoftObservation {
                    freq: f,
                    time: t,
                    metric: i + 1,
                    baseline: false,
                    nsync,
                    llr: scaled(llr),
                });
            }
            let combined: Vec<_> = (0..N_LDPC)
                .map(|i| probe.bmeta[i] + probe.bmetb[i] + probe.bmetc[i])
                .collect();
            record_soft(SoftObservation {
                freq: f,
                time: t,
                metric: 6,
                baseline: false,
                nsync,
                llr: combined,
            });
            for (i, llr) in probe_pilot_likelihoods(&probe).into_iter().enumerate() {
                record_soft(SoftObservation {
                    freq: f,
                    time: t,
                    metric: i + 7,
                    baseline: false,
                    nsync,
                    llr,
                });
            }
        }
    }
}

fn probe_pilot_likelihoods(workspace: &DecodeWorkspace) -> [Vec<f64>; 4] {
    use rustfft::num_complex::Complex;
    let symbol = |tone: usize, k: usize| {
        Complex::new(
            workspace.cs_re[tone * NN + k],
            workspace.cs_im[tone * NN + k],
        )
    };
    let data_positions = (7..36).chain(43..72);
    let noise: [f64; 8] = std::array::from_fn(|tone| {
        let mut powers: Vec<_> = data_positions
            .clone()
            .map(|k| symbol(tone, k).norm_sqr())
            .collect();
        powers.sort_by(f64::total_cmp);
        powers[powers.len() / 2].max(f64::MIN_POSITIVE)
    });
    let pilots = [0, 36, 72].map(|offset| {
        COSTAS
            .iter()
            .enumerate()
            .map(|(k, &tone)| symbol(tone as usize, offset + k))
            .sum::<Complex<f64>>()
            / 7.0
    });
    let mut metrics: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::with_capacity(N_LDPC));
    for k in data_positions {
        let group = usize::from(k >= 43);
        let frac = (k - group * 36 - 3) as f64 / 36.0;
        let anchor = pilots[group] * (1.0 - frac) + pilots[group + 1] * frac;
        let scores: [[f64; 8]; 4] = std::array::from_fn(|metric| {
            std::array::from_fn(|t| {
                let tone = GRAY_MAP[t] as usize;
                let bin = symbol(tone, k);
                match metric {
                    0 => bin.norm_sqr() / noise[tone],
                    1 => (bin * anchor.conj()).re / noise[tone],
                    2 => (bin * anchor.conj()).re,
                    _ => (bin.norm_sqr() + 2.0 * (bin * anchor.conj()).re) / noise[tone],
                }
            })
        });
        for (metric, scores) in metrics.iter_mut().zip(scores) {
            for mask in [4, 2, 1] {
                let one = scores
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i & mask != 0)
                    .map(|(_, &v)| v)
                    .fold(f64::NEG_INFINITY, f64::max);
                let zero = scores
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i & mask == 0)
                    .map(|(_, &v)| v)
                    .fold(f64::NEG_INFINITY, f64::max);
                metric.push(one - zero);
            }
        }
    }
    for metric in &mut metrics {
        normalize_bmet(metric);
        for v in metric {
            *v = (2.83f32 * *v as f32) as f64;
        }
    }
    metrics
}

/// Recover one current RR73 from the final residual of an incomplete-call Plus session.
/// Candidates come from the standard Costas search and FEC is unmasked.
pub(in crate::decode::lib_wsjtx_plus) fn recover_rr73(
    audio: &[f64],
    focus: f64,
    band: std::ops::RangeInclusive<f64>,
    target: &str,
    mut on_trial: impl FnMut(),
) -> Option<crate::stream::session::StreamDecodedMessage> {
    use super::super::{create_decode_workspace, sync8, DT2, NFFT1_LONG};
    use super::{
        compute_snr, find_best_frequency_shift, find_best_time_offset, refine_time_offset,
        spectrum_noise_power,
    };
    use crate::decode::genft8::get_ft8_tones_from_codeword;
    use crate::decode::lib_wsjtx_plus::{decode174_91::decode174_91, fft::four2a_r2c_packed};
    use crate::decode::packjt77::{is_stdcall, unpack77};
    use crate::stream::session::StreamDecodedMessage;
    use std::time::{Duration, Instant};

    let lo = (focus - 12.5).max(*band.start()).max(3.125);
    let hi = (focus + 12.5).min(*band.end()).min(5000.0);
    if !focus.is_finite() || lo + 6.25 > hi || !is_stdcall(target) {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(125);
    let (candidates, sbase) = sync8(audio, lo, hi, 1.5, focus, 8);
    if candidates.is_empty() || Instant::now() >= deadline {
        return None;
    }
    let mut cx_re = vec![0.0; NFFT1_LONG];
    let mut cx_im = vec![0.0; NFFT1_LONG];
    cx_re[..audio.len()].copy_from_slice(audio);
    four2a_r2c_packed(&mut cx_re, &mut cx_im);
    let mut workspace = create_decode_workspace();

    for candidate in candidates
        .into_iter()
        .filter(|c| (c.freq - focus).abs() <= 7.0)
        .take(2)
    {
        ft8_downsample(&cx_re, &cx_im, candidate.freq, &mut workspace);
        let time = find_best_time_offset(&workspace.cd0_re, &workspace.cd0_im, candidate.dt).ibest;
        let shift = find_best_frequency_shift(&workspace.cd0_re, &workspace.cd0_im, time);
        let freq = candidate.freq + shift.delfbest;
        ft8_downsample(&cx_re, &cx_im, freq, &mut workspace);
        let time = refine_time_offset(
            &workspace.cd0_re,
            &workspace.cd0_im,
            time,
            &mut workspace.ss,
        )
        .ibest;
        for offset in [0.0625, -0.0625, 0.125, -0.125] {
            let f = freq + offset;
            if !band.contains(&f) {
                continue;
            }
            ft8_downsample(&cx_re, &cx_im, f, &mut workspace);
            for dt in [0, -1, 1, -3, 3] {
                if Instant::now() >= deadline {
                    return None;
                }
                extract_soft_symbols(time + dt, &mut workspace);
                if compute_nsync(&workspace.s8) < 14 {
                    continue;
                }
                let [_, whitened, coherent, _] = probe_pilot_likelihoods(&workspace);
                for llr in [coherent, whitened] {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    on_trial();
                    let Some(result) = decode174_91(&llr, &[0; N_LDPC], 3) else {
                        continue;
                    };
                    if result.nharderrors > 36 {
                        continue;
                    }
                    let bits = &result.message91[..77];
                    // Type 1/2 standard message with the current RR73 wire
                    // field. The call is checked only after FEC and unpacking.
                    let field = bits[59..74]
                        .iter()
                        .fold(0u16, |a, &b| (a << 1) | u16::from(b));
                    if field != 32403
                        || bits[58] != 0
                        || !matches!(&bits[74..], [0, 0, 1] | [0, 1, 0])
                    {
                        continue;
                    }
                    let Some(msg) = unpack77(bits, None) else {
                        continue;
                    };
                    let mut fields = msg.split_whitespace();
                    let (Some(other), Some(sender), Some("RR73"), None) =
                        (fields.next(), fields.next(), fields.next(), fields.next())
                    else {
                        continue;
                    };
                    if !sender.eq_ignore_ascii_case(target) || !is_stdcall(other) {
                        continue;
                    }
                    let itone = get_ft8_tones_from_codeword(&result.cw);
                    let tones = itone.map(|tone| tone as u8);
                    let xbase = spectrum_noise_power(&sbase, f)?;
                    let (_, snr) = compute_snr(&workspace.s8, &tones, xbase);
                    return Some(StreamDecodedMessage {
                        freq: f,
                        dt: (time + dt - 1) as f64 * DT2 - 0.5,
                        msg,
                        snr: snr.max(-25.0),
                        sync: candidate.sync,
                        itone,
                    });
                }
            }
        }
    }
    None
}
