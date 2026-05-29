use super::{
    FrequencyShiftSyncTemplate, SyncTemplate, COSTAS_BLOCKS, COSTAS_SYMBOL_LEN, DT2, PI_F32,
    TAPER_SIZE, TWO_PI_F32,
};
use crate::ft8::constants::COSTAS;
use std::sync::OnceLock;

/// Lazy-initialized FT8 downsample taper.
pub(super) fn build_taper() -> &'static Vec<f64> {
    static T: OnceLock<Vec<f64>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![0.0; TAPER_SIZE];
        let last = TAPER_SIZE - 1;
        for (i, slot) in t.iter_mut().enumerate().take(TAPER_SIZE) {
            let x = (i as f32 * PI_F32) / last as f32;
            *slot = (0.5f32 * (1.0f32 + x.cos())) as f64;
        }
        t
    })
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

pub(super) fn build_frequency_shift_sync_templates() -> &'static Vec<FrequencyShiftSyncTemplate> {
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
