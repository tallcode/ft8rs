//! Mirrors JTDX `lib/ft8_downsample.f90`.

use std::f64::consts::PI;

use crate::util::{four2a_c2c, four2a_r2c};

use super::ft8_params::{NFFT1_LONG, NFFT2};

pub const C_LOW: isize = -800;
pub const C_HIGH: isize = 4000;
pub const C_LEN: usize = (C_HIGH - C_LOW + 1) as usize;

#[derive(Clone, Debug)]
pub struct ComplexC {
    pub re: Vec<f64>,
    pub im: Vec<f64>,
}

impl ComplexC {
    pub fn zeroed() -> Self {
        Self {
            re: vec![0.0; C_LEN],
            im: vec![0.0; C_LEN],
        }
    }

    #[inline]
    pub fn idx(i: isize) -> usize {
        debug_assert!((C_LOW..=C_HIGH).contains(&i));
        (i - C_LOW) as usize
    }

    pub fn clear(&mut self) {
        self.re.fill(0.0);
        self.im.fill(0.0);
    }
}

#[derive(Debug)]
pub struct DownsampleWorkspace {
    cxx_re: Vec<f64>,
    cxx_im: Vec<f64>,
    c1_re: Vec<f64>,
    c1_im: Vec<f64>,
    shift_re: Vec<f64>,
    shift_im: Vec<f64>,
    windowc1: [f64; 55],
    facc1: f64,
    has_cxx: bool,
}

impl Default for DownsampleWorkspace {
    fn default() -> Self {
        Self::new()
    }
}

impl DownsampleWorkspace {
    pub fn new() -> Self {
        Self {
            cxx_re: vec![0.0; NFFT1_LONG],
            cxx_im: vec![0.0; NFFT1_LONG],
            c1_re: vec![0.0; NFFT2],
            c1_im: vec![0.0; NFFT2],
            shift_re: vec![0.0; NFFT2],
            shift_im: vec![0.0; NFFT2],
            windowc1: windowc1(),
            facc1: 0.01 / 61440.0f64.sqrt(),
            has_cxx: false,
        }
    }

    pub fn clear_cache(&mut self) {
        self.has_cxx = false;
    }
}

#[derive(Debug)]
pub struct DownsampleOutput {
    pub c0: ComplexC,
    pub c2: ComplexC,
    pub c3: ComplexC,
}

impl Default for DownsampleOutput {
    fn default() -> Self {
        Self {
            c0: ComplexC::zeroed(),
            c2: ComplexC::zeroed(),
            c3: ComplexC::zeroed(),
        }
    }
}

pub fn ft8_downsample(
    workspace: &mut DownsampleWorkspace,
    dd8: &[f32],
    newdat1: bool,
    f0: f32,
    nqso: usize,
    lhighsens: bool,
    lsubtracted: &mut bool,
    npos: &mut usize,
    freqsub: &[f32],
    out: &mut DownsampleOutput,
) {
    let mut ldofft = false;
    if *lsubtracted {
        for freq in freqsub.iter().take(*npos) {
            if (f0 - *freq).abs() < 50.0 {
                ldofft = true;
                *lsubtracted = false;
                break;
            }
        }
    }

    if newdat1 || ldofft || !workspace.has_cxx {
        workspace.cxx_re.fill(0.0);
        workspace.cxx_im.fill(0.0);
        for (dst, src) in workspace.cxx_re.iter_mut().zip(dd8.iter().copied()) {
            *dst = src as f64;
        }
        four2a_r2c(&mut workspace.cxx_re, &mut workspace.cxx_im);
        *npos = 0;
        workspace.has_cxx = true;
    }

    downsample_from_cached_spectrum(workspace, f0, nqso, lhighsens, out);
}

