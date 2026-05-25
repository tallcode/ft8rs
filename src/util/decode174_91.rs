/// LDPC (174,91) Belief Propagation decoder for FT8.

use crate::util::constants::N_LDPC;
use crate::util::crc::check_crc14;
use crate::util::ldpc_tables::*;

const KK: usize = 91;
const M_LDPC: usize = N_LDPC - KK; // 83

pub struct DecodeResult {
    pub message91: Vec<u8>,
    pub cw: Vec<u8>,
    pub nharderrors: usize,
    pub dmin: f64,
    pub ntype: usize,
}

fn platanh(x: f64) -> f64 {
    if x > 0.9999999 {
        return 18.71;
    }
    if x < -0.9999999 {
        return -18.71;
    }
    0.5 * ((1.0 + x) / (1.0 - x)).ln()
}

/// BP decoding result with accumulated posteriors for OSD.
pub struct BPResult {
    pub decoded: Option<DecodeResult>,
    pub zsave: Vec<Vec<f64>>,
}

pub fn bp_decode174_91_with_posteriors(
    llr: &[f64],
    apmask: &[i8],
    max_iterations: usize,
    nosd: usize,
    bp_save_limit: usize,
    channel_llr_osd: bool,
) -> BPResult {
    let n = N_LDPC;
    let m = M_LDPC;

    let mut tov = vec![0.0; NCW * n];
    let mut toc = vec![0.0; 7 * m];
    let mut tanhtoc = vec![0.0; 7 * m];
    let mut zn = vec![0.0; n];
    let mut cw = vec![0i8; n];
    let mut zsum = vec![0.0; n];
    let mut zsave: Vec<Vec<f64>> = vec![vec![0.0; n]; nosd];
    if channel_llr_osd && nosd >= 1 {
        zsave[0].copy_from_slice(llr);
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
                let mut dmin = 0.0;
                for i in 0..n {
                    if ((2 * cw[i] as i32 - 1) as f64) * llr[i] < 0.0 {
                        nharderrors += 1;
                        dmin += llr[i].abs();
                    }
                }
                return BPResult {
                    decoded: Some(DecodeResult {
                        message91: bits91,
                        cw: cw.iter().map(|&b| b as u8).collect(),
                        nharderrors,
                        dmin,
                        ntype: 1,
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
                return BPResult { decoded: None, zsave };
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
                let mut tmn = 1.0;
                for k in 0..w {
                    if nm[k] != j {
                        tmn *= tanhtoc[k * m + ichk];
                    }
                }
                tov[i * n + j] = 2.0 * platanh(-tmn);
            }
        }
    }

    BPResult { decoded: None, zsave }
}

/// BP decoder for (174,91) LDPC code (backward-compatible).
pub fn bp_decode174_91(llr: &[f64], apmask: &[i8], max_iterations: usize) -> Option<DecodeResult> {
    bp_decode174_91_with_posteriors(llr, apmask, max_iterations, 0, 0, false).decoded
}

/// Hybrid BP + OSD decoder.
pub fn decode174_91(llr: &[f64], apmask: &[i8], maxosd: isize) -> Option<DecodeResult> {
    let max_iterations: usize = 30;
    let maxosd = maxosd.min(3);
    let (nosd, bp_save_limit, channel_llr_osd) = if maxosd < 0 {
        (0, 0, false)
    } else if maxosd == 0 {
        (1, 0, true)
    } else {
        (maxosd as usize, maxosd as usize, false)
    };

    let bp = bp_decode174_91_with_posteriors(
        llr,
        apmask,
        max_iterations,
        nosd,
        bp_save_limit,
        channel_llr_osd,
    );

    if let Some(result) = bp.decoded {
        return Some(result);
    }

    // Try OSD with accumulated BP posteriors (WSJT-X approach)
    if nosd >= 1 {
        for i in 0..nosd {
            if let Some(result) = osd_decode174_91(&bp.zsave[i], apmask, 2) {
                if result.nharderrors > 0 {
                    return Some(result);
                }
            }
        }
    }

    None
}

/// Simplified OSD decoder.
fn osd_decode174_91(llr: &[f64], apmask: &[i8], norder: usize) -> Option<DecodeResult> {
    let n = N_LDPC;
    let k = KK;

    let gen = get_generator();
    let absllr: Vec<f64> = llr.iter().map(|&x| x.abs()).collect();

    // Sort by reliability (descending)
    let mut indices: Vec<usize> = (0..n).collect();
    indices.sort_by(|&a, &b| absllr[b].partial_cmp(&absllr[a]).unwrap());

    // Reorder generator matrix columns
    let mut genmrb = vec![0u8; k * n];
    for row_idx in 0..k {
        let row = row_idx * n;
        for i in 0..n {
            genmrb[row + i] = gen[row + indices[i]];
        }
    }

    // Gaussian elimination
    let max_pivot_col = (k + 20).min(n);
    for id in 0..k {
        let mut found = false;
        let id_row = id * n;
        for icol in id..max_pivot_col {
            if genmrb[id_row + icol] == 1 {
                if icol != id {
                    for row_idx in 0..k {
                        let r = row_idx * n;
                        genmrb.swap(r + id, r + icol);
                    }
                    indices.swap(id, icol);
                }
                for ii in 0..k {
                    if ii == id {
                        continue;
                    }
                    let ii_row = ii * n;
                    if genmrb[ii_row + id] == 1 {
                        for c in 0..n {
                            genmrb[ii_row + c] ^= genmrb[id_row + c];
                        }
                    }
                }
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }

    // Hard decisions on reordered received word
    let mut hdec = vec![0i8; n];
    for i in 0..n {
        hdec[i] = if llr[indices[i]] >= 0.0 { 1 } else { 0 };
    }
    let absrx: Vec<f64> = (0..n).map(|i| absllr[indices[i]]).collect();
    let apmaskr: Vec<i8> = (0..n).map(|i| apmask[indices[i]]).collect();

    // Encode hard decision on MRB
    let m0: Vec<u8> = hdec[..k].iter().map(|&b| b as u8).collect();
    let c0 = mrb_encode(&m0, &genmrb, n);

    let mut dmin = 0.0;
    for i in 0..n {
        let x = (c0[i] ^ hdec[i] as u8) as f64;
        dmin += x * absrx[i];
    }

    let mut best_cw = c0.clone();

    if norder > 0 {
        let mut ndeep = norder.min(6);
        if ndeep == 0 {
            ndeep = 1;
        }
        let (nord, npre1, _npre2, nt, ntheta) = match ndeep {
            1 => (1usize, false, false, 40usize, 12usize),
            2 => (1usize, true, false, 40usize, 10usize),
            3 => (1usize, true, true, 40usize, 12usize),
            4 => (2usize, true, true, 40usize, 12usize),
            5 => (3usize, true, true, 40usize, 12usize),
            _ => (4usize, true, true, 95usize, 12usize),
        };

        for iorder in 1..=nord {
            let mut misub = vec![0u8; k];
            for slot in misub.iter_mut().take(k).skip(k - iorder) {
                *slot = 1;
            }
            let mut iflag = Some(k - iorder);
            while let Some(flag) = iflag {
                let iend = if iorder == nord && !npre1 { flag } else { 0 };
                let mut d1 = 0.0;
                let mut e2sub = vec![0u8; n - k];
                for n1 in (iend..=flag).rev() {
                    let mut mi = misub.clone();
                    mi[n1] = 1;
                    if mi
                        .iter()
                        .zip(apmaskr.iter())
                        .take(k)
                        .any(|(&m, &a)| m == 1 && a == 1)
                    {
                        continue;
                    }

                    let me: Vec<u8> = m0.iter().zip(mi.iter()).map(|(&a, &b)| a ^ b).collect();
                    let (e2, nd1kpt) = if n1 == flag {
                        let ce = mrb_encode(&me, &genmrb, n);
                        for j in k..n {
                            e2sub[j - k] = ce[j] ^ hdec[j] as u8;
                        }
                        d1 = me
                            .iter()
                            .zip(hdec.iter())
                            .zip(absrx.iter())
                            .take(k)
                            .map(|((&m, &h), &a)| (m ^ h as u8) as f64 * a)
                            .sum();
                        let nd = e2sub.iter().take(nt).filter(|&&b| b == 1).count() + 1;
                        (e2sub.clone(), nd)
                    } else {
                        let mut e2 = e2sub.clone();
                        for j in k..n {
                            e2[j - k] ^= genmrb[n1 * n + j];
                        }
                        let nd = e2.iter().take(nt).filter(|&&b| b == 1).count() + 2;
                        (e2, nd)
                    };

                    if nd1kpt <= ntheta {
                        let ce = mrb_encode(&me, &genmrb, n);
                        let dd = if n1 == flag {
                            d1 + e2sub
                                .iter()
                                .zip(absrx.iter().skip(k))
                                .map(|(&e, &a)| e as f64 * a)
                                .sum::<f64>()
                        } else {
                            d1 + (ce[n1] ^ hdec[n1] as u8) as f64 * absrx[n1]
                                + e2.iter()
                                    .zip(absrx.iter().skip(k))
                                    .map(|(&e, &a)| e as f64 * a)
                                    .sum::<f64>()
                        };
                        if dd < dmin {
                            dmin = dd;
                            best_cw = ce;
                        }
                    }
                }
                iflag = nextpat91(&mut misub, iorder);
            }
        }
    }

    // Reorder codeword back to original order
    let mut final_cw = vec![0u8; n];
    for i in 0..n {
        final_cw[indices[i]] = best_cw[i];
    }

    let bits91: Vec<u8> = final_cw[..KK].to_vec();
    if !check_crc14(&bits91) {
        return None;
    }

    let mut dmin_orig = 0.0;
    let mut nhe = 0;
    for i in 0..n {
        let hard = if llr[i] >= 0.0 { 1 } else { 0 };
        let x = (final_cw[i] as i8 ^ hard) as usize;
        nhe += x;
        dmin_orig += x as f64 * absllr[i];
    }

    Some(DecodeResult {
        message91: bits91,
        cw: final_cw,
        nharderrors: nhe,
        dmin: dmin_orig,
        ntype: 2,
    })
}

fn mrb_encode(message: &[u8], genmrb: &[u8], n: usize) -> Vec<u8> {
    let mut codeword = vec![0u8; n];
    for (i, &bit) in message.iter().enumerate() {
        if bit != 1 {
            continue;
        }
        let row = i * n;
        for j in 0..n {
            codeword[j] ^= genmrb[row + j];
        }
    }
    codeword
}

fn nextpat91(mi: &mut [u8], iorder: usize) -> Option<usize> {
    let k = mi.len();
    let mut ind = None;
    for i in 0..k.saturating_sub(1) {
        if mi[i] == 0 && mi[i + 1] == 1 {
            ind = Some(i);
        }
    }
    let ind = ind?;
    let mut ms = vec![0u8; k];
    ms[..ind].copy_from_slice(&mi[..ind]);
    ms[ind] = 1;
    ms[ind + 1] = 0;
    if ind + 1 < k {
        let ones = ms.iter().filter(|&&b| b == 1).count();
        let nz = iorder.saturating_sub(ones);
        for slot in ms.iter_mut().take(k).skip(k - nz) {
            *slot = 1;
        }
    }
    mi.copy_from_slice(&ms);
    mi.iter().position(|&b| b == 1)
}

fn get_generator() -> Vec<u8> {
    let k = KK;
    let n = N_LDPC;

    let mut gen = vec![0u8; k * n];
    for i in 0..k {
        gen[i * n + i] = 1;
    }

    use crate::util::constants::G_HEX;
    for m_idx in 0..83 {
        let hex_str = G_HEX[m_idx];
        for j in 0..23 {
            let byte = hex_str.as_bytes()[j];
            let val = u8::from_str_radix(&format!("{}", byte as char), 16).unwrap_or(0);
            let limit = if j == 22 { 3 } else { 4 };
            for jj in 1..=limit {
                let col = j * 4 + jj - 1;
                if col < k && (val & (1 << (4 - jj))) != 0 {
                    gen[col * n + k + m_idx] = 1;
                }
            }
        }
    }

    gen
}
