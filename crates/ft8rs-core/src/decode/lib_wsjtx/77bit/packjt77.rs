//! FT8/JT77 message packing.
//!
//! Source mapping:
//! - `wsjtx/lib/77bit/packjt77.f90`
///
/// Supported message types:
///  0.0  Free text (≤13 chars from the 42-char FT8 alphabet)
///  0.1  DXpedition special message
///  0.3/0.4 ARRL Field Day
///  0.5  Telemetry
///  1    Standard (two callsigns + grid/report/RR73/73)
///  3    ARRL RTTY contest exchange
///  4    One nonstandard (<hash>) call + one standard call
///  5    EU VHF contest hashed-call exchange
use num_bigint::BigInt;
use std::cell::RefCell;

pub(crate) const FT_ALPH: &[u8; 42] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ+-./?";
pub(crate) const A1: &[u8; 37] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
pub(crate) const A2: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
pub(crate) const A3: &[u8; 10] = b"0123456789";
pub(crate) const A4: &[u8; 27] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ";
pub(crate) const C38: &[u8; 38] = b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ/";

pub(crate) const N_TOKENS: usize = 2_063_592;
pub(crate) const MAX22: usize = 4_194_304; // 2^22
pub(crate) const MAX28: usize = 268_435_456; // 2^28
pub(crate) const MAXGRID4: usize = 32_400;
const MAX_HASH22_ENTRIES: usize = 1000;

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

/// Pack a message at the native `genft8` compatibility boundary. Structured
/// messages use the upstream grammar; eligible free text retains its explicit
/// 13-character fallback. Decoder template probes use `pack77_strict`.
pub fn pack77(msg: &str) -> Vec<u8> {
    let msg = normalized_source(msg);
    if let Some(bits) = pack77_strict(&msg) {
        return bits;
    }
    let payload: String = msg.chars().take(13).collect();
    if payload.is_empty() || !payload.bytes().all(|c| FT_ALPH.contains(&c)) {
        return Vec::new();
    }
    pack_free_text(&payload)
}

