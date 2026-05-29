//! CRC14 helper for FT8 LDPC message words.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/get_crc14.f90`

const CRC14_POLY: u16 = 0x2757;

pub(crate) fn compute_crc14(msg77: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for bit_idx in 0..96 {
        let next_bit = if bit_idx < 77 { msg77[bit_idx] } else { 0 };
        if (crc & 0x2000) != 0 {
            crc = ((crc << 1) | next_bit as u16) ^ CRC14_POLY;
        } else {
            crc = (crc << 1) | next_bit as u16;
        }
        crc &= 0x3fff;
    }
    crc
}
