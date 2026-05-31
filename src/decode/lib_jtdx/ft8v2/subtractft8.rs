//! Mirrors JTDX `lib/ft8v2/subtractft8.f90`.

use std::f64::consts::PI;
use std::sync::OnceLock;

use crate::decode::lib_jtdx::ft8_mod1::{NFILT1, NFILT2};
use crate::decode::lib_jtdx::gen_ft8wave::{gen_ft8wave, NFRAME};
use crate::util::four2a_c2c;

const NFFT: usize = 180_000;
const NMAX: usize = 180_000;
const SAMPLE_RATE: f64 = 12000.0;

pub(crate) fn subtractft8(dd8: &mut [f32], itone: &[i32; 79], f0: f32, dt: f32, swl: bool) {
    let (cref_re, cref_im) = gen_ft8wave(itone, f0 as f64);
    let nstart = (dt as f64 * SAMPLE_RATE) as isize + 1;
    let mut cfilt_re = vec![0.0f64; NFFT];
    let mut cfilt_im = vec![0.0f64; NFFT];

    for i in 0..NFRAME {
        let id = nstart - 1 + (i + 1) as isize;
        if id >= 1 && id <= NMAX as isize && id as usize <= dd8.len() {
            let sample = dd8[(id - 1) as usize] as f64;
            cfilt_re[i] = sample * cref_re[i];
            cfilt_im[i] = -sample * cref_im[i];
        }
    }

    let lpf = if swl {
        lpf_data(NFILT2)
    } else {
        lpf_data(NFILT1)
    };
    four2a_c2c(&mut cfilt_re, &mut cfilt_im, -1);
    for i in 0..NFFT {
        let re = cfilt_re[i];
        let im = cfilt_im[i];
        cfilt_re[i] = re * lpf.fft_re[i] - im * lpf.fft_im[i];
        cfilt_im[i] = re * lpf.fft_im[i] + im * lpf.fft_re[i];
    }
    four2a_c2c(&mut cfilt_re, &mut cfilt_im, 1);

    for j in 0..=lpf.half_filt {
        let correction = lpf.endcorrection[j];
        cfilt_re[j] *= correction;
        cfilt_im[j] *= correction;
        let tail = NFRAME - 1 - j;
        cfilt_re[tail] *= correction;
        cfilt_im[tail] *= correction;
    }

    for i in 0..NFRAME {
        let j = nstart + i as isize;
        if j >= 1 && j <= NMAX as isize && j as usize <= dd8.len() {
            let z_re = cfilt_re[i] * cref_re[i] - cfilt_im[i] * cref_im[i];
            dd8[(j - 1) as usize] -= (2.0 * z_re) as f32;
        }
    }
}

struct LpfData {
    fft_re: Vec<f64>,
    fft_im: Vec<f64>,
    endcorrection: Vec<f64>,
    half_filt: usize,
}

fn lpf_data(nfilt: usize) -> &'static LpfData {
    static LPF1: OnceLock<LpfData> = OnceLock::new();
    static LPF2: OnceLock<LpfData> = OnceLock::new();
    if nfilt == NFILT2 {
        LPF2.get_or_init(|| build_lpf_data(NFILT2))
    } else {
        LPF1.get_or_init(|| build_lpf_data(NFILT1))
    }
}

fn build_lpf_data(nfilt: usize) -> LpfData {
    let half_filt = nfilt / 2;
    let mut sumw = 0.0f64;
    let mut window = vec![0.0f64; nfilt + 1];
    for (j, value) in window.iter_mut().enumerate() {
        let j_signed = j as isize - half_filt as isize;
        *value = (PI * j_signed as f64 / nfilt as f64).cos().powi(2);
        sumw += *value;
    }

    let mut cw_re = vec![0.0f64; NFFT];
    for j in 0..=nfilt {
        cw_re[j] = window[j] / sumw;
    }
    let shift = half_filt + 1;
    let mut fft_re = vec![0.0f64; NFFT];
    for i in 0..NFFT {
        fft_re[i] = cw_re[(i + shift) % NFFT];
    }
    let mut fft_im = vec![0.0f64; NFFT];
    four2a_c2c(&mut fft_re, &mut fft_im, -1);
    let fac = 1.0 / NFFT as f64;
    for i in 0..NFFT {
        fft_re[i] *= fac;
        fft_im[i] *= fac;
    }

    let mut endcorrection = vec![0.0f64; half_filt + 1];
    let mut tail_sum = 0.0f64;
    for j in (0..=half_filt).rev() {
        tail_sum += window[j + half_filt];
        endcorrection[j] = 1.0 / (1.0 - tail_sum / sumw);
    }

    LpfData {
        fft_re,
        fft_im,
        endcorrection,
        half_filt,
    }
}
