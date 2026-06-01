//! Mirrors JTDX `lib/tonesd.f90`.

use super::ft8v2::packjt77sd::genft8sd;
use super::gen_ft8wave::gen_ft8wave;

#[derive(Clone, Debug)]
pub struct TonesdTemplates {
    pub(crate) csyncsd_re: [[f64; 32]; 19],
    pub(crate) csyncsd_im: [[f64; 32]; 19],
    pub(crate) csyncsdcq_re: [[f64; 32]; 58],
    pub(crate) csyncsdcq_im: [[f64; 32]; 58],
}

impl Default for TonesdTemplates {
    fn default() -> Self {
        Self {
            csyncsd_re: [[0.0; 32]; 19],
            csyncsd_im: [[0.0; 32]; 19],
            csyncsdcq_re: [[0.0; 32]; 58],
            csyncsdcq_im: [[0.0; 32]; 58],
        }
    }
}

pub(crate) fn tonesd(msgd: &str, lcq: bool) -> Option<TonesdTemplates> {
    let msgd = msgd.trim();
    let mut out = TonesdTemplates::default();
    let itone = if lcq {
        genft8sd(msgd)?.2
    } else {
        let (c1, c2, _) = split_message3(msgd)?;
        genft8sd(&format!("{c1} {c2} +09"))?.2
    };

    let (wave_re, wave_im) = gen_ft8wave(&itone, 0.0);
    let mut m = 7 * 1920;
    if lcq {
        for i in 0..58 {
            if i == 29 {
                m += 7 * 1920;
            }
            for j in 0..32 {
                out.csyncsdcq_re[i][j] = wave_re[m];
                out.csyncsdcq_im[i][j] = wave_im[m];
                m += 60;
            }
        }
    } else {
        for i in 0..19 {
            for j in 0..32 {
                out.csyncsd_re[i][j] = wave_re[m];
                out.csyncsd_im[i][j] = wave_im[m];
                m += 60;
            }
        }
    }
    Some(out)
}

fn split_message3(msg: &str) -> Option<(&str, &str, &str)> {
    let mut parts = msg.split_whitespace();
    Some((parts.next()?, parts.next()?, parts.next()?))
}
