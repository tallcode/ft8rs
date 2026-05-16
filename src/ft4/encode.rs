/// FT4 encoder.

use crate::ft4::constants::GRAYMAP;
use crate::ft4::scramble::xor_with_scrambler;
use crate::ft8::encode::encode174_91;
use crate::util::pack_jt77::pack77;
use crate::util::waveform::{generate_ft4_waveform, WaveformOptions};

const COSTAS_A: [u8; 4] = [0, 1, 3, 2];
const COSTAS_B: [u8; 4] = [1, 0, 2, 3];
const COSTAS_C: [u8; 4] = [2, 3, 1, 0];
const COSTAS_D: [u8; 4] = [3, 2, 0, 1];

/// Convert FT4 LDPC codeword bits into 103 channel tones.
pub fn get_tones(codeword: &[u8]) -> Vec<u8> {
    let mut data_tones = vec![0u8; 87];
    for i in 0..87 {
        let b0 = codeword[2 * i];
        let b1 = codeword[2 * i + 1];
        let symbol = b1 + 2 * b0;
        data_tones[i] = GRAYMAP[symbol as usize];
    }

    let mut tones = vec![0u8; 103];
    tones[0..4].copy_from_slice(&COSTAS_A);
    tones[4..33].copy_from_slice(&data_tones[0..29]);
    tones[33..37].copy_from_slice(&COSTAS_B);
    tones[37..66].copy_from_slice(&data_tones[29..58]);
    tones[66..70].copy_from_slice(&COSTAS_C);
    tones[70..99].copy_from_slice(&data_tones[58..87]);
    tones[99..103].copy_from_slice(&COSTAS_D);
    tones
}

pub fn encode_message(msg: &str) -> Vec<u8> {
    let bits77 = pack77(msg);
    let scrambled = xor_with_scrambler(&bits77);
    let codeword = encode174_91(&scrambled);
    get_tones(&codeword)
}

pub fn encode(msg: &str, options: WaveformOptions) -> Vec<f32> {
    generate_ft4_waveform(&encode_message(msg), options)
}
