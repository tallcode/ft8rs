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

/// BP decoder for (174,91) LDPC code.
pub fn bp_decode174_91(llr: &[f64], apmask: &[i8], max_iterations: usize) -> Option<DecodeResult> {
    let n = N_LDPC;
    let m = M_LDPC;

    let mut tov = vec![0.0; NCW * n];
    let mut toc = vec![0.0; 7 * m];
    let mut tanhtoc = vec![0.0; 7 * m];
    let mut zn = vec![0.0; n];
    let mut cw = vec![0i8; n];

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
                    if ((2 * cw[i] as i32 - 1) as f64) * llr[i] < 0.0 {
                        nharderrors += 1;
                    }
                }
                return Some(DecodeResult {
                    message91: bits91,
                    cw: cw.iter().map(|&b| b as u8).collect(),
                    nharderrors,
                    dmin: 0.0,
                    ntype: 1,
                });
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
                return None;
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

    None
}

/// Hybrid BP + OSD decoder.
pub fn decode174_91(llr: &[f64], apmask: &[i8], maxosd: isize) -> Option<DecodeResult> {
    let max_iterations = 30;
    if let Some(result) = bp_decode174_91(llr, apmask, max_iterations) {
        return Some(result);
    }

    if maxosd >= 0 {
        return osd_decode174_91(llr, apmask, if maxosd >= 2 { 2 } else if maxosd >= 1 { 2 } else { 1 });
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

    // Encode hard decision on MRB
    let mut c0 = vec![0u8; n];
    for i in 0..k {
        if hdec[i] != 1 {
            continue;
        }
        let row = i * n;
        for j in 0..n {
            c0[j] ^= genmrb[row + j];
        }
    }

    let mut dmin = 0.0;
    for i in 0..n {
        let x = (c0[i] ^ hdec[i] as u8) as f64;
        dmin += x * absrx[i];
    }
    let mut best_flip1: isize = -1;
    let mut best_flip2: isize = -1;

    // Order-1
    for i1 in (0..k).rev() {
        if apmask[indices[i1]] == 1 {
            continue;
        }
        let row1 = i1 * n;
        let mut dd = 0.0;
        for j in 0..n {
            let x = (c0[j] ^ genmrb[row1 + j] ^ hdec[j] as u8) as f64;
            dd += x * absrx[j];
        }
        if dd < dmin {
            dmin = dd;
            best_flip1 = i1 as isize;
            best_flip2 = -1;
        }
    }

    // Order-2
    if norder >= 2 {
        let ntry = 64.min(k);
        let i_min = k.saturating_sub(ntry);
        for i1 in (i_min..k).rev() {
            if apmask[indices[i1]] == 1 {
                continue;
            }
            let row1 = i1 * n;
            for i2 in i_min..i1 {
                if apmask[indices[i2]] == 1 {
                    continue;
                }
                let row2 = i2 * n;
                let mut dd = 0.0;
                for j in 0..n {
                    let x = (c0[j] ^ genmrb[row1 + j] ^ genmrb[row2 + j] ^ hdec[j] as u8) as f64;
                    dd += x * absrx[j];
                }
                if dd < dmin {
                    dmin = dd;
                    best_flip1 = i1 as isize;
                    best_flip2 = i2 as isize;
                }
            }
        }
    }

    let mut best_cw = c0.clone();
    if best_flip1 >= 0 {
        let row1 = best_flip1 as usize * n;
        for j in 0..n {
            best_cw[j] ^= genmrb[row1 + j];
        }
        if best_flip2 >= 0 {
            let row2 = best_flip2 as usize * n;
            for j in 0..n {
                best_cw[j] ^= genmrb[row2 + j];
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
