//! Mirrors JTDX `lib/ft8v2/bpdecode174_91.f90`.

use super::chkcrc14a::chkcrc14a;
use super::ldpc_174_91_c_reordered_parity::{mn, nm, nrw, NCW};

pub(crate) const K: usize = 91;
pub(crate) const N: usize = 174;
pub(crate) const M: usize = N - K;

#[derive(Clone, Debug)]
pub(crate) struct BpDecodeResult {
    pub(crate) message77: [u8; 77],
    pub(crate) cw: [u8; N],
    pub(crate) nharderror: isize,
    pub(crate) dmin: f32,
}

pub(crate) fn bpdecode174_91(
    llr: &[f32; N],
    apmask: &[i8; N],
    maxiterations: usize,
) -> Option<BpDecodeResult> {
    let mut decoded = [0u8; K];
    let mut toc = [[0.0f32; M]; 7];
    let mut tov = [[0.0f32; N]; NCW];
    let mut tanhtoc = [[0.0f32; M]; 7];
    let mut zn = [0.0f32; N];
    let mut cw = [0u8; N];
    let mut nclast = 0usize;

    for j in 0..M {
        for (i, &bit) in nm(j).iter().enumerate() {
            toc[i][j] = llr[bit];
        }
    }

    let mut ncnt = 0usize;

    for iter in 0..=maxiterations {
        for i in 0..N {
            if apmask[i] != 1 {
                let mut sum_tov = 0.0f32;
                for row in tov.iter().take(NCW) {
                    sum_tov += row[i];
                }
                zn[i] = llr[i] + sum_tov;
            } else {
                zn[i] = llr[i];
            }
        }

        for i in 0..N {
            cw[i] = if zn[i] > 0.0 { 1 } else { 0 };
        }

        let mut ncheck = 0usize;
        for i in 0..M {
            let mut syndrome = 0usize;
            for &bit in nm(i) {
                syndrome += cw[bit] as usize;
            }
            if syndrome % 2 != 0 {
                ncheck += 1;
            }
        }

        if ncheck == 0 {
            decoded.copy_from_slice(&cw[..K]);
            if chkcrc14a(&decoded) {
                let mut nharderror = 0isize;
                for i in 0..N {
                    if ((2 * cw[i] as i32 - 1) as f32) * llr[i] < 0.0 {
                        nharderror += 1;
                    }
                }
                let mut message77 = [0u8; 77];
                message77.copy_from_slice(&decoded[..77]);
                return Some(BpDecodeResult {
                    message77,
                    cw,
                    nharderror,
                    dmin: 0.0,
                });
            }
        }

        if iter > 0 {
            let nd = ncheck as isize - nclast as isize;
            if nd < 0 {
                ncnt = 0;
            } else {
                ncnt += 1;
            }
            if ncnt >= 5 && iter >= 10 && ncheck > 15 {
                return None;
            }
        }
        nclast = ncheck;

        for j in 0..M {
            for (i, &ibj) in nm(j).iter().enumerate() {
                let mut val = zn[ibj];
                for kk in 0..NCW {
                    if mn(ibj)[kk] == j {
                        val -= tov[kk][ibj];
                    }
                }
                toc[i][j] = val;
            }
        }

        for i in 0..M {
            for k in 0..7 {
                tanhtoc[k][i] = (-toc[k][i] / 2.0).tanh();
            }
        }

        for j in 0..N {
            for i in 0..NCW {
                let ichk = mn(j)[i];
                let mut tmn = 1.0f32;
                for (k, &bit) in nm(ichk).iter().enumerate().take(nrw(ichk)) {
                    if bit != j {
                        tmn *= tanhtoc[k][ichk];
                    }
                }
                tov[i][j] = 2.0 * platanh(-tmn);
            }
        }
    }

    None
}

fn platanh(x: f32) -> f32 {
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
