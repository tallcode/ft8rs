//! Mirrors JTDX `lib/sync8d.f90`.

use std::sync::OnceLock;

use super::ft8_downsample::{ComplexC, C_HIGH, C_LOW};
use super::ft8_mod1::ICOS7;
use super::ft8_params::{DT2, TWO_PI};
use super::tone8::CsyncE;
use super::tonesd::TonesdTemplates;

const COSTAS_BLOCKS: usize = 7;
const COSTAS_SYMBOL_LEN: usize = 32;

pub struct SyncTemplates {
    pub csync_re: Vec<f64>,
    pub csync_im: Vec<f64>,
    pub csynccq_re: [[f64; 32]; 8],
    pub csynccq_im: [[f64; 32]; 8],
}

pub fn build_csync() -> &'static SyncTemplates {
    static T: OnceLock<SyncTemplates> = OnceLock::new();
    T.get_or_init(|| {
        let mut csync_re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        let mut csync_im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        let mut csynccq_re = [[0.0; 32]; 8];
        let mut csynccq_im = [[0.0; 32]; 8];
        if let Some((_, _, itone)) = super::genft8sd::genft8sd("CQ 2E0DLA IO92") {
            let (wave_re, wave_im) = super::gen_ft8wave::gen_ft8wave(&itone, 0.0);
            let mut m = 7 * 32 * 60;
            for i in 0..8 {
                for j in 0..32 {
                    csynccq_re[i][j] = wave_re[m];
                    csynccq_im[i][j] = wave_im[m];
                    m += 60;
                }
            }
            m = 0;
            for i in 0..COSTAS_BLOCKS {
                for j in 0..COSTAS_SYMBOL_LEN {
                    let idx = i * COSTAS_SYMBOL_LEN + j;
                    csync_re[idx] = wave_re[m];
                    csync_im[idx] = wave_im[m];
                    m += 60;
                }
            }
        } else {
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
        }
        SyncTemplates {
            csync_re,
            csync_im,
            csynccq_re,
            csynccq_im,
        }
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
pub struct Sync8dContext<'a> {
    pub ipass: usize,
    pub lastsync: bool,
    pub iqso: usize,
    pub lcq: bool,
    pub lcallsstd: bool,
    pub lcqcand: bool,
    pub tonesd: Option<&'a TonesdTemplates>,
    pub csynce: Option<&'a CsyncE>,
}

pub fn sync8d(
    cd0: &ComplexC,
    i0: isize,
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
    context: Sync8dContext<'_>,
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
                let z1 = avg_hold_last(&zt1, i);
                let z2 = avg_hold_last(&zt2, i);
                let z3 = avg_hold_last(&zt3, i);
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

    if ctwk_re.is_some() && context.lcqcand && context.iqso == 1 {
        let mut zt4 = [(0.0f64, 0.0f64); 8];
        for (i, z) in zt4.iter_mut().enumerate() {
            let i4 = i0 + (i as isize + 7) * 32;
            *z = sync_sum_explicit(
                cd0,
                i4,
                &templates.csynccq_re[i],
                &templates.csynccq_im[i],
                ctwk_re,
                ctwk_im,
            );
        }
        for i in 0..8 {
            let z = if matches!(context.ipass, 3 | 4 | 8) {
                avg_hold_last(&zt4, i)
            } else {
                zt4[i]
            };
            sync += sync_component(z, context.ipass);
        }
    }

    if !context.lastsync {
        if (context.iqso == 2 || context.iqso == 3) && context.lcallsstd {
            if let Some(csynce) = context.csynce {
                sync += sync_csynce(cd0, i0, ctwk_re, ctwk_im, context, csynce);
            }
        }
        if context.iqso == 4 {
            if let Some(tonesd) = context.tonesd {
                if context.lcq {
                    sync += sync_superdeep_cq(cd0, i0, ctwk_re, ctwk_im, context, tonesd);
                } else {
                    sync += sync_superdeep_qso(cd0, i0, ctwk_re, ctwk_im, context, tonesd);
                }
            }
        }
    }

    if !sync.is_finite() {
        sync = 0.0;
    }
    sync
}

fn sync_csynce(
    cd0: &ComplexC,
    i0: isize,
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
    context: Sync8dContext<'_>,
    templates: &CsyncE,
) -> f64 {
    let mut zt5 = [(0.0f64, 0.0f64); 19];
    for (i, z) in zt5.iter_mut().enumerate() {
        let i4 = i0 + (i as isize + 7) * 32;
        *z = sync_sum_explicit(
            cd0,
            i4,
            &templates.re[i],
            &templates.im[i],
            ctwk_re,
            ctwk_im,
        );
    }
    let mut sync = 0.0;
    for i in 0..19 {
        let z = if matches!(context.ipass, 2 | 6 | 7) {
            avg_hold_last(&zt5, i)
        } else {
            zt5[i]
        };
        sync += sync_component(z, context.ipass);
    }
    sync
}

fn sync_superdeep_qso(
    cd0: &ComplexC,
    i0: isize,
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
    context: Sync8dContext<'_>,
    templates: &TonesdTemplates,
) -> f64 {
    let mut zt5 = [(0.0f64, 0.0f64); 19];
    for (i, z) in zt5.iter_mut().enumerate() {
        let i4 = i0 + (i as isize + 7) * 32;
        *z = sync_sum_explicit(
            cd0,
            i4,
            &templates.csyncsd_re[i],
            &templates.csyncsd_im[i],
            ctwk_re,
            ctwk_im,
        );
    }
    let mut sync = 0.0;
    for i in 0..19 {
        let z = if matches!(context.ipass, 2 | 6 | 7) {
            avg_hold_last(&zt5, i)
        } else {
            zt5[i]
        };
        sync += sync_component(z, context.ipass);
    }
    sync
}

fn sync_superdeep_cq(
    cd0: &ComplexC,
    i0: isize,
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
    context: Sync8dContext<'_>,
    templates: &TonesdTemplates,
) -> f64 {
    let mut sync = 0.0;
    for i in 0..58 {
        let k = if i < 29 { i + 7 } else { i + 14 };
        let i4 = i0 + k as isize * 32;
        let z = sync_sum_explicit(
            cd0,
            i4,
            &templates.csyncsdcq_re[i],
            &templates.csyncsdcq_im[i],
            ctwk_re,
            ctwk_im,
        );
        sync += sync_component(z, context.ipass);
    }
    sync
}

fn sync_component(z: (f64, f64), ipass: usize) -> f64 {
    match ipass {
        1 | 5 | 9 => magnitude(z),
        2 | 6 | 7 => power(z),
        3 | 4 | 8 => z.0.abs() + z.1.abs(),
        _ => power(z),
    }
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
    if i0 < C_LOW || i0 + 31 > C_HIGH {
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

fn sync_sum_explicit(
    cd0: &ComplexC,
    i0: isize,
    tpl_re: &[f64; 32],
    tpl_im: &[f64; 32],
    ctwk_re: Option<&[f64; 32]>,
    ctwk_im: Option<&[f64; 32]>,
) -> (f64, f64) {
    if i0 < C_LOW || i0 + 31 > C_HIGH {
        return (0.0, 0.0);
    }
    let mut out_re = 0.0;
    let mut out_im = 0.0;
    for j in 0..32 {
        let cd_idx = ComplexC::idx(i0 + j as isize);
        let mut re = tpl_re[j];
        let mut im = tpl_im[j];
        if let (Some(twk_re), Some(twk_im)) = (ctwk_re, ctwk_im) {
            let r = twk_re[j] * re - twk_im[j] * im;
            let q = twk_re[j] * im + twk_im[j] * re;
            re = r;
            im = q;
        }
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

fn avg_hold_last(values: &[(f64, f64)], i: usize) -> (f64, f64) {
    if i + 1 < values.len() {
        avg(values[i], values[i + 1])
    } else if values.len() >= 2 {
        avg(values[values.len() - 2], values[values.len() - 1])
    } else {
        values.get(i).copied().unwrap_or((0.0, 0.0))
    }
}
