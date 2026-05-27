use crate::ft8::protocol::*;
/// FT8 message packing – Rust port of packjt77.f90
///
/// Supported message types:
///  0.0  Free text (≤13 chars from the 42-char FT8 alphabet)
///  0.1  DXpedition special message
///  0.3/0.4 ARRL Field Day
///  0.5  Telemetry
///  0.6  WSPR-style Type 1/2 message
///  1    Standard (two callsigns + grid/report/RR73/73)
///  3    ARRL RTTY contest exchange
///  4    One nonstandard (<hash>) call + one standard call
///  5    EU VHF contest hashed-call exchange
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

/// Pack a message into 77 bits.
pub fn pack77(msg: &str) -> Vec<u8> {
    let parts = split77(msg);
    if parts.is_empty() {
        panic!("Empty message");
    }

    if let Some(bits) = try_pack_type01(&parts) {
        return bits;
    }

    if let Some(bits) = try_pack_type03_04(&parts) {
        return bits;
    }

    if let Some(bits) = try_pack_type05_telemetry(&parts) {
        return bits;
    }

    if let Some(bits) = try_pack_type06_wspr(&parts) {
        return bits;
    }

    // Try Type 1/2: standard message
    if let Some(bits) = try_pack_type1(&parts) {
        return bits;
    }

    if let Some(bits) = try_pack_type3(&parts) {
        return bits;
    }

    // Try Type 4: one hash call
    if let Some(bits) = try_pack_type4(&parts) {
        return bits;
    }

    if let Some(bits) = try_pack_type5(&parts) {
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

    let first_two_have_letter = chars.iter().take(2).any(|c| c.is_ascii_uppercase());
    let q_prefix_ok = chars.first() != Some(&'Q') || call.starts_with("QU1RK");
    let standard = (1..=2).contains(&iarea)
        && first_two_have_letter
        && q_prefix_ok
        && nplet >= 1
        && npdig < iarea
        && (1..=3).contains(&nslet);

    CallsignParse {
        basecall: call,
        is_standard: standard,
        suffix,
    }
}

pub fn pack28(token: &str) -> usize {
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
                let j = if ('A'..='Z').contains(&c) {
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
        let mut cs_padded = [' '; 6];
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

fn ihashcall(c0: &str, width: usize) -> usize {
    let s = format!("{:<11}", c0).to_uppercase();
    let mut n8: u64 = 0;
    for c in s.chars().take(11) {
        let j = C38.iter().position(|&x| x == c as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
    }
    const MAGIC: u64 = 47055833459;
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> (64 - width as u32)) & ((1u64 << width as u32) - 1)) as usize
}

fn ihashcall22(c0: &str) -> usize {
    ihashcall(c0, 22)
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
            let irpt = if (-50..=-31).contains(&irpt) {
                irpt + 101
            } else {
                irpt
            };
            return MAXGRID4 + (irpt + 35) as usize;
        }
    }

    if let Ok(irpt) = s.parse::<i32>() {
        let irpt = if (-50..=-31).contains(&irpt) {
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

fn clean_hash_call(token: &str) -> Option<String> {
    let t = token.trim();
    if t.starts_with('<') && t.ends_with('>') && t.len() > 2 {
        Some(t[1..t.len() - 1].to_string())
    } else {
        None
    }
}

fn parse_report_i32(token: &str) -> Option<i32> {
    token.parse::<i32>().ok()
}

fn base_call_from_compound(call: &str) -> Option<String> {
    let trimmed = call.trim();
    if trimmed.len() > 11 || trimmed.contains(['.', '+', '-', '?']) {
        return None;
    }
    let slash = trimmed.find('/')?;
    if slash == 0 || slash == trimmed.len() - 1 {
        return None;
    }
    let prefix = &trimmed[..slash];
    let suffix = &trimmed[slash + 1..];
    if prefix.len().max(suffix.len()) > 6 {
        return None;
    }
    let base = if prefix.len() <= suffix.len() {
        suffix
    } else {
        prefix
    };
    if parse_callsign(base).is_standard {
        Some(base.to_string())
    } else {
        None
    }
}

fn grid6_24(grid: &str) -> Option<usize> {
    let bytes = grid.as_bytes();
    if bytes.len() != 6
        || !(b'A'..=b'R').contains(&bytes[0])
        || !(b'A'..=b'R').contains(&bytes[1])
        || !bytes[2].is_ascii_digit()
        || !bytes[3].is_ascii_digit()
        || !(b'A'..=b'X').contains(&bytes[4])
        || !(b'A'..=b'X').contains(&bytes[5])
    {
        return None;
    }
    Some(
        (bytes[0] - b'A') as usize * 18 * 10 * 10 * 24 * 24
            + (bytes[1] - b'A') as usize * 10 * 10 * 24 * 24
            + (bytes[2] - b'0') as usize * 10 * 24 * 24
            + (bytes[3] - b'0') as usize * 24 * 24
            + (bytes[4] - b'A') as usize * 24
            + (bytes[5] - b'A') as usize,
    )
}

fn try_pack_type01(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() != 5 || parts[1] != "RR73;" {
        return None;
    }
    let hash_call = clean_hash_call(&parts[3])?;
    let report = parse_report_i32(&parts[4])?;
    let p1 = parse_callsign(&parts[0]);
    let p2 = parse_callsign(&parts[2]);
    if !p1.is_standard || !p2.is_standard {
        return None;
    }

    let n5 = ((report + 30) / 2).clamp(0, 31) as usize;
    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, pack28(&parts[0]), 28);
    append_bits(&mut bits, pack28(&parts[2]), 28);
    append_bits(&mut bits, ihashcall(&hash_call, 10), 10);
    append_bits(&mut bits, n5, 5);
    append_bits(&mut bits, 1, 3);
    append_bits(&mut bits, 0, 3);
    Some(bits)
}

fn try_pack_type03_04(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 4 || parts.len() > 5 {
        return None;
    }
    let p1 = parse_callsign(&parts[0]);
    let p2 = parse_callsign(&parts[1]);
    if !p1.is_standard || !p2.is_standard {
        return None;
    }
    if parts.len() == 5 && parts[2] != "R" {
        return None;
    }
    let section = parts.last()?;
    let isec = CSEC.iter().position(|sec| sec == section)? + 1;
    let tx_class = &parts[parts.len() - 2];
    if tx_class.len() < 2 {
        return None;
    }
    let (ntx_text, class_text) = tx_class.split_at(tx_class.len() - 1);
    let ntx = ntx_text.parse::<usize>().ok()?;
    if !(1..=32).contains(&ntx) {
        return None;
    }
    let class = class_text.as_bytes()[0];
    if !(b'A'..=b'H').contains(&class) {
        return None;
    }
    let mut n3 = 3;
    let mut intx = ntx - 1;
    if intx >= 16 {
        n3 = 4;
        intx = ntx - 17;
    }
    let ir = usize::from(parts.len() == 5 && parts[2] == "R");
    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, pack28(&parts[0]), 28);
    append_bits(&mut bits, pack28(&parts[1]), 28);
    append_bits(&mut bits, ir, 1);
    append_bits(&mut bits, intx, 4);
    append_bits(&mut bits, (class - b'A') as usize, 3);
    append_bits(&mut bits, isec, 7);
    append_bits(&mut bits, n3, 3);
    append_bits(&mut bits, 0, 3);
    Some(bits)
}

fn try_pack_type05_telemetry(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() != 1 {
        return None;
    }
    let token = parts[0].trim();
    if token.is_empty() || token.len() > 18 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let padded = format!("{token:>018}");
    let n1 = usize::from_str_radix(&padded[0..6], 16).ok()?;
    if n1 >= (1 << 23) {
        return None;
    }
    let n2 = usize::from_str_radix(&padded[6..12], 16).ok()?;
    let n3tel = usize::from_str_radix(&padded[12..18], 16).ok()?;
    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, n1, 23);
    append_bits(&mut bits, n2, 24);
    append_bits(&mut bits, n3tel, 24);
    append_bits(&mut bits, 5, 3);
    append_bits(&mut bits, 0, 3);
    Some(bits)
}

