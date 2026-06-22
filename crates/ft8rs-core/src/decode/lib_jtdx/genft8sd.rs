//! Mirrors JTDX `lib/genft8sd.f90`.

use super::ft8_mod1::{GRAYMAP, ICOS7};
use super::ft8v2::encode174_91::encode174_91;
use super::ft8v2::packjt77sd::{pack77sd, unpack77sd};

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
