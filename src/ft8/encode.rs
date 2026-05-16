/// FT8 encoder.

use crate::util::constants::G_HEX;
use crate::util::pack_jt77::pack77;
use crate::util::waveform::{generate_ft8_waveform, WaveformOptions};
use crate::ft8::constants::{COSTAS, GRAY_MAP};

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

/// Encode 77-bit message into 174-bit LDPC codeword.
pub fn encode174_91(msg77: &[u8]) -> Vec<u8> {
    let g = generate_ldpc_g_matrix();
    let poly = 0x2757u16;
    let mut crc: u16 = 0;

    // CRC computation (with 19 zeros padding: 3 + 16)
    for bit_idx in 0..96 {
        let next_bit = if bit_idx < 77 { msg77[bit_idx] } else { 0 };
        if (crc & 0x2000) != 0 {
            crc = ((crc << 1) | next_bit as u16) ^ poly;
        } else {
            crc = (crc << 1) | next_bit as u16;
        }
        crc &= 0x3fff;
    }

    // Build 91-bit message (77 + 14 CRC)
    let mut msg91 = msg77.to_vec();
    for i in 0..14 {
        msg91.push(((crc >> (13 - i)) & 1) as u8);
    }

    // Generate parity bits
    let mut codeword = msg91.clone();
    for i in 0..83 {
        let mut sum = 0;
        for j in 0..91 {
            sum += msg91[j] * g[i][j];
        }
        codeword.push(sum % 2);
    }
    codeword
}

/// Convert LDPC codeword bits into 79 channel tones.
pub fn get_tones(codeword: &[u8]) -> Vec<u8> {
    let mut tones = vec![0u8; 79];

    for i in 0..7 {
        tones[i] = COSTAS[i];
        tones[36 + i] = COSTAS[i];
        tones[72 + i] = COSTAS[i];
    }

    let mut k = 7;
    for j in 1..=58 {
        let i = j * 3 - 3;
        if j == 30 {
            k += 7;
        }
        let indx = (codeword[i] as usize) * 4
            + (codeword[i + 1] as usize) * 2
            + (codeword[i + 2] as usize);
        tones[k] = GRAY_MAP[indx];
        k += 1;
    }
    tones
}

/// Encode a message string into tones.
pub fn encode_message(msg: &str) -> Vec<u8> {
    let bits77 = pack77(msg);
    let codeword = encode174_91(&bits77);
    get_tones(&codeword)
}

/// Encode a message string into a waveform.
pub fn encode(msg: &str, options: WaveformOptions) -> Vec<f32> {
    generate_ft8_waveform(&encode_message(msg), options)
}