fn try_pack_type06_wspr(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() == 3 {
        let call = parse_callsign(&parts[0]);
        if !call.is_standard || !is_grid4(&parts[1]) {
            return None;
        }
        let power = parts[2].parse::<i32>().ok()?.clamp(0, 60);
        let idbm = (0.3 * power as f64).round() as usize;
        let mut bits = Vec::with_capacity(77);
        append_bits(&mut bits, pack28(&parts[0]), 28);
        append_bits(&mut bits, pack_grid4(&parts[1]), 15);
        append_bits(&mut bits, idbm, 5);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 0, 21);
        append_bits(&mut bits, 6, 3);
        append_bits(&mut bits, 0, 3);
        return Some(bits);
    }

    if parts.len() != 2 {
        return None;
    }
    let compound = &parts[0];
    let power_text = &parts[1];
    if !(5..=10).contains(&compound.len())
        || power_text.len() > 2
        || !power_text.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let slash = compound.find('/')?;
    if slash == 0 || slash == compound.len() - 1 {
        return None;
    }
    if slash == compound.len().saturating_sub(4)
        && !compound.as_bytes()[compound.len() - 1].is_ascii_digit()
    {
        return None;
    }
    let basecall = base_call_from_compound(compound)?;
    let prefix = &compound[..slash];
    let suffix = &compound[slash + 1..];
    let nzzz = 46_656usize;
    let npfx = if slash <= 3 {
        let mut value = 0usize;
        for c in prefix.bytes() {
            let idx = A2.iter().position(|&x| x == c)?;
            value = 36 * value + idx;
        }
        value
    } else {
        let value = match suffix.len() {
            1 => A2.iter().position(|&x| x == suffix.as_bytes()[0])?,
            2 => {
                36 * A2.iter().position(|&x| x == suffix.as_bytes()[0])?
                    + A2.iter().position(|&x| x == suffix.as_bytes()[1])?
            }
            3 => {
                if !suffix.as_bytes()[2].is_ascii_digit() {
                    return None;
                }
                36 * 10 * A2.iter().position(|&x| x == suffix.as_bytes()[0])?
                    + 10 * A2.iter().position(|&x| x == suffix.as_bytes()[1])?
                    + A2.iter().position(|&x| x == suffix.as_bytes()[2])?
            }
            _ => return None,
        };
        value + nzzz
    };
    let power = power_text.parse::<i32>().ok()?.clamp(0, 60);
    let idbm = (0.3 * power as f64).round() as usize;
    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, pack28(&basecall), 28);
    append_bits(&mut bits, npfx, 16);
    append_bits(&mut bits, idbm, 5);
    append_bits(&mut bits, 1, 1);
    append_bits(&mut bits, 0, 21);
    append_bits(&mut bits, 6, 3);
    append_bits(&mut bits, 0, 3);
    Some(bits)
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
    if parts.len() == 2 && w2.contains('/') {
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
        let ir = if parts.len() == 4 && parts[2] == "R" {
            1
        } else {
            0
        };
        (pack_grid4(&last_upper), ir)
    } else if last_upper == "RRR" {
        (MAXGRID4 + 2, 0)
    } else if last_upper == "RR73" {
        (MAXGRID4 + 3, 0)
    } else if last_upper == "73" {
        (MAXGRID4 + 4, 0)
    } else if let Some(rest) = last_upper.strip_prefix('R') {
        if let Ok(irpt) = rest.parse::<i32>() {
            let irpt = if (-50..=-31).contains(&irpt) {
                irpt + 101
            } else {
                irpt
            };
            (MAXGRID4 + (irpt + 35) as usize, 1)
        } else {
            return None;
        }
    } else if let Ok(irpt) = last_upper.parse::<i32>() {
        let irpt = if (-50..=-31).contains(&irpt) {
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
    ihashcall(c0, 12)
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

fn try_pack_type3(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 4 || parts.len() > 6 {
        return None;
    }
    if parts[0].starts_with('<') && parts[1].starts_with('<') {
        return None;
    }
    let itu = usize::from(parts[0] == "TU;");
    if parts.len() < itu + 4 {
        return None;
    }
    let call1 = &parts[itu];
    let call2 = &parts[itu + 1];
    if !parse_callsign(call1).is_standard || !parse_callsign(call2).is_standard {
        return None;
    }

    let report = &parts[parts.len() - 2];
    if report.contains('-') || report.contains('+') {
        return None;
    }
    let report_num = report.parse::<i32>().ok()?;
    if !(529..=599).contains(&report_num) || report_num % 10 != 9 {
        return None;
    }
    let mut irpt = (report_num - 509) / 10 - 2;
    irpt = irpt.clamp(0, 7);

    let last = parts.last()?;
    let serial = last.parse::<usize>().ok();
    let mult = CMULT.iter().position(|m| m == last).map(|idx| idx + 1);
    let nexch = if let Some(mult) = mult {
        8000 + mult
    } else {
        let serial = serial?;
        if !(1..=7999).contains(&serial) {
            return None;
        }
        serial
    };

    let ir = usize::from(parts.get(itu + 2).map(|s| s.as_str()) == Some("R"));
    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, itu, 1);
    append_bits(&mut bits, pack28(call1), 28);
    append_bits(&mut bits, pack28(call2), 28);
    append_bits(&mut bits, ir, 1);
    append_bits(&mut bits, irpt as usize, 3);
    append_bits(&mut bits, nexch, 13);
    append_bits(&mut bits, 3, 3);
    Some(bits)
}

fn try_pack_type5(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 4 || parts.len() > 5 {
        return None;
    }
    let call1 = clean_hash_call(&parts[0])?;
    let call2 = clean_hash_call(&parts[1])?;
    let exchange = parts[parts.len() - 2].parse::<usize>().ok()?;
    if !(520001..=594095).contains(&exchange) {
        return None;
    }
    let grid6 = parts.last()?;
    let igrid6 = grid6_24(grid6)?;
    let ir = usize::from(parts.len() == 5 && parts[2] == "R");
    let irpt = exchange / 10000 - 52;
    if irpt > 7 {
        return None;
    }
    let iserial = (exchange % 10000).min(2047);

    let mut bits = Vec::with_capacity(77);
    append_bits(&mut bits, ihashcall(&call1, 12), 12);
    append_bits(&mut bits, ihashcall(&call2, 22), 22);
    append_bits(&mut bits, ir, 1);
    append_bits(&mut bits, irpt, 3);
    append_bits(&mut bits, iserial, 11);
    append_bits(&mut bits, igrid6, 25);
    append_bits(&mut bits, 5, 3);
    Some(bits)
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
        bits.push((byte0 >> b) & 1);
    }
    // limbs 1..8 give 8 bits each
    for li in 1..=8 {
        let byte = qa_bytes[li];
        for b in (0..8).rev() {
            bits.push((byte >> b) & 1);
        }
    }
    bits
}

