//! subtractft8 — precise port of WSJTX lib/ft8/subtractft8.f90
//!
//! Algorithm (from Fortran comments):
//!   Measured signal  : dd(t) = a(t)·cos(2πf₀t + θ(t))
//!   Reference signal : cref(t) = exp(j·(2πf₀t + φ(t)))
//!   Complex amp      : cfilt(t) = LPF[ dd(t)·CONJG(cref(t)) ]
//!   Subtract         : dd(t) ← dd(t) - 2·REAL(cref(t)·cfilt(t))
//!
//! LPF implementation: FFT-based linear convolution of circularly-extended camp.
//! Camp is pre-extended with HALF_FILT samples on each side (circular halo), then
//! zero-padded to NFFT_CONV. This produces identical results to time-domain circular
//! convolution but uses O(N·logN) FFT instead of O(N·M) direct computation.
//!
//! Key parameters:
//!   NFRAME = 151680 (79 symbols × 1920 samples/symbol)
//!   NFILT  = 4000 (cos² LPF window, ±2000 taps)
//!   NFFT_CONV = 262144 (next pow2 of NFRAME + 2*NFILT for zero-padded linear conv)
//!   NSPS   = 1920 (waveform generation resolution, NOT detection rate of 48)

use crate::util::fft_complex;
use std::f64::consts::PI;
use std::sync::OnceLock;

const NFRAME: usize = 1920 * 79;   // 151680
const NFILT: usize = 4000;
const HALF_FILT: usize = NFILT / 2; // 2000
const SAMPLE_RATE: f64 = 12000.0;
const NSPS_WAVE: usize = 1920;

// NFFT for FFT-based linear convolution with circular halo:
// ext_len = NFRAME + NFILT = 155680, next pow2 = 262144
const NFFT_CONV: usize = 262144;

/// Precomputed FFT of the LPF window. Computed once and reused across all subtract calls.
fn lpf_window_fft() -> &'static (Vec<f64>, Vec<f64>) {
    static WINDOW_FFT: OnceLock<(Vec<f64>, Vec<f64>)> = OnceLock::new();
    WINDOW_FFT.get_or_init(|| {
        let nfft = NFFT_CONV;

        // Build cos² window: w(j) = cos²(π·j/NFILT) for j = -2000..2000
        let mut sumw: f64 = 0.0;
        let mut win = vec![0.0f64; nfft];
        for j in 0..=NFILT {
            let j_signed = j as isize - HALF_FILT as isize;
            win[j] = (PI * j_signed as f64 / NFILT as f64).cos().powi(2);
            sumw += win[j];
        }
        // Normalize window by sumw (matching Fortran window/sumw)
        for j in 0..=NFILT {
            win[j] /= sumw;
        }

        // FFT of window (window at indices 0..4000, zero-padded to 262144)
        let mut w_im = vec![0.0f64; nfft];
        fft_complex(&mut win, &mut w_im, false);

        (win, w_im)
    })
}

/// Cached GFSK pulse: computed once, reused across all gen_ft8wave calls.
fn gsfk_pulse_cached() -> &'static Vec<f64> {
    static PULSE: OnceLock<Vec<f64>> = OnceLock::new();
    PULSE.get_or_init(|| {
        let bt = 2.0f64;
        let nsps = NSPS_WAVE;
        (0..3 * nsps)
            .map(|i| {
                let tt = (i as f64 + 1.0 - 1.5 * nsps as f64) / nsps as f64;
                let c = PI * (2.0 / std::f64::consts::LN_2).sqrt();
                0.5 * (erf_approx(c * bt * (tt + 0.5)) - erf_approx(c * bt * (tt - 0.5)))
            })
            .collect()
    })
}

/// Abramowitz & Stegun 7.1.26 erf approximation.
fn erf_approx(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t
        + 0.254829592) * t) * (-ax * ax).exp();
    sign * y
}

