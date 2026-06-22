//! Mirrors JTDX `lib/chkfalse8.f90`.

use super::call_q::call_q_pair_reject;
use super::callsign_q::callsign_q_reject;
use super::chkflscall::chkflscall;
use super::chkgrid::{chkgrid, is_grid4};
use super::chklong8::chklong8;
use super::chkspecial8::chkspecial8;
use super::filtersfree::filtersfree;

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
    lcall2hash: bool,
    context: &FilterContext,
) -> bool {
    chkfalse8(
        msg37, msg37_2, i3, n3, iaptype, lcall1hash, lcall2hash, context,
    )
}

pub(crate) fn chkfalse8(
    msg37: &str,
    msg37_2: &str,
    i3: usize,
    n3: usize,
    iaptype: i32,
    lcall1hash: bool,
    lcall2hash: bool,
    context: &FilterContext,
) -> bool {
    let msg = msg37.trim();
    let words: Vec<&str> = msg.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }

    let primary_false_check = context.quality < 0.39
        || context.xsnr < -20.5
        || context.rxdt < -0.5
        || context.rxdt > 1.9
        || (1..4).contains(&iaptype)
        || matches!(iaptype, 11 | 21 | 40 | 41);

    if rejects_post_label_protocol_shape(msg, &words, i3, n3, iaptype) {
        return false;
    }

    let source_skips_pre_label_filters = matches!(iaptype, 2 | 3 | 11 | 21 | 40 | 41)
        || (primary_false_check && matches!(iaptype, 4..=6));
    if !source_skips_pre_label_filters && rejects_pre_label_protocol_shape(msg, &words, i3, n3) {
        return false;
    }

    if primary_false_check && i3 == 0 && (msg.starts_with("CQ_") || msg.contains('^')) {
        return false;
    }

    if iaptype == 1 && msg.starts_with("CQ DE AA00") {
        return false;
    }

    if primary_false_check && matches!(iaptype, 4..=6) {
        return true;
    }

    if primary_false_check && i3 == 0 && n3 == 1 {
        return chkspecial8(msg, msg37_2, &context.mycall, &context.hiscall);
    }

    if words[0] == "CQ" {
        return if primary_false_check {
            accept_cq(msg, &words, i3, n3, iaptype)
        } else {
            accept_late_cq_grid_check(&words, i3, iaptype)
        };
    }

    if iaptype == 2 {
        return accept_ap_type2_message(&words);
    }

    if iaptype == 40 {
        return accept_ap_type40_message(&words);
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
        if rejects_ap_qso_grid_message(&words, iaptype, context) {
            return false;
        }
        return true;
    }

    if rejects_r_grid_message(&words, i3, lcall2hash) {
        return false;
    }

    if iaptype == 0 && i3 == 1 && n3 == 0 && msg.contains("/R ") {
        if rejects_standard_r_portable_message(&words) {
            return false;
        }
    }

    if rejects_r_or_portable_call_pair(msg, &words, i3, context) {
        return false;
    }

    if primary_false_check && rejects_first_hash_grid_message(&words, i3, lcall1hash, context) {
        return false;
    }

    if msg.starts_with("<...>")
        && i3 == 1
        && n3 == 1
        && (context.xsnr < -18.0 || context.rxdt < -0.5 || context.rxdt > 1.0)
        && rejects_hash_call_grid_message(&words)
    {
        return false;
    }

    if iaptype != 2
        && (((i3 == 1 || i3 == 3) && !msg.contains(" R ") && !msg.contains('/'))
            || (i3 == 0 && n3 == 3))
        && primary_false_check
        && words.len() >= 2
        && words[0] != context.mycall
        && !words[0].starts_with("<.")
        && (call_q_pair_reject(words[0], words[1]) || chkflscall(words[0], words[1]))
    {
        return false;
    }

    if i3 == 0 && (3..5).contains(&n3) && !accept_arrl_field_day_shape(msg, &words, context) {
        return false;
    }

    if primary_false_check && i3 == 4 && words.len() >= 2 {
        if rejects_i3_4_hash_call_shape(msg) {
            return false;
        }
        let first_call = words[0].trim_matches(['<', '>']);
        let second_call = words[1].trim_matches(['<', '>']);
        if !lcall1hash && (callsign_q_reject(first_call) || chklong8(first_call)) {
            return false;
        }
        if callsign_q_reject(second_call) || chklong8(second_call) {
            return false;
        }
    }

    if primary_false_check && i3 == 3 && msg.starts_with("TU;") && rejects_tu_message(&words) {
        return false;
    }

    if primary_false_check && i3 == 0 && n3 == 0 {
        if msg.contains("/.") || filtersfree(&msg[..msg.len().min(22)]) {
            return false;
        }
    }

    if !context.hisgrid4.is_empty() && words.iter().any(|w| *w == context.hisgrid4) {
        return true;
    }

    true
}

