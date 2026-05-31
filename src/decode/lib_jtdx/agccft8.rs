//! Mirrors JTDX `lib/agccft8.f90`.

use crate::util::four2a_r2c;

const NFFT: usize = 1024;
const NHSYM: usize = 178;
const NHSTEP: usize = 960;

#[derive(Clone, Copy, Debug, Default)]
pub struct AgcResult {
    pub lagccbail: bool,
    pub forcedt: f32,
}

pub fn agccft8(dd8: &mut [f32], nfa: i32, nfb: i32, lforcesync: bool) -> AgcResult {
    let w11 = window();
    let fac1 = 1.1e-3f32;
    let nfblim = nfb.min(4999);
    let nf1 = ((NFFT as i32 * nfa) / 12000 + 1).max(1) as usize;
    let nf2 = ((NFFT as i32 * nfblim) / 12000).max(nf1 as i32) as usize;
    let nf2 = nf2.min(NFFT / 2);
    let nsb = nf2.saturating_sub(nf1) + 1;
    let nmed = (nf2.saturating_sub(nf1)) / 2;

    if lforcesync {
        let mut jmin = 0usize;
        let mut specmin = f32::MAX;
        for j in 1..=175usize {
            let i0 = (j - 1) * NFFT;
            let mut x_re = vec![0.0f64; NFFT];
            let mut x_im = vec![0.0f64; NFFT];
            fill_windowed(&mut x_re, dd8, i0, fac1, &w11);
            four2a_r2c(&mut x_re, &mut x_im);
            let spec = spectrum_sum(&x_re, &x_im, nf1, nf2);
            if spec > 0.001 && spec < specmin {
                specmin = spec;
                jmin = j;
            }
        }
        let mut forcedt = 15.0 * jmin as f32 / 175.0;
        if forcedt > 7.5 {
            forcedt -= 15.0;
        }
        return AgcResult {
            lagccbail: false,
            forcedt,
        };
    }

    let mut s3 = [1.0f32; NHSYM + 1];
    let mut x_re = vec![0.0f64; NFFT];
    let mut x_im = vec![0.0f64; NFFT];

    for j in 1..=10usize {
        s3[j] = median_symbol_level(
            dd8, j, nf1, nf2, nsb, nmed, fac1, &w11, &mut x_re, &mut x_im,
        );
    }
    for j in 169..=178usize {
        s3[j] = median_symbol_level(
            dd8, j, nf1, nf2, nsb, nmed, fac1, &w11, &mut x_re, &mut x_im,
        );
    }

    let s3min1 = s3[1..=10].iter().copied().fold(f32::INFINITY, f32::min);
    let s3min2 = s3[169..=178].iter().copied().fold(f32::INFINITY, f32::min);
    let mut s3min = s3min1.min(s3min2);
    let s3max1 = s3[1..=10].iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let s3max2 = s3[169..=178]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let mut s3max = s3max1.max(s3max2);
    if s3min < 0.1 {
        s3min = 1.0;
        s3max = 1.0;
    }

    if s3max / s3min < 1.26 {
        return AgcResult {
            lagccbail: true,
            forcedt: 0.0,
        };
    }

    let mut k = 0usize;
    for level in s3.iter().take(11).skip(1) {
        divide_range(dd8, k, k + NHSTEP, *level);
        k += NHSTEP;
    }
    k = 168 * NHSTEP;
    for level in s3.iter().take(179).skip(169) {
        divide_range(dd8, k, k + NHSTEP, *level);
        k += NHSTEP;
    }
    divide_range(dd8, 170_880, 171_650, s3[178]);
    for sample in dd8
        .iter_mut()
        .skip(171_650)
        .take(180_000usize.saturating_sub(171_650))
    {
        *sample = 0.0;
    }

    AgcResult {
        lagccbail: false,
        forcedt: 0.0,
    }
}

fn median_symbol_level(
    dd8: &[f32],
    j: usize,
    nf1: usize,
    nf2: usize,
    nsb: usize,
    nmed: usize,
    fac1: f32,
    w11: &[f32; NFFT],
    x_re: &mut [f64],
    x_im: &mut [f64],
) -> f32 {
    let i0 = (j - 1) * NHSTEP;
    fill_windowed(x_re, dd8, i0, fac1, w11);
    x_im.fill(0.0);
    four2a_r2c(x_re, x_im);
    let mut sa33 = Vec::with_capacity(nsb);
    for i in nf1..=nf2 {
        sa33.push((x_re[i] * x_re[i] + x_im[i] * x_im[i]).sqrt().sqrt() as f32);
    }
    sa33.sort_by(|a, b| a.total_cmp(b));
    let smed = sa33[nmed.min(sa33.len().saturating_sub(1))];
    if smed > 1e-6 {
        smed
    } else {
        1.0
    }
}

fn fill_windowed(x_re: &mut [f64], dd8: &[f32], i0: usize, fac1: f32, w11: &[f32; NFFT]) {
    x_re.fill(0.0);
    for k in 0..NFFT {
        x_re[k] = (fac1 * w11[k] * dd8.get(i0 + k).copied().unwrap_or(0.0)) as f64;
    }
}

fn spectrum_sum(x_re: &[f64], x_im: &[f64], nf1: usize, nf2: usize) -> f32 {
    (nf1..=nf2)
        .map(|i| (x_re[i] * x_re[i] + x_im[i] * x_im[i]).sqrt() as f32)
        .sum()
}

fn divide_range(dd8: &mut [f32], start: usize, end: usize, level: f32) {
    if level <= 1e-6 {
        return;
    }
    let end = end.min(dd8.len());
    for sample in dd8.iter_mut().take(end).skip(start) {
        *sample /= level;
    }
}

fn window() -> [f32; NFFT] {
    let mut out = [0.0f32; NFFT];
    for k in 1..=NFFT {
        out[k - 1] = (std::f32::consts::TAU * (k as f32 + 2.0) / 2048.0).sin();
    }
    out
}
