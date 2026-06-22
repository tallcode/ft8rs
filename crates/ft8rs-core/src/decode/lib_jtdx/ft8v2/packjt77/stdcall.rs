pub fn is_stdcall(callsign: &str) -> bool {
    let c = callsign.trim().to_uppercase();
    let bytes = c.as_bytes();
    let n = bytes.len();
    if n < 2 || n > 12 {
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
    for i in 0..(iarea as usize) {
        if bytes[i].is_ascii_digit() {
            npdig += 1;
        }
        if bytes[i].is_ascii_uppercase() {
            nplet += 1;
        }
    }

    // Count letters in suffix
    let mut nslet = 0;
    for i in (iarea as usize + 1)..n {
        if bytes[i].is_ascii_uppercase() {
            nslet += 1;
        }
    }

    nplet >= 1 && (npdig as i32) < iarea && nslet <= 3
}
