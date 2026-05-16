/// FT8 message packing – Rust port of packjt77.f90
///
/// Supported message types:
///  0.0  Free text (≤13 chars from the 42-char FT8 alphabet)
///  1    Standard (two callsigns + grid/report/RR73/73)
///  4    One nonstandard (<hash>) call + one standard call

use num_bigint::BigInt;
use crate::util::constants::*;

/// Pack a message into 77 bits.
pub fn pack77(msg: &str) -> Vec<u8> {
    let parts = split77(msg);
    if parts.is_empty() {
        panic!("Empty message");
    }

    // Try Type 1/2: standard message
    if let Some(bits) = try_pack_type1(&parts) {
        return bits;
    }

    // Try Type 4: one hash call
    if let Some(bits) = try_pack_type4(&parts) {
        return bits;
    }

    // Default: Type 0.0 free text
    pack_free_text(msg)
}

fn split77(msg: &str) -> Vec<String> {
    let parts: Vec<String> = msg
        .trim()
        .to_uppercase()
        .split_whitespace()
        .map(String::from)
        .collect();

    if parts.len() >= 3 && parts[0] == "CQ" {
        let w3 = parts[2].replace("/R", "").replace("/P", "");
        if parse_callsign(&w3).is_standard {
            let mut merged = vec![format!("CQ_{}", parts[1])];
            merged.extend_from_slice(&parts[2..]);
            return merged;
        }
    }
    parts
}

#[derive(Debug, Clone)]
struct CallsignParse {
    basecall: String,
    is_standard: bool,
    suffix: Option<String>,
}

fn parse_callsign(raw: &str) -> CallsignParse {
    let mut call = raw.trim().to_uppercase();
    let mut suffix: Option<String> = None;

    if call.ends_with("/R") {
        suffix = Some("/R".into());
        call = call[..call.len() - 2].into();
    }
    if call.ends_with("/P") {
        suffix = Some("/P".into());
        call = call[..call.len() - 2].into();
    }

    let is_letter = |c: char| c.is_ascii_uppercase();
    let is_digit = |c: char| c.is_ascii_digit();

    let chars: Vec<char> = call.chars().collect();
    let mut iarea: i32 = -1;
    for i in (1..chars.len()).rev() {
        if is_digit(chars[i]) {
            iarea = i as i32;
            break;
        }
    }

    if iarea < 1 {
        return CallsignParse {
            basecall: call,
            is_standard: false,
            suffix,
        };
    }

    let mut npdig = 0;
    let mut nplet = 0;
    for i in 0..iarea as usize {
        if is_digit(chars[i]) {
            npdig += 1;
        }
        if is_letter(chars[i]) {
            nplet += 1;
        }
    }
    let mut nslet = 0;
    for i in (iarea as usize + 1)..chars.len() {
        if is_letter(chars[i]) {
            nslet += 1;
        }
    }

    let standard = iarea >= 1 && iarea <= 2 && nplet >= 1 && npdig < iarea && nslet <= 3;

    CallsignParse {
        basecall: call,
        is_standard: standard,
        suffix,
    }
}

fn pack28(token: &str) -> usize {
    let t = token.trim().to_uppercase();

    if t == "DE" {
        return 0;
    }
    if t == "QRZ" {
        return 1;
    }
    if t == "CQ" {
        return 2;
    }

    // CQ_nnn or CQ_aaaa
    if let Some(rest) = t.strip_prefix("CQ_") {
        if rest.len() == 3 && rest.chars().all(|c| c.is_ascii_digit()) {
            let nqsy: usize = rest.parse().unwrap_or(0);
            return 3 + nqsy;
        }
        if rest.len() <= 4 && rest.chars().all(|c| c.is_ascii_uppercase()) {
            let padded = format!("{:>4}", rest);
            let mut m: usize = 0;
            for c in padded.chars() {
                let j = if c >= 'A' && c <= 'Z' {
                    (c as usize) - 64
                } else {
                    0
                };
                m = 27 * m + j;
            }
            return 3 + 1000 + m;
        }
    }

    // <...> hash calls
    if t.starts_with('<') && t.ends_with('>') {
        let inner = &t[1..t.len() - 1];
        let n22 = ihashcall22(inner);
        return (N_TOKENS + n22) & (MAX28 - 1);
    }

    // Standard callsign
    let parsed = parse_callsign(&t);
    if parsed.is_standard {
        let basecall = &parsed.basecall;
        let chars: Vec<char> = basecall.chars().collect();
        let mut iarea_d: i32 = -1;
        for i in (1..chars.len()).rev() {
            if chars[i].is_ascii_digit() {
                iarea_d = i as i32;
                break;
            }
        }

        let cs = if iarea_d == 1 {
            format!(" {}", &basecall[..basecall.len().min(5)])
        } else if iarea_d == 2 {
            basecall[..basecall.len().min(6)].to_string()
        } else {
            basecall.clone()
        };

        let cs_chars: Vec<char> = cs.chars().collect();
        let mut cs_padded = vec![' '; 6];
        for i in 0..cs_chars.len().min(6) {
            cs_padded[i] = cs_chars[i];
        }

        let i1 = find_char(A1, cs_padded[0]);
        let i2 = find_char(A2, cs_padded[1]);
        let i3 = find_char(A3, cs_padded[2]);
        let i4 = find_char(A4, cs_padded[3]);
        let i5 = find_char(A4, cs_padded[4]);
        let i6 = find_char(A4, cs_padded[5]);

        let n28 = 36 * 10 * 27 * 27 * 27 * i1
            + 10 * 27 * 27 * 27 * i2
            + 27 * 27 * 27 * i3
            + 27 * 27 * i4
            + 27 * i5
            + i6;
        return (n28 + N_TOKENS + MAX22) & (MAX28 - 1);
    }

    // Non-standard → 22-bit hash
    let n22 = ihashcall22(&parsed.basecall);
    (N_TOKENS + n22) & (MAX28 - 1)
}