fn rejects_post_label_protocol_shape(
    msg: &str,
    words: &[&str],
    i3: usize,
    n3: usize,
    iaptype: i32,
) -> bool {
    if i3 == 2 && !msg.contains("/P ") {
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

    false
}

fn rejects_pre_label_protocol_shape(msg: &str, words: &[&str], i3: usize, _n3: usize) -> bool {
    if msg.starts_with("<...>")
        && words
            .get(3)
            .is_some_and(|word| matches!(*word, "RRR" | "RR73" | "73"))
    {
        return true;
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
        let cq_body = msg.strip_prefix("CQ ").unwrap_or("").trim();
        let slash = msg.find('/');
        let callsign = if let Some(pos) = slash {
            if pos > 3 && words.get(1).is_some_and(|word| pos < 3 + word.len()) {
                let compact = cq_body;
                let rel_slash = pos.saturating_sub(3);
                let (left, right) = compact.split_at(rel_slash);
                let right = right.trim_start_matches('/');
                if left.len() <= right.len() {
                    right.split_whitespace().next().unwrap_or("")
                } else {
                    left.trim()
                }
            } else {
                ""
            }
        } else {
            if cq_body.is_empty()
                || cq_body.contains(' ')
                || cq_body.as_bytes().last().is_some_and(u8::is_ascii_digit)
            {
                return false;
            }
            cq_body
        };
        if callsign.is_empty() || callsign_q_reject(callsign) {
            return false;
        }
        if callsign.len() >= 10 && chklong8(callsign) {
            return false;
        }
        if rejects_single_slash_11_call(callsign) {
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

fn accept_late_cq_grid_check(words: &[&str], i3: usize, iaptype: i32) -> bool {
    if iaptype != 0 || i3 != 1 || words.len() < 4 {
        return true;
    }
    if !(words[1].len() == 3 || words[1].len() == 4) || words[2].len() <= 3 {
        return true;
    }
    let grid = words[3];
    if is_grid4(grid) {
        let check = chkgrid(words[2], grid);
        return check.lgvalid && !check.lwrongcall;
    }
    true
}

fn rejects_ap_qso_grid_message(words: &[&str], iaptype: i32, context: &FilterContext) -> bool {
    if words.len() < 3 {
        return false;
    }
    let mycall = context.mycall.trim();
    let hiscall = context.hiscall.trim();
    if mycall.is_empty() || hiscall.is_empty() || words[0] != mycall || words[1] != hiscall {
        return false;
    }

    let grid = if words.get(2) == Some(&"R") {
        words.get(3).copied().unwrap_or("")
    } else {
        words[2]
    };
    if !is_grid4(grid) || grid == context.hisgrid4 {
        return false;
    }
    if iaptype == 21 {
        return grid != "RR73";
    }
    !chkgrid(hiscall, grid).lgvalid
}

fn rejects_r_grid_message(words: &[&str], i3: usize, lcall2hash: bool) -> bool {
    if !(i3 == 1 || i3 == 2) || words.len() < 4 || words[2] != "R" {
        return false;
    }

    let grid = words[3];
    if !is_grid4(grid) {
        return false;
    }

    if lcall2hash {
        let call_a = strip_portable_suffix(words[0].trim_matches(['<', '>']));
        return call_a.len() > 2 && chkflscall("CQ", call_a);
    }

    let call_b = strip_portable_suffix(words[1].trim_matches(['<', '>']));
    if call_b.len() <= 2 {
        return false;
    }
    let check = chkgrid(call_b, grid);
    check.lwrongcall || !check.lgvalid
}

fn rejects_tu_message(words: &[&str]) -> bool {
    if words.len() < 3 {
        return false;
    }
    let call_a = words[1];
    let call_b = words[2];
    call_q_pair_reject(call_a, call_b) || chkflscall(call_a, call_b)
}

fn rejects_i3_4_hash_call_shape(msg: &str) -> bool {
    if let Some(hash_pos) = msg.find("<.") {
        if hash_pos > 3 {
            let callsign = msg[..hash_pos].trim_end();
            if callsign.is_empty() {
                return false;
            }
            if callsign.contains(' ')
                || (!callsign.contains('/')
                    && callsign.as_bytes().last().is_some_and(u8::is_ascii_digit))
            {
                return true;
            }
            if callsign_q_reject(callsign) || chklong8(callsign) {
                return true;
            }
        }
    }

    if msg.starts_with("<.") || msg.find(".>").is_some_and(|pos| pos > 3) {
        let callsign = if msg.starts_with("<.") {
            strip_hash_leading_call(msg)
        } else if let Some(hash_pos) = msg.find("<.") {
            msg[..hash_pos].trim_end()
        } else {
            ""
        };
        if callsign.is_empty() {
            return false;
        }
        if callsign.contains(' ') {
            return true;
        }
        let islash = callsign.find('/');
        if (islash.is_none() && callsign.as_bytes().last().is_some_and(u8::is_ascii_digit))
            || islash == Some(callsign.len().saturating_sub(1))
        {
            return true;
        }
        if callsign.len() == 11
            && islash.is_some()
            && !callsign[islash.unwrap() + 1..].contains('/')
        {
            if rejects_single_slash_11_call(callsign) {
                return true;
            }
        }
        return callsign_q_reject(callsign) || chklong8(callsign);
    }

    false
}

fn strip_hash_leading_call(msg: &str) -> &str {
    let rest = msg.trim_start_matches("<...>").trim_start();
    for suffix in [" RR73", " RRR", " 73"] {
        if let Some(stripped) = rest.strip_suffix(suffix) {
            return stripped.trim_end();
        }
    }
    rest.trim_end()
}

fn rejects_single_slash_11_call(callsign: &str) -> bool {
    let Some(islash) = callsign.find('/') else {
        return false;
    };
    if callsign[islash + 1..].contains('/') || callsign.len() != 11 {
        return false;
    }
    let bytes = callsign.as_bytes();
    if islash < 6
        && ((bytes.get(islash + 1).is_some_and(u8::is_ascii_digit)
            && bytes.get(islash + 2).is_some_and(u8::is_ascii_digit))
            || bytes.get(islash + 1) == Some(&b'Q')
            || bytes.last().is_some_and(u8::is_ascii_digit))
    {
        return true;
    }
    if islash > 4
        && (bytes
            .get(islash.saturating_sub(1))
            .is_some_and(u8::is_ascii_digit)
            || (bytes.first().is_some_and(u8::is_ascii_digit)
                && bytes.get(1).is_some_and(u8::is_ascii_digit))
            || bytes.first() == Some(&b'Q'))
    {
        return true;
    }
    if (4..8).contains(&islash) {
        let call_a = &callsign[..islash];
        let call_b = &callsign[islash + 1..];
        return chkflscall(call_a, call_b);
    }
    false
}

fn accept_ap_type2_message(words: &[&str]) -> bool {
    if words.len() < 2 {
        return false;
    }
    let call = strip_portable_suffix(words[1]);
    if callsign_q_reject(call) {
        return false;
    }

    if let Some(r_pos) = words.iter().position(|word| *word == "R") {
        if let Some(grid) = words.get(r_pos + 1) {
            if grid.len() == 4 && !is_grid4(grid) {
                return false;
            }
            if is_grid4(grid) {
                let check = chkgrid(call, grid);
                if check.lwrongcall || !check.lgvalid {
                    return false;
                }
            }
        }
    } else if let Some(grid) = words.get(2) {
        if grid.len() == 4 && !is_grid4(grid) {
            return false;
        }
        if is_grid4(grid) {
            let check = chkgrid(call, grid);
            if check.lwrongcall || !check.lgvalid {
                return false;
            }
        }
    }

    true
}

fn accept_ap_type40_message(words: &[&str]) -> bool {
    if words.len() < 2 {
        return false;
    }
    let call = words[1];
    if callsign_q_reject(call) {
        return false;
    }

    let grid = words
        .iter()
        .position(|word| *word == "R")
        .and_then(|r_pos| words.get(r_pos + 1))
        .or_else(|| words.get(2));
    if let Some(grid) = grid {
        if is_grid4(grid) {
            let check = chkgrid(call, grid);
            if check.lwrongcall || !check.lgvalid {
                return false;
            }
        }
    }
    true
}

fn accept_arrl_field_day_shape(msg: &str, words: &[&str], context: &FilterContext) -> bool {
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

    if context.xsnr < -19.0 || context.rxdt < -0.5 || context.rxdt > 1.0 {
        return !(call_q_pair_reject(call_a, call_b) || chkflscall(call_a, call_b));
    }

    true
}

fn rejects_standard_r_portable_message(words: &[&str]) -> bool {
    if words.len() < 2 {
        return false;
    }
    let call_a = strip_portable_suffix(words[0]);
    let call_b = strip_portable_suffix(words[1]);
    call_a.len() > 2 && call_b.len() > 2 && chkflscall(call_a, call_b)
}

fn rejects_r_or_portable_call_pair(
    msg: &str,
    words: &[&str],
    i3: usize,
    context: &FilterContext,
) -> bool {
    if !(1..=3).contains(&i3)
        || words.len() < 2
        || !(msg.contains(" R ") || msg.contains("/R ") || msg.contains("/P "))
    {
        return false;
    }

    let first = words[0];
    let second = words[1];
    if !msg.contains('/') {
        if second == context.hiscall.trim() {
            return false;
        }
        return call_q_pair_reject(first, second) || chkflscall(first, second);
    }

    let first_slash = first.find('/');
    let second_slash = second.find('/');
    let (call_a, call_b) = if first_slash.is_none() {
        (first, second_slash.map_or(second, |idx| &second[..idx]))
    } else {
        (
            first_slash.map_or(first, |idx| &first[..idx]),
            second_slash.map_or(second, |idx| &second[..idx]),
        )
    };

    if call_a == context.mycall.trim() || call_b == context.hiscall.trim() {
        return false;
    }

    call_q_pair_reject(call_a, call_b) || chkflscall(call_a, call_b)
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

fn rejects_first_hash_grid_message(
    words: &[&str],
    i3: usize,
    lcall1hash: bool,
    context: &FilterContext,
) -> bool {
    if !lcall1hash || i3 != 1 || words.len() < 3 || words[0] == context.mycall.trim() {
        return false;
    }

    let callsign = words[1];
    if callsign.contains('/') || callsign == context.hiscall.trim() {
        return false;
    }
    if callsign_q_reject(callsign) {
        return true;
    }

    let grid = words[2];
    if !is_grid4(grid) {
        return false;
    }
    let check = chkgrid(callsign, grid);
    check.lwrongcall || !check.lgvalid
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
