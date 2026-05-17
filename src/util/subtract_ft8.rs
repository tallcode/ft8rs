//! subtractft8 — precise port of WSJTX lib/ft8/subtractft8.f90
//! 
//! Uses time-domain convolution for the LPF to avoid FFT size issues.

use std::f64::consts::PI;

const NFRAME: usize = 1920 * 79;   // 151680
const NFILT: usize = 4000;
const HALF_FILT: usize = NFILT / 2; // 2000
const SAMPLE_RATE: f64 = 12000.0;
const NSPS_WAVE: usize = 1920;

/// GFSK pulse: g(t) = 0.5 * [erf(cb·(t+0.5)) - erf(cb·(t-0.5))]
fn gfsk_pulse(bt: f64, tt: f64) -> f64 {
    let c = PI * (2.0 / std::f64::consts::LN_2).sqrt();
    0.5 * (erf_approx(c * bt * (tt + 0.5)) - erf_approx(c * bt * (tt - 0.5)))
}

fn erf_approx(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t
        - 0.284496736) * t + 0.254829592) * t)
        * (-ax * ax).exp();
    sign * y
}

/// Generate complex FT8 reference waveform — exact port of gen_ft8wave with icmplx=1.
fn gen_ft8wave(itone: &[i32; 79], f0: f64) -> (Vec<f64>, Vec<f64>) {
    let nsps: usize = NSPS_WAVE;
    let nsym: usize = 79;
    let bt: f64 = 2.0;
    let twopi = 2.0 * PI;
    let dt = 1.0 / SAMPLE_RATE;
    let ntab: usize = 65536;
    let twopi_over_ntab = twopi / ntab as f64;

    // GFSK pulse
    let pulse: Vec<f64> = (0..3 * nsps)
        .map(|i| {
            let tt = (i as f64 + 1.0 - 1.5 * nsps as f64) / nsps as f64;
            gfsk_pulse(bt, tt)
        })
        .collect();

    // dphi: (nsym+2)*nsps
    let dphi_len = (nsym + 2) * nsps;
    let mut dphi = vec![0.0f64; dphi_len];
    let dphi_peak = twopi / nsps as f64;

    for j in 0..nsym {
        let ib = j * nsps;
        let tone = itone[j] as f64;
        for i in 0..3 * nsps {
            dphi[ib + i] += dphi_peak * pulse[i] * tone;
        }
    }

    // Dummy symbols
    let first_tone = itone[0] as f64;
    for i in 0..2 * nsps {
        dphi[i] += dphi_peak * first_tone * pulse[nsps + i];
    }
    let last_tone = itone[nsym - 1] as f64;
    let tail_base = nsym * nsps;
    for i in 0..2 * nsps {
        dphi[tail_base + i] += dphi_peak * last_tone * pulse[i];
    }

    let carrier_dphi = twopi * f0 * dt;
    for i in 0..dphi_len {
        dphi[i] += carrier_dphi;
    }

    // Generate complex waveform (skip first nsps dummy samples)
    let nwave = nsym * nsps; // 151680
    debug_assert_eq!(nwave, NFRAME);
    let mut cwave_re = vec![0.0f64; nwave];
    let mut cwave_im = vec![0.0f64; nwave];
    let mut phi = 0.0f64;

    for j in nsps..(nsps + nwave) {
        let k = j - nsps;
        let idx = ((phi / twopi_over_ntab) as usize) % ntab;
        cwave_re[k] = (idx as f64 * twopi_over_ntab).cos();
        cwave_im[k] = (idx as f64 * twopi_over_ntab).sin();
        phi += dphi[j];
        while phi >= twopi { phi -= twopi; }
    }

    // Envelope shaping
    let nramp = (nsps as f64 / 8.0).round() as usize;
    for i in 0..nramp {
        let env = (1.0 - (twopi * i as f64) / (2.0 * nramp as f64)).cos() / 2.0;
        cwave_re[i] *= env;
        cwave_im[i] *= env;
    }
    let k1 = nsym * nsps - nramp;
    for i in 0..nramp {
        let env = (1.0 + (twopi * i as f64) / (2.0 * nramp as f64)).cos() / 2.0;
        cwave_re[k1 + i] *= env;
        cwave_im[k1 + i] *= env;
    }

    (cwave_re, cwave_im)
}

