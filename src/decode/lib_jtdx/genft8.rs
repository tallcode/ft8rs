//! Mirrors JTDX `lib/genft8.f90`.

use super::ft8_mod1::{GRAYMAP, ICOS7};
use super::ft8v2::encode174_91::encode174_91;
use super::ft8v2::packjt77::{pack77, unpack77};

pub(crate) fn genft8(msg: &str) -> Option<(String, [u8; 77], [i32; 79])> {
    let bits = pack77(msg);
    if bits.len() != 77 {
        return None;
    }

    let mut msgbits = [0u8; 77];
    msgbits.copy_from_slice(&bits);
    let msgsent = unpack77(&msgbits, None)?;
    let itone = get_tones_from_77bits(&msgbits);
    Some((msgsent, msgbits, itone))
}

pub(crate) fn get_tones_from_77bits(msgbits: &[u8; 77]) -> [i32; 79] {
    let codeword = encode174_91(msgbits);
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
    itone
}
