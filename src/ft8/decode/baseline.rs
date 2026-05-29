use super::{nint_wsjtx_f32, NFFT1, NMAX, NSPS};
use crate::ft8::indexx::indexx_ascending;
use crate::ft8::protocol::SAMPLE_RATE;
use crate::util::four2a_r2c;

fn compute_baseline(savg: &[f64], nfa: f64, nfb: f64, df: f64, nh1: usize) -> Vec<f64> {
    // WSJT-X stores sbase(1:NH1), with FFT bin 0/DC omitted. Keep index 0
    // unused so callers can use nint(f/df) directly, matching Fortran.
    let mut sbase = vec![0.0; nh1 + 1];
    let ia = nint_wsjtx_f32(nfa / df).max(1) as usize;
    let ib = (nint_wsjtx_f32(nfb / df).max(0) as usize).min(nh1);

    let db_range = (ib - ia + 1).max(1);
    let mut sdb = vec![0.0; nh1 + 1];
    for i in ia..=ib {
        sdb[i] = 10.0 * savg[i].max(1e-30).log10();
    }

    let nseg: usize = 10;
    let nlen = db_range / nseg;
    if nlen < 1 {
        let window = 50;
        for i in 1..=nh1 {
            let lo = ia.max(i.saturating_sub(window));
            let hi = ib.min(i + window);
            let mut sum = 0.0;
            let mut count = 0;
            for j in lo..=hi {
                sum += savg[j];
                count += 1;
            }
            sbase[i] = if count > 0 {
                10.0 * (1e-30f64.max(sum / count as f64)).log10()
            } else {
                0.0
            };
        }
        return sbase;
    }

    let npct: usize = 10;
    let mut env_x: Vec<f64> = Vec::new();
    let mut env_y: Vec<f64> = Vec::new();
    let i0 = db_range / 2;

    for n in 0..nseg {
        let ja = ia + n * nlen;
        let jb = (ja + nlen - 1).min(ib);
        if ja > ib || ja >= sdb.len() {
            break;
        }
        let slice = &sdb[ja..=jb.min(nh1)];
        let pval = percentile(slice, npct);
        for i in ja..=jb.min(nh1) {
            if sdb[i] <= pval {
                let x = (i as isize - i0 as isize) as f64;
                if env_x.len() < 1000 {
                    env_x.push(x);
                    env_y.push(sdb[i]);
                } else {
                    env_x[999] = x;
                    env_y[999] = sdb[i];
                }
            }
        }
    }

    // WSJT-X baseline.f90 uses nterms=5, i.e. five coefficients a(1:5)
    // and a degree-4 polynomial. Rust polyfit() takes the degree.
    let a = polyfit(&env_x, &env_y, 4);

    for i in ia..=ib.min(nh1) {
        let t = (i as isize - i0 as isize) as f64;
        sbase[i] = evpoly(&a, t) + 0.65;
    }

    sbase
}

fn percentile(slice: &[f64], k: usize) -> f64 {
    if slice.is_empty() {
        return 0.0;
    }
    let indx = indexx_ascending(slice);
    let idx = ((slice.len() as f64 * 0.01 * k as f64).round() as usize)
        .min(slice.len())
        .max(1)
        - 1;
    slice[indx[idx]]
}

fn polyfit(x: &[f64], y: &[f64], d: usize) -> Vec<f64> {
    let n = x.len().min(y.len());
    if n <= d {
        return vec![0.0; d + 1];
    }
    let m = d + 1;
    let mut a = vec![vec![0.0; m]; m];
    let mut b = vec![0.0; m];

    for i in 0..n {
        for j in 0..m {
            let xj = x[i].powi(j as i32);
            for k2 in 0..m {
                a[j][k2] += xj * x[i].powi(k2 as i32);
            }
            b[j] += xj * y[i];
        }
    }

    for col in 0..m {
        let mut max_val = a[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..m {
            if a[row][col].abs() > max_val {
                max_val = a[row][col].abs();
                max_row = row;
            }
        }
        if max_val < 1e-30 {
            break;
        }
        if max_row != col {
            a.swap(col, max_row);
            b.swap(col, max_row);
        }
        for row in (col + 1)..m {
            let factor = a[row][col] / a[col][col];
            for k2 in col..m {
                a[row][k2] -= factor * a[col][k2];
            }
            b[row] -= factor * b[col];
        }
    }

    let mut coeffs = vec![0.0; m];
    for i in (0..m).rev() {
        let mut sum = 0.0;
        for (j, coeff) in coeffs.iter().enumerate().skip(i + 1) {
            sum += a[i][j] * coeff;
        }
        if a[i][i].abs() >= 1e-30 {
            coeffs[i] = (b[i] - sum) / a[i][i];
        }
    }
    coeffs
}

fn evpoly(a: &[f64], t: f64) -> f64 {
    let mut result = 0.0;
    for i in (0..a.len()).rev() {
        result = result * t + a[i];
    }
    result
}

/// Nuttall 4-term window (matching WSJT-X nuttal_window.f90).
fn nuttall_window(n: usize) -> Vec<f64> {
    let mut w = vec![0.0; n];
    let nf = n as f64;
    let a0 = 0.3635819;
    let a1 = -0.4891775;
    let a2 = 0.1365995;
    let a3 = -0.0106411;
    for (i, slot) in w.iter_mut().enumerate().take(n) {
        let x = 2.0 * std::f64::consts::PI * i as f64 / nf;
        *slot = a0 + a1 * x.cos() + a2 * (2.0 * x).cos() + a3 * (3.0 * x).cos();
    }
    w
}

/// WSJT-X get_spectrum_baseline: Welch method with Nuttall window.
pub(super) fn get_spectrum_baseline(dd: &[f64], mut nfa: f64, mut nfb: f64) -> Vec<f64> {
    let nfft = NFFT1;
    let nh1 = nfft / 2;
    let nst = nh1;
    let nf = 93usize;
    let window = nuttall_window(nfft);
    let wsum: f64 = window.iter().sum();
    let wscale = NSPS as f64 * 2.0 / 300.0 / wsum;
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
            x_re[i] = sample * window[i] * wscale;
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
    compute_baseline(&savg, nfa, nfb, df, nh1)
}
