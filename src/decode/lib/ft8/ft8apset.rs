//! AP mask setup for FT8.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/ft8apset.f90`

use super::Ft8ApSet;
use crate::decode::packjt77::{is_stdcall, pack77, unpack77, C38};

pub(super) const MCQ: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
];
pub(super) const MCQRU: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0,
];
pub(super) const MCQFD: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0,
];
pub(super) const MCQTEST: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 0,
];
pub(super) const MCQWW: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1, 0,
];
pub(super) const MRRR: [i8; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1];
pub(super) const M73: [i8; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 1];
pub(super) const MRR73: [i8; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 0, 1];

pub(super) fn ft8_ap_set(mycall: Option<&str>, hiscall: Option<&str>, ncontest: usize) -> Ft8ApSet {
    let mut apsym = [0i8; 58];
    apsym[0] = 99;
    apsym[29] = 99;
    let mut aph10 = [0i8; 10];
    aph10[0] = 99;

    let Some(mycall_raw) = mycall.map(str::trim).filter(|s| s.len() >= 3) else {
        return Ft8ApSet { apsym, aph10 };
    };
    let mycall = mycall_raw.to_ascii_uppercase();

    let hiscall_trimmed = hiscall.map(str::trim).unwrap_or("").to_ascii_uppercase();
    let no_hiscall = hiscall_trimmed.len() < 3;
    let hiscall_for_pack = if no_hiscall {
        "KA1ABC"
    } else {
        hiscall_trimmed.as_str()
    };

    if !no_hiscall {
        let n10 = ihashcall_bits(hiscall_for_pack, 10);
        for (i, slot) in aph10.iter_mut().enumerate() {
            let bit = ((n10 >> (9 - i)) & 1) as i8;
            *slot = 2 * bit - 1;
        }
    }

    let msg = if is_stdcall(&mycall) {
        format!("{} {} RRR", mycall, hiscall_for_pack)
    } else {
        format!("<{}> {} RRR", mycall, hiscall_for_pack)
    };
    let bits = pack77(&msg);
    if bits.len() != 77 {
        return Ft8ApSet { apsym, aph10 };
    }

    let i3 = ((bits[74] as usize) << 2) | ((bits[75] as usize) << 1) | bits[76] as usize;
    let unpacked = unpack77(&bits, None);
    if ncontest == 7 && (i3 != 1 || unpacked.is_none()) {
        return Ft8ApSet { apsym, aph10 };
    }
    if ncontest <= 5 && (i3 != 1 || unpacked.as_deref() != Some(msg.as_str())) {
        return Ft8ApSet { apsym, aph10 };
    }

    for i in 0..58 {
        apsym[i] = 2 * bits[i] as i8 - 1;
    }
    if no_hiscall {
        apsym[29] = 99;
        aph10[0] = 99;
    }

    Ft8ApSet { apsym, aph10 }
}

fn ihashcall_bits(call: &str, m: usize) -> usize {
    let mut n8: u64 = 0;
    let mut count = 0;
    for c in call.chars() {
        if count >= 11 {
            break;
        }
        let uc = c.to_ascii_uppercase();
        let j = C38.iter().position(|&x| x == uc as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
        count += 1;
    }
    while count < 11 {
        let j = C38.iter().position(|&x| x == b' ').unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
        count += 1;
    }
    const MAGIC: u64 = 47055833459;
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> (64 - m as u32)) & ((1u64 << m as u32) - 1)) as usize
}
