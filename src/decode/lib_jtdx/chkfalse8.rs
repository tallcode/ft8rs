//! Mirrors JTDX `lib/chkfalse8.f90`.

use super::callsign_q::callsign_q_reject;
use super::chkflscall::chkflscall;
use super::chkgrid::{chkgrid, is_grid4};
use super::chkspecial8::chkspecial8;

#[derive(Clone, Debug, Default)]
pub(crate) struct FilterContext {
    pub(crate) mycall: String,
    pub(crate) hiscall: String,
    pub(crate) hisgrid4: String,
    pub(crate) quality: f32,
    pub(crate) xsnr: f32,
    pub(crate) rxdt: f32,
}

pub(crate) fn accept_decoded_message(
    msg37: &str,
    msg37_2: &str,
    i3: usize,
    n3: usize,
    iaptype: i32,
    lcall1hash: bool,
    context: &FilterContext,
) -> bool {
    if i3 == 0 && n3 == 1 {
        return chkspecial8(msg37, msg37_2, &context.mycall, &context.hiscall);
    }
    chkfalse8(msg37, i3, n3, iaptype, lcall1hash, context)
}

pub(crate) fn chkfalse8(
    msg37: &str,
    i3: usize,
    n3: usize,
    iaptype: i32,
    lcall1hash: bool,
    context: &FilterContext,
) -> bool {
    let msg = msg37.trim();
    let words: Vec<&str> = msg.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }

    if violates_protocol_shape(msg, &words, i3, n3, iaptype) {
        return false;
    }

    if iaptype == 1 && msg.starts_with("CQ DE AA00") {
        return false;
    }

    let primary_false_check = context.quality < 0.39
        || context.xsnr < -20.5
        || context.rxdt < -0.5
        || context.rxdt > 1.9
        || (1..4).contains(&iaptype)
        || matches!(iaptype, 11 | 21 | 40 | 41);

    if words[0] == "CQ" {
        return accept_cq(msg, &words, i3, n3, iaptype);
    }

    if iaptype == 2 || iaptype == 40 {
        return accept_second_call_and_optional_grid(&words);
    }

    if (35..40).contains(&iaptype)
        && words.first().is_some_and(|call| {
            call.ends_with("/R") && chkflscall("CQ", strip_portable_suffix(call))
        })
    {
        return false;
    }

    if (35..40).contains(&iaptype)
        && (context.xsnr < -21.0 || context.rxdt < -0.5 || context.rxdt > 1.0)
    {
        let call = strip_portable_suffix(words[0]);
        if chkflscall("CQ", call) {
            return false;
        }
    }

    if primary_false_check && matches!(iaptype, 3 | 11 | 21 | 41) {
        if iaptype == 21 && msg.contains(" R ") {
            return false;
        }
        return accept_optional_report_grid(&words);
    }

    if i3 == 1 || i3 == 2 {
        if msg.contains(" R ") && words.len() >= 4 && words[2] == "R" && !is_grid4(words[3]) {
            return false;
        }
        if words.len() >= 3 && is_grid4(words[2]) {
            let check = chkgrid(words[0], words[2]);
            if check.lwrongcall || !check.lgvalid {
                return false;
            }
        }
    }

    if iaptype == 0 && i3 == 1 && n3 == 0 && msg.contains("/R ") {
        if rejects_standard_r_portable_message(&words) {
            return false;
        }
    }

    if msg.starts_with("<...>")
        && i3 == 1
        && n3 == 1
        && (context.xsnr < -18.0 || context.rxdt < -0.5 || context.rxdt > 1.0)
        && rejects_hash_call_grid_message(&words)
    {
        return false;
    }

    if i3 == 0 && (3..5).contains(&n3) && !accept_arrl_field_day_shape(msg, &words) {
        return false;
    }

    if i3 == 4 && words.len() >= 2 {
        let first_call = words[0].trim_matches(['<', '>']);
        let second_call = words[1].trim_matches(['<', '>']);
        if !lcall1hash && callsign_q_reject(first_call) {
            return false;
        }
        if callsign_q_reject(second_call) {
            return false;
        }
    }

    if !context.hisgrid4.is_empty() && words.iter().any(|w| *w == context.hisgrid4) {
        return true;
    }

    true
}

fn violates_protocol_shape(msg: &str, words: &[&str], i3: usize, n3: usize, iaptype: i32) -> bool {
    if i3 == 2 && !msg.contains("/P ") {
        return true;
    }

    if msg.starts_with("<...>")
        && words
            .get(3)
            .is_some_and(|word| matches!(*word, "RRR" | "RR73" | "73"))
    {
        return true;
    }

    if iaptype == 2 {
        if let Some(hash_pos) = words.iter().position(|word| *word == "<...>") {
            if (1..=2).contains(&hash_pos) && words.get(hash_pos + 1).is_some_and(|w| is_grid4(w)) {
                return true;
            }
        }
    }

    if i3 == 4 && n3 == 0 && words.iter().any(|word| word.ends_with("/R")) {
        if words.iter().any(|word| is_grid4(word)) {
            return true;
        }
    }

    if i3 == 1 && words.len() >= 3 && words[1] == "<...>" && is_grid4(words[2]) {
        let call = strip_portable_suffix(words[0]);
        if chkflscall("CQ", call) {
            return true;
        }
    }

    false
}