fn normalized_source(msg: &str) -> String {
    msg.chars()
        .take(37)
        .map(|c| {
            if c == '\0' {
                ' '
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect::<String>()
        .split(' ')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn pack77_strict(msg: &str) -> Option<Vec<u8>> {
    let msg = normalized_source(msg);
    if !msg.is_ascii() || msg.is_empty() {
        return None;
    }
    let bits = pack77_candidate(&msg)?;
    // Native pack77_05 accepts a hexadecimal telemetry value directly;
    // its fixed-width 18-digit display may add leading zeroes.
    if bits[74..] == [0, 0, 0] && bits[71..74] == [1, 0, 1] {
        return Some(bits);
    }
    // Upstream's neutral round-trip gate compares source tokens, allowing
    // auto-hashed compound calls to acquire brackets. No live hash effects.
    let book = HashCallBook::new();
    for token in msg.split(' ') {
        book.save(token.trim_matches(['<', '>']));
    }
    let decoded = unpack77(&bits, Some(&book))?;
    let source_tokens: Vec<_> = msg.split(' ').collect();
    let decoded_tokens: Vec<_> = decoded.split(' ').collect();
    if source_tokens.len() != decoded_tokens.len() || decoded.len() > 37 {
        return None;
    }
    source_tokens
        .iter()
        .zip(decoded_tokens)
        .all(|(a, b)| *a == b || format!("<{a}>") == b)
        .then_some(bits)
}

fn pack77_candidate(msg: &str) -> Option<Vec<u8>> {
    let parts = split77(msg);
    if parts.is_empty() {
        return None;
    }

    if let Some(bits) = try_pack_type01(&parts) {
        return Some(bits);
    }

    if let Some(bits) = try_pack_type03_04(&parts) {
        return Some(bits);
    }

    if let Some(bits) = try_pack_type05_telemetry(&parts) {
        return Some(bits);
    }

    // Try Type 1/2: standard message
    if let Some(bits) = try_pack_type1(&parts) {
        return Some(bits);
    }

    if let Some(bits) = try_pack_type3(&parts) {
        return Some(bits);
    }

    // Try Type 4: one hash call
    if let Some(bits) = try_pack_type4(&parts) {
        return Some(bits);
    }

    if let Some(bits) = try_pack_type5(&parts) {
        return Some(bits);
    }

    // Default: Type 0.0 free text
    (msg.len() <= 13 && msg.bytes().all(|c| FT_ALPH.contains(&c))).then(|| pack_free_text(msg))
}

fn split77(msg: &str) -> Vec<String> {
    let parts: Vec<String> = msg
        .trim()
        .to_uppercase()
        .split_whitespace()
        .map(String::from)
        .collect();

    if parts.len() >= 3 && parts[0] == "CQ" && valid_cq_modifier(&parts[1]) {
        let mut merged = vec![format!("CQ_{}", parts[1])];
        merged.extend_from_slice(&parts[2..]);
        return merged;
    }
    parts
}

fn valid_cq_modifier(token: &str) -> bool {
    (token.len() == 3 && token.bytes().all(|b| b.is_ascii_digit()))
        || ((1..=4).contains(&token.len()) && token.bytes().all(|b| b.is_ascii_uppercase()))
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
    for chars_value in chars.iter().take(iarea as usize) {
        if is_digit(*chars_value) {
            npdig += 1;
        }
        if is_letter(*chars_value) {
            nplet += 1;
        }
    }
    let mut nslet = 0;
    for chars_value in chars.iter().skip(iarea as usize + 1) {
        if is_letter(*chars_value) {
            nslet += 1;
        }
    }

    let first_two_have_letter = chars.iter().take(2).any(|c| c.is_ascii_uppercase());
    let q_prefix_ok = chars.first() != Some(&'Q');
    let standard = (3..=6).contains(&call.len())
        && call.bytes().all(|b| b.is_ascii_alphanumeric())
        && chars[iarea as usize + 1..]
            .iter()
            .all(|c| c.is_ascii_uppercase())
        && (1..=2).contains(&iarea)
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
                let j = if c.is_ascii_uppercase() {
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
    // Native pack28 permits an empty suffix (e.g. A0). Receive validation
    // is separate; WSJT-X and JTDX intentionally have different rules there.
    let empty_suffix =
        t.len() == 2 && t.as_bytes()[0].is_ascii_uppercase() && t.as_bytes()[1].is_ascii_digit();
    if parsed.is_standard || empty_suffix {
        let basecall = if empty_suffix { &t } else { &parsed.basecall };
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
        let len = cs_chars.len().min(6);
        cs_padded[..len].copy_from_slice(&cs_chars[..len]);

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

/// FT8 callsign hash (`packjt77.f90` `ihashcall`).
pub(crate) fn ihashcall(c0: &str, width: usize) -> usize {
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

pub struct HashCallBook {
    calls10: RefCell<Vec<Option<String>>>,
    calls12: RefCell<Vec<Option<String>>>,
    hash22_entries: RefCell<Vec<(usize, String)>>,
}

impl Default for HashCallBook {
    fn default() -> Self {
        Self::new()
    }
}

impl HashCallBook {
    pub fn new() -> Self {
        HashCallBook {
            calls10: RefCell::new(vec![None; 1024]),
            calls12: RefCell::new(vec![None; 4096]),
            hash22_entries: RefCell::new(Vec::new()),
        }
    }

    pub fn save(&self, callsign: &str) {
        let trimmed = callsign.trim();
        if trimmed.is_empty() || trimmed == "<...>" {
            return;
        }

        let clean = if trimmed.starts_with('<') && trimmed.ends_with('>') {
            &trimmed[1..trimmed.len() - 1]
        } else if let Some(inner) = trimmed.strip_prefix('<') {
            inner.split_once('>').map_or(inner, |(call, _)| call)
        } else {
            trimmed
        };

        if clean.len() < 3 {
            return;
        }

        let cw = clean.to_uppercase();

        let n10 = ihashcall(&cw, 10);
        if n10 <= 1023 {
            self.calls10.borrow_mut()[n10] = Some(cw.clone());
        }

        let n12 = ihashcall(&cw, 12);
        if n12 <= 4095 {
            self.calls12.borrow_mut()[n12] = Some(cw.clone());
        }

        let n22 = ihashcall(&cw, 22);
        let mut entries = self.hash22_entries.borrow_mut();
        if let Some(pos) = entries.iter().position(|(h, _)| *h == n22) {
            entries[pos].1 = cw;
        } else {
            if entries.len() >= MAX_HASH22_ENTRIES {
                entries.pop();
            }
            entries.insert(0, (n22, cw));
        }
    }

    pub fn lookup10(&self, n10: usize) -> Option<String> {
        if n10 <= 1023 {
            self.calls10.borrow()[n10].clone()
        } else {
            None
        }
    }

    pub fn lookup12(&self, n12: usize) -> Option<String> {
        if n12 <= 4095 {
            self.calls12.borrow()[n12].clone()
        } else {
            None
        }
    }

    pub fn lookup22(&self, n22: usize) -> Option<String> {
        self.hash22_entries
            .borrow()
            .iter()
            .find(|(h, _)| *h == n22)
            .map(|(_, c)| c.clone())
    }

    pub fn size(&self) -> usize {
        self.hash22_entries.borrow().len()
    }

    pub fn clear(&self) {
        self.calls10.borrow_mut().iter_mut().for_each(|c| *c = None);
        self.calls12.borrow_mut().iter_mut().for_each(|c| *c = None);
        self.hash22_entries.borrow_mut().clear();
    }

    pub fn get_calls(&self) -> Vec<String> {
        let mut calls: Vec<String> = Vec::new();
        for entry in self.hash22_entries.borrow().iter() {
            if !calls.contains(&entry.1) {
                calls.push(entry.1.clone());
            }
        }
        calls
    }

    pub fn clone_book(&self) -> HashCallBook {
        HashCallBook {
            calls10: RefCell::new(self.calls10.borrow().clone()),
            calls12: RefCell::new(self.calls12.borrow().clone()),
            hash22_entries: RefCell::new(self.hash22_entries.borrow().clone()),
        }
    }
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
    if t.starts_with('<')
        && t.ends_with('>')
        && (5..=13).contains(&t.len())
        && !t[1..t.len() - 1].contains(['<', '>'])
    {
        Some(t[1..t.len() - 1].to_string())
    } else {
        None
    }
}

fn parse_report_i32(token: &str) -> Option<i32> {
    let b = token.as_bytes();
    (b.len() == 3 && matches!(b[0], b'+' | b'-') && b[1..].iter().all(u8::is_ascii_digit))
        .then(|| token.parse::<i32>().ok())
        .flatten()
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
    if !valid_c28_source(&parts[0])
        || !valid_c28_source(&parts[2])
        || !(-30..=32).contains(&report)
        || (report + 30) % 2 != 0
    {
        return None;
    }

    let n5 = ((report + 30) / 2) as usize;
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
    if !valid_c28_source(&parts[0]) || !valid_c28_source(&parts[1]) {
        return None;
    }
    if parts.len() == 5 && parts[2] != "R" {
        return None;
    }
    let section = parts.last()?;
    let isec = CSEC.iter().position(|sec| sec == section)? + 1;
    let tx_class = &parts[parts.len() - 2];
    if !(2..=3).contains(&tx_class.len()) || tx_class.starts_with('0') {
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

fn try_pack_type1(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 2 || parts.len() > 4 {
        return None;
    }

    // pack77_1 accepts a fourth word only for "CALL CALL R GRID".
    if parts.len() == 4 && parts[2] != "R" {
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
    if (w1.ends_with("/R") && w2.ends_with("/P")) || (w1.ends_with("/P") && w2.ends_with("/R")) {
        return None;
    }
    let cq = w1 == "CQ" || w1.starts_with("CQ_");
    if cq && w2.starts_with('<') {
        return None;
    }

    let i1psfx = ipa == 1 && (w1.ends_with("/P") || w1.contains("/P "));
    let i2psfx = ipb == 1 && (w2.ends_with("/P") || w2.contains("/P "));
    let i3 = if i1psfx || i2psfx { 2 } else { 1 };

    let w_last = parts.last().unwrap();
    let last_upper = w_last.to_uppercase();

    let (igrid4, ir) = if parts.len() == 2 {
        (MAXGRID4 + 1, 0)
    } else if parts.len() == 4 && (!is_grid4(&last_upper) || cq) {
        return None;
    } else if !cq && last_upper == "RR73" {
        (MAXGRID4 + 3, 0)
    } else if is_grid4(&last_upper) {
        let ir = if parts.len() == 4 && parts[2] == "R" {
            1
        } else {
            0
        };
        (pack_grid4(&last_upper), ir)
    } else if cq {
        return None;
    } else if last_upper == "RRR" {
        (MAXGRID4 + 2, 0)
    } else if last_upper == "RR73" {
        (MAXGRID4 + 3, 0)
    } else if last_upper == "73" {
        (MAXGRID4 + 4, 0)
    } else if last_upper.starts_with("R+") || last_upper.starts_with("R-") {
        let rest = &last_upper[1..];
        let irpt = parse_report_i32(rest).filter(|r| (-50..=50).contains(r))?;
        let irpt = if (-50..=-31).contains(&irpt) {
            irpt + 101
        } else {
            irpt
        };
        (MAXGRID4 + (irpt + 35) as usize, 1)
    } else if last_upper.starts_with('+') || last_upper.starts_with('-') {
        let irpt = parse_report_i32(&last_upper).filter(|r| (-50..=50).contains(r))?;
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
    if w == "CQ" || w == "DE" || w == "QRZ" || w.strip_prefix("CQ_").is_some_and(valid_cq_modifier)
    {
        return (w.into(), true, 0);
    }
    if clean_hash_call(w).is_some() {
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
    if clean_hash_call(w).is_some() {
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

    let valid_c11 = |w: &str| (1..=11).contains(&w.len()) && w.bytes().all(|b| C38.contains(&b));
    let parsed_w1 = parse_callsign(w1);
    let parsed_w2 = parse_callsign(w2);

    let standard1 = parsed_w1.is_standard && parsed_w1.suffix.is_none();
    let standard2 = parsed_w2.is_standard && parsed_w2.suffix.is_none();
    if standard1 && standard2 && !w1.starts_with('<') && !w2.starts_with('<') {
        return None;
    }

    let (n12, c11, iflip, icq) = if w1 == "CQ" {
        if w2.len() <= 4 || parts.len() != 2 || !valid_c11(w2) || standard2 {
            return None;
        }
        (ihashcall12(w2), w2.clone(), 0, 1)
    } else if clean_hash_call(w1).is_some() && valid_c11(w2) && !standard2 {
        let inner = &w1[1..w1.len() - 1];
        (ihashcall12(inner), w2.clone(), 0, 0)
    } else if clean_hash_call(w2).is_some() && valid_c11(w1) && !standard1 {
        let inner = &w2[1..w2.len() - 1];
        (ihashcall12(inner), w1.clone(), 1, 0)
    } else {
        return None;
    };
    if w3.is_some_and(|s| !matches!(s.as_str(), "RRR" | "RR73" | "73")) {
        return None;
    }

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
    if !valid_c28_source(call1) || !valid_c28_source(call2) {
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
    if parts.len() != itu + 4 + ir || (mult.is_none() && last.len() != 4) {
        return None;
    }
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

fn valid_c28_source(call: &str) -> bool {
    if !(3..=13).contains(&call.len())
        || !call.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'/')
    {
        return false;
    }
    if !call.contains('/') {
        return callok(call);
    }
    let mut parts = call.split('/');
    !parts.next().unwrap().is_empty()
        && parts.next().is_some_and(|p| !p.is_empty())
        && parts.next().is_none()
}

fn try_pack_type5(parts: &[String]) -> Option<Vec<u8>> {
    if parts.len() < 4 || parts.len() > 5 {
        return None;
    }
    let call1 = clean_hash_call(&parts[0])?;
    let call2 = clean_hash_call(&parts[1])?;
    let exchange = parts[parts.len() - 2].parse::<usize>().ok()?;
    if parts[parts.len() - 2].len() != 6
        || !(520001..=592047).contains(&exchange)
        || (parts.len() == 5 && parts[2] != "R")
    {
        return None;
    }
    let grid6 = parts.last()?;
    let igrid6 = grid6_24(grid6)?;
    let ir = usize::from(parts.len() == 5 && parts[2] == "R");
    let irpt = exchange / 10000 - 52;
    if irpt > 7 {
        return None;
    }
    let iserial = exchange % 10000;
    if !(1..=2047).contains(&iserial) {
        return None;
    }

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
    for qa_bytes_value in qa_bytes.iter().skip(1) {
        let byte = *qa_bytes_value;
        for b in (0..8).rev() {
            bits.push((byte >> b) & 1);
        }
    }
    bits
}

/// Check if a callsign is standard per WSJT-X `stdcall` logic.
pub fn is_stdcall(callsign: &str) -> bool {
    let c = callsign.trim().to_uppercase();
    let bytes = c.as_bytes();
    let n = bytes.len();
    if !(2..=12).contains(&n) {
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
    for bytes_value in bytes.iter().take(iarea as usize) {
        if (*bytes_value).is_ascii_digit() {
            npdig += 1;
        }
        if (*bytes_value).is_ascii_uppercase() {
            nplet += 1;
        }
    }

    // Count letters in suffix
    let mut nslet = 0;
    for bytes_value in bytes.iter().take(n).skip(iarea as usize + 1) {
        if (*bytes_value).is_ascii_uppercase() {
            nslet += 1;
        }
    }

    nplet >= 1 && npdig < iarea && nslet <= 3
}

/// Upstream `stdcall_suffix`: A8 treats a standard call ending in /R or /P
/// as standard, while the original `stdcall` remains unchanged for AP setup.
pub(crate) fn is_stdcall_suffix(callsign: &str) -> bool {
    let call = callsign.trim();
    let base = call.strip_suffix("/R").or_else(|| call.strip_suffix("/P"));
    is_stdcall(if call.len() > 3 {
        base.unwrap_or(call)
    } else {
        call
    })
}

#[cfg(test)]
mod tests {
    use super::is_stdcall;
    use super::HashCallBook;

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
        let msg = crate::decode::packjt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "K1ABC W9XYZ R FN42");
    }

    #[test]
    fn report_tokens_are_not_standard_callsigns_for_split77() {
        assert!(!super::parse_callsign("RR73").is_standard);
        assert!(!super::parse_callsign("73").is_standard);
    }

    #[test]
    fn two_word_type1_accepts_suffix_like_current_wsjtx_grammar() {
        assert!(super::try_pack_type1(&["K1ABC".into(), "W9XYZ/R".into()]).is_some());
    }

    #[test]
    fn split77_directed_cq_uses_wsjtx_chkcall_for_compound_call() {
        assert_eq!(
            super::split77("CQ DX PJ4/KA1ABC FN42"),
            vec!["CQ_DX", "PJ4/KA1ABC", "FN42"]
        );
    }

    #[test]
    fn type01_dxpedition_round_trips_with_hash10_book() {
        let book = HashCallBook::new();
        book.save("R5AF/O");
        let bits = super::pack77("RA3Y RR73; JR1FTJ <R5AF/O> +00");
        let msg = crate::decode::packjt77::unpack77(&bits, Some(&book)).unwrap();
        assert_eq!(msg, "RA3Y RR73; JR1FTJ <R5AF/O> +00");
    }

    #[test]
    fn type03_field_day_round_trips() {
        let bits = super::pack77("WA9XYZ KA1ABC R 16A EMA");
        let msg = crate::decode::packjt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "WA9XYZ KA1ABC R 16A EMA");
    }

    #[test]
    fn type05_telemetry_round_trips() {
        let bits = super::pack77("0123456789ABCDEF01");
        let msg = crate::decode::packjt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "0123456789ABCDEF01");
    }

    #[test]
    fn type3_rtty_round_trips() {
        let bits = super::pack77("TU; W9XYZ K1ABC R 579 MA");
        let msg = crate::decode::packjt77::unpack77(&bits, None).unwrap();
        assert_eq!(msg, "TU; W9XYZ K1ABC R 579 MA");
    }

    #[test]
    fn type5_eu_vhf_round_trips_with_hash_book() {
        let book = HashCallBook::new();
        book.save("K1ABC");
        book.save("G4ABC/P");
        let bits = super::pack77("<K1ABC> <G4ABC/P> R 590003 IO91NP");
        let msg = crate::decode::packjt77::unpack77(&bits, Some(&book)).unwrap();
        assert_eq!(msg, "<K1ABC> <G4ABC/P> R 590003 IO91NP");
    }
}

// ---- receive unpack side of packjt77 ----
#[derive(Clone, Copy, Default)]
pub struct UnpackContext<'a> {
    pub book: Option<&'a HashCallBook>,
    pub mycall: Option<&'a str>,
    pub hiscall: Option<&'a str>,
}

impl<'a> UnpackContext<'a> {
    pub fn new(book: Option<&'a HashCallBook>) -> Self {
        Self {
            book,
            mycall: None,
            hiscall: None,
        }
    }

    pub fn with_calls(
        book: Option<&'a HashCallBook>,
        mycall: Option<&'a str>,
        hiscall: Option<&'a str>,
    ) -> Self {
        Self {
            book,
            mycall: mycall.filter(|call| call.trim().len() >= 3),
            hiscall: hiscall.filter(|call| call.trim().len() >= 3),
        }
    }

    fn mycall_clean(self) -> Option<String> {
        self.mycall.map(clean_context_call)
    }

    fn hiscall_clean(self) -> Option<String> {
        self.hiscall.map(clean_context_call)
    }
}

fn clean_context_call(call: &str) -> String {
    call.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_ascii_uppercase()
}

fn bits_to_uint(bits: &[u8], start: usize, len: usize) -> usize {
    let mut val: usize = 0;
    for i in 0..len {
        val = val * 2 + bits[start + i] as usize;
    }
    val
}

fn callok(call: &str) -> bool {
    let w = call.trim();
    let bytes = w.as_bytes();
    let n = bytes.len();
    if !(3..=6).contains(&n) || bytes.first() == Some(&b'Q') {
        return false;
    }

    let Some(i0) = bytes.iter().rposition(|b| b.is_ascii_digit()) else {
        return false;
    };
    if i0 != 1 && i0 != 2 {
        return false;
    }

    let pfx = &bytes[..i0];
    if !pfx.iter().all(|b| b.is_ascii_alphanumeric())
        || !pfx.iter().any(|b| b.is_ascii_alphabetic())
    {
        return false;
    }

    bytes[i0 + 1..].iter().all(|b| b.is_ascii_alphabetic())
}

fn finish_message(msg: String) -> Option<String> {
    if msg.starts_with("CQ <") {
        None
    } else {
        Some(msg)
    }
}

fn unpack28(n28: usize, context: UnpackContext<'_>) -> Option<String> {
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

    if (1003..=532_443).contains(&n28) {
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

    // Native unpack28 falls through reserved token space to an unresolved
    // hash, rather than aliasing it modulo 27^4 to a directed CQ token.
    if (532_444..N_TOKENS).contains(&n28) {
        return Some("<...>".into());
    }

    if (N_TOKENS..N_TOKENS + MAX22).contains(&n28) {
        let n22 = n28 - N_TOKENS;
        if let Some(book) = context.book {
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

    if call.is_empty() || !callok(&call) || call.contains(' ') {
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

fn lookup_hash10(context: UnpackContext<'_>, n10: usize) -> String {
    if let Some(hiscall) = context.hiscall_clean() {
        if ihashcall(&hiscall, 10) == n10 {
            return format!("<{hiscall}>");
        }
    }
    context
        .book
        .and_then(|book| book.lookup10(n10))
        .map(|call| format!("<{}>", call))
        .unwrap_or_else(|| "<...>".to_string())
}

fn lookup_hash12(context: UnpackContext<'_>, n12: usize) -> String {
    context
        .book
        .and_then(|book| book.lookup12(n12))
        .map(|call| format!("<{}>", call))
        .unwrap_or_else(|| "<...>".to_string())
}

fn lookup_hash22(context: UnpackContext<'_>, n22: usize) -> String {
    context
        .book
        .and_then(|book| book.lookup22(n22))
        .map(|call| format!("<{}>", call))
        .unwrap_or_else(|| "<...>".to_string())
}

fn format_signed_2(value: i32) -> String {
    // Native format(i3.2) fills all three columns for values >=100.
    // Such wire values can occur even though the source grammar limits SNR.
    if (100..=999).contains(&value) {
        value.to_string()
    } else if (-99..=99).contains(&value) {
        format!("{value:+03}")
    } else {
        "***".into()
    }
}

fn unpack_text77(bits71: &[u8]) -> String {
    // Reconstruct 9 bytes from 71 bits
    // First 7 bits go into byte 0 (bits 0-6, i.e., low 7 bits)
    // Remaining 64 bits go into bytes 1-8
    let mut qa_bytes = [0u8; 9];

    // First 7 bits → byte 0 (MSB first, so bits 0-6 become the byte value)
    let mut val: u8 = 0;
    for bits71_value in bits71.iter().take(7) {
        val = (val << 1) | *bits71_value;
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
    unpack77_with_context(bits77, UnpackContext::new(book))
}

pub fn unpack77_with_context(bits77: &[u8], context: UnpackContext<'_>) -> Option<String> {
    if bits77.len() != 77 || bits77.iter().any(|&b| b > 1) {
        return None;
    }
    let mut staged_calls = Vec::new();
    let decoded = unpack77_core(bits77, context, &mut staged_calls)?;
    // Upstream 50f6cc83 stages hash effects until the entire payload is
    // accepted. A rejected CQ/report or malformed contest frame learns no call.
    if let Some(book) = context.book {
        for call in staged_calls {
            book.save(&call);
        }
    }
    Some(decoded)
}

fn unpack77_core(
    bits77: &[u8],
    context: UnpackContext<'_>,
    staged_calls: &mut Vec<String>,
) -> Option<String> {
    let n3 = bits_to_uint(bits77, 71, 3);
    let i3 = bits_to_uint(bits77, 74, 3);

    if i3 == 0 && n3 == 0 {
        let msg = unpack_text77(&bits77[..71]);
        if msg.trim().is_empty() {
            return None;
        }
        return finish_message(msg.trim().into());
    }

    if i3 == 0 && n3 == 1 {
        let n28a = bits_to_uint(bits77, 0, 28);
        let n28b = bits_to_uint(bits77, 28, 28);
        let n10 = bits_to_uint(bits77, 56, 10);
        let n5 = bits_to_uint(bits77, 66, 5);
        if n28a <= 2 || n28b <= 2 {
            return None;
        }
        let call1 = unpack28(n28a, context)?;
        let call2 = unpack28(n28b, context)?;
        let call3 = lookup_hash10(context, n10);
        let report = format_signed_2(2 * n5 as i32 - 30);
        return finish_message(format!("{call1} RR73; {call2} {call3} {report}"));
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
        let call1 = unpack28(n28a, context)?;
        let call2 = unpack28(n28b, context)?;
        let ntx = intx + 1 + if n3 == 4 { 16 } else { 0 };
        let class = (b'A' + nclass as u8) as char;
        let sec = CSEC[isec - 1];
        let exchange = format!("{ntx}{class}");
        let msg = if ir == 0 {
            format!("{call1} {call2} {exchange} {sec}")
        } else {
            format!("{call1} {call2} R {exchange} {sec}")
        };
        return finish_message(msg);
    }

    if i3 == 0 && n3 == 5 {
        let n1 = bits_to_uint(bits77, 0, 23);
        let n2 = bits_to_uint(bits77, 23, 24);
        let n3tel = bits_to_uint(bits77, 47, 24);
        let msg = format!("{n1:06X}{n2:06X}{n3tel:06X}");
        return finish_message(msg);
    }

    if i3 == 0 && n3 == 6 {
        return None;
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

        let mut call1 = unpack28(n28a, context)?;
        if let Some(mycall) = context.mycall_clean() {
            if n28a >= N_TOKENS && ihashcall(&mycall, 22) == n28a - N_TOKENS {
                call1 = format!("<{mycall}>");
            }
        }
        let call2_raw = unpack28(n28b, context)?;

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
            if c2.len() >= 3 {
                staged_calls.push(c2.clone());
            }
        }

        if igrid4 <= MAXGRID4 {
            let grid = to_grid4(igrid4)?;
            if ir == 0 {
                finish_message(format!("{} {} {}", c1, c2, grid))
            } else {
                if is_cq_head(&c1) {
                    None
                } else {
                    finish_message(format!("{} {} R {}", c1, c2, grid))
                }
            }
        } else {
            let irpt = igrid4 - MAXGRID4;
            if is_cq_head(&c1) && irpt >= 2 {
                return None;
            }
            match irpt {
                1 => finish_message(format!("{} {}", c1, c2)),
                2 => finish_message(format!("{} {} RRR", c1, c2)),
                3 => finish_message(format!("{} {} RR73", c1, c2)),
                4 => finish_message(format!("{} {} 73", c1, c2)),
                _ if irpt >= 5 => {
                    let mut isnr = irpt as i32 - 35;
                    if isnr > 50 {
                        isnr -= 101;
                    }
                    let report = format_signed_2(isnr);
                    if ir == 0 {
                        finish_message(format!("{} {} {}", c1, c2, report))
                    } else {
                        finish_message(format!("{} {} R{}", c1, c2, report))
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

        let call1 = unpack28(n28a, context)?;
        let call2 = unpack28(n28b, context)?;
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
        let rendered = format!("{prefix}{call1} {call2}{r} {report} {exchange}");
        (rendered.len() <= 37)
            .then(|| finish_message(rendered))
            .flatten()
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

        let mut call3 = if let Some(book) = context.book {
            if let Some(resolved) = book.lookup12(n12) {
                format!("<{}>", resolved)
            } else {
                "<...>".into()
            }
        } else {
            "<...>".into()
        };

        let (call1, call2) = if iflip == 0 {
            staged_calls.push(c11.clone());
            if let Some(mycall) = context.mycall_clean() {
                if let Some(hiscall) = context.hiscall_clean() {
                    if c11 == hiscall && ihashcall(&mycall, 12) == n12 {
                        call3 = format!("<{mycall}>");
                    }
                }
                if call3 == "<...>" && ihashcall(&mycall, 12) == n12 {
                    call3 = format!("<{mycall}>");
                }
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
        finish_message(msg)
    } else if i3 == 5 {
        let n12 = bits_to_uint(bits77, 0, 12);
        let n22 = bits_to_uint(bits77, 12, 22);
        let ir = bits77[34] as usize;
        let irpt = bits_to_uint(bits77, 35, 3);
        let iserial = bits_to_uint(bits77, 38, 11);
        let igrid6 = bits_to_uint(bits77, 49, 25);
        if igrid6 > 18_662_399 || iserial == 0 {
            return None;
        }
        let mut call1 = lookup_hash12(context, n12);
        if let Some(mycall) = context.mycall_clean() {
            if ihashcall(&mycall, 12) == n12 {
                call1 = format!("<{mycall}>");
            }
        }
        let call2 = lookup_hash22(context, n22);
        let exchange = format!("{}{:04}", 52 + irpt, iserial);
        let grid6 = to_grid6_24(igrid6)?;
        if ir == 0 {
            finish_message(format!("{call1} {call2} {exchange} {grid6}"))
        } else {
            finish_message(format!("{call1} {call2} R {exchange} {grid6}"))
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
mod unpack_tests {
    #[test]
    fn original_test28_calls_and_integer_boundaries() {
        crate::decoder_protocol_tests::assert_original_pack28_vectors(
            "wsjtx",
            super::pack28,
            |number, seed| {
                let book = super::HashCallBook::new();
                if let Some(call) = seed {
                    book.save(call);
                }
                super::unpack28(number, super::UnpackContext::new(Some(&book)))
            },
        );
    }

    use super::unpack77;
    use super::HashCallBook;
    use super::{pack28, pack77, C38};

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
        // The current source encoder rejects this grammar. Construct the
        // invalid wire payload so the receive-side gate remains exercised.
        let mut bits = pack77("CQ K1ABC FN42");
        bits[58] = 1;
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn cq_ack_report_is_rejected_like_wsjtx() {
        let mut bits = pack77("CQ K1ABC FN42");
        let value = super::MAXGRID4 + 2;
        for i in 0..15 {
            bits[59 + i] = ((value >> (14 - i)) & 1) as u8;
        }
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn cq_unresolved_hash_is_rejected_like_wsjtx() {
        let mut bits = pack77("CQ K1ABC FN42");
        let value = super::N_TOKENS + ihashcall("NOHASH", 22);
        for i in 0..28 {
            bits[29 + i] = ((value >> (27 - i)) & 1) as u8;
        }
        assert!(unpack77(&bits, None).is_none());
    }

    #[test]
    fn unpack28_rejects_invalid_standard_call_like_wsjtx_callok() {
        assert!(super::unpack28(2_063_592 + 4_194_304, super::UnpackContext::default()).is_none());
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
    fn type01_uses_hiscall_hash10_like_wsjtx_receive_unpack() {
        let mut bits = Vec::new();
        append_bits(&mut bits, pack28("RA3Y"), 28);
        append_bits(&mut bits, pack28("JR1FTJ"), 28);
        append_bits(&mut bits, ihashcall("R5AF/O", 10), 10);
        append_bits(&mut bits, 15, 5);
        append_bits(&mut bits, 1, 3);
        append_bits(&mut bits, 0, 3);
        let context = super::UnpackContext::with_calls(None, None, Some("R5AF/O"));
        assert_eq!(
            super::unpack77_with_context(&bits, context).unwrap(),
            "RA3Y RR73; JR1FTJ <R5AF/O> +00"
        );
    }

    #[test]
    fn type1_uses_mycall_hash22_like_wsjtx_receive_unpack() {
        let mut bits = Vec::new();
        append_bits(&mut bits, super::N_TOKENS + ihashcall("K1ABC", 22), 28);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, pack28("W9XYZ"), 28);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, super::MAXGRID4 + 1, 15);
        append_bits(&mut bits, 1, 3);
        let context = super::UnpackContext::with_calls(None, Some("K1ABC"), None);
        assert_eq!(
            super::unpack77_with_context(&bits, context).unwrap(),
            "<K1ABC> W9XYZ"
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
        assert_eq!(unpack77(&bits, None).unwrap(), "0123456789ABCDEF01");
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
    fn type_3_rtty_special_token_matches_current_native_wire_decoder() {
        let mut bits = Vec::new();
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, pack28("CQ_001"), 28);
        append_bits(&mut bits, pack28("IZ7MMG"), 28);
        append_bits(&mut bits, 0, 1);
        append_bits(&mut bits, 3, 3);
        append_bits(&mut bits, 2025, 13);
        append_bits(&mut bits, 3, 3);

        assert_eq!(bits.len(), 77);
        assert_eq!(
            unpack77(&bits, None).as_deref(),
            Some("CQ 001 IZ7MMG 559 2025")
        );
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

    #[test]
    fn type5_uses_mycall_hash12_like_wsjtx_receive_unpack() {
        let mut bits = Vec::new();
        append_bits(&mut bits, ihashcall("K1ABC", 12), 12);
        append_bits(&mut bits, ihashcall("G4ABC/P", 22), 22);
        append_bits(&mut bits, 1, 1);
        append_bits(&mut bits, 7, 3);
        append_bits(&mut bits, 3, 11);
        append_bits(&mut bits, grid6_24("IO91NP"), 25);
        append_bits(&mut bits, 5, 3);
        let context = super::UnpackContext::with_calls(None, Some("K1ABC"), None);
        assert_eq!(
            super::unpack77_with_context(&bits, context).unwrap(),
            "<K1ABC> <...> R 590003 IO91NP"
        );
    }
}
