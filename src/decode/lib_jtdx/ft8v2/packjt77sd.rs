//! Mirrors JTDX `lib/ft8v2/packjt77sd.f90`.

use super::packjt77::{pack77, unpack77};

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
        Some(out)
    } else {
        None
    }
}

pub(crate) fn unpack77sd(bits77: &[u8; 77]) -> Option<String> {
    unpack77(bits77, None)
}

fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}
