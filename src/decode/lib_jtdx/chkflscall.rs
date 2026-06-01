//! Mirrors JTDX `lib/chkflscall.f90`.

use super::searchcalls::searchcalls;

pub(crate) fn chkflscall(call_a: &str, call_b: &str) -> bool {
    let call_a = call_a.trim();
    let call_b = call_b.trim();
    if call_a.starts_with("<.") {
        return false;
    }
    if call_a == "MYCALL" || call_a == "CQ" {
        return !searchcalls(call_b, "");
    }
    if call_b.starts_with('<') {
        return !searchcalls(call_a, "");
    }
    !searchcalls(call_a, call_b)
}
