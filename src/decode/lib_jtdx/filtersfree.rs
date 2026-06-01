//! Mirrors the deterministic string filters in JTDX `lib/filtersfree.f90`.

pub(crate) fn filtersfree(decoded: &str) -> bool {
    let decoded = fixed22(decoded);
    if slice(&decoded, 11, 12) == "73" || slice(&decoded, 12, 13) == "73" {
        return false;
    }
    if ch(&decoded, 1) == b'/' {
        return true;
    }

    let mut ndot = 0usize;
    let mut nsign = 0usize;
    let mut nother = 0usize;
    for i in 1..=13 {
        let d2 = slice(&decoded, i, i + 1);
        let d3 = slice(&decoded, i, i + 2);
        let d4 = slice(&decoded, i, i + 3);
        let d5 = slice(&decoded, i, i + 4);
        if matches!(d2.as_str(), "73" | "TU" | "GL") {
            return false;
        }
        if i > 4 {
            if matches!(d2.as_str(), " -" | " +") && (b'0'..b'3').contains(&ch(&decoded, i + 2)) {
                return false;
            }
            if matches!(d3.as_str(), " R-" | " R+") && (b'0'..b'3').contains(&ch(&decoded, i + 3)) {
                return false;
            }
        }
        if matches!(
            d3.as_str(),
            "QSL" | "TNX" | "QSO" | "CFM" | "BND" | "QSY" | "MAS" | "HNY" | "HPY" | " DT" | "XMS"
        ) || matches!(d2.as_str(), "RR")
            || matches!(
                d4.as_str(),
                "LOTW" | "BAND" | "TEST" | "/GIF" | "/JPG" | "/AVI" | "/MP4" | "/CMD"
            )
            || matches!(
                d5.as_str(),
                "EASTE" | "PIRAT" | "/HYBR" | "/HTML" | "/PHOT" | "/IMAG" | "/JOIN" | "QRZ.C"
            )
        {
            return false;
        }
        match ch(&decoded, i) {
            b'.' => ndot += 1,
            b'-' | b'+' | b'?' => nsign += 1,
            b'/' => nother += 1,
            _ => {}
        }
        if ch(&decoded, i) == b'?' {
            nother += 1;
        }
    }

    if ndot >= 2 || nsign >= 2 || nother >= 2 {
        return true;
    }
    if slice(&decoded, 1, 2) == "-0" {
        return true;
    }
    if ch(&decoded, 1) != b' ' && ch(&decoded, 2) == b' ' {
        return true;
    }
    if (ch(&decoded, 13).is_ascii_uppercase() || ch(&decoded, 13).is_ascii_digit())
        && ch(&decoded, 12) == b' '
    {
        return true;
    }
    if matches!(ch(&decoded, 1), b'.' | b'+' | b'?' | b'/')
        || matches!(ch(&decoded, 13), b'.' | b'+' | b'-' | b'/')
        || matches!(ch(&decoded, 2), b'-' | b'+')
        || matches!(ch(&decoded, 12), b'.' | b'+' | b'-')
    {
        return true;
    }
    if ch(&decoded, 12).is_ascii_digit()
        && ch(&decoded, 13).is_ascii_digit()
        && !matches!(slice(&decoded, 12, 13).as_str(), "55" | "73" | "88")
    {
        return true;
    }
    if ch(&decoded, 1) == b'0' && ch(&decoded, 2) != b'.' {
        return true;
    }

    for i in 1..=12 {
        if i < 12 && ch(&decoded, i) == b'?' && ch(&decoded, i + 1) != b' ' {
            return true;
        }
        if i < 12
            && ch(&decoded, i) != b' '
            && ch(&decoded, i + 1) == b'+'
            && (ch(&decoded, i + 2) <= b'0' || ch(&decoded, i + 2) >= b'9')
        {
            return true;
        }
        if ch(&decoded, i).is_ascii_uppercase()
            && ch(&decoded, i + 1) == b'.'
            && ch(&decoded, i + 2).is_ascii_uppercase()
        {
            return true;
        }
        if matches!(slice(&decoded, i + 1, i + 2).as_str(), "/ " | " /") {
            return true;
        }
        if ch(&decoded, i).is_ascii_digit()
            && ch(&decoded, i + 1) == b'/'
            && ch(&decoded, i + 2).is_ascii_digit()
        {
            return true;
        }
        if ch(&decoded, i) == b' '
            && ch(&decoded, i + 1).is_ascii_uppercase()
            && !matches!(ch(&decoded, i + 1), b'F' | b'G' | b'I')
            && ch(&decoded, i + 2) == b'/'
        {
            return true;
        }
        if ch(&decoded, i) == b' '
            && ch(&decoded, i + 1).is_ascii_uppercase()
            && matches!(ch(&decoded, i + 2), b'.' | b'+' | b'-')
        {
            return true;
        }
    }

    if (b'1'..=b'9').contains(&ch(&decoded, 1))
        && ch(&decoded, 2).is_ascii_digit()
        && ch(&decoded, 3).is_ascii_digit()
        && ch(&decoded, 4) != b'W'
    {
        return true;
    }
    if (b'1'..=b'9').contains(&ch(&decoded, 1))
        && ch(&decoded, 2).is_ascii_digit()
        && ch(&decoded, 3) != b'W'
    {
        return true;
    }
    if (b'1'..=b'9').contains(&ch(&decoded, 1)) && slice(&decoded, 2, 3) != "EL" {
        return true;
    }
    if ch(&decoded, 10) != b'/'
        && ch(&decoded, 11) != b'/'
        && ch(&decoded, 12).is_ascii_uppercase()
        && ch(&decoded, 13).is_ascii_digit()
    {
        return true;
    }

    // JTDX then calls datacor(datapwr, datacorr) and rejects when datacorr < 1.55.
    // `datapwr` is not represented in the current Rust JTDX filter boundary, so
    // this mirror keeps only the deterministic text-shape filters.
    let first_len = decoded
        .iter()
        .take(13)
        .position(|&byte| byte == b' ')
        .unwrap_or(13);
    first_len == 13
}

fn fixed22(value: &str) -> [u8; 22] {
    let mut out = [b' '; 22];
    for (idx, byte) in value.as_bytes().iter().take(22).enumerate() {
        out[idx] = byte.to_ascii_uppercase();
    }
    out
}

fn ch(bytes: &[u8; 22], idx1: usize) -> u8 {
    bytes.get(idx1.saturating_sub(1)).copied().unwrap_or(b' ')
}

fn slice(bytes: &[u8; 22], start1: usize, end1: usize) -> String {
    let start = start1.saturating_sub(1).min(bytes.len());
    let end = end1.min(bytes.len());
    String::from_utf8_lossy(&bytes[start..end]).into_owned()
}
