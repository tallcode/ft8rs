//! Belief-propagation decoder for FT8 LDPC(174,91).
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/bpdecode174_91.f90`

use crate::decode::chkcrc14a::check_crc14;
use crate::decode::decode174_91::{DecodeResult, KK, M_LDPC, N_LDPC};
use crate::decode::ldpc_174_91_c_parity::*;

/// BP decoding result with accumulated posteriors for OSD.
pub(crate) struct BPResult {
    pub(crate) decoded: Option<DecodeResult>,
    pub(crate) zsave: Vec<Vec<f32>>,
}

pub(crate) fn bp_decode174_91_with_posteriors(
    llr: &[f64],
    apmask: &[i8],
    max_iterations: usize,
    nosd: usize,
    bp_save_limit: usize,
    channel_llr_osd: bool,
) -> BPResult {
    let n = N_LDPC;
    let m = M_LDPC;

    let llr: Vec<f32> = llr.iter().map(|&x| x as f32).collect();
    let mut tov = vec![0.0f32; NCW * n];
    let mut toc = vec![0.0f32; 7 * m];
    let mut tanhtoc = vec![0.0f32; 7 * m];
    let mut zn = vec![0.0f32; n];
    let mut cw = vec![0i8; n];
    let mut zsum = vec![0.0f32; n];
    let mut zsave: Vec<Vec<f32>> = vec![vec![0.0; n]; nosd];
    if channel_llr_osd && nosd >= 1 {
        zsave[0].copy_from_slice(&llr);
    }

    // Initialize messages to checks
    for j in 0..m {
        let w = nrw(j);
        let nm = nm(j);
        for i in 0..w {
            toc[i * m + j] = llr[nm[i]];
        }
    }

    let mut nclast = 0;
    let mut ncnt = 0;

    for iter in 0..=max_iterations {
        // Update bit LLRs
        for i in 0..n {
            if apmask[i] != 1 {
                let mut sum = 0.0;
                for k in 0..NCW {
                    sum += tov[k * n + i];
                }
                zn[i] = llr[i] + sum;
            } else {
                zn[i] = llr[i];
            }
        }

        // WSJT-X: zsum=zsum+zn, save zsave at iter=1..maxosd
        for i in 0..n {
            zsum[i] += zn[i];
        }
        if iter >= 1 && iter <= bp_save_limit && iter <= zsave.len() {
            zsave[iter - 1].copy_from_slice(&zsum);
        }

        // Hard decision
        for i in 0..n {
            cw[i] = if zn[i] > 0.0 { 1 } else { 0 };
        }

        // Check parity
        let mut ncheck = 0;
        for i in 0..m {
            let w = nrw(i);
            let nm = nm(i);
            let mut s = 0;
            for k in 0..w {
                s += cw[nm[k]] as usize;
            }
            if s % 2 != 0 {
                ncheck += 1;
            }
        }

        if ncheck == 0 {
            let bits91: Vec<u8> = cw[..KK].iter().map(|&b| b as u8).collect();
            if check_crc14(&bits91) {
                let mut nharderrors = 0;
                for i in 0..n {
                    if ((2 * cw[i] as i32 - 1) as f32) * llr[i] < 0.0 {
                        nharderrors += 1;
                    }
                }
                return BPResult {
                    decoded: Some(DecodeResult {
                        message91: bits91,
                        cw: cw.iter().map(|&b| b as u8).collect(),
                        nharderrors,
                    }),
                    zsave,
                };
            }
        }

        // Early stopping
        if iter > 0 {
            let nd = ncheck as isize - nclast as isize;
            if nd < 0 {
                ncnt = 0;
            } else {
                ncnt += 1;
            }
            if ncnt >= 5 && iter >= 10 && ncheck > 15 {
                return BPResult {
                    decoded: None,
                    zsave,
                };
            }
        }
        nclast = ncheck;

        // Send messages from bits to check nodes
        for j in 0..m {
            let w = nrw(j);
            let nm = nm(j);
            for i in 0..w {
                let ibj = nm[i];
                let mut val = zn[ibj];
                for kk in 0..NCW {
                    if mn(ibj)[kk] == j {
                        val -= tov[kk * n + ibj];
                    }
                }
                toc[i * m + j] = val;
            }
        }

        // Send messages from check nodes to variable nodes
        for i in 0..m {
            for k in 0..7 {
                tanhtoc[k * m + i] = (-toc[k * m + i] / 2.0).tanh();
            }
        }

        for j in 0..n {
            for i in 0..NCW {
                let ichk = mn(j)[i];
                let w = nrw(ichk);
                let nm = nm(ichk);
                let mut tmn = 1.0f32;
                for k in 0..w {
                    if nm[k] != j {
                        tmn *= tanhtoc[k * m + ichk];
                    }
                }
                tov[i * n + j] = 2.0 * crate::decode::platanh::platanh(-tmn);
            }
        }
    }

    BPResult {
        decoded: None,
        zsave,
    }
}
