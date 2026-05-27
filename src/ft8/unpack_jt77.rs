use crate::ft8::hashcall::HashCallBook;
use crate::ft8::protocol::*;
/// FT8 message unpacking – Rust port of unpack77
use num_bigint::BigInt;

const CSEC: [&str; 86] = [
    "AB", "AK", "AL", "AR", "AZ", "BC", "CO", "CT", "DE", "EB", "EMA", "ENY", "EPA", "EWA", "GA",
    "GH", "IA", "ID", "IL", "IN", "KS", "KY", "LA", "LAX", "NS", "MB", "MDC", "ME", "MI", "MN",
    "MO", "MS", "MT", "NC", "ND", "NE", "NFL", "NH", "NL", "NLI", "NM", "NNJ", "NNY", "TER", "NTX",
    "NV", "OH", "OK", "ONE", "ONN", "ONS", "OR", "ORG", "PAC", "PR", "QC", "RI", "SB", "SC", "SCV",
    "SD", "SDG", "SF", "SFL", "SJV", "SK", "SNJ", "STX", "SV", "TN", "UT", "VA", "VI", "VT", "WCF",
    "WI", "WMA", "WNY", "WPA", "WTX", "WV", "WWA", "WY", "DX", "PE", "NB",
];

const CMULT: [&str; 171] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "FL", "GA", "HI", "ID", "IL", "IN", "IA", "KS",
    "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM", "NY",
    "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA", "WV",
    "WI", "WY", "NB", "NS", "QC", "ON", "MB", "SK", "AB", "BC", "NWT", "NF", "LB", "NU", "YT",
    "PEI", "DC", "DR", "FR", "GD", "GR", "OV", "ZH", "ZL", "X01", "X02", "X03", "X04", "X05",
    "X06", "X07", "X08", "X09", "X10", "X11", "X12", "X13", "X14", "X15", "X16", "X17", "X18",
    "X19", "X20", "X21", "X22", "X23", "X24", "X25", "X26", "X27", "X28", "X29", "X30", "X31",
    "X32", "X33", "X34", "X35", "X36", "X37", "X38", "X39", "X40", "X41", "X42", "X43", "X44",
    "X45", "X46", "X47", "X48", "X49", "X50", "X51", "X52", "X53", "X54", "X55", "X56", "X57",
    "X58", "X59", "X60", "X61", "X62", "X63", "X64", "X65", "X66", "X67", "X68", "X69", "X70",
    "X71", "X72", "X73", "X74", "X75", "X76", "X77", "X78", "X79", "X80", "X81", "X82", "X83",
    "X84", "X85", "X86", "X87", "X88", "X89", "X90", "X91", "X92", "X93", "X94", "X95", "X96",
    "X97", "X98", "X99",
];

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

fn to_grid6_24(mut n: usize) -> Option<String> {
    if n > 18 * 18 * 10 * 10 * 24 * 24 - 1 {
        return None;
    }
    let j1 = n / (18 * 10 * 10 * 24 * 24);
    n -= j1 * 18 * 10 * 10 * 24 * 24;
    let j2 = n / (10 * 10 * 24 * 24);
    n -= j2 * 10 * 10 * 24 * 24;
    let j3 = n / (10 * 24 * 24);
    n -= j3 * 10 * 24 * 24;
    let j4 = n / (24 * 24);
    n -= j4 * 24 * 24;
    let j5 = n / 24;
    let j6 = n - j5 * 24;
    if j1 > 17 || j2 > 17 || j3 > 9 || j4 > 9 || j5 > 23 || j6 > 23 {
        return None;
    }
    Some(format!(
        "{}{}{}{}{}{}",
        (b'A' + j1 as u8) as char,
        (b'A' + j2 as u8) as char,
        j3,
        j4,
        (b'A' + j5 as u8) as char,
        (b'A' + j6 as u8) as char
    ))
}