fn find_char(alphabet: &[u8], c: char) -> usize {
    let b = c as u8;
    alphabet.iter().position(|&x| x == b).unwrap_or(0)
}

fn ihashcall22(c0: &str) -> usize {
    let s = format!("{:<11}", c0).to_uppercase();
    let mut n8: u64 = 0;
    for c in s.chars().take(11) {
        let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
    }
    const MAGIC: u64 = 47055833459;
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> 42) & 0x3fffff) as usize
}

fn is_grid4(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 4
        && bytes[0] >= b'A'
        && bytes[0] <= b'R'
        && bytes[1] >= b'A'
        && bytes[1] <= b'R'
        && bytes[2] >= b'0'
        && bytes[2] <= b'9'
        && bytes[3] >= b'0'
        && bytes[3] <= b'9'
}

fn pack_grid4(s: &str) -> usize {
    if s == "RRR" {
        return MAXGRID4 + 2;
    }
    if s == "73" {
        return MAXGRID4 + 4;
    }

    // Numeric report
    if let Some(rest) = s.strip_prefix('R') {
        if let Ok(irpt) = rest.parse::<i32>() {
            let irpt = if irpt >= -50 && irpt <= -31 {
                irpt + 101
            } else {
                irpt
            };
            return MAXGRID4 + (irpt + 35) as usize;
        }
    }

    if let Ok(irpt) = s.parse::<i32>() {
        let irpt = if irpt >= -50 && irpt <= -31 {
            irpt + 101
        } else {
            irpt
        };
        return MAXGRID4 + (irpt + 35) as usize;
    }

    // 4-char grid locator
    let bytes = s.as_bytes();
    let j1 = (bytes[0] - b'A') as usize * 18 * 10 * 10;
    let j2 = (bytes[1] - b'A') as usize * 10 * 10;
    let j3 = (bytes[2] - b'0') as usize * 10;
    let j4 = (bytes[3] - b'0') as usize;
    j1 + j2 + j3 + j4
}

fn append_bits(bits: &mut Vec<u8>, val: usize, width: usize) {
    for i in (0..width).rev() {
        bits.push(((val >> i) & 1) as u8);
    }
}

fn try_pack_type1(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 2 || parts.len() > 4 {
        return None;
    }

    let w1 = &parts[0];
    let w2 = &parts[1];

    if w1.starts_with('<') && w2.contains('/') {
        return None;
    }
    if w2.starts_with('<') && w1.contains('/') {
        return None;
    }

    let (call1, ok1, ipa) = parse_callsign1(w1);
    let (call2, ok2, ipb) = parse_callsign2(w2);

    if !ok1 || !ok2 {
        return None;
    }

    let i1psfx = ipa == 1 && (w1.ends_with("/P") || w1.contains("/P "));
    let i2psfx = ipb == 1 && (w2.ends_with("/P") || w2.contains("/P "));
    let i3 = if i1psfx || i2psfx { 2 } else { 1 };

    let w_last = parts.last().unwrap();
    let last_upper = w_last.to_uppercase();

    let (igrid4, ir) = if parts.len() == 2 {
        (MAXGRID4 + 1, 0)
    } else if is_grid4(&last_upper) {
        let ir = if parts.len() == 4 && parts[1] == "R" { 1 } else { 0 };
        (pack_grid4(&last_upper), ir)
    } else if last_upper == "RRR" {
        (MAXGRID4 + 2, 0)
    } else if last_upper == "RR73" {
        (MAXGRID4 + 3, 0)
    } else if last_upper == "73" {
        (MAXGRID4 + 4, 0)
    } else if let Some(rest) = last_upper.strip_prefix('R') {
        if let Ok(irpt) = rest.parse::<i32>() {
            let irpt = if irpt >= -50 && irpt <= -31 {
                irpt + 101
            } else {
                irpt
            };
            (MAXGRID4 + (irpt + 35) as usize, 1)
        } else {
            return None;
        }
    } else if let Ok(irpt) = last_upper.parse::<i32>() {
        let irpt = if irpt >= -50 && irpt <= -31 {
            irpt + 101
        } else {
            irpt
        };
        (MAXGRID4 + (irpt + 35) as usize, 0)
    } else {
        return None;
    };

    let n28a = pack28(&call1);
    let n28b = pack28(&call2);

    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, n28a, 28);
    append_bits(&mut bits, ipa, 1);
    append_bits(&mut bits, n28b, 28);
    append_bits(&mut bits, ipb, 1);
    append_bits(&mut bits, ir, 1);
    append_bits(&mut bits, igrid4, 15);
    append_bits(&mut bits, i3, 3);
    Some(bits)
}

