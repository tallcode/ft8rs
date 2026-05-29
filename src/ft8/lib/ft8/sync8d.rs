//! Costas sync correlation helper.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/sync8d.f90`

use super::{COSTAS_BLOCKS, COSTAS_SYMBOL_LEN, DT2, NP2, TWO_PI_F32};
use std::sync::OnceLock;

const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];

pub(crate) struct SyncTemplate {
    pub(crate) re: Vec<f64>,
    pub(crate) im: Vec<f64>,
}

pub(crate) struct FrequencyShiftSyncTemplate {
    pub(crate) delf: f64,
    pub(crate) re: Vec<f64>,
    pub(crate) im: Vec<f64>,
}

pub(crate) fn build_costas_sync_templates() -> &'static SyncTemplate {
    static T: OnceLock<SyncTemplate> = OnceLock::new();
    T.get_or_init(|| {
        let mut re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        let mut im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        for i in 0..COSTAS_BLOCKS {
            let mut phi = 0.0f32;
            let dphi = TWO_PI_F32 * COSTAS[i] as f32 / COSTAS_SYMBOL_LEN as f32;
            for j in 0..COSTAS_SYMBOL_LEN {
                re[i * COSTAS_SYMBOL_LEN + j] = phi.cos() as f64;
                im[i * COSTAS_SYMBOL_LEN + j] = phi.sin() as f64;
                phi = (phi + dphi) % TWO_PI_F32;
            }
        }
        SyncTemplate { re, im }
    })
}

pub(crate) fn build_frequency_shift_sync_templates() -> &'static Vec<FrequencyShiftSyncTemplate> {
    static T: OnceLock<Vec<FrequencyShiftSyncTemplate>> = OnceLock::new();
    T.get_or_init(|| {
        let cs = build_costas_sync_templates();
        let mut templates = Vec::new();
        for ifr in -5..=5 {
            let delf = ifr as f64 * 0.5;
            let dphi = TWO_PI_F32 * delf as f32 * DT2 as f32;
            let mut twk_re = [0.0; COSTAS_SYMBOL_LEN];
            let mut twk_im = [0.0; COSTAS_SYMBOL_LEN];
            let mut phi = 0.0f32;
            for j in 0..COSTAS_SYMBOL_LEN {
                twk_re[j] = phi.cos() as f64;
                twk_im[j] = phi.sin() as f64;
                phi = (phi + dphi) % TWO_PI_F32;
            }
            let mut re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
            let mut im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
            for i in 0..COSTAS_BLOCKS {
                for j in 0..COSTAS_SYMBOL_LEN {
                    let idx = i * COSTAS_SYMBOL_LEN + j;
                    let twk_re = twk_re[j] as f32;
                    let twk_im = twk_im[j] as f32;
                    let cs_re = cs.re[idx] as f32;
                    let cs_im = cs.im[idx] as f32;
                    re[idx] = (twk_re * cs_re - twk_im * cs_im) as f64;
                    im[idx] = (twk_re * cs_im + twk_im * cs_re) as f64;
                }
            }
            templates.push(FrequencyShiftSyncTemplate { delf, re, im });
        }
        templates
    })
}

/// WSJT-X lib/ft8/sync8d.f90 with `itwk=0`.
pub(crate) fn sync8d(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i0: isize,
    sync_re: &[f64],
    sync_im: &[f64],
) -> f64 {
    let mut sync = 0.0f32;
    let stride = 36 * COSTAS_SYMBOL_LEN;

    for i in 0..COSTAS_BLOCKS {
        let base = i * COSTAS_SYMBOL_LEN;
        let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize);

        for _block in 0..3 {
            if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                let i_start = i_start as usize;
                let mut z_re = 0.0f32;
                let mut z_im = 0.0f32;
                for j in 0..COSTAS_SYMBOL_LEN {
                    let s_re = sync_re[base + j] as f32;
                    let s_im = sync_im[base + j] as f32;
                    let d_re = cd0_re[i_start + j] as f32;
                    let d_im = cd0_im[i_start + j] as f32;
                    z_re += d_re * s_re + d_im * s_im;
                    z_im += d_im * s_re - d_re * s_im;
                }
                sync += z_re * z_re + z_im * z_im;
            }
            i_start += stride as isize;
        }
    }

    sync as f64
}

/// WSJT-X lib/ft8/sync8d.f90 with `itwk=1`.
pub(crate) fn sync8d_twk(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i0: isize,
    sync_re: &[f64],
    sync_im: &[f64],
    twk_re: &[f64; 32],
    twk_im: &[f64; 32],
) -> f64 {
    let mut sync = 0.0f32;
    let stride = 36 * COSTAS_SYMBOL_LEN;

    for i in 0..COSTAS_BLOCKS {
        let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize);
        for _block in 0..3 {
            if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                let i_start = i_start as usize;
                let mut z_re = 0.0f32;
                let mut z_im = 0.0f32;
                for j in 0..COSTAS_SYMBOL_LEN {
                    let base = i * COSTAS_SYMBOL_LEN + j;
                    let twk_re = twk_re[j] as f32;
                    let twk_im = twk_im[j] as f32;
                    let sync_re = sync_re[base] as f32;
                    let sync_im = sync_im[base] as f32;
                    let tpl_re = twk_re * sync_re - twk_im * sync_im;
                    let tpl_im = twk_re * sync_im + twk_im * sync_re;
                    let d_re = cd0_re[i_start + j] as f32;
                    let d_im = cd0_im[i_start + j] as f32;
                    z_re += d_re * tpl_re + d_im * tpl_im;
                    z_im += d_im * tpl_re - d_re * tpl_im;
                }
                sync += z_re * z_re + z_im * z_im;
            }
            i_start += stride as isize;
        }
    }

    sync as f64
}
