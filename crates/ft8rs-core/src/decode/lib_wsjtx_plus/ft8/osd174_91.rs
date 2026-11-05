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
    let _span = super::metrics::Span::new(super::metrics::Kind::Osd);
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

    // Preserve pivot search/column-swap order, but eliminate packed integer
    // rows instead of touching 174 individual bytes for every row XOR.
    let mut packed = pack_rows(&genmrb);
    if !eliminate(&mut packed, &mut indices) {
        return None;
    }
    for (row, words) in genmrb.as_chunks_mut::<N_LDPC>().0.iter_mut().zip(&packed) {
        for (bit, value) in row.iter_mut().enumerate() {
            *value = ((words[bit / 64] >> (bit % 64)) & 1) as u8;
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
    let c0 = mrbencode91(&m0, &packed, n);

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
                let mut e2sub_low = 0u64;
                let protected_sub = misub.iter().zip(&apmaskr).any(|(&m, &a)| m == 1 && a == 1);
                e2sub.fill(0);
                for n1 in (iend..=flag).rev() {
                    if protected_sub || apmaskr[n1] == 1 {
                        continue;
                    }

                    let nd1kpt = if n1 == flag {
                        for j in 0..k {
                            me[j] = m0[j] ^ misub[j];
                        }
                        me[n1] = m0[n1] ^ 1;
                        mrbencode91_into(&me, &packed, n, &mut ce);
                        for j in k..n {
                            e2sub[j - k] = ce[j] ^ hdec[j] as u8;
                        }
                        e2sub_low = e2sub
                            .iter()
                            .take(nt.min(64))
                            .enumerate()
                            .fold(0u64, |bits, (i, &value)| bits | (u64::from(value) << i));
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
                    } else if nt <= 64 {
                        // GF(2) parity weight is exactly a population count.
                        // Keep byte construction for accepted candidates only.
                        let row = packed[n1];
                        let parity_low = (row[1] >> (k % 64)) | (row[2] << (64 - k % 64));
                        let mask = u64::MAX >> (64 - nt);
                        ((e2sub_low ^ parity_low) & mask).count_ones() as usize + 2
                    } else {
                        e2.copy_from_slice(&e2sub);
                        gf2_row_xor(&mut e2, &genmrb[n1 * n + k..n1 * n + n]);
                        e2.iter().take(nt).map(|&b| b as usize).sum::<usize>() + 2
                    };

                    if nd1kpt <= ntheta {
                        if n1 != flag {
                            for j in 0..k {
                                me[j] = m0[j] ^ misub[j];
                            }
                            me[n1] = m0[n1] ^ 1;
                            if nt <= 64 {
                                e2.copy_from_slice(&e2sub);
                                gf2_row_xor(&mut e2, &genmrb[n1 * n + k..n1 * n + n]);
                            }
                            mrbencode91_into(&me, &packed, n, &mut ce);
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
                mrbencode91_into(&me, &packed, n, &mut ce);
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
                        mrbencode91_into(&me, &packed, n, &mut ce);
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

// Three machine words retain exactly the same GF(2) rows. XOR association
// cannot change integer results; weighted floating-point scores are untouched.
fn pack_rows(matrix: &[u8]) -> Vec<[u64; 3]> {
    matrix
        .as_chunks::<N_LDPC>()
        .0
        .iter()
        .map(|row| {
            let mut words = [0u64; 3];
            for (bit, &value) in row.iter().enumerate() {
                words[bit / 64] |= u64::from(value) << (bit % 64);
            }
            words
        })
        .collect()
}

fn eliminate(rows: &mut [[u64; 3]], indices: &mut [usize]) -> bool {
    let max_pivot_col = (KK + 20).min(N_LDPC);
    for id in 0..KK {
        let Some(column) =
            (id..max_pivot_col).find(|&col| (rows[id][col / 64] >> (col % 64)) & 1 == 1)
        else {
            return false;
        };
        if column != id {
            for row in rows.iter_mut() {
                let flip = ((row[id / 64] >> (id % 64)) ^ (row[column / 64] >> (column % 64))) & 1;
                row[id / 64] ^= flip << (id % 64);
                row[column / 64] ^= flip << (column % 64);
            }
            indices.swap(id, column);
        }
        let pivot = rows[id];
        for (i, row) in rows.iter_mut().enumerate() {
            if i != id && (row[id / 64] >> (id % 64)) & 1 == 1 {
                for (word, value) in row.iter_mut().zip(pivot) {
                    *word ^= value;
                }
            }
        }
    }
    true
}

fn mrbencode91(message: &[u8], packed: &[[u64; 3]], n: usize) -> Vec<u8> {
    let mut codeword = vec![0u8; n];
    mrbencode91_into(message, packed, n, &mut codeword);
    codeword
}

fn mrbencode91_into(message: &[u8], packed: &[[u64; 3]], n: usize, codeword: &mut [u8]) {
    debug_assert_eq!(codeword.len(), n);
    let mut words = [0u64; 3];
    for (&bit, row) in message.iter().zip(packed) {
        if bit == 1 {
            for (word, &value) in words.iter_mut().zip(row) {
                *word ^= value;
            }
        }
    }
    for (bit, value) in codeword.iter_mut().enumerate() {
        *value = ((words[bit / 64] >> (bit % 64)) & 1) as u8;
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

#[test]
fn packed_reencoding_matches_byte_rows() {
    let mut seed = 0x12345678abcdefu64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed & 1) as u8
    };
    for _ in 0..48 {
        let matrix: Vec<_> = (0..KK * N_LDPC).map(|_| random()).collect();
        let packed = pack_rows(&matrix);
        for _ in 0..12 {
            let message: Vec<_> = (0..KK).map(|_| random()).collect();
            let mut expected = vec![0u8; N_LDPC];
            for (i, &bit) in message.iter().enumerate() {
                if bit == 1 {
                    for j in 0..N_LDPC {
                        expected[j] ^= matrix[i * N_LDPC + j];
                    }
                }
            }
            assert_eq!(mrbencode91(&message, &packed, N_LDPC), expected);
        }
    }
}

#[test]
fn packed_osd_preserves_full_results() {
    let mut seed = 0x12345678abcdefu64;
    for case in 0..64 {
        let llr: Vec<_> = (0..N_LDPC)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                if case == 0 {
                    0.0
                } else {
                    ((seed >> 32) as i32 as f32 / i32::MAX as f32) * ((case % 12 + 1) as f32)
                }
            })
            .collect();
        let mask: Vec<_> = (0..N_LDPC)
            .map(|i| i8::from(case % 3 == 0 && i % 7 == 0))
            .collect();
        for order in [0, 1, 2, 3] {
            let old = crate::decode::osd174_91::osd_decode174_91(&llr, &mask, order);
            let new = osd_decode174_91(&llr, &mask, order);
            assert_eq!(
                old.as_ref().map(|r| (&r.message91, &r.cw, r.nharderrors)),
                new.as_ref().map(|r| (&r.message91, &r.cw, r.nharderrors)),
                "case={case} order={order}"
            );
        }
    }
}

#[test]
fn packed_elimination_preserves_matrix_and_pivot_order() {
    let mut seed = 0x12345678abcdefu64;
    for case in 0..40 {
        let mut bytes: Vec<u8> = (0..KK * N_LDPC)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed & 1) as u8
            })
            .collect();
        if case == 0 {
            bytes.fill(0);
        }
        let mut packed = pack_rows(&bytes);
        let mut old_indices: Vec<_> = (0..N_LDPC).collect();
        let mut new_indices = old_indices.clone();
        let mut old_success = true;
        for id in 0..KK {
            let Some(col) = (id..(KK + 20).min(N_LDPC)).find(|&c| bytes[id * N_LDPC + c] == 1)
            else {
                old_success = false;
                break;
            };
            if col != id {
                for row in 0..KK {
                    bytes.swap(row * N_LDPC + id, row * N_LDPC + col);
                }
                old_indices.swap(id, col);
            }
            let pivot = bytes[id * N_LDPC..(id + 1) * N_LDPC].to_vec();
            for row in 0..KK {
                if row != id && bytes[row * N_LDPC + id] == 1 {
                    for col in 0..N_LDPC {
                        bytes[row * N_LDPC + col] ^= pivot[col];
                    }
                }
            }
        }
        assert_eq!(old_success, eliminate(&mut packed, &mut new_indices));
        assert_eq!(old_indices, new_indices, "pivot case={case}");
        assert_eq!(pack_rows(&bytes), packed, "matrix case={case}");
    }
}
