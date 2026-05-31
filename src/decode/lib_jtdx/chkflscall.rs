//! Mirrors JTDX `lib/chkflscall.f90`.

use super::callsign_q::callsign_q_reject;

pub(crate) fn chkflscall(call_a: &str, call_b: &str) -> bool {
    let call_a = call_a.trim();
    let call_b = call_b.trim();
    if call_a.starts_with("<.") {
        return false;
    }
    if call_a == "MYCALL" || call_a == "CQ" {
        return callsign_q_reject(call_b);
    }
    if call_b.starts_with('<') {
        return callsign_q_reject(call_a);
    }
    callsign_q_reject(call_a) || callsign_q_reject(call_b)
}