/// Generate complex FT8 reference waveform (exact port of gen_ft8wave with icmplx=1).
/// NSPS=1920 at 12000 Hz sample rate, returns (cref_re, cref_im) of length NFRAME.
fn gen_ft8wave(itone: &[i32; 79], f0: f64) -> (Vec<f64>, Vec<f64>) {
    let nsps: usize = NSPS_WAVE;
    let nsym: usize = 79;
    let twopi = 2.0 * PI;
    let dt = 1.0 / SAMPLE_RATE;
    let ntab: usize = 65536;
    let twopi_over_ntab = twopi / ntab as f64;

    let pulse = gsfk_pulse_cached();
    let pulse_len = pulse.len();

    let dphi_len = (nsym + 2) * nsps;
    let mut dphi = vec![0.0f64; dphi_len];
    let dphi_peak = twopi / nsps as f64;

    for j in 0..nsym {
        let ib = j * nsps;
        let tone = itone[j] as f64;
        for i in 0..pulse_len {
            dphi[ib + i] += dphi_peak * pulse[i] * tone;
        }
    }
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

    let nwave = nsym * nsps;
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

/// FFT-based linear convolution with circular halo extension.
/// Produces IDENTICAL results to time-domain circular convolution within NFRAME.
fn lpf_convolve(camp_re: &[f64], camp_im: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let (win_fft_re, win_fft_im) = lpf_window_fft();
    let n = camp_re.len();
    debug_assert_eq!(n, NFRAME);

    // Build extended array with NFILT-sample circular halo:
    // ext[j] = camp[(j - HALF_FILT) mod NFRAME] for j = 0..NFRAME+NFILT
    // This gives the correct circular indexing for the FFT linear convolution.
    let nfft = NFFT_CONV;
    let mut ext_re = vec![0.0f64; nfft];
    let mut ext_im = vec![0.0f64; nfft];

    let ext_len = NFRAME + NFILT; // 155680
    for j in 0..ext_len {
        let camp_idx = if j < HALF_FILT {
            NFRAME - HALF_FILT + j  // circular prepend: camp end
        } else if j < HALF_FILT + NFRAME {
            j - HALF_FILT             // main data
        } else {
            j - HALF_FILT - NFRAME    // circular append: camp beginning
        };
        ext_re[j] = camp_re[camp_idx];
        ext_im[j] = camp_im[camp_idx];
    }

    // FFT → multiply → IFFT (linear convolution, not circular)
    fft_complex(&mut ext_re, &mut ext_im, false);

    for i in 0..nfft {
        let cr = ext_re[i];
        let ci = ext_im[i];
        ext_re[i] = cr * win_fft_re[i] - ci * win_fft_im[i];
        ext_im[i] = cr * win_fft_im[i] + ci * win_fft_re[i];
    }

    fft_complex(&mut ext_re, &mut ext_im, true);

    // Extract cfilt: cfilt[i] = linear_conv[i + NFILT]
    // The convolution with NFILT-sample halo gives the correct circular result
    // at offset NFILT in the linear convolution output.
    let mut cfilt_re = vec![0.0f64; n];
    let mut cfilt_im = vec![0.0f64; n];
    for i in 0..n {
        cfilt_re[i] = ext_re[NFILT + i];
        cfilt_im[i] = ext_im[NFILT + i];
    }

    (cfilt_re, cfilt_im)
}

/// Main subtract function. dd0 is modified in-place.
/// dt: time offset from data start in seconds (matching WSJTX convention).
pub fn subtract_ft8(dd0: &mut Vec<f64>, itone: &[i32; 79], f0: f64, dt: f64) {
    subtract_ft8_refined(dd0, itone, f0, dt, false)
}

/// Subtract with optional dt refinement (WSJT-X lrefinedt).
/// When refined=true, searches ±90 samples for optimal dt using energy minimization.
pub fn subtract_ft8_refined(dd0: &mut Vec<f64>, itone: &[i32; 79], f0: f64, dt: f64, refined: bool) {
    let (cref_re, cref_im) = gen_ft8wave(itone, f0);
    let nmax = 15 * 12000;
    
    // dt refinement (WSJT-X lrefinedt)
    let refined_dt = if refined {
        let offset = refine_dt(dd0, &cref_re, &cref_im, f0, dt);
        if offset.abs() > 90 {
            return; // No acceptable minimum: do not subtract
        }
        dt + (offset as f64 / SAMPLE_RATE)
    } else {
        dt
    };
    
    let nstart = (refined_dt * SAMPLE_RATE).round() as isize + 1;

    // IQ mix: camp(i) = dd0[j] × conjg(cref(i))
    let mut camp_re = vec![0.0f64; NFRAME];
    let mut camp_im = vec![0.0f64; NFRAME];
    for i in 0..NFRAME {
        let j = nstart - 1 + i as isize;
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let d = dd0[(j - 1) as usize];
            camp_re[i] = d * cref_re[i];
            camp_im[i] = -d * cref_im[i];
        }
    }

    // FFT-based LPF convolution
    let (cfilt_re, cfilt_im) = lpf_convolve(&camp_re, &camp_im);

    // Subtract: dd0[j] -= 2 × REAL(cfilt[i] × cref(i))
    for i in 0..NFRAME {
        let j = nstart - 1 + i as isize;
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let z_re = cfilt_re[i] * cref_re[i] - cfilt_im[i] * cref_im[i];
            dd0[(j - 1) as usize] -= 2.0 * z_re;
        }
    }
}