fn downsample_from_cached_spectrum(
    workspace: &mut DownsampleWorkspace,
    f0: f32,
    nqso: usize,
    lhighsens: bool,
    out: &mut DownsampleOutput,
) {
    let df = 0.0625f32;
    let i0 = nint(f0 / df);
    let ft = f0 + 55.75;
    let it = nint(ft / df).clamp(0, (NFFT1_LONG / 2) as i32) as usize;
    let fb = f0 - 5.75;
    let ib = nint(fb / df).max(1) as usize;

    workspace.c1_re.fill(0.0);
    workspace.c1_im.fill(0.0);

    let k = it.saturating_sub(ib).min(NFFT2 - 1);
    for dst in 0..=k {
        let src = ib + dst;
        workspace.c1_re[dst] = workspace.cxx_re[src];
        workspace.c1_im[dst] = workspace.cxx_im[src];
    }

    for i in 0..=54 {
        let tap = workspace.windowc1[54 - i];
        workspace.c1_re[i] *= tap;
        workspace.c1_im[i] *= tap;
    }
    if k >= 54 {
        for i in 0..=54 {
            let idx = k - 54 + i;
            let tap = workspace.windowc1[i];
            workspace.c1_re[idx] *= tap;
            workspace.c1_im[idx] *= tap;
        }
    }

    let shift = i0 - ib as i32;
    for i in 0..NFFT2 {
        let src = (i as i32 + shift).rem_euclid(NFFT2 as i32) as usize;
        workspace.shift_re[i] = workspace.c1_re[src];
        workspace.shift_im[i] = workspace.c1_im[src];
    }
    workspace.c1_re.copy_from_slice(&workspace.shift_re);
    workspace.c1_im.copy_from_slice(&workspace.shift_im);

    if lhighsens {
        for (idx, scale) in [(0, 1.93), (799, 1.7), (800, 1.7), (3199, 1.93)] {
            workspace.c1_re[idx] *= scale;
            workspace.c1_im[idx] *= scale;
        }
    } else {
        for idx in [45, 54, 3145, 3154] {
            workspace.c1_re[idx] *= 1.49;
            workspace.c1_im[idx] *= 1.49;
        }
    }

    four2a_c2c(&mut workspace.c1_re, &mut workspace.c1_im, 1);

    out.c0.clear();
    for i in 0..NFFT2 {
        let dst = ComplexC::idx(i as isize);
        out.c0.re[dst] = workspace.facc1 * workspace.c1_re[i];
        out.c0.im[dst] = workspace.facc1 * workspace.c1_im[i];
    }

    out.c2.clear();
    if nqso > 1 {
        for i in 0..3199 {
            let dst = ComplexC::idx(i as isize);
            let a = ComplexC::idx(i as isize);
            let b = ComplexC::idx(i as isize + 1);
            out.c2.re[dst] = 0.5 * (out.c0.re[a] + out.c0.re[b]);
            out.c2.im[dst] = 0.5 * (out.c0.im[a] + out.c0.im[b]);
        }
        let dst = ComplexC::idx(3199);
        out.c2.re[dst] = 0.5 * out.c0.re[dst];
        out.c2.im[dst] = 0.5 * out.c0.im[dst];
    }

    out.c3.clear();
    if nqso == 3 {
        let zero = ComplexC::idx(0);
        out.c3.re[zero] = 0.5 * out.c0.re[zero];
        out.c3.im[zero] = 0.5 * out.c0.im[zero];
        for i in 1..3200 {
            let dst = ComplexC::idx(i as isize);
            let a = ComplexC::idx(i as isize - 1);
            let b = ComplexC::idx(i as isize);
            out.c3.re[dst] = 0.5 * (out.c0.re[a] + out.c0.re[b]);
            out.c3.im[dst] = 0.5 * (out.c0.im[a] + out.c0.im[b]);
        }
    }
}

fn windowc1() -> [f64; 55] {
    let mut out = [0.0; 55];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = (1.0 + ((i as f64 * PI) / 55.0).cos()) / 2.0;
    }
    out
}

fn nint(x: f32) -> i32 {
    x.round() as i32
}