fn parse_callsign1(w: &str) -> (String, bool, usize) {
    if w == "CQ" || w == "DE" || w == "QRZ" || w.starts_with("CQ_") {
        return (w.into(), true, 0);
    }
    if w.starts_with('<') && w.ends_with('>') {
        return (w.into(), true, 0);
    }
    let p = parse_callsign(w);
    let ipa = if p.suffix.as_deref() == Some("/R") || p.suffix.as_deref() == Some("/P") {
        1
    } else {
        0
    };
    (p.basecall, p.is_standard, ipa)
}

fn parse_callsign2(w: &str) -> (String, bool, usize) {
    if w.starts_with('<') && w.ends_with('>') {
        return (w.into(), true, 0);
    }
    let p = parse_callsign(w);
    let ipb = if p.suffix.as_deref() == Some("/R") || p.suffix.as_deref() == Some("/P") {
        1
    } else {
        0
    };
    (p.basecall, p.is_standard, ipb)
}

fn try_pack_type4(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }

    let w1 = &parts[0];
    let w2 = &parts[1];
    let w3 = parts.get(2);

    let parsed_w1 = parse_callsign(w1);
    let parsed_w2 = parse_callsign(w2);

    if parsed_w1.is_standard
        && parsed_w2.is_standard
        && !w1.starts_with('<')
        && !w2.starts_with('<')
    {
        return None;
    }

    let (n12, c11, iflip, icq) = if w1 == "CQ" {
        if w2.len() <= 4 {
            return None;
        }
        (ihashcall12(w2), w2.clone(), 0, 1)
    } else if w1.starts_with('<') && w1.ends_with('>') {
        let inner = &w1[1..w1.len() - 1];
        (ihashcall12(inner), w2.clone(), 0, 0)
    } else if w2.starts_with('<') && w2.ends_with('>') {
        let inner = &w2[1..w2.len() - 1];
        (ihashcall12(inner), w1.clone(), 1, 0)
    } else {
        return None;
    };

    let n58 = encode_c11(&c11);
    let nrpt = decode_rpt(w3.map(|s| s.as_str()));

    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, n12, 12);
    // n58 as 58-bit BigInt
    for b in (0..58).rev() {
        bits.push(((n58 >> b) & 1) as u8);
    }
    append_bits(&mut bits, iflip, 1);
    append_bits(&mut bits, nrpt, 2);
    append_bits(&mut bits, icq, 1);
    append_bits(&mut bits, 4, 3);
    Some(bits)
}

fn ihashcall12(c0: &str) -> usize {
    let s = format!("{:<11}", c0).to_uppercase();
    let mut n8: u64 = 0;
    for c in s.chars().take(11) {
        let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
    }
    const MAGIC: u64 = 47055833459;
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> 52) & 0xfff) as usize
}

fn encode_c11(c11: &str) -> u64 {
    let padded = format!("{:>11}", c11);
    let mut n: u64 = 0;
    for c in padded.chars().take(11) {
        let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
        n = n * 38 + j;
    }
    n
}

fn decode_rpt(w: Option<&str>) -> usize {
    match w {
        Some("RRR") => 1,
        Some("RR73") => 2,
        Some("73") => 3,
        _ => 0,
    }
}

fn pack_free_text(msg: &str) -> Vec<u8> {
    let raw = msg.to_uppercase();
    let chars: Vec<char> = raw.chars().take(13).collect();
    let bits71 = pack_text77(&chars);

    let mut bits = bits71;
    bits.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    bits
}

fn pack_text77(chars: &[char]) -> Vec<u8> {
    let mut w = vec![' '; 13];
    let start = 13 - chars.len();
    for (i, &c) in chars.iter().enumerate() {
        w[start + i] = c;
    }

    let mut qa = BigInt::from(0u64);
    for &c in &w {
        let j = FT_ALPH.iter().position(|&x| x == c as u8).unwrap_or(0);
        qa = &qa * 42 + j;
    }

    // Extract 71 bits
    let (_sign, bytes) = qa.to_bytes_be(); // big-endian
    let mut qa_bytes = [0u8; 9];
    let start = 9usize.saturating_sub(bytes.len());
    for (i, &b) in bytes.iter().take(9).enumerate() {
        qa_bytes[start + i] = b;
    }

    let mut bits = Vec::with_capacity(71);
    // limb 0 gives 7 bits (bits 0-6 of byte 0, i.e., top 7 bits)
    let byte0 = qa_bytes[0];
    for b in (0..7).rev() {
        bits.push(((byte0 >> b) & 1) as u8);
    }
    // limbs 1..8 give 8 bits each
    for li in 1..=8 {
        let byte = qa_bytes[li];
        for b in (0..8).rev() {
            bits.push(((byte >> b) & 1) as u8);
        }
    }
    bits
}
