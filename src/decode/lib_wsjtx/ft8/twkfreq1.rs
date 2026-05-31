//! Polynomial frequency correction for downsampled FT8 symbols.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/twkfreq1.f90`

pub(crate) fn twkfreq1(
    ca_re: &[f64],
    ca_im: &[f64],
    npts: usize,
    fsample: f64,
    a: &[f64; 5],
) -> (Vec<f64>, Vec<f64>) {
    let twopi = 6.283185307;
    let x0 = 0.5 * (npts as f64 + 1.0);
    let s = 2.0 / npts as f64;
    let mut cb_re = Vec::with_capacity(npts);
    let mut cb_im = Vec::with_capacity(npts);
    let mut w_re = 1.0f64;
    let mut w_im = 0.0f64;
    for i in 1..=npts {
        let x = s * (i as f64 - x0);
        let p2 = 1.5 * x * x - 0.5;
        let p3 = 2.5 * x.powi(3) - 1.5 * x;
        let p4 = 4.375 * x.powi(4) - 3.75 * x * x + 0.375;
        let dphi = (a[0] + x * a[1] + p2 * a[2] + p3 * a[3] + p4 * a[4]) * (twopi / fsample);
        let ws_re = dphi.cos();
        let ws_im = dphi.sin();
        let nw_re = w_re * ws_re - w_im * ws_im;
        let nw_im = w_re * ws_im + w_im * ws_re;
        w_re = nw_re;
        w_im = nw_im;
        cb_re.push(w_re * ca_re[i - 1] - w_im * ca_im[i - 1]);
        cb_im.push(w_re * ca_im[i - 1] + w_im * ca_re[i - 1]);
    }
    (cb_re, cb_im)
}
