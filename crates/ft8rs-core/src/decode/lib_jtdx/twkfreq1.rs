//! Mirrors JTDX `lib/twkfreq1.f90`.

use super::ft8_downsample::ComplexC;

pub fn twkfreq1(
    ca: &ComplexC,
    _nbot: isize,
    npts: isize,
    _ntop: isize,
    fsample: f64,
    a: &[f64; 5],
) -> ComplexC {
    let twopi = 6.283_185_307_f64;
    let dphi = a[0] * (twopi / fsample);
    let wstep_re = dphi.cos();
    let wstep_im = dphi.sin();
    let mut w_re = 1.0f64;
    let mut w_im = 0.0f64;
    let mut cb = ca.clone();

    for i in 0..=npts {
        let nw_re = w_re * wstep_re - w_im * wstep_im;
        let nw_im = w_re * wstep_im + w_im * wstep_re;
        w_re = nw_re;
        w_im = nw_im;

        let idx = ComplexC::idx(i);
        cb.re[idx] = w_re * ca.re[idx] - w_im * ca.im[idx];
        cb.im[idx] = w_re * ca.im[idx] + w_im * ca.re[idx];
    }

    cb
}
