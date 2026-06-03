//! Mirrors JTDX `lib/chkspecial8.f90`.

use super::chkflscall::chkflscall;

pub(crate) fn chkspecial8(msg37: &str, msg37_2: &str, mycall: &str, hiscall: &str) -> bool {
    let Some(ispc1) = msg37.find(' ') else {
        return true;
    };
    let Some(ispc2_rel) = msg37[ispc1 + 1..].find(' ') else {
        return true;
    };
    let ispc2 = ispc1 + 1 + ispc2_rel;
    let Some(ispc12) = msg37_2.find(' ') else {
        return true;
    };

    if ispc1 + 1 <= 3 || ispc2 + 1 <= 7 {
        return true;
    }

    let call_a = &msg37[..ispc1];
    let call_b = &msg37_2[..ispc12];
    let call_c = &msg37[ispc1 + 1..ispc2];
    if call_a == mycall || call_b == mycall || call_c == hiscall {
        return true;
    }

    if chkspecial8_call_reject(call_a) || chkspecial8_call_reject(call_b) {
        return false;
    }
    !chkflscall(call_a, call_b)
}

fn chkspecial8_call_reject(callsign: &str) -> bool {
    let call = callsign.trim().to_ascii_uppercase();
    let bytes = call.as_bytes();
    if bytes.is_empty() {
        return true;
    }
    if matches!(bytes[0], b'Q' | b'0') {
        return true;
    }
    bytes.len() >= 2 && bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit()
}