/// Check if a callsign is "standard" per WSJT-X stdcall.
/// Port of wsjtx/lib/qra/q65/q65_set_list.f90:stdcall
pub fn is_stdcall(callsign: &str) -> bool {
    let c = callsign.trim().to_uppercase();
    let bytes = c.as_bytes();
    let n = bytes.len();
    if n < 2 || n > 12 {
        return false;
    }

    // Find right-most digit (call area)
    let mut iarea: i32 = -1;
    for i in (1..n).rev() {
        if bytes[i].is_ascii_digit() {
            iarea = i as i32;
            break;
        }
    }

    // WSJT-X stdcall uses 1-based iarea in [2, 3]. Rust indices are 0-based.
    if !(1..=2).contains(&iarea) {
        return false;
    }

    // Count digits and letters before call area
    let mut npdig = 0;
    let mut nplet = 0;
    for i in 0..(iarea as usize) {
        if bytes[i].is_ascii_digit() {
            npdig += 1;
        }
        if bytes[i].is_ascii_uppercase() {
            nplet += 1;
        }
    }

    // Count letters in suffix
    let mut nslet = 0;
    for i in (iarea as usize + 1)..n {
        if bytes[i].is_ascii_uppercase() {
            nslet += 1;
        }
    }

    nplet >= 1 && (npdig as i32) < iarea && nslet <= 3
}

