//! Mirrors JTDX `lib/ft8v2/osd174_91.f90`.

use super::bpdecode174_91::{BpDecodeResult, K, N};
use super::chkcrc14a::chkcrc14a;
use crate::decode::lib_jtdx::indexx::indexx_ascending;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) fn osd174_91(llr: &[f32; N], apmask: &[i8; N], ndeep: usize) -> Option<BpDecodeResult> {
    let n = N;
    let k = K;

    let gen = get_generator();
    let mut absllr = vec![0.0f32; n];
    for i in 0..n {
        absllr[i] = llr[i].abs();
    }

    let indx = indexx_ascending(&absllr);
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
    for id in 0..k {
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
                break;
            }
        }
    }

    // Hard decisions on reordered received word
    let mut hdec = vec![0i8; n];
    for i in 0..n {
        hdec[i] = if llr[indices[i]] >= 0.0 { 1 } else { 0 };
    }
    let mut absrx = vec![0.0f32; n];
    let mut apmaskr = vec![0i8; n];
    for i in 0..n {
        absrx[i] = absllr[indices[i]];
        apmaskr[i] = apmask[indices[i]];
    }

    // Encode hard decision on MRB
    let mut m0 = vec![0u8; k];
    for i in 0..k {
        m0[i] = hdec[i] as u8;
    }
    let c0 = mrbencode91(&m0, &genmrb, n);

    let mut dmin = 0.0f32;
    for i in 0..n {
        let x = (c0[i] ^ hdec[i] as u8) as f32;
        dmin += x * absrx[i];
    }

    let mut best_cw = c0.clone();

    if ndeep > 0 {
        let ndeep = ndeep.min(5);
        let (nord, npre1, npre2, nt, ntheta, ntau) = match ndeep {
            1 => (1usize, false, false, 40usize, 12usize, 0usize),
            2 => (1usize, true, false, 40usize, 12usize, 0usize),
            3 => (1usize, true, true, 40usize, 12usize, 14usize),
            4 => (2usize, true, false, 40usize, 12usize, 19usize),
            _ => (2usize, true, true, 40usize, 12usize, 19usize),
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
                        d1 = xor_weight_sum_message(&me, &hdec, &absrx, k);
                        e2.copy_from_slice(&e2sub);
                        e2sub.iter().take(nt).filter(|&&b| b == 1).count() + 1
                    } else {
                        e2.copy_from_slice(&e2sub);
                        for j in k..n {
                            e2[j - k] ^= genmrb[n1 * n + j];
                        }
                        e2.iter().take(nt).filter(|&&b| b == 1).count() + 2
                    };

                    if nd1kpt <= ntheta {
                        if n1 != flag {
                            mrbencode91_into(&me, &genmrb, n, &mut ce);
                        }
                        let dd = if n1 == flag {
                            d1 + error_weight_sum(&e2sub, &absrx, k)
                        } else {
                            d1 + (ce[n1] ^ hdec[n1] as u8) as f32 * absrx[n1]
                                + error_weight_sum(&e2, &absrx, k)
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
                        if sum_bits(&mi) < nord + npre1 as usize + npre2 as usize {
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
                        let dd = xor_weight_sum_codeword(&ce, &hdec, &absrx, n);
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

    let mut nharderror = 0isize;
    for i in 0..n {
        let hard = if llr[i] >= 0.0 { 1 } else { 0 };
        let x = final_cw[i] as i8 ^ hard;
        nharderror += x as isize;
    }

    let mut decoded91 = [0u8; K];
    decoded91.copy_from_slice(&final_cw[..K]);
    if !chkcrc14a(&decoded91) {
        nharderror = -nharderror;
    }

    let mut message77 = [0u8; 77];
    message77.copy_from_slice(&decoded91[..77]);
    let mut cw = [0u8; N];
    cw.copy_from_slice(&final_cw);

    Some(BpDecodeResult {
        message77,
        cw,
        nharderror,
        dmin,
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
        for j in 0..n {
            codeword[j] ^= genmrb[row + j];
        }
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

fn xor_weight_sum_message(me: &[u8], hdec: &[i8], absrx: &[f32], k: usize) -> f32 {
    let mut sum = 0.0f32;
    for i in 0..k {
        sum += (me[i] ^ hdec[i] as u8) as f32 * absrx[i];
    }
    sum
}

fn error_weight_sum(error: &[u8], absrx: &[f32], k: usize) -> f32 {
    let mut sum = 0.0f32;
    for i in 0..error.len() {
        sum += error[i] as f32 * absrx[k + i];
    }
    sum
}

fn xor_weight_sum_codeword(cw: &[u8], hdec: &[i8], absrx: &[f32], n: usize) -> f32 {
    let mut sum = 0.0f32;
    for i in 0..n {
        sum += (cw[i] ^ hdec[i] as u8) as f32 * absrx[i];
    }
    sum
}

fn sum_bits(bits: &[u8]) -> usize {
    let mut sum = 0usize;
    for &bit in bits {
        sum += bit as usize;
    }
    sum
}

fn get_generator() -> &'static [u8] {
    static GENERATOR: OnceLock<Vec<u8>> = OnceLock::new();
    GENERATOR
        .get_or_init(|| {
            let k = K;
            let n = N;

            let mut gen = vec![0u8; k * n];
            for i in 0..k {
                gen[i * n + i] = 1;
            }

            use super::ldpc_174_91_c_generator::G_HEX;
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
