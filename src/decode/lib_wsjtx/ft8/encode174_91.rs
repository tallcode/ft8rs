//! Add CRC14 and encode FT8 LDPC(174,91) codewords.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/encode174_91.f90`

use crate::decode::get_crc14::compute_crc14;
use crate::decode::ldpc_174_91_c_generator::G_HEX;

pub(crate) fn encode174_91(message77: &[u8]) -> Vec<u8> {
    let g = generate_ldpc_g_matrix();
    let crc = compute_crc14(message77);

    let mut message = message77.to_vec();
    for i in 0..14 {
        message.push(((crc >> (13 - i)) & 1) as u8);
    }

    let mut codeword = message.clone();
    for row in g.iter().take(83) {
        let mut sum = 0;
        for j in 0..91 {
            sum += message[j] * row[j];
        }
        codeword.push(sum % 2);
    }
    codeword
}

fn generate_ldpc_g_matrix() -> Vec<Vec<u8>> {
    let k = 91;
    let m = 83;
    let mut gen = vec![vec![0u8; k]; m];

    for i in 0..m {
        let hex_str = G_HEX[i];
        for j in 0..23 {
            let byte = hex_str.as_bytes()[j];
            let val = u8::from_str_radix(&format!("{}", byte as char), 16).unwrap_or(0);
            let limit = if j == 22 { 3 } else { 4 };
            for jj in 1..=limit {
                let col = j * 4 + jj - 1;
                if (val & (1 << (4 - jj))) != 0 {
                    gen[i][col] = 1;
                }
            }
        }
    }
    gen
}
