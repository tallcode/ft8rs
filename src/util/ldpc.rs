use crate::util::constants::G_HEX;

pub fn codeword_174_91(msg77: &[u8]) -> Vec<u8> {
    let g = generate_ldpc_g_matrix();
    let poly = 0x2757u16;
    let mut crc: u16 = 0;

    for bit_idx in 0..96 {
        let next_bit = if bit_idx < 77 { msg77[bit_idx] } else { 0 };
        if (crc & 0x2000) != 0 {
            crc = ((crc << 1) | next_bit as u16) ^ poly;
        } else {
            crc = (crc << 1) | next_bit as u16;
        }
        crc &= 0x3fff;
    }

    let mut msg91 = msg77.to_vec();
    for i in 0..14 {
        msg91.push(((crc >> (13 - i)) & 1) as u8);
    }

    let mut codeword = msg91.clone();
    for row in g.iter().take(83) {
        let mut sum = 0;
        for j in 0..91 {
            sum += msg91[j] * row[j];
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