fn to_grid_25(mut n: usize) -> Option<String> {
    if n > 18 * 18 * 10 * 10 * 25 * 25 - 1 {
        return None;
    }
    let j1 = n / (18 * 10 * 10 * 25 * 25);
    n -= j1 * 18 * 10 * 10 * 25 * 25;
    let j2 = n / (10 * 10 * 25 * 25);
    n -= j2 * 10 * 10 * 25 * 25;
    let j3 = n / (10 * 25 * 25);
    n -= j3 * 10 * 25 * 25;
    let j4 = n / (25 * 25);
    n -= j4 * 25 * 25;
    let j5 = n / 25;
    let j6 = n - j5 * 25;
    if j1 > 17 || j2 > 17 || j3 > 9 || j4 > 9 || j5 > 24 || j6 > 24 {
        return None;
    }
    let mut grid = format!(
        "{}{}{}{}",
        (b'A' + j1 as u8) as char,
        (b'A' + j2 as u8) as char,
        j3,
        j4
    );
    if j5 != 24 || j6 != 24 {
        grid.push((b'A' + j5 as u8) as char);
        grid.push((b'A' + j6 as u8) as char);
    }
    Some(grid)
}

fn lookup_hash10(book: Option<&HashCallBook>, n10: usize) -> String {
    book.and_then(|book| book.lookup10(n10))
        .map(|call| format!("<{}>", call))
        .unwrap_or_else(|| "<...>".to_string())
}

fn lookup_hash12(book: Option<&HashCallBook>, n12: usize) -> String {
    book.and_then(|book| book.lookup12(n12))
        .map(|call| format!("<{}>", call))
        .unwrap_or_else(|| "<...>".to_string())
}

fn lookup_hash22(book: Option<&HashCallBook>, n22: usize) -> String {
    book.and_then(|book| book.lookup22(n22))
        .map(|call| format!("<{}>", call))
        .unwrap_or_else(|| "<...>".to_string())
}

