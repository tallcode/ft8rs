//! WSJT-X FT8 coarse downsample path.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/ft8_downsample.f90`

use super::{
    nint_wsjtx_real, DecodeWorkspace, DOWNSAMPLE_BAUD, DOWNSAMPLE_DF, DOWNSAMPLE_FAC, NFFT1_LONG,
    NFFT2, PI_F32, TAPER_SIZE,
};
use crate::util::four2a_c2c;
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

pub(super) fn ft8_downsample(
    cx_re: &[f64],
    cx_im: &[f64],
    f0: f64,
    workspace: &mut DecodeWorkspace,
) {
    ft8_downsample_from_cx(
        cx_re,
        cx_im,
        f0,
        &mut workspace.cd0_re,
        &mut workspace.cd0_im,
        &mut workspace.shift_re,
        &mut workspace.shift_im,
    );
}

pub(crate) fn ft8_downsample_from_cx(
    cx_re: &[f64],
    cx_im: &[f64],
    f0: f64,
    cd0_re: &mut [f64],
    cd0_im: &mut [f64],
    shift_re: &mut [f64],
    shift_im: &mut [f64],
) {
    let df = DOWNSAMPLE_DF;
    let baud = DOWNSAMPLE_BAUD;
    let f0 = f0 as f32;
    let i0 = nint_wsjtx_real(f0 / df).max(0) as usize;
    let ft = f0 + 8.5f32 * baud;
    let it = (nint_wsjtx_real(ft / df).max(0) as usize).min(NFFT1_LONG / 2);
    let fb = f0 - 1.5f32 * baud;
    let ib = 1.max(nint_wsjtx_real(fb / df).max(0) as usize);

    debug_assert!(cd0_re.len() >= NFFT2);
    debug_assert!(cd0_im.len() >= NFFT2);
    debug_assert!(shift_re.len() >= NFFT2);
    debug_assert!(shift_im.len() >= NFFT2);

    cd0_re[..NFFT2].fill(0.0);
    cd0_im[..NFFT2].fill(0.0);
    let mut k = 0;
    for i in ib..=it {
        if k >= NFFT2 {
            break;
        }
        cd0_re[k] = cx_re[i];
        cd0_im[k] = cx_im[i];
        k += 1;
    }

    let taper_data = build_taper();
    for i in 0..TAPER_SIZE {
        if i >= NFFT2 {
            break;
        }
        let tap = taper_data[TAPER_SIZE - 1 - i];
        cd0_re[i] *= tap;
        cd0_im[i] *= tap;
    }

    let end_tap = k - 1;
    for i in 0..TAPER_SIZE {
        let idx = end_tap - TAPER_SIZE + 1 + i;
        if idx < NFFT2 {
            let tap = taper_data[i];
            cd0_re[idx] *= tap;
            cd0_im[idx] *= tap;
        }
    }

    let shift = i0 as isize - ib as isize;
    if shift != 0 {
        for i in 0..NFFT2 {
            let src_idx = (i as isize + shift).rem_euclid(NFFT2 as isize) as usize;
            shift_re[i] = cd0_re[src_idx];
            shift_im[i] = cd0_im[src_idx];
        }
        cd0_re[..NFFT2].copy_from_slice(&shift_re[..NFFT2]);
        cd0_im[..NFFT2].copy_from_slice(&shift_im[..NFFT2]);
    }

    four2a_c2c(&mut cd0_re[..NFFT2], &mut cd0_im[..NFFT2], 1);

    for i in 0..NFFT2 {
        cd0_re[i] = ((cd0_re[i] as f32) * DOWNSAMPLE_FAC) as f64;
        cd0_im[i] = ((cd0_im[i] as f32) * DOWNSAMPLE_FAC) as f64;
    }
}
