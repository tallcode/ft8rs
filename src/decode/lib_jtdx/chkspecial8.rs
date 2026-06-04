//! Mirrors JTDX `lib/chkspecial8.f90`.

use super::call_q::call_q_pair_reject;
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

    if call_q_pair_reject(call_a, call_b) {
        return false;
    }
    !chkflscall(call_a, call_b)
}
