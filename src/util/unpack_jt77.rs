use crate::util::constants::*;
use crate::util::hashcall::HashCallBook;
/// FT8 message unpacking – Rust port of unpack77
use num_bigint::BigInt;

fn bits_to_uint(bits: &[u8], start: usize, len: usize) -> usize {
    let mut val: usize = 0;
    for i in 0..len {
        val = val * 2 + bits[start + i] as usize;
    }
    val
}

fn unpack28(n28: usize, book: Option<&HashCallBook>) -> Option<String> {
    if n28 >= MAX28 {
        return None;
    }

    if n28 == 0 {
        return Some("DE".into());
    }
    if n28 == 1 {
        return Some("QRZ".into());
    }
    if n28 == 2 {
        return Some("CQ".into());
    }

    if (3..3 + 1000).contains(&n28) {
        let nqsy = n28 - 3;
        return Some(format!("CQ {:03}", nqsy));
    }

    if (1003..N_TOKENS).contains(&n28) {
        let mut m = n28 - 1003;
        let mut chars = String::new();
        for _ in 0..4 {
            let j = m % 27;
            m /= 27;
            if j == 0 {
                chars.insert(0, ' ');
            } else {
                chars.insert(0, (64 + j as u8) as char);
            }
        }
        let directed = chars.trim().to_string();
        if directed.is_empty() {
            return Some("CQ".into());
        }
        return Some(format!("CQ {}", directed));
    }

    if (N_TOKENS..N_TOKENS + MAX22).contains(&n28) {
        let n22 = n28 - N_TOKENS;
        if let Some(book) = book {
            if let Some(resolved) = book.lookup22(n22) {
                return Some(format!("<{}>", resolved));
            }
        }
        return Some("<...>".into());
    }

    // Standard callsign
    let mut n = n28 - N_TOKENS - MAX22;
    let i6 = n % 27;
    n /= 27;
    let i5 = n % 27;
    n /= 27;
    let i4 = n % 27;
    n /= 27;
    let i3 = n % 10;
    n /= 10;
    let i2 = n % 36;
    n /= 36;
    let i1 = n;

    if i1 >= A1.len()
        || i2 >= A2.len()
        || i3 >= A3.len()
        || i4 >= A4.len()
        || i5 >= A4.len()
        || i6 >= A4.len()
    {
        return None;
    }

    let call = format!(
        "{}{}{}{}{}{}",
        A1[i1] as char,
        A2[i2] as char,
        A3[i3] as char,
        A4[i4] as char,
        A4[i5] as char,
        A4[i6] as char
    );
    let call = call.trim().to_string();

    if call.is_empty() {
        None
    } else {
        Some(call)
    }
}

fn to_grid4(igrid4: usize) -> Option<String> {
    if igrid4 > MAXGRID4 {
        return None;
    }
    let mut n = igrid4;
    let j4 = n % 10;
    n /= 10;
    let j3 = n % 10;
    n /= 10;
    let j2 = n % 18;
    n /= 18;
    let j1 = n;
    if j1 > 17 || j2 > 17 {
        return None;
    }
    Some(format!(
        "{}{}{}{}",
        (65 + j1) as u8 as char,
        (65 + j2) as u8 as char,
        j3,
        j4
    ))
}

fn unpack_text77(bits71: &[u8]) -> String {
    // Reconstruct 9 bytes from 71 bits
    // First 7 bits go into byte 0 (bits 0-6, i.e., low 7 bits)
    // Remaining 64 bits go into bytes 1-8
    let mut qa_bytes = [0u8; 9];

    // First 7 bits → byte 0 (MSB first, so bits 0-6 become the byte value)
    let mut val: u8 = 0;
    for b in 0..7 {
        val = (val << 1) | bits71[b];
    }
    qa_bytes[0] = val;

    // Next 64 bits → bytes 1-8
    for li in 1..=8 {
        let mut val: u8 = 0;
        for b in 0..8 {
            val = (val << 1) | bits71[7 + (li - 1) * 8 + b];
        }
        qa_bytes[li] = val;
    }

    // Convert bytes to BigInt (big-endian)
    let qa = BigInt::from_bytes_be(num_bigint::Sign::Plus, &qa_bytes);

    // Decode from base-42
    let mut chars = Vec::with_capacity(13);
    let mut qa_copy = qa.clone();
    for _ in 0..13 {
        let (_, digits) = (&qa_copy % 42u64).to_u64_digits();
        let j = digits.first().copied().unwrap_or(0) as usize;
        qa_copy /= 42u64;
        chars.push(FT_ALPH[j] as char);
    }
    chars.reverse();

    let s: String = chars.into_iter().collect();
    s.trim_start().to_string()
}

