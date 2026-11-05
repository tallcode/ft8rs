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
    let _span = super::metrics::Span::new(super::metrics::Kind::Bp);
    let graph = graph();
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
        let w = graph.weights[j];
        let nm = &graph.checks[j];
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
            let w = graph.weights[i];
            let nm = &graph.checks[i];
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
            let w = graph.weights[j];
            let nm = &graph.checks[j];
            for i in 0..w {
                let ibj = nm[i];
                let val = zn[ibj] - tov[graph.incoming[j][i]];
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
                let edge = &graph.products[j][i];
                let mut tmn = 1.0f32;
                for &index in &edge.indices[..edge.len] {
                    tmn *= tanhtoc[index];
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

struct Product {
    indices: [usize; 6],
    len: usize,
}

struct Graph {
    checks: [[usize; 7]; M_LDPC],
    weights: [usize; M_LDPC],
    incoming: [[usize; 7]; M_LDPC],
    products: [[Product; NCW]; N_LDPC],
}

fn graph() -> &'static Graph {
    static GRAPH: std::sync::OnceLock<Graph> = std::sync::OnceLock::new();
    GRAPH.get_or_init(|| {
        let checks = std::array::from_fn(|j| {
            let mut row = [0; 7];
            row[..nrw(j)].copy_from_slice(nm(j));
            row
        });
        let weights = std::array::from_fn(nrw);
        let incoming = std::array::from_fn(|j| {
            std::array::from_fn(|i| {
                if i >= weights[j] {
                    return 0;
                }
                let bit = checks[j][i];
                let slot = mn(bit).iter().position(|&check| check == j).unwrap();
                slot * N_LDPC + bit
            })
        });
        let products = std::array::from_fn(|bit| {
            std::array::from_fn(|edge| {
                let check = mn(bit)[edge];
                let mut product = Product {
                    indices: [0; 6],
                    len: 0,
                };
                for (k, &other) in checks[check][..weights[check]].iter().enumerate() {
                    if other != bit {
                        product.indices[product.len] = k * M_LDPC + check;
                        product.len += 1;
                    }
                }
                assert_eq!(product.len + 1, weights[check]);
                product
            })
        });
        Graph {
            checks,
            weights,
            incoming,
            products,
        }
    })
}

#[test]
fn precomputed_bp_preserves_posteriors_and_decisions() {
    let mut seed = 0x123456789abcdefu64;
    for case in 0..96 {
        let llr: Vec<_> = (0..N_LDPC)
            .map(|i| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                if case == 0 {
                    0.0
                } else if case == 1 {
                    8.0
                } else {
                    ((seed >> 32) as i32 as f64 / i32::MAX as f64) * ((case % 12 + 1) as f64)
                        + (i % 3) as f64 * 0.1
                }
            })
            .collect();
        let mask: Vec<_> = (0..N_LDPC)
            .map(|i| i8::from(case % 3 == 0 && i % 7 == 0))
            .collect();
        for (nosd, limit, channel) in [(0, 0, false), (1, 0, true), (3, 3, false)] {
            let old = crate::decode::bpdecode174_91::bp_decode174_91_with_posteriors(
                &llr, &mask, 30, nosd, limit, channel,
            );
            let new = bp_decode174_91_with_posteriors(&llr, &mask, 30, nosd, limit, channel);
            assert_eq!(old.zsave.len(), new.zsave.len());
            for (a, b) in old.zsave.iter().zip(&new.zsave) {
                assert!(
                    a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()),
                    "posterior case={case}"
                );
            }
            assert_eq!(
                old.decoded
                    .as_ref()
                    .map(|r| (&r.message91, &r.cw, r.nharderrors)),
                new.decoded
                    .as_ref()
                    .map(|r| (&r.message91, &r.cw, r.nharderrors)),
                "decision case={case}"
            );
        }
    }
}
