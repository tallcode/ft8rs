//! Mirrors JTDX `lib/ft8v2/packjt77sd.f90`.

use super::encode174_91::encode174_91;
use super::packjt77::{pack77, unpack77};
use crate::decode::lib_jtdx::ft8_mod1::{GRAYMAP, ICOS7};

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

pub(crate) fn genft8sd(msg: &str) -> Option<(String, [u8; 77], [i32; 79])> {
    let msgbits = pack77sd(msg)?;
    let msgsent = unpack77sd(&msgbits)?;
    let codeword = encode174_91(&msgbits);
    let mut itone = [0i32; 79];
    for i in 0..7 {
        itone[i] = ICOS7[i];
        itone[36 + i] = ICOS7[i];
        itone[72 + i] = ICOS7[i];
    }
    let mut k = 7usize;
    for j in 1..=58 {
        let i = (j - 1) * 3;
        if j == 30 {
            k += 7;
        }
        let indx =
            codeword[i] as usize * 4 + codeword[i + 1] as usize * 2 + codeword[i + 2] as usize;
        itone[k] = GRAYMAP[indx];
        k += 1;
    }
    Some((msgsent, msgbits, itone))
}

fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}
