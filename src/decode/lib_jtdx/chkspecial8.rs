//! Mirrors JTDX `lib/chkspecial8.f90`.

use super::callsign_q::callsign_q_reject;
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

    if callsign_q_reject(call_a) || callsign_q_reject(call_b) {
        return false;
    }
    !chkflscall(call_a, call_b)
}
