//! Nuttall window used by WSJT-X spectrum routines.
//!
//! Source mapping:
//! - `wsjtx/lib/nuttal_window.f90`

pub(crate) fn nuttal_window(n: usize) -> Vec<f64> {
    let mut win = vec![0.0; n];
    let nf = n as f64;
    let a0 = 0.3635819;
    let a1 = -0.4891775;
    let a2 = 0.1365995;
    let a3 = -0.0106411;
    for (i, slot) in win.iter_mut().enumerate().take(n) {
        let x = 2.0 * std::f64::consts::PI * i as f64 / nf;
        *slot = a0 + a1 * x.cos() + a2 * (2.0 * x).cos() + a3 * (3.0 * x).cos();
    }
    win
}
