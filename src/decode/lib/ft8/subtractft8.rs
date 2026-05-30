//! FT8 residual subtraction.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/subtractft8.f90`
//!
//! Algorithm (from Fortran comments):
//!   Measured signal  : dd(t) = a(t)·cos(2πf₀t + θ(t))
//!   Reference signal : cref(t) = exp(j·(2πf₀t + φ(t)))
//!   Complex amp      : cfilt(t) = LPF[ dd(t)·CONJG(cref(t)) ]
//!   Subtract         : dd(t) ← dd(t) - 2·REAL(cref(t)·cfilt(t))
//!
//! LPF implementation: WSJT-X-style NMAX-point circular FFT filtering using a
//! cshifted cos² window and the same edge correction factors.
//!
//! Key parameters:
//!   NFRAME = 151680 (79 symbols × 1920 samples/symbol)
//!   NFILT  = 4000 (cos² LPF window, ±2000 taps)
//!   NFFT   = NMAX = 180000
//!   NSPS   = 1920 (waveform generation resolution, NOT detection rate of 48)

use crate::decode::gen_ft8wave::{gen_ft8wave, NFRAME};
use crate::util::{four2a_c2c, four2a_r2c};
use std::f64::consts::PI;
use std::sync::OnceLock;

const NFILT: usize = 4000;
const HALF_FILT: usize = NFILT / 2; // 2000
const SAMPLE_RATE: f64 = 12000.0;
const NFFT: usize = 15 * 12_000;

fn wsjtx_subtract_sample_index(nstart_1based: isize, rust_i: usize) -> isize {
    nstart_1based + rust_i as isize
}

fn wsjtx_subtract_nstart(dt: f64, idt: isize) -> isize {
    (dt * SAMPLE_RATE) as isize + 1 + idt
}

struct LpfData {
    fft_re: Vec<f64>,
    fft_im: Vec<f64>,
    endcorrection: Vec<f64>,
}

/// Precomputed WSJT-X subtractft8 LPF data.
fn lpf_data() -> &'static LpfData {
    static LPF: OnceLock<LpfData> = OnceLock::new();
    LPF.get_or_init(|| {
        let mut sumw: f64 = 0.0;
        let mut window = vec![0.0f64; NFILT + 1];
        for j in 0..=NFILT {
            let j_signed = j as isize - HALF_FILT as isize;
            window[j] = (PI * j_signed as f64 / NFILT as f64).cos().powi(2);
            sumw += window[j];
        }

        let mut cw_re = vec![0.0f64; NFFT];
        for j in 0..=NFILT {
            cw_re[j] = window[j] / sumw;
        }

        // Fortran: cw=cshift(cw,NFILT/2+1) before the forward FFT.
        let shift = HALF_FILT + 1;
        let mut shifted_re = vec![0.0f64; NFFT];
        for i in 0..NFFT {
            shifted_re[i] = cw_re[(i + shift) % NFFT];
        }
        let mut shifted_im = vec![0.0f64; NFFT];
        four2a_c2c(&mut shifted_re, &mut shifted_im, -1);
        let fac = 1.0 / NFFT as f64;
        for i in 0..NFFT {
            shifted_re[i] *= fac;
            shifted_im[i] *= fac;
        }

        let mut endcorrection = vec![0.0f64; HALF_FILT + 1];
        let mut tail_sum = 0.0;
        for j_signed in (0..=HALF_FILT).rev() {
            tail_sum += window[j_signed + HALF_FILT];
            endcorrection[j_signed] = 1.0 / (1.0 - tail_sum / sumw);
        }

        LpfData {
            fft_re: shifted_re,
            fft_im: shifted_im,
            endcorrection,
        }
    })
}

/// WSJT-X subtractft8 LPF: NMAX-point FFT, multiply by shifted window FFT,
/// inverse FFT, then apply the endpoint correction.
fn lpf_convolve(camp_re: &[f64], camp_im: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let lpf = lpf_data();
    let n = camp_re.len();
    debug_assert_eq!(n, NFRAME);

    let mut cfilt_re = vec![0.0f64; NFFT];
    let mut cfilt_im = vec![0.0f64; NFFT];
    cfilt_re[..NFRAME].copy_from_slice(camp_re);
    cfilt_im[..NFRAME].copy_from_slice(camp_im);

    four2a_c2c(&mut cfilt_re, &mut cfilt_im, -1);

    for i in 0..NFFT {
        let cr = cfilt_re[i];
        let ci = cfilt_im[i];
        cfilt_re[i] = cr * lpf.fft_re[i] - ci * lpf.fft_im[i];
        cfilt_im[i] = cr * lpf.fft_im[i] + ci * lpf.fft_re[i];
    }

    four2a_c2c(&mut cfilt_re, &mut cfilt_im, 1);

    for j in 0..=HALF_FILT {
        let correction = lpf.endcorrection[j];
        cfilt_re[j] *= correction;
        cfilt_im[j] *= correction;
        let end_idx = NFRAME - 1 - j;
        cfilt_re[end_idx] *= correction;
        cfilt_im[end_idx] *= correction;
    }

    cfilt_re.truncate(NFRAME);
    cfilt_im.truncate(NFRAME);
    (cfilt_re, cfilt_im)
}