/// Unpack a 77-bit FT8 message.
pub fn unpack77(bits77: &[u8], book: Option<&HashCallBook>) -> Option<String> {
    let n3 = bits_to_uint(bits77, 71, 3);
    let i3 = bits_to_uint(bits77, 74, 3);

    if i3 == 0 && n3 == 0 {
        let msg = unpack_text77(&bits77[..71]);
        if msg.trim().is_empty() {
            return None;
        }
        return Some(msg.trim().into());
    }

    if i3 == 1 || i3 == 2 {
        let n28a = bits_to_uint(bits77, 0, 28);
        let ipa = bits77[28] as usize;
        let n28b = bits_to_uint(bits77, 29, 28);
        let ipb = bits77[57] as usize;
        let ir = bits77[58] as usize;
        let igrid4 = bits_to_uint(bits77, 59, 15);

        let call1 = unpack28(n28a, book)?;
        let call2_raw = unpack28(n28b, book)?;

        let mut c1 = call1;
        let mut c2 = call2_raw;

        if c1.starts_with("CQ_") {
            c1 = c1.replacen('_', " ", 1);
        }

        if !c1.contains('<') {
            if ipa == 1 && i3 == 1 && c1.len() >= 3 {
                c1.push_str("/R");
            }
            if ipa == 1 && i3 == 2 && c1.len() >= 3 {
                c1.push_str("/P");
            }
        }
        if !c2.contains('<') {
            if ipb == 1 && i3 == 1 && c2.len() >= 3 {
                c2.push_str("/R");
            }
            if ipb == 1 && i3 == 2 && c2.len() >= 3 {
                c2.push_str("/P");
            }
            if let Some(book) = book {
                if c2.len() >= 3 {
                    book.save(&c2);
                }
            }
        }

        if igrid4 <= MAXGRID4 {
            let grid = to_grid4(igrid4)?;
            if ir == 0 {
                Some(format!("{} {} {}", c1, c2, grid))
            } else {
                if is_cq_head(&c1) {
                    None
                } else {
                    Some(format!("{} {} R {}", c1, c2, grid))
                }
            }
        } else {
            let irpt = igrid4 - MAXGRID4;
            if is_cq_head(&c1) && irpt >= 2 {
                return None;
            }
            match irpt {
                1 => Some(format!("{} {}", c1, c2)),
                2 => Some(format!("{} {} RRR", c1, c2)),
                3 => Some(format!("{} {} RR73", c1, c2)),
                4 => Some(format!("{} {} 73", c1, c2)),
                _ if irpt >= 5 => {
                    let mut isnr = irpt as i32 - 35;
                    if isnr > 50 {
                        isnr -= 101;
                    }
                    let sign = if isnr >= 0 { '+' } else { '-' };
                    let abs_str = format!("{:02}", isnr.abs());
                    if ir == 0 {
                        Some(format!("{} {} {}{}", c1, c2, sign, abs_str))
                    } else {
                        Some(format!("{} {} R{}{}", c1, c2, sign, abs_str))
                    }
                }
                _ => None,
            }
        }
    } else if i3 == 4 {
        let n12 = bits_to_uint(bits77, 0, 12);
        let mut n58: u64 = 0;
        for i in 0..58 {
            n58 = n58 * 2 + bits77[12 + i] as u64;
        }
        let iflip = bits77[70] as usize;
        let nrpt = bits_to_uint(bits77, 71, 2);
        let icq = bits77[73] as usize;

        let mut c11_chars = Vec::with_capacity(11);
        let mut remain = n58;
        for _ in 0..11 {
            let j = (remain % 38) as usize;
            remain /= 38;
            c11_chars.push(C38[j] as char);
        }
        c11_chars.reverse();
        let c11: String = c11_chars.into_iter().collect::<String>().trim().to_string();

        let call3 = if let Some(book) = book {
            if let Some(resolved) = book.lookup12(n12) {
                format!("<{}>", resolved)
            } else {
                "<...>".into()
            }
        } else {
            "<...>".into()
        };

        let (call1, call2) = if iflip == 0 {
            if let Some(book) = book {
                book.save(&c11);
            }
            (call3, c11)
        } else {
            (c11, call3)
        };

        let msg = if icq == 1 {
            format!("CQ {}", call2)
        } else {
            match nrpt {
                0 => format!("{} {}", call1, call2),
                1 => format!("{} {} RRR", call1, call2),
                2 => format!("{} {} RR73", call1, call2),
                _ => format!("{} {} 73", call1, call2),
            }
        };
        Some(msg)
    } else {
        None
    }
}

fn is_cq_head(call: &str) -> bool {
    let c = call.trim_end();
    c == "CQ" || c.starts_with("CQ ")
}

#[cfg(test)]
mod tests {
    use super::unpack77;
    use crate::util::pack_jt77::pack77;

    #[test]
    fn cq_r_grid_is_rejected_like_wsjtx() {
        let bits = pack77("CQ K1ABC R FN42");
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn cq_ack_report_is_rejected_like_wsjtx() {
        let bits = pack77("CQ K1ABC RRR");
        assert!(unpack77(&bits, None).is_none());
    }
}
