//! Mirrors the `csynce` construction in JTDX `lib/tone8.f90`.

use super::ft8v2::packjt77sd::genft8sd;
use super::gen_ft8wave::gen_ft8wave;

#[derive(Clone, Debug)]
pub struct CsyncE {
    pub re: [[f64; 32]; 19],
    pub im: [[f64; 32]; 19],
}

impl Default for CsyncE {
    fn default() -> Self {
        Self {
            re: [[0.0; 32]; 19],
            im: [[0.0; 32]; 19],
        }
    }
}

pub(crate) fn build_csynce(mycall: &str, hiscall: &str) -> Option<CsyncE> {
    let msg = format!("{} {} -01", mycall.trim(), hiscall.trim());
    let (_, _, itone) = genft8sd(&msg)?;
    let (wave_re, wave_im) = gen_ft8wave(&itone, 0.0);
    let mut out = CsyncE::default();
    let mut m = 7 * 1920;
    for j in 0..19 {
        for k in 0..32 {
            out.re[j][k] = wave_re[m];
            out.im[j][k] = wave_im[m];
            m += 60;
        }
    }
    Some(out)
}
