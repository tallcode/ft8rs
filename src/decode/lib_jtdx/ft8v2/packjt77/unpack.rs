// ---- receive unpack side of packjt77 ----
use super::*;

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
    if n < 3 || bytes.first() == Some(&b'Q') {
        return false;
    }

    let Some(i0) = bytes.iter().rposition(|b| b.is_ascii_digit()) else {
        return false;
    };
    if i0 != 1 && i0 != 2 {
        return false;
    }

    let pfx = &bytes[..i0];
    if !pfx.iter().any(|b| b.is_ascii_alphabetic()) {
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

pub(super) fn unpack28(n28: usize, context: UnpackContext<'_>) -> Option<String> {
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
    unpack77_with_context(bits77, UnpackContext::new(book))
}

pub fn unpack77_with_context(bits77: &[u8], context: UnpackContext<'_>) -> Option<String> {
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
        } else if ntx < 10 {
            format!("{call1} {call2} R{exchange} {sec}")
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
        return finish_message(msg.trim_start_matches('0').to_string());
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
            if let Some(book) = context.book {
                if c2.len() >= 3 {
                    book.save(&c2);
                }
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
                    let sign = if isnr >= 0 { '+' } else { '-' };
                    let abs_str = format!("{:02}", isnr.abs());
                    if ir == 0 {
                        finish_message(format!("{} {} {}{}", c1, c2, sign, abs_str))
                    } else {
                        finish_message(format!("{} {} R{}{}", c1, c2, sign, abs_str))
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
        if !is_rtty_call_token(&call1) || !is_rtty_call_token(&call2) {
            return None;
        }
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
        finish_message(format!("{prefix}{call1} {call2}{r} {report} {exchange}"))
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
            if let Some(book) = context.book {
                book.save(&c11);
            }
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
        if igrid6 > 18_662_399 {
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

fn is_rtty_call_token(call: &str) -> bool {
    let c = call.trim();
    !(c == "DE" || c == "QRZ" || is_cq_head(c))
}
