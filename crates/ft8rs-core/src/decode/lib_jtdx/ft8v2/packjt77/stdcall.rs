pub fn is_stdcall(callsign: &str) -> bool {
    let c = callsign.trim().to_uppercase();
    let bytes = c.as_bytes();
    let n = bytes.len();
    if !(2..=12).contains(&n) {
        return false;
    }

    // Find right-most digit (call area)
    let mut iarea: i32 = -1;
    for i in (1..n).rev() {
        if bytes[i].is_ascii_digit() {
            iarea = i as i32;
            break;
        }
    }

    // WSJT-X stdcall uses 1-based iarea in [2, 3]. Rust indices are 0-based.
    if !(1..=2).contains(&iarea) {
        return false;
    }

    // Count digits and letters before call area
    let mut npdig = 0;
    let mut nplet = 0;
    for bytes_value in bytes.iter().take(iarea as usize) {
        if (*bytes_value).is_ascii_digit() {
            npdig += 1;
        }
        if (*bytes_value).is_ascii_uppercase() {
            nplet += 1;
        }
    }

    // Count letters in suffix
    let mut nslet = 0;
    for bytes_value in bytes.iter().take(n).skip(iarea as usize + 1) {
        if (*bytes_value).is_ascii_uppercase() {
            nslet += 1;
        }
    }

    nplet >= 1 && npdig < iarea && nslet <= 3
}
