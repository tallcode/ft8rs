//! Mirrors JTDX `lib/ft8v2/encode174_91.f90`.

use super::chkcrc14a::crc14;
use super::ldpc_174_91_c_generator::G_HEX;

const N: usize = 174;
const K: usize = 91;
const M: usize = N - K;

pub(crate) fn encode174_91(message77: &[u8]) -> [u8; N] {
    let gen = generator_matrix();
    let ncrc14 = crc14(message77);
    let mut message = [0u8; K];
    message[..77].copy_from_slice(&message77[..77]);
    for i in 0..14 {
        message[77 + i] = ((ncrc14 >> (13 - i)) & 1) as u8;
    }

    let mut codeword = [0u8; N];
    codeword[..K].copy_from_slice(&message);
    for i in 0..M {
        let mut nsum = 0u8;
        for j in 0..K {
            nsum ^= message[j] & gen[i][j];
        }
        codeword[K + i] = nsum & 1;
    }
    codeword
}

fn generator_matrix() -> [[u8; K]; M] {
    let mut gen = [[0u8; K]; M];
    for i in 0..M {
        let hex = G_HEX[i].as_bytes();
        for j in 0..23 {
            let istr = (hex[j] as char).to_digit(16).unwrap_or(0) as u8;
            let ibmax = if j == 22 { 3 } else { 4 };
            for jj in 1..=ibmax {
                let icol = j * 4 + jj - 1;
                if (istr & (1 << (4 - jj))) != 0 {
                    gen[i][icol] = 1;
                }
            }
        }
    }
    gen
}
