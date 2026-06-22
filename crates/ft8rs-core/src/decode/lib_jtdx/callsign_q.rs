//! Mirrors JTDX `lib/callsign_q.f90`.

pub(crate) fn callsign_q_reject(callsign: &str) -> bool {
    let call = callsign
        .trim()
        .trim_matches(['<', '>'])
        .to_ascii_uppercase();
    let bytes = call.as_bytes();
    if bytes.is_empty() {
        return true;
    }
    if matches!(bytes[0], b'Q' | b'0' | b'/') {
        return true;
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_digit() && bytes[1].is_ascii_digit() {
        return true;
    }
    if bytes.len() >= 4
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_uppercase()
        && bytes[2].is_ascii_uppercase()
        && bytes[3].is_ascii_uppercase()
    {
        return true;
    }
    if bytes.len() >= 3
        && bytes[0].is_ascii_uppercase()
        && bytes[1].is_ascii_uppercase()
        && bytes[2].is_ascii_uppercase()
    {
        return true;
    }
    if bytes.len() >= 3
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_uppercase()
        && bytes[2].is_ascii_uppercase()
        && call != "3DA"
        && call != "3XY"
        && !call.starts_with("3DA")
        && !call.starts_with("3XY")
    {
        return true;
    }
    false
}
