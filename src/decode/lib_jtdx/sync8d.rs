//! Mirrors JTDX `lib/sync8d.f90`.

use std::sync::OnceLock;

use super::ft8_downsample::ComplexC;
use super::ft8_mod1::ICOS7;
use super::ft8_params::{DT2, NP2, TWO_PI};

const COSTAS_BLOCKS: usize = 7;
const COSTAS_SYMBOL_LEN: usize = 32;

pub struct SyncTemplates {
    pub csync_re: Vec<f64>,
    pub csync_im: Vec<f64>,
}

pub fn build_csync() -> &'static SyncTemplates {
    static T: OnceLock<SyncTemplates> = OnceLock::new();
    T.get_or_init(|| {
        let mut csync_re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        let mut csync_im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        for i in 0..COSTAS_BLOCKS {
            let mut phi = 0.0f64;
            let dphi = TWO_PI * ICOS7[i] as f64 / COSTAS_SYMBOL_LEN as f64;
            for j in 0..COSTAS_SYMBOL_LEN {
                let idx = i * COSTAS_SYMBOL_LEN + j;
                csync_re[idx] = phi.cos();
                csync_im[idx] = phi.sin();
                phi = (phi + dphi) % TWO_PI;
            }
        }
        SyncTemplates { csync_re, csync_im }
    })
}

pub fn build_ctwk(delf: f64) -> ([f64; 32], [f64; 32]) {
    let dphi = TWO_PI * delf * DT2;
    let mut re = [0.0; 32];
    let mut im = [0.0; 32];
    let mut phi = 0.0f64;
    for i in 0..32 {
        re[i] = phi.cos();
        im[i] = phi.sin();
        phi = (phi + dphi) % TWO_PI;
    }
    (re, im)
}

#[derive(Clone, Copy, Debug)]
pub struct Sync8dContext {
    pub ipass: usize,
    pub lastsync: bool,
    pub iqso: usize,
    pub lcq: bool,
    pub lcallsstd: bool,
    pub lcqcand: bool,
}

pub fn sync8d(
    cd0: &ComplexC,
    i0: isize,
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
    context: Sync8dContext,
) -> f64 {
    let templates = build_csync();
    let mut zt1 = [(0.0f64, 0.0f64); 7];
    let mut zt2 = [(0.0f64, 0.0f64); 7];
    let mut zt3 = [(0.0f64, 0.0f64); 7];

    for i in 0..7 {
        let i1 = i0 + i as isize * 32;
        let i2 = i1 + 1152;
        let i3 = i1 + 2304;
        zt1[i] = sync_sum(
            cd0,
            i1,
            i,
            &templates.csync_re,
            &templates.csync_im,
            ctwk_re,
            ctwk_im,
        );
        zt2[i] = sync_sum(
            cd0,
            i2,
            i,
            &templates.csync_re,
            &templates.csync_im,
            ctwk_re,
            ctwk_im,
        );
        zt3[i] = sync_sum(
            cd0,
            i3,
            i,
            &templates.csync_re,
            &templates.csync_im,
            ctwk_re,
            ctwk_im,
        );
    }

    let mut sync = match context.ipass {
        1 | 5 | 9 if !context.lastsync => zt1
            .iter()
            .chain(zt2.iter())
            .chain(zt3.iter())
            .map(|z| magnitude(*z))
            .sum(),
        2 | 6 | 7 if !context.lastsync => zt1
            .iter()
            .chain(zt2.iter())
            .chain(zt3.iter())
            .map(|z| power(*z))
            .sum(),
        3 | 4 | 8 if !context.lastsync => {
            let mut sum = 0.0;
            for i in 0..7 {
                let z1 = if i < 6 {
                    avg(zt1[i], zt1[i + 1])
                } else {
                    zt1[i]
                };
                let z2 = if i < 6 {
                    avg(zt2[i], zt2[i + 1])
                } else {
                    zt2[i]
                };
                let z3 = if i < 6 {
                    avg(zt3[i], zt3[i + 1])
                } else {
                    zt3[i]
                };
                sum += z1.0.abs() + z1.1.abs() + z2.0.abs() + z2.1.abs() + z3.0.abs() + z3.1.abs();
            }
            sum
        }
        _ => zt1
            .iter()
            .chain(zt2.iter())
            .chain(zt3.iter())
            .map(|z| power(*z))
            .sum(),
    };

    let _ = (
        context.iqso,
        context.lcq,
        context.lcallsstd,
        context.lcqcand,
    );
    if !sync.is_finite() {
        sync = 0.0;
    }
    sync
}

fn sync_sum(
    cd0: &ComplexC,
    i0: isize,
    tone: usize,
    tpl_re: &[f64],
    tpl_im: &[f64],
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
) -> (f64, f64) {
    if i0 < 0 || i0 + 31 > NP2 as isize {
        return (0.0, 0.0);
    }
    let mut out_re = 0.0;
    let mut out_im = 0.0;
    for j in 0..32 {
        let cd_idx = ComplexC::idx(i0 + j as isize);
        let mut re = tpl_re[tone * 32 + j];
        let mut im = tpl_im[tone * 32 + j];
        if let (Some(twk_re), Some(twk_im)) = (ctwk_re, ctwk_im) {
            let r = twk_re[j] * re - twk_im[j] * im;
            let q = twk_re[j] * im + twk_im[j] * re;
            re = r;
            im = q;
        }
        // cd0 * conjg(template)
        out_re += cd0.re[cd_idx] * re + cd0.im[cd_idx] * im;
        out_im += cd0.im[cd_idx] * re - cd0.re[cd_idx] * im;
    }
    (out_re, out_im)
}

fn magnitude(z: (f64, f64)) -> f64 {
    (z.0 * z.0 + z.1 * z.1).sqrt()
}

fn power(z: (f64, f64)) -> f64 {
    z.0 * z.0 + z.1 * z.1
}

fn avg(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}
