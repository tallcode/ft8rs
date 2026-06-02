//! Mirrors JTDX `lib/chkspecial8.f90`.

use super::chkflscall::chkflscall;

pub(crate) fn chkspecial8(msg37: &str, msg37_2: &str, mycall: &str, hiscall: &str) -> bool {
    let words: Vec<&str> = msg37.split_whitespace().collect();
    let words2: Vec<&str> = msg37_2.split_whitespace().collect();
    if words.len() < 2 || words2.is_empty() {
        return true;
    }

    let call_a = words[0];
    let call_b = words2[0];
    let call_c = words[1];
    if call_a == mycall || call_b == mycall || call_c == hiscall {
        return true;
    }

    if chkspecial8_call_reject(call_a) || chkspecial8_call_reject(call_b) {
        return false;
    }
    !chkflscall(call_a, call_b)
}

fn chkspecial8_call_reject(callsign: &str) -> bool {
    let call = callsign
        .trim()
        .trim_matches(['<', '>'])
        .to_ascii_uppercase();
    let bytes = call.as_bytes();
    if bytes.is_empty() {
        return true;
    }
    if matches!(bytes[0], b'Q' | b'0') {
        return true;
    }
    bytes.len() >= 2 && bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit()
}
