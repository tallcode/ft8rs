use super::{COSTAS_BLOCKS, COSTAS_SYMBOL_LEN, NP2};

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
