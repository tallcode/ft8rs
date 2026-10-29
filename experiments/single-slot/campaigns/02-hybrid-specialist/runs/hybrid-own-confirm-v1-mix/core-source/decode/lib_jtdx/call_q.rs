//! Mirrors JTDX `lib/call_q.f90`.

pub(crate) fn call_q_reject(call: &str) -> bool {
    let bytes = call.trim().as_bytes();
    if bytes.is_empty() {
        return false;
    }
    matches!(bytes[0], b'Q' | b'0')
        || (bytes.len() >= 2 && bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit())
}

pub(crate) fn call_q_pair_reject(call_a: &str, call_b: &str) -> bool {
    call_q_reject(call_a) || call_q_reject(call_b)
}