#[cfg(test)]
mod tests {
    use super::is_stdcall;
    use crate::ft8::hashcall::HashCallBook;

    #[test]
    fn stdcall_matches_wsjtx_one_based_iarea() {
        assert!(is_stdcall("D1DX"));
        assert!(is_stdcall("R6KEE"));
        assert!(is_stdcall("F1PPH"));
        assert!(is_stdcall("IW1PUR"));
        assert!(is_stdcall("DL8YHR"));
        assert!(!is_stdcall("KN87"));
    }

    #[test]
    fn type1_r_grid_uses_third_word_like_wsjtx() {
        let bits = super::pack77("K1ABC W9XYZ R FN42");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "K1ABC W9XYZ R FN42");
    }

    #[test]
    fn report_tokens_are_not_standard_callsigns_for_split77() {
        assert!(!super::parse_callsign("RR73").is_standard);
        assert!(!super::parse_callsign("73").is_standard);
    }

    #[test]
    fn two_word_type1_rejects_second_call_slash_like_wsjtx() {
        assert!(super::try_pack_type1(&["K1ABC".into(), "W9XYZ/R".into()]).is_none());
    }

    #[test]
    fn type01_dxpedition_round_trips_with_hash10_book() {
        let book = HashCallBook::new();
        book.save("R5AF/O");
        let bits = super::pack77("RA3Y RR73; JR1FTJ <R5AF/O> +00");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, Some(&book)).unwrap();
        assert_eq!(msg, "RA3Y RR73; JR1FTJ <R5AF/O> +00");
    }

    #[test]
    fn type03_field_day_round_trips() {
        let bits = super::pack77("WA9XYZ KA1ABC R 16A EMA");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "WA9XYZ KA1ABC R 16A EMA");
    }

    #[test]
    fn type05_telemetry_round_trips() {
        let bits = super::pack77("0123456789ABCDEF01");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "123456789ABCDEF01");
    }

    #[test]
    fn type06_wspr_type1_round_trips() {
        let bits = super::pack77("K1ABC FN42 30");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "K1ABC FN42 30");
    }

    #[test]
    fn type06_wspr_type2_prefix_round_trips() {
        let bits = super::pack77("PJ4/K1ABC 30");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "PJ4/K1ABC 30");
    }

    #[test]
    fn type06_wspr_type2_suffix_round_trips() {
        let bits = super::pack77("K1ABC/P 30");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "K1ABC/P 30");
    }

    #[test]
    fn type3_rtty_round_trips() {
        let bits = super::pack77("TU; W9XYZ K1ABC R 579 MA");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "TU; W9XYZ K1ABC R 579 MA");
    }

    #[test]
    fn type5_eu_vhf_round_trips_with_hash_book() {
        let book = HashCallBook::new();
        book.save("K1ABC");
        book.save("G4ABC/P");
        let bits = super::pack77("<K1ABC> <G4ABC/P> R 590003 IO91NP");
        let msg = crate::ft8::unpack_jt77::unpack77(&bits, Some(&book)).unwrap();
        assert_eq!(msg, "<K1ABC> <G4ABC/P> R 590003 IO91NP");
    }
}
