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
use crate::decode::lib_wsjtx_plus::fft::{fft_complex_in_place, four2a_c2c, four2a_r2c};
use rustfft::num_complex::Complex;
use std::f64::consts::PI;
use std::ops::Range;
use std::sync::OnceLock;

const NFILT: usize = 4000;
const HALF_FILT: usize = NFILT / 2; // 2000
const SAMPLE_RATE: f64 = 12000.0;
const NFFT: usize = 15 * 12_000;

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
        for (j, window_value) in window.iter_mut().enumerate().take(NFILT + 1) {
            let j_signed = j as isize - HALF_FILT as isize;
            *window_value = (PI * j_signed as f64 / NFILT as f64).cos().powi(2);
            sumw += *window_value;
        }

        // Upstream 8a0bddd9: construct the shifted filter in place instead
        // of copying and shifting a full NFFT buffer. Fortran's 1-based
        // modulo(j-1,NFFT)+1 maps to the zero-based index below.
        let mut shifted_re = vec![0.0f64; NFFT];
        for (i, &value) in window.iter().enumerate() {
            let j = i as isize - HALF_FILT as isize;
            shifted_re[(j - 1).rem_euclid(NFFT as isize) as usize] = value / sumw;
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

/// WSJT-X subtractft8 LPF: NMAX-point FFT, multiply by the shifted window
/// spectrum, inverse FFT, then apply endpoint corrections.
fn lpf_convolve(filtered: &mut [Complex<f64>]) {
    let lpf = lpf_data();
    fft_complex_in_place(filtered, false);

    for (value, (&re, &im)) in filtered.iter_mut().zip(lpf.fft_re.iter().zip(&lpf.fft_im)) {
        let cr = value.re;
        let ci = value.im;
        *value = Complex::new(cr * re - ci * im, cr * im + ci * re);
    }

    fft_complex_in_place(filtered, true);

    for (j, &correction) in lpf.endcorrection.iter().enumerate() {
        filtered[j] *= correction;
        filtered[NFRAME - 1 - j] *= correction;
    }
}

#[derive(Default)]
struct SubtractScratch {
    filtered: Vec<Complex<f64>>,
    band_re: Vec<f64>,
    band_im: Vec<f64>,
}

thread_local! {
    static SUBTRACT_SCRATCH: std::cell::RefCell<SubtractScratch> =
        std::cell::RefCell::new(SubtractScratch::default());
}

impl SubtractScratch {
    fn filter(&mut self, audio: &[f64], re: &[f64], im: &[f64], first: usize) {
        self.filtered.resize(NFFT, Complex::ZERO);
        let (prefix, rest) = self.filtered.split_at_mut(first);
        let (frame, tail) = rest.split_at_mut(audio.len());
        prefix.fill(Complex::ZERO);
        tail.fill(Complex::ZERO);
        for (value, (&sample, (&re, &im))) in
            frame.iter_mut().zip(audio.iter().zip(re.iter().zip(im)))
        {
            *value = Complex::new(sample * re, -sample * im);
        }
        lpf_convolve(&mut self.filtered);
    }

    fn band_energy(&mut self, audio: &[f64], re: &[f64], im: &[f64], first: usize, f0: f64) -> f32 {
        self.filter(audio, re, im, first);
        self.band_re.resize(NFFT, 0.0);
        self.band_im.resize(NFFT, 0.0);
        self.band_re.fill(0.0);
        self.band_im.fill(0.0);
        for (i, (&sample, (&re, &im))) in audio.iter().zip(re.iter().zip(im)).enumerate() {
            self.band_re[first + i] = residual(sample, self.filtered[first + i], re, im);
        }

        four2a_r2c(&mut self.band_re, &mut self.band_im);
        let df = SAMPLE_RATE / NFFT as f64;
        let ia = ((f0 - 1.5 * 6.25) / df).max(0.0) as usize;
        let ib = ((f0 + 8.5 * 6.25) / df).min((NFFT / 2) as f64) as usize;
        let mut energy = 0.0f32;
        for i in ia..=ib {
            let re = self.band_re[i];
            let im = self.band_im[i];
            energy += (re * re + im * im) as f32;
        }
        energy
    }
}

fn residual(sample: f64, filtered: Complex<f64>, re: f64, im: f64) -> f64 {
    let z_re = filtered.re * re - filtered.im * im;
    sample - 2.0 * z_re
}

/// Map the Fortran one-based start onto matching waveform and audio slices.
fn subtract_ranges(nstart: isize, audio_len: usize) -> (Range<usize>, Range<usize>) {
    let start = nstart.saturating_sub(1);
    let first = start.saturating_neg().max(0) as usize;
    let limit = audio_len.min(NFFT) as isize;
    let end = (limit.saturating_sub(start).max(0) as usize).min(NFRAME);
    if first >= end {
        return (0..0, 0..0);
    }
    let audio_first = (start + first as isize) as usize;
    (first..end, audio_first..audio_first + end - first)
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
    let _span = crate::decode::lib_wsjtx_plus::metrics::Span::new(
        crate::decode::lib_wsjtx_plus::metrics::Kind::Subtract,
    );
    let (cref_re, cref_im) = gen_ft8wave(itone, f0);

    SUBTRACT_SCRATCH.with_borrow_mut(|scratch| {
        let final_offset = if refined {
            let mut energy = |offset| {
                let nstart = wsjtx_subtract_nstart(dt, offset);
                let (wave, audio) = subtract_ranges(nstart, dd0.len());
                let first = wave.start;
                scratch.band_energy(
                    &dd0[audio],
                    &cref_re[wave.clone()],
                    &cref_im[wave],
                    first,
                    f0,
                )
            };
            let sqa = energy(-90);
            let sqb = energy(90);
            let sq0 = energy(0);
            let dx = peakup(sqa, sq0, sqb);
            if dx.abs() > 1.0 {
                return;
            }
            (90.0f32 * dx).round() as isize
        } else {
            0
        };

        let nstart = wsjtx_subtract_nstart(dt, final_offset);
        let (wave, audio) = subtract_ranges(nstart, dd0.len());
        let re = &cref_re[wave.clone()];
        let im = &cref_im[wave.clone()];
        let samples = &mut dd0[audio];
        scratch.filter(samples, re, im, wave.start);
        for ((sample, &filtered), (&re, &im)) in samples
            .iter_mut()
            .zip(&scratch.filtered[wave])
            .zip(re.iter().zip(im))
        {
            *sample = residual(*sample, filtered, re, im);
        }
        dd0.truncate(NFFT);
    });
}

fn peakup(ym: f32, y0: f32, yp: f32) -> f32 {
    let b = yp - ym;
    let c = yp + ym - 2.0f32 * y0;
    -b / (2.0f32 * c)
}

#[cfg(test)]
mod tests {
    #[test]
    fn subtract_sample_index_matches_wsjtx_one_based_loop() {
        for (nstart, len) in [(6001, 180000), (-9, 20), (179999, 180001), (8, 7), (1, 0)] {
            let (wave, audio) = super::subtract_ranges(nstart, len);
            let expected: Vec<_> = (0..super::NFRAME)
                .filter_map(|i| {
                    let j = nstart + i as isize;
                    (j >= 1 && j as usize <= len.min(super::NFFT)).then(|| (i, (j - 1) as usize))
                })
                .collect();
            assert_eq!(wave.zip(audio).collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn subtract_nstart_matches_fortran_implicit_integer_assignment() {
        assert_eq!(super::wsjtx_subtract_nstart(0.5009, 0), 6011);
        assert_eq!(super::wsjtx_subtract_nstart(0.5009, -90), 5921);
        assert_eq!(super::wsjtx_subtract_nstart(-0.0009, 0), -9);
    }
}