/// Precomputed LPF window (time domain)
use std::sync::OnceLock;

static LPF_WINDOW: OnceLock<(Vec<f64>, f64)> = OnceLock::new();

fn lpf_window_data() -> &'static (Vec<f64>, f64) {
    LPF_WINDOW.get_or_init(|| {
        let mut window = vec![0.0f64; NFILT + 1];
        let mut sumw = 0.0f64;
        for j in 0..=NFILT {
            let j_signed = j as isize - HALF_FILT as isize;
            window[j] = (PI * j_signed as f64 / NFILT as f64).cos().powi(2);
            sumw += window[j];
        }
        (window, sumw)
    })
}

/// Time-domain LPF convolution: cfilt = camp * window / sumw
/// Matches Fortran: four2a → multiply → four2a (circular convolution)
/// But we do it directly in time domain to avoid FFT size issues.
fn lpf_convolve(camp_re: &[f64], camp_im: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let (window, sumw) = lpf_window_data();
    let sumw = *sumw;
    let n = camp_re.len();

        let mut cfilt_re = vec![0.0f64; n];
        let mut cfilt_im = vec![0.0f64; n];

    // Optimized convolution: pre-extend camp with HALF_FILT samples of circular halo
        // to avoid expensive rem_euclid in inner loop.
        // Original: camp_idx = (i + HALF_FILT - tau) mod n → need modulo
        // With extension: ext[i + NFILT - tau] → direct indexing, no modulo
        let ext_len = n + NFILT;
        let mut ext_re = vec![0.0f64; ext_len];
        let mut ext_im = vec![0.0f64; ext_len];
        // ext[j] = camp[ (j - HALF_FILT) mod n ]
        // So ext[i + NFILT - tau] = camp[ (i + NFILT - tau - HALF_FILT) mod n ] = camp[ (i + HALF_FILT - tau) mod n ] ✓
        for j in 0..ext_len {
            let camp_idx = (j as isize - HALF_FILT as isize).rem_euclid(n as isize) as usize;
            ext_re[j] = camp_re[camp_idx];
            ext_im[j] = camp_im[camp_idx];
        }

        // Parallel convolution: each output position is independent
        // Use par_iter for 4-core speedup on 151,680 iterations
        use rayon::prelude::*;
        cfilt_re.par_iter_mut().enumerate().zip(cfilt_im.par_iter_mut().enumerate()).for_each(|((i, re_out), (_, im_out))| {
            let mut sum_re = 0.0f64;
            let mut sum_im = 0.0f64;
            for tau in 0..=NFILT {
                let idx = i + NFILT - tau;
                let w = window[tau] / sumw;
                sum_re += ext_re[idx] * w;
                sum_im += ext_im[idx] * w;
            }
            *re_out = sum_re;
            *im_out = sum_im;
        });

    (cfilt_re, cfilt_im)
}

/// Main subtract function — matches Fortran subtractft8 exactly.
/// dd0 is modified in-place. dt: time offset from data start in seconds.
pub fn subtract_ft8(dd0: &mut Vec<f64>, itone: &[i32; 79], f0: f64, dt: f64) {


    let (cref_re, cref_im) = gen_ft8wave(itone, f0);
    let nmax = 15 * 12000; // 180000
    let nstart = (dt * SAMPLE_RATE).round() as isize + 1; // Fortran 1-indexed

    // camp(i) = dd0[j] * conjg(cref(i)), j = nstart - 1 + i
    let mut camp_re = vec![0.0f64; NFRAME];
    let mut camp_im = vec![0.0f64; NFRAME];

    for i in 0..NFRAME {
        let j = (nstart - 1 + i as isize) as isize;
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let d = dd0[(j - 1) as usize];
            camp_re[i] = d * cref_re[i];
            camp_im[i] = -d * cref_im[i];
        }
    }

    // LPF via time-domain convolution (matches FFT-based convolution)
    let (cfilt_re, cfilt_im) = lpf_convolve(&camp_re, &camp_im);

    // Subtract: dd0[j] -= 2 * real(cfilt(i) * cref(i))
    for i in 0..NFRAME {
        let j = (nstart - 1 + i as isize) as isize;
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let z_re = cfilt_re[i] * cref_re[i] - cfilt_im[i] * cref_im[i];
            dd0[(j - 1) as usize] -= 2.0 * z_re;
        }
    }
}