fn accept_cq(msg: &str, words: &[&str], i3: usize, n3: usize, iaptype: i32) -> bool {
    if words.len() < 2 {
        return false;
    }

    if i3 == 4 {
        let callsign = if let Some(pos) = msg.find('/') {
            let compact = msg[3..].trim();
            let (left, right) = compact.split_at(pos.saturating_sub(3));
            let right = right.trim_start_matches('/');
            if left.len() <= right.len() {
                right.split_whitespace().next().unwrap_or("")
            } else {
                left.trim()
            }
        } else {
            words[1]
        };
        if callsign.is_empty() || callsign_q_reject(callsign) {
            return false;
        }
        if words.len() == 2 && looks_like_standard_6_call(callsign) {
            return false;
        }
        return !chkflscall("CQ", callsign);
    }

    if iaptype == 1 && words.len() >= 3 {
        let call = strip_portable_suffix(words[1]);
        if callsign_q_reject(call) {
            return false;
        }
        if is_grid4(words[2]) {
            let check = chkgrid(call, words[2]);
            if check.lwrongcall || !check.lgvalid {
                return false;
            }
        }
    }

    if words.len() >= 4
        && words[1].chars().all(|c| c.is_ascii_digit())
        && matches!(i3, 1 | 2)
        && n3 == 5
    {
        let call = strip_portable_suffix(words[2]);
        if callsign_q_reject(call) || chkflscall("MYCALL", call) {
            return false;
        }
        return is_grid4(words[3]);
    }

    if i3 == 1 && iaptype == 0 && words.len() == 3 && is_grid4(words[2]) {
        let call = strip_portable_suffix(words[1]);
        let check = chkgrid(call, words[2]);
        if check.lwrongcall || !check.lgvalid {
            return false;
        }
    }

    true
}

fn accept_second_call_and_optional_grid(words: &[&str]) -> bool {
    if words.len() < 2 {
        return false;
    }
    let call = strip_portable_suffix(words[1]);
    if callsign_q_reject(call) {
        return false;
    }
    if words.len() >= 3 && is_grid4(words[2]) {
        let check = chkgrid(call, words[2]);
        if check.lwrongcall || !check.lgvalid {
            return false;
        }
    }
    true
}

fn accept_optional_report_grid(words: &[&str]) -> bool {
    if words.len() < 3 {
        return true;
    }
    if words[2] == "R" {
        if words.len() >= 4 {
            return is_grid4(words[3]);
        }
        return false;
    }
    if is_grid4(words[2]) {
        let check = chkgrid(words[1], words[2]);
        return check.lgvalid && !check.lwrongcall;
    }
    true
}

fn accept_arrl_field_day_shape(msg: &str, words: &[&str]) -> bool {
    if words.len() < 4 {
        return true;
    }

    let call_a = strip_portable_suffix(words[0]);
    let call_b = strip_portable_suffix(words[1]);
    if call_a.len() < 3 || call_b.len() < 3 {
        return true;
    }

    if !field_day_call_region_can_use_section(call_b) && !msg.contains(" DX ") {
        return false;
    }

    !chkflscall(call_a, call_b)
}

fn rejects_standard_r_portable_message(words: &[&str]) -> bool {
    if words.len() < 2 {
        return false;
    }
    let call_a = strip_portable_suffix(words[0]);
    let call_b = strip_portable_suffix(words[1]);
    call_a.len() > 2 && call_b.len() > 2 && chkflscall(call_a, call_b)
}

fn rejects_hash_call_grid_message(words: &[&str]) -> bool {
    if words.len() < 3 || words[0] != "<...>" || !is_grid4(words[2]) {
        return false;
    }
    let callsign = strip_portable_suffix(words[1]);
    let check = chkgrid(callsign, words[2]);
    if check.lwrongcall {
        return true;
    }
    (check.lchkcall || !check.lgvalid) && chkflscall("CQ", callsign)
}

fn field_day_call_region_can_use_section(call: &str) -> bool {
    let call = call.trim().to_ascii_uppercase();
    let bytes = call.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    match bytes[0] as char {
        'K' | 'N' | 'W' => true,
        'A' => (b'A'..=b'L').contains(&bytes[1]),
        'C' => (b'F'..=b'K').contains(&bytes[1]) || matches!(bytes[1], b'Y' | b'Z'),
        'V' => (b'A'..=b'G').contains(&bytes[1]) || matches!(bytes[1], b'O' | b'X' | b'Y'),
        'X' => (b'J'..=b'O').contains(&bytes[1]),
        _ => false,
    }
}

fn strip_portable_suffix(call: &str) -> &str {
    call.strip_suffix("/R")
        .or_else(|| call.strip_suffix("/P"))
        .unwrap_or(call)
}

fn looks_like_standard_6_call(call: &str) -> bool {
    let bytes = call.as_bytes();
    if bytes.len() != 6 || call.contains('/') {
        return false;
    }
    let mut mask = [b'0'; 6];
    for (idx, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_digit() {
            mask[idx] = b'1';
        }
    }
    matches!(&mask, b"001000" | b"101000" | b"011000")
}
