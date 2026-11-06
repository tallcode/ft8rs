//! Test-only alternative soft measurements on a full-slot decoder candidate.
use super::{
    build_bit_metrics, compute_nsync, extract_soft_symbols, ft8_downsample, normalize_bmet,
    DecodeWorkspace, COSTAS, GRAY_MAP, NN, N_LDPC,
};
use crate::decode::lib_wsjtx_plus::validation::rr73_replay::{
    record_soft, watch_soft, SoftObservation,
};

fn scaled(metric: &[f64]) -> Vec<f64> {
    metric
        .iter()
        .map(|value| (2.83f32 * *value as f32) as f64)
        .collect()
}

pub(super) fn capture(
    cx_re: &[f64],
    cx_im: &[f64],
    freq: f64,
    time: isize,
    imetric: usize,
    original: &DecodeWorkspace,
) {
    if !watch_soft(freq) {
        return;
    }
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
            llr: scaled(llr),
        });
    }
    let mut probe = super::super::create_decode_workspace();
    for df in [-0.50, -0.25, 0.0, 0.25, 0.50] {
        let f = freq + df;
        ft8_downsample(cx_re, cx_im, f, &mut probe);
        for dt in [-3, -1, 0, 1, 3] {
            let t = time + dt;
            extract_soft_symbols(t, &mut probe);
            if compute_nsync(&probe.s8) < 8 {
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
                llr: combined,
            });
            for (i, llr) in probe_pilot_likelihoods(&probe).into_iter().enumerate() {
                record_soft(SoftObservation {
                    freq: f,
                    time: t,
                    metric: i + 7,
                    baseline: false,
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