fn format_signed_2(value: i32) -> String {
    format!("{value:+03}")
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

    if i3 == 0 && n3 == 1 {
        let n28a = bits_to_uint(bits77, 0, 28);
        let n28b = bits_to_uint(bits77, 28, 28);
        let n10 = bits_to_uint(bits77, 56, 10);
        let n5 = bits_to_uint(bits77, 66, 5);
        if n28a <= 2 || n28b <= 2 {
            return None;
        }
        let call1 = unpack28(n28a, book)?;
        let call2 = unpack28(n28b, book)?;
        let call3 = lookup_hash10(book, n10);
        let report = format_signed_2(2 * n5 as i32 - 30);
        return Some(format!("{call1} RR73; {call2} {call3} {report}"));
    }

    if i3 == 0 && n3 == 2 {
        return None;
    }

    if i3 == 0 && (n3 == 3 || n3 == 4) {
        let n28a = bits_to_uint(bits77, 0, 28);
        let n28b = bits_to_uint(bits77, 28, 28);
        let ir = bits77[56] as usize;
        let intx = bits_to_uint(bits77, 57, 4);
        let nclass = bits_to_uint(bits77, 61, 3);
        let isec = bits_to_uint(bits77, 64, 7);
        if n28a <= 2 || n28b <= 2 || isec < 1 || isec > CSEC.len() {
            return None;
        }
        let call1 = unpack28(n28a, book)?;
        let call2 = unpack28(n28b, book)?;
        let ntx = intx + 1 + if n3 == 4 { 16 } else { 0 };
        let class = (b'A' + nclass as u8) as char;
        let sec = CSEC[isec - 1];
        let exchange = format!("{ntx}{class}");
        let msg = if ir == 0 {
            format!("{call1} {call2} {exchange} {sec}")
        } else if ntx < 10 {
            format!("{call1} {call2} R{exchange} {sec}")
        } else {
            format!("{call1} {call2} R {exchange} {sec}")
        };
        return Some(msg);
    }

    if i3 == 0 && n3 == 5 {
        let n1 = bits_to_uint(bits77, 0, 23);
        let n2 = bits_to_uint(bits77, 23, 24);
        let n3tel = bits_to_uint(bits77, 47, 24);
        let msg = format!("{n1:06X}{n2:06X}{n3tel:06X}");
        return Some(msg.trim_start_matches('0').to_string());
    }

    if i3 == 0 && n3 == 6 {
        let j48 = bits77[47] as usize;
        let j49 = bits77[48] as usize;
        let j50 = bits77[49] as usize;
        let itype = if j50 == 1 {
            2
        } else if j49 == 0 {
            1
        } else if j48 == 0 {
            3
        } else {
            return None;
        };

        return match itype {
            1 => {
                let n28 = bits_to_uint(bits77, 0, 28);
                let igrid4 = bits_to_uint(bits77, 28, 15);
                let idbm = ((bits_to_uint(bits77, 43, 5) as f64) * 10.0 / 3.0).round() as i32;
                if !(0..=60).contains(&idbm) {
                    return None;
                }
                let call1 = unpack28(n28, book)?;
                let grid4 = to_grid4(igrid4)?;
                if let Some(book) = book {
                    book.save(&call1);
                }
                Some(format!("{call1} {grid4} {idbm}"))
            }
            2 => {
                let n28 = bits_to_uint(bits77, 0, 28);
                let mut npfx = bits_to_uint(bits77, 28, 16);
                let idbm = ((bits_to_uint(bits77, 44, 5) as f64) * 10.0 / 3.0).round() as i32;
                if !(0..=60).contains(&idbm) {
                    return None;
                }
                let call1 = unpack28(n28, book)?;
                let nzzz = 46_656usize;
                let msg = if npfx < nzzz {
                    let mut chars = Vec::new();
                    for _ in 0..3 {
                        let j = npfx % 36;
                        chars.push(A2[j] as char);
                        npfx /= 36;
                        if npfx == 0 {
                            break;
                        }
                    }
                    chars.reverse();
                    let prefix: String = chars.into_iter().collect();
                    format!("{prefix}/{call1} {idbm}")
                } else {
                    npfx -= nzzz;
                    let suffix = if npfx <= 35 {
                        (A2[npfx] as char).to_string()
                    } else if npfx <= 1295 {
                        format!("{}{}", A2[npfx / 36] as char, A2[npfx % 36] as char)
                    } else if npfx <= 12959 {
                        format!(
                            "{}{}{}",
                            A2[npfx / 360] as char,
                            A2[(npfx / 10) % 36] as char,
                            A2[npfx % 10] as char
                        )
                    } else {
                        return None;
                    };
                    format!("{call1}/{suffix} {idbm}")
                };
                Some(msg.trim_end().to_string())
            }
            3 => {
                let n22 = bits_to_uint(bits77, 0, 22);
                let igrid6 = bits_to_uint(bits77, 22, 25);
                let n28 = n22 + N_TOKENS;
                let call1 = unpack28(n28, book)?;
                let grid6 = to_grid_25(igrid6)?;
                Some(format!("{call1} {grid6}"))
            }
            _ => None,
        };
    }

    if i3 == 0 && n3 > 6 {
        return None;
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
    } else if i3 == 3 {
        let itu = bits77[0] as usize;
        let n28a = bits_to_uint(bits77, 1, 28);
        let n28b = bits_to_uint(bits77, 29, 28);
        let ir = bits77[57] as usize;
        let irpt = bits_to_uint(bits77, 58, 3);
        let nexch = bits_to_uint(bits77, 61, 13);

        let call1 = unpack28(n28a, book)?;
        let call2 = unpack28(n28b, book)?;
        let report = format!("5{}9", irpt + 2);

        let mut imult = 0usize;
        let mut serial = 0usize;
        if nexch > 8000 {
            imult = nexch - 8000;
        } else if nexch < 8000 {
            serial = nexch;
        }

        let exchange = if (1..=CMULT.len()).contains(&imult) {
            CMULT[imult - 1].to_string()
        } else if (1..=7999).contains(&serial) {
            format!("{serial:04}")
        } else {
            return None;
        };

        let prefix = if itu == 1 { "TU; " } else { "" };
        let r = if ir == 1 { " R" } else { "" };
        Some(format!("{prefix}{call1} {call2}{r} {report} {exchange}"))
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
    } else if i3 == 5 {
        let n12 = bits_to_uint(bits77, 0, 12);
        let n22 = bits_to_uint(bits77, 12, 22);
        let ir = bits77[34] as usize;
        let irpt = bits_to_uint(bits77, 35, 3);
        let iserial = bits_to_uint(bits77, 38, 11);
        let igrid6 = bits_to_uint(bits77, 49, 25);
        if igrid6 > 18_662_399 {
            return None;
        }
        let call1 = lookup_hash12(book, n12);
        let call2 = lookup_hash22(book, n22);
        let exchange = format!("{}{:04}", 52 + irpt, iserial);
        let grid6 = to_grid6_24(igrid6)?;
        if ir == 0 {
            Some(format!("{call1} {call2} {exchange} {grid6}"))
        } else {
            Some(format!("{call1} {call2} R {exchange} {grid6}"))
        }
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
    use crate::ft8::hashcall::HashCallBook;
    use crate::ft8::pack_jt77::pack28;
    use crate::ft8::pack_jt77::pack77;
    use crate::ft8::protocol::C38;

    fn append_bits(bits: &mut Vec<u8>, value: usize, width: usize) {
        for bit in (0..width).rev() {
            bits.push(((value >> bit) & 1) as u8);
        }
    }

    fn ihashcall(call: &str, width: usize) -> usize {
        let mut n8: u64 = 0;
        for c in format!("{:<11}", call.to_ascii_uppercase())
            .chars()
            .take(11)
        {
            let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
            n8 = 38 * n8 + j;
        }
        let prod = 47_055_833_459u64.wrapping_mul(n8);
        ((prod >> (64 - width as u32)) & ((1u64 << width as u32) - 1)) as usize
    }

    fn grid6_24(grid: &str) -> usize {
        let bytes = grid.as_bytes();
        (bytes[0] - b'A') as usize * 18 * 10 * 10 * 24 * 24
            + (bytes[1] - b'A') as usize * 10 * 10 * 24 * 24
            + (bytes[2] - b'0') as usize * 10 * 24 * 24
            + (bytes[3] - b'0') as usize * 24 * 24
            + (bytes[4] - b'A') as usize * 24
            + (bytes[5] - b'A') as usize
    }

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

    #[test]
    fn unpacks_type_01_dxpedition_rr73_semicolon_message() {
        let book = HashCallBook::new();
        book.save("R5AF/O");
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("RA3Y"), 28);
        append_bits(&mut bits, pack28("JR1FTJ"), 28);
        append_bits(&mut bits, ihashcall("R5AF/O", 10), 10);
        append_bits(&mut bits, 15, 5); // +00 => (0 + 30) / 2
        append_bits(&mut bits, 1, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(
            unpack77(&bits, Some(&book)).unwrap(),
            "RA3Y RR73; JR1FTJ <R5AF/O> +00"
        );
    }

    #[test]
    fn unpacks_type_03_field_day_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("WA9XYZ"), 28);
        append_bits(&mut bits, pack28("KA1ABC"), 28);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 15, 4);
        append_bits(&mut bits, 0, 3);
        append_bits(&mut bits, 11, 7); // EMA, WSJT-X 1-based section index
        append_bits(&mut bits, 3, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "WA9XYZ KA1ABC R 16A EMA");
    }

    #[test]
    fn unpacks_type_05_telemetry_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, 0x012345, 23);
        append_bits(&mut bits, 0x6789AB, 24);
        append_bits(&mut bits, 0xCDEF01, 24);
        append_bits(&mut bits, 5, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "123456789ABCDEF01");
    }

    #[test]
    fn unpacks_type_06_wspr_type1_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("K1ABC"), 28);
        append_bits(&mut bits, 5 * 18 * 10 * 10 + 13 * 10 * 10 + 4 * 10 + 2, 15); // FN42
        append_bits(&mut bits, 9, 5); // round(9 * 10 / 3) = 30 dBm
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 0, 21);
        append_bits(&mut bits, 6, 3);
        append_bits(&mut bits, 0, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "K1ABC FN42 30");
    }

    #[test]
    fn unpacks_type_3_rtty_message() {
        let mut bits = Vec::new();
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, pack28("W9XYZ"), 28);
        append_bits(&mut bits, pack28("K1ABC"), 28);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 5, 3);
        append_bits(&mut bits, 8000 + 21, 13); // MA, WSJT-X 1-based mult index
        append_bits(&mut bits, 3, 3);
        assert_eq!(unpack77(&bits, None).unwrap(), "TU; W9XYZ K1ABC R 579 MA");
    }

    #[test]
    fn unpacks_type_5_eu_vhf_hashed_calls_message() {
        let book = HashCallBook::new();
        book.save("K1ABC");
        book.save("G4ABC/P");
        let mut bits = Vec::new();
        append_bits(&mut bits, ihashcall("K1ABC", 12), 12);
        append_bits(&mut bits, ihashcall("G4ABC/P", 22), 22);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 7, 3);
        append_bits(&mut bits, 3, 11);
        append_bits(&mut bits, grid6_24("IO91NP"), 25);
        append_bits(&mut bits, 5, 3);
        assert_eq!(
            unpack77(&bits, Some(&book)).unwrap(),
            "<K1ABC> <G4ABC/P> R 590003 IO91NP"
        );
    }
}
