//! Mirrors JTDX `lib/ft8v2/packjt77sd.f90`.

use super::packjt77::{pack77, pack_free_text, unpack77};

pub(crate) fn pack77sd(msg: &str) -> Option<[u8; 77]> {
    let bits = pack77(msg);
    if bits.len() != 77 {
        return None;
    }
    let i3 = bits_to_usize(&bits[74..77]);
    let n3 = bits_to_usize(&bits[71..74]);
    if matches!(i3, 1 | 2)
        || (i3 == 4 && msg.trim_start().starts_with("CQ "))
        || (i3 == 0 && n3 == 0)
    {
        let mut out = [0u8; 77];
        out.copy_from_slice(&bits);
        return Some(out);
    }

    let mut out = [0u8; 77];
    let bits = pack_free_text(msg);
    out.copy_from_slice(&bits);
    Some(out)
}

pub(crate) fn unpack77sd(bits77: &[u8; 77]) -> Option<String> {
    let n3 = bits_to_usize(&bits77[71..74]);
    let i3 = bits_to_usize(&bits77[74..77]);
    let msg = unpack77(bits77, None)?;

    if i3 == 0 && n3 == 0 {
        return Some(msg);
    }
    if i3 == 1 || i3 == 2 {
        return Some(msg);
    }
    if i3 == 4 {
        let icq = bits77[73];
        if icq == 1 && msg.starts_with("CQ ") && !msg.starts_with("CQ <") {
            return Some(msg);
        }
    }
    None
}

fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}
