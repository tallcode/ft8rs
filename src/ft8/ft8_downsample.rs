use super::*;

pub(super) fn ft8_downsample(
    cx_re: &[f64],
    cx_im: &[f64],
    f0: f64,
    workspace: &mut DecodeWorkspace,
) {
    let df = DOWNSAMPLE_DF;
    let baud = DOWNSAMPLE_BAUD;
    let f0 = f0 as f32;
    let i0 = nint_wsjtx_real(f0 / df).max(0) as usize;
    let ft = f0 + 8.5f32 * baud;
    let it = (nint_wsjtx_real(ft / df).max(0) as usize).min(NFFT1_LONG / 2);
    let fb = f0 - 1.5f32 * baud;
    let ib = 1.max(nint_wsjtx_real(fb / df).max(0) as usize);

    workspace.cd0_re.fill(0.0);
    workspace.cd0_im.fill(0.0);
    let mut k = 0;
    for i in ib..=it {
        if k >= NFFT2 {
            break;
        }
        workspace.cd0_re[k] = cx_re[i];
        workspace.cd0_im[k] = cx_im[i];
        k += 1;
    }

    let taper_data = build_taper();
    for i in 0..TAPER_SIZE {
        if i >= NFFT2 {
            break;
        }
        let tap = taper_data[TAPER_SIZE - 1 - i];
        workspace.cd0_re[i] *= tap;
        workspace.cd0_im[i] *= tap;
    }

    let end_tap = k - 1;
    for i in 0..TAPER_SIZE {
        let idx = end_tap - TAPER_SIZE + 1 + i;
        if idx < NFFT2 {
            let tap = taper_data[i];
            workspace.cd0_re[idx] *= tap;
            workspace.cd0_im[idx] *= tap;
        }
    }

    let shift = i0 as isize - ib as isize;
    if shift != 0 {
        for i in 0..NFFT2 {
            let src_idx = (i as isize + shift).rem_euclid(NFFT2 as isize) as usize;
            workspace.shift_re[i] = workspace.cd0_re[src_idx];
            workspace.shift_im[i] = workspace.cd0_im[src_idx];
        }
        workspace.cd0_re.copy_from_slice(&workspace.shift_re);
        workspace.cd0_im.copy_from_slice(&workspace.shift_im);
    }

    four2a_c2c(&mut workspace.cd0_re, &mut workspace.cd0_im, 1);

    for i in 0..NFFT2 {
        workspace.cd0_re[i] = ((workspace.cd0_re[i] * DOWNSAMPLE_FAC) as f32) as f64;
        workspace.cd0_im[i] = ((workspace.cd0_im[i] * DOWNSAMPLE_FAC) as f32) as f64;
    }
}