/// Main subtract function. dd0 is modified in-place.
/// dt: time offset from data start in seconds (matching WSJTX convention).
pub fn subtract_ft8(dd0: &mut Vec<f64>, itone: &[i32; 79], f0: f64, dt: f64) {
    subtract_ft8_refined(dd0, itone, f0, dt, false)
}

/// Subtract with optional dt refinement (WSJT-X lrefinedt).
/// When refined=true, searches ±90 samples for optimal dt using energy minimization.
pub fn subtract_ft8_refined(
    dd0: &mut Vec<f64>,
    itone: &[i32; 79],
    f0: f64,
    dt: f64,
    refined: bool,
) {
    let (cref_re, cref_im) = gen_ft8wave(itone, f0);

    let final_offset = if refined {
        let sqa = subtract_sqf_band_energy(dd0, &cref_re, &cref_im, f0, dt, -90);
        let sqb = subtract_sqf_band_energy(dd0, &cref_re, &cref_im, f0, dt, 90);
        let sq0 = subtract_sqf_band_energy(dd0, &cref_re, &cref_im, f0, dt, 0);
        let dx = peakup(sqa, sq0, sqb);
        if dx.abs() > 1.0 {
            return;
        }
        (90.0f32 * dx).round() as isize
    } else {
        0
    };

    let subtracted = subtract_sqf(dd0, &cref_re, &cref_im, f0, dt, final_offset, false);
    *dd0 = subtracted.dd;
}

fn peakup(ym: f32, y0: f32, yp: f32) -> f32 {
    let b = yp - ym;
    let c = yp + ym - 2.0f32 * y0;
    -b / (2.0f32 * c)
}

struct SqfResult {
    dd: Vec<f64>,
    band_energy: f32,
}

fn subtract_sqf_band_energy(
    dd0: &[f64],
    cref_re: &[f64],
    cref_im: &[f64],
    f0: f64,
    dt: f64,
    offset: isize,
) -> f32 {
    subtract_sqf(dd0, cref_re, cref_im, f0, dt, offset, true).band_energy
}

fn subtract_sqf(
    dd0: &[f64],
    cref_re: &[f64],
    cref_im: &[f64],
    f0: f64,
    dt: f64,
    offset: isize,
    compute_band_energy: bool,
) -> SqfResult {
    let nmax = 15 * 12000;
    let nstart = wsjtx_subtract_nstart(dt, offset);
    let mut dd = vec![0.0f64; NFFT];
    let copy_len = dd0.len().min(NFFT);
    dd[..copy_len].copy_from_slice(&dd0[..copy_len]);

    let mut camp_re = vec![0.0f64; NFRAME];
    let mut camp_im = vec![0.0f64; NFRAME];
    for i in 0..NFRAME {
        let j = wsjtx_subtract_sample_index(nstart, i);
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let d = dd[(j - 1) as usize];
            camp_re[i] = d * cref_re[i];
            camp_im[i] = -d * cref_im[i];
        }
    }

    let (cfilt_re, cfilt_im) = lpf_convolve(&camp_re, &camp_im);

    let mut x_re = compute_band_energy.then(|| vec![0.0f64; NFFT]);
    for i in 0..NFRAME {
        let j = wsjtx_subtract_sample_index(nstart, i);
        if j >= 1 && j <= nmax as isize && j as usize <= dd0.len() {
            let z_re = cfilt_re[i] * cref_re[i] - cfilt_im[i] * cref_im[i];
            let residual = dd[(j - 1) as usize] - 2.0 * z_re;
            dd[(j - 1) as usize] = residual;
            if let Some(x_re) = x_re.as_mut() {
                x_re[i] = residual;
            }
        }
    }

    let band_energy = if let Some(mut x_re) = x_re {
        let mut x_im = vec![0.0f64; NFFT];
        four2a_r2c(&mut x_re, &mut x_im);
        let df = SAMPLE_RATE / NFFT as f64;
        let ia = ((f0 - 1.5 * 6.25) / df).max(0.0) as usize;
        let ib = ((f0 + 8.5 * 6.25) / df).min((NFFT / 2) as f64) as usize;
        let mut sqq = 0.0f32;
        for i in ia..=ib {
            sqq += (x_re[i] * x_re[i] + x_im[i] * x_im[i]) as f32;
        }
        sqq
    } else {
        0.0f32
    };

    dd.truncate(dd0.len());
    SqfResult { dd, band_energy }
}

#[cfg(test)]
mod tests {
    #[test]
    fn subtract_sample_index_matches_wsjtx_one_based_loop() {
        let nstart = 6001;
        assert_eq!(super::wsjtx_subtract_sample_index(nstart, 0), nstart);
        assert_eq!(super::wsjtx_subtract_sample_index(nstart, 1), nstart + 1);
    }

    #[test]
    fn subtract_nstart_matches_fortran_implicit_integer_assignment() {
        assert_eq!(super::wsjtx_subtract_nstart(0.5009, 0), 6011);
        assert_eq!(super::wsjtx_subtract_nstart(0.5009, -90), 5921);
        assert_eq!(super::wsjtx_subtract_nstart(-0.0009, 0), -9);
    }
}
