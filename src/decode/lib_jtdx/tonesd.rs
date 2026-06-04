//! Mirrors JTDX `lib/tonesd.f90`.

use super::gen_ft8wave::gen_ft8wave;
use super::genft8sd::genft8sd;

#[derive(Clone, Debug)]
pub(crate) struct TonesdMessage {
    pub(crate) msg37: String,
    pub(crate) msgbits: [u8; 77],
    pub(crate) itone: [i32; 79],
    pub(crate) idtone: [i32; 58],
}

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

pub(crate) fn tonesd_messages(c1: &str, c2: &str, grid: &str) -> Option<Vec<TonesdMessage>> {
    const RPT: [&str; 75] = [
        "+09", "+08", "+07", "+06", "+05", "+04", "+03", "+02", "+01", "+00", "-01", "-02", "-03",
        "-04", "-05", "-06", "-07", "-08", "-09", "-10", "-11", "-12", "-13", "-14", "-15", "-16",
        "-17", "-18", "-19", "-20", "-21", "-22", "-23", "-24", "-25", "-26", "R+09", "R+08",
        "R+07", "R+06", "R+05", "R+04", "R+03", "R+02", "R+01", "R+00", "R-01", "R-02", "R-03",
        "R-04", "R-05", "R-06", "R-07", "R-08", "R-09", "R-10", "R-11", "R-12", "R-13", "R-14",
        "R-15", "R-16", "R-17", "R-18", "R-19", "R-20", "R-21", "R-22", "R-23", "R-24", "R-25",
        "R-26", "RRR", "RR73", "73",
    ];

    let mut out = Vec::with_capacity(76);
    for rpt in RPT {
        out.push(tonesd_message(&format!("{c1} {c2} {rpt}"))?);
    }
    out.push(tonesd_message(&format!("{c1} {c2} {grid}"))?);
    Some(out)
}

fn tonesd_message(msg: &str) -> Option<TonesdMessage> {
    let (msg37, msgbits, itone) = genft8sd(msg)?;
    let mut idtone = [0i32; 58];
    idtone[..29].copy_from_slice(&itone[7..36]);
    idtone[29..].copy_from_slice(&itone[43..72]);
    Some(TonesdMessage {
        msg37,
        msgbits,
        itone,
        idtone,
    })
}

fn split_message3(msg: &str) -> Option<(&str, &str, &str)> {
    let mut parts = msg.split_whitespace();
    Some((parts.next()?, parts.next()?, parts.next()?))
}