/// Refine dt by minimizing residual energy in signal band (WSJT-X lrefinedt).
/// Tests offsets -90, 0, +90 samples and uses quadratic interpolation.
fn refine_dt(
    dd0: &[f64],
    cref_re: &[f64],
    cref_im: &[f64],
    f0: f64,
    dt: f64,
) -> isize {
    let _nmax = 15 * 12000;
    
    // Compute residual energy at three offsets
    let sqa = compute_residual_energy(dd0, cref_re, cref_im, f0, dt, -90);
    let sq0 = compute_residual_energy(dd0, cref_re, cref_im, f0, dt, 0);
    let sqb = compute_residual_energy(dd0, cref_re, cref_im, f0, dt, 90);
    
    // Quadratic interpolation to find minimum
    // Peakup: fits parabola through (-90, sqa), (0, sq0), (90, sqb)
    // Minimum at dx = 90 * (sqa - sqb) / (2 * (sqa - 2*sq0 + sqb))
    let denom = 2.0 * (sqa - 2.0 * sq0 + sqb);
    if denom.abs() < 1e-30 {
        return 0;
    }
    let dx = 90.0 * (sqa - sqb) / denom;
    dx.round() as isize
}

/// Compute residual energy in signal band after subtraction at given offset.
fn compute_residual_energy(
    dd0: &[f64],
    cref_re: &[f64],
    cref_im: &[f64],
    _f0: f64,
    dt: f64,
    offset: isize,
) -> f64 {
    let nmax = 15 * 12000;
    let nstart = (dt * SAMPLE_RATE).round() as isize + 1 + offset;
    
    // IQ mix
    let mut camp_re = vec![0.0f64; NFRAME];
    let mut camp_im = vec![0.0f64; NFRAME];
    for i in 0..NFRAME {
        let j = nstart - 1 + i as isize;
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let d = dd0[(j - 1) as usize];
            camp_re[i] = d * cref_re[i];
            camp_im[i] = -d * cref_im[i];
        }
    }
    
    // LPF convolution
    let (cfilt_re, cfilt_im) = lpf_convolve(&camp_re, &camp_im);
    
    // Compute residual and its energy in signal band
    let mut energy = 0.0;
    for i in 0..NFRAME {
        let j = nstart - 1 + i as isize;
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let z_re = cfilt_re[i] * cref_re[i] - cfilt_im[i] * cref_im[i];
            let residual = dd0[(j - 1) as usize] - 2.0 * z_re;
            energy += residual * residual;
        }
    }
    energy
}
