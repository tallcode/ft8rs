//! Welch spectrum average plus baseline fit for FT8.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/get_spectrum_baseline.f90`

use super::{baseline, NFFT1, NMAX, NSPS, SAMPLE_RATE};
use crate::util::four2a_r2c;

pub(super) fn get_spectrum_baseline(dd: &[f64], mut nfa: f64, mut nfb: f64) -> Vec<f64> {
    let nfft = NFFT1;
    let nh1 = nfft / 2;
    let nst = nh1;
    let nf = 93usize;
    let mut window = crate::ft8::nuttal_window::nuttal_window(nfft);
    let wsum: f64 = window.iter().sum();
    let wscale = NSPS as f64 * 2.0 / 300.0 / wsum;
    for slot in &mut window {
        *slot *= wscale;
    }
    let mut savg = vec![0.0; nh1 + 1];
    for j in 0..nf {
        let ia = j * nst;
        let ib = ia + nfft;
        if ib > NMAX {
            break;
        }
        let mut x_re = vec![0.0; nfft];
        let mut x_im = vec![0.0; nfft];
        for i in 0..nfft {
            let sample = dd.get(ia + i).copied().unwrap_or(0.0);
            x_re[i] = sample * window[i];
        }
        four2a_r2c(&mut x_re, &mut x_im);
        for i in 1..=nh1 {
            savg[i] += x_re[i] * x_re[i] + x_im[i] * x_im[i];
        }
    }
    let nwin = nfb - nfa;
    if nfa < 100.0 {
        nfa = 100.0;
        if nwin < 100.0 {
            nfb = nfa + nwin;
        }
    }
    if nfb > 4910.0 {
        nfb = 4910.0;
        if nwin < 100.0 {
            nfa = nfb - nwin;
        }
    }
    let df = SAMPLE_RATE as f64 / nfft as f64;
    baseline(&savg, nfa, nfb, df, nh1)
}
