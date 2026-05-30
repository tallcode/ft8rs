//! FT8 message tone generation.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/genft8.f90`

use crate::decode::encode174_91::encode174_91;

const ICOS7: [usize; 7] = [3, 1, 4, 0, 6, 5, 2];
const GRAY_MAP: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];

#[allow(dead_code)]
pub(crate) fn get_ft8_tones_from_77bits(msgbits: &[u8]) -> [i32; 79] {
    let codeword = encode174_91(msgbits);
    get_ft8_tones_from_codeword(&codeword)
}

pub(crate) fn get_ft8_tones_from_codeword(codeword: &[u8]) -> [i32; 79] {
    let mut itone = [0i32; 79];
    for i in 0..7 {
        itone[i] = ICOS7[i] as i32;
        itone[36 + i] = ICOS7[i] as i32;
        itone[72 + i] = ICOS7[i] as i32;
    }
    let mut k = 7;
    for j in 1..=58 {
        let i = (j - 1) * 3;
        if j == 30 {
            k += 7;
        }
        let indx =
            (codeword[i] as usize) * 4 + (codeword[i + 1] as usize) * 2 + codeword[i + 2] as usize;
        itone[k] = GRAY_MAP[indx] as i32;
        k += 1;
    }
    itone
}
