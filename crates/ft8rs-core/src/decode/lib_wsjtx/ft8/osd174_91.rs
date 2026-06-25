//! Ordered-statistics decoder for FT8 LDPC(174,91).
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/osd174_91.f90`

use crate::decode::chkcrc14a::check_crc14;
use crate::decode::decode174_91::{DecodeResult, KK, N_LDPC};
use crate::decode::gf2::gf2_row_xor;
use crate::decode::indexx::indexx_ascending;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) fn osd_decode174_91(llr: &[f32], apmask: &[i8], norder: usize) -> Option<DecodeResult> {
    let n = N_LDPC;
    let k = KK;

    let gen = get_generator();
    let absllr: Vec<f32> = llr.iter().map(|&x| x.abs()).collect();

    // WSJT-X osd174_91.f90 uses indexx(absrx,N,indx), then consumes the
    // ascending index vector in reverse to get decreasing reliability.
    let absllr_for_index: Vec<f64> = absllr.iter().map(|&x| x as f64).collect();
    let indx = indexx_ascending(&absllr_for_index);
    let mut indices: Vec<usize> = indx.into_iter().rev().collect();

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
    // Non-aliasing copy of the pivot row (unchanged during the sweep since ii == id
    // is skipped), so gf2_row_xor sees two distinct slices it can vectorize —
    // bit-identical to the original in-place index XOR.
    let mut pivot = vec![0u8; n];
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
                pivot.copy_from_slice(&genmrb[id_row..id_row + n]);
                for ii in 0..k {
                    if ii == id {
                        continue;
                    }
                    let ii_row = ii * n;
                    if genmrb[ii_row + id] == 1 {
                        gf2_row_xor(&mut genmrb[ii_row..ii_row + n], &pivot);
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
    let absrx: Vec<f32> = (0..n).map(|i| absllr[indices[i]]).collect();
    let apmaskr: Vec<i8> = (0..n).map(|i| apmask[indices[i]]).collect();

    // Encode hard decision on MRB
    let m0: Vec<u8> = hdec[..k].iter().map(|&b| b as u8).collect();
    let c0 = mrbencode91(&m0, &genmrb, n);

    let mut dmin = 0.0f32;
    for i in 0..n {
        let x = (c0[i] ^ hdec[i] as u8) as f32;
        dmin += x * absrx[i];
    }

    let mut best_cw = c0.clone();

    if norder > 0 {
        let mut ndeep = norder.min(6);
        if ndeep == 0 {
            ndeep = 1;
        }
        let (nord, npre1, npre2, nt, ntheta, ntau) = match ndeep {
            1 => (1usize, false, false, 40usize, 12usize, 0usize),
            2 => (1usize, true, false, 40usize, 10usize, 0usize),
            3 => (1usize, true, true, 40usize, 12usize, 14usize),
            4 => (2usize, true, true, 40usize, 12usize, 17usize),
            5 => (3usize, true, true, 40usize, 12usize, 15usize),
            _ => (4usize, true, true, 95usize, 12usize, 15usize),
        };

        let mut misub = vec![0u8; k];
        let mut mi = vec![0u8; k];
        let mut me = vec![0u8; k];
        let mut ce = vec![0u8; n];
        let mut e2 = vec![0u8; n - k];
        let mut e2sub = vec![0u8; n - k];

        for iorder in 1..=nord {
            misub.fill(0);
            for slot in misub.iter_mut().take(k).skip(k - iorder) {
                *slot = 1;
            }
            let mut iflag = Some(k - iorder);
            while let Some(flag) = iflag {
                let iend = if iorder == nord && !npre1 { flag } else { 0 };
                let mut d1 = 0.0f32;
                e2sub.fill(0);
                for n1 in (iend..=flag).rev() {
                    mi.copy_from_slice(&misub);
                    mi[n1] = 1;
                    if mi
                        .iter()
                        .zip(apmaskr.iter())
                        .take(k)
                        .any(|(&m, &a)| m == 1 && a == 1)
                    {
                        continue;
                    }

                    for j in 0..k {
                        me[j] = m0[j] ^ mi[j];
                    }
                    let nd1kpt = if n1 == flag {
                        mrbencode91_into(&me, &genmrb, n, &mut ce);
                        for j in k..n {
                            e2sub[j - k] = ce[j] ^ hdec[j] as u8;
                        }
                        d1 = me
                            .iter()
                            .zip(hdec.iter())
                            .zip(absrx.iter())
                            .take(k)
                            .map(|((&m, &h), &a)| (m ^ h as u8) as f32 * a)
                            .sum();
                        e2.copy_from_slice(&e2sub);
                        // e2sub holds 0/1, so the set-bit count == the byte sum.
                        // Branchless sum auto-vectorizes; bit-identical to filter().count().
                        e2sub.iter().take(nt).map(|&b| b as usize).sum::<usize>() + 1
                    } else {
                        e2.copy_from_slice(&e2sub);
                        // e2[..] ^= row n1's parity columns. Clean non-aliasing slices
                        // let gf2_row_xor auto-vectorize (P2.0 pattern); bit-identical.
                        gf2_row_xor(&mut e2, &genmrb[n1 * n + k..n1 * n + n]);
                        e2.iter().take(nt).map(|&b| b as usize).sum::<usize>() + 2
                    };

                    if nd1kpt <= ntheta {
                        if n1 != flag {
                            mrbencode91_into(&me, &genmrb, n, &mut ce);
                        }
                        let dd = if n1 == flag {
                            d1 + e2sub
                                .iter()
                                .zip(absrx.iter().skip(k))
                                .map(|(&e, &a)| e as f32 * a)
                                .sum::<f32>()
                        } else {
                            d1 + (ce[n1] ^ hdec[n1] as u8) as f32 * absrx[n1]
                                + e2.iter()
                                    .zip(absrx.iter().skip(k))
                                    .map(|(&e, &a)| e as f32 * a)
                                    .sum::<f32>()
                        };
                        if dd < dmin {
                            dmin = dd;
                            best_cw.copy_from_slice(&ce);
                        }
                    }
                }
                iflag = nextpat91(&mut misub, iorder);
            }
        }

        if npre2 {
            let mut boxes: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
            for i1 in (0..k).rev() {
                for i2 in (0..i1).rev() {
                    let ipat = boxit91_pattern(&genmrb, n, k, ntau, i1, i2);
                    boxes.entry(ipat).or_default().push((i1, i2));
                }
            }

            misub.fill(0);
            for slot in misub.iter_mut().take(k).skip(k - nord) {
                *slot = 1;
            }
            let mut iflag = Some(k - nord);
            while iflag.is_some() {
                for j in 0..k {
                    me[j] = m0[j] ^ misub[j];
                }
                mrbencode91_into(&me, &genmrb, n, &mut ce);
                for j in k..n {
                    e2sub[j - k] = ce[j] ^ hdec[j] as u8;
                }

                for i2 in 0..=ntau {
                    let mut ipat = fetchit91_pattern(&e2sub[..ntau]);
                    if i2 > 0 {
                        let bit = 1usize << (ntau - i2);
                        ipat ^= bit;
                    }
                    let Some(pairs) = boxes.get(&ipat) else {
                        continue;
                    };
                    for &(in1, in2) in pairs {
                        mi.copy_from_slice(&misub);
                        mi[in1] = 1;
                        mi[in2] = 1;
                        if mi.iter().map(|&bit| bit as usize).sum::<usize>()
                            < nord + npre1 as usize + npre2 as usize
                        {
                            continue;
                        }
                        if mi
                            .iter()
                            .zip(apmaskr.iter())
                            .take(k)
                            .any(|(&m, &a)| m == 1 && a == 1)
                        {
                            continue;
                        }

                        for j in 0..k {
                            me[j] = m0[j] ^ mi[j];
                        }
                        mrbencode91_into(&me, &genmrb, n, &mut ce);
                        let dd: f32 = ce
                            .iter()
                            .zip(hdec.iter())
                            .zip(absrx.iter())
                            .map(|((&c, &h), &a)| (c ^ h as u8) as f32 * a)
                            .sum();
                        if dd < dmin {
                            dmin = dd;
                            best_cw.copy_from_slice(&ce);
                        }
                    }
                }
                iflag = nextpat91(&mut misub, nord);
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

    let mut nhe = 0;
    for i in 0..n {
        let hard = if llr[i] >= 0.0 { 1 } else { 0 };
        let x = (final_cw[i] as i8 ^ hard) as usize;
        nhe += x;
    }

    Some(DecodeResult {
        message91: bits91,
        cw: final_cw,
        nharderrors: nhe,
    })
}

fn mrbencode91(message: &[u8], genmrb: &[u8], n: usize) -> Vec<u8> {
    let mut codeword = vec![0u8; n];
    mrbencode91_into(message, genmrb, n, &mut codeword);
    codeword
}

fn mrbencode91_into(message: &[u8], genmrb: &[u8], n: usize, codeword: &mut [u8]) {
    debug_assert_eq!(codeword.len(), n);
    codeword.fill(0);
    for (i, &bit) in message.iter().enumerate() {
        if bit != 1 {
            continue;
        }
        let row = i * n;
        gf2_row_xor(codeword, &genmrb[row..row + n]);
    }
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

    mi[ind] = 1;
    mi[ind + 1..].fill(0);
    if ind + 1 < k {
        let ones = mi[..=ind].iter().filter(|&&b| b == 1).count();
        let nz = iorder.saturating_sub(ones);
        for slot in mi.iter_mut().take(k).skip(k - nz) {
            *slot = 1;
        }
    }
    mi.iter().position(|&b| b == 1)
}

fn boxit91_pattern(genmrb: &[u8], n: usize, k: usize, ntau: usize, i1: usize, i2: usize) -> usize {
    let mut ipat = 0usize;
    for j in 0..ntau {
        let bit = genmrb[i1 * n + k + j] ^ genmrb[i2 * n + k + j];
        if bit == 1 {
            ipat += 1usize << (ntau - 1 - j);
        }
    }
    ipat
}

fn fetchit91_pattern(bits: &[u8]) -> usize {
    let ntau = bits.len();
    let mut ipat = 0usize;
    for (i, &bit) in bits.iter().enumerate() {
        if bit == 1 {
            ipat += 1usize << (ntau - 1 - i);
        }
    }
    ipat
}

fn get_generator() -> &'static [u8] {
    static GENERATOR: OnceLock<Vec<u8>> = OnceLock::new();
    GENERATOR
        .get_or_init(|| {
            let k = KK;
            let n = N_LDPC;

            let mut gen = vec![0u8; k * n];
            for i in 0..k {
                gen[i * n + i] = 1;
            }

            use crate::decode::ldpc_174_91_c_generator::G_HEX;
            for (m_idx, hex_str) in G_HEX.iter().enumerate().take(83) {
                for j in 0..23 {
                    let byte = hex_str.as_bytes()[j];
                    let val = match byte {
                        b'0'..=b'9' => byte - b'0',
                        b'a'..=b'f' => byte - b'a' + 10,
                        b'A'..=b'F' => byte - b'A' + 10,
                        _ => 0,
                    };
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
        })
        .as_slice()
}

#[cfg(test)]
mod tests {
    use super::{fetchit91_pattern, osd_decode174_91};
    use crate::decode::decode174_91::N_LDPC;

    #[test]
    fn fetchit91_pattern_matches_wsjtx_left_shift_order() {
        assert_eq!(fetchit91_pattern(&[1, 0, 1, 1]), 0b1011);
        assert_eq!(fetchit91_pattern(&[0, 0, 0, 1]), 0b0001);
    }

    #[test]
    fn osd_ndeep3_path_accepts_valid_zero_codeword() {
        let mut llr = vec![-5.0f32; N_LDPC];
        let apmask = vec![0i8; N_LDPC];
        let decoded = osd_decode174_91(&llr, &apmask, 3).expect("valid all-zero codeword");
        assert!(decoded.message91.iter().all(|&bit| bit == 0));
        assert!(decoded.cw.iter().all(|&bit| bit == 0));
        assert_eq!(decoded.nharderrors, 0);

        // Keep a second call to guard the saved generator and npre2 boxes path
        // against accidental dependence on one-shot initialization.
        llr[120] = -4.0;
        let decoded = osd_decode174_91(&llr, &apmask, 3).expect("repeat valid all-zero codeword");
        assert!(decoded.cw.iter().all(|&bit| bit == 0));
    }
}
