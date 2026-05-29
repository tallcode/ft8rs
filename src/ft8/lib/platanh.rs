//! Piecewise approximation used by WSJT-X BP decoders.
//!
//! Source mapping:
//! - `wsjtx/lib/platanh.f90`

pub(crate) fn platanh(x: f32) -> f32 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let z = x.abs();
    if z <= 0.664 {
        x / 0.83
    } else if z <= 0.9217 {
        sign * (z - 0.4064) / 0.322
    } else if z <= 0.9951 {
        sign * (z - 0.8378) / 0.0524
    } else if z <= 0.9998 {
        sign * (z - 0.9914) / 0.0012
    } else {
        sign * 7.0
    }
}
