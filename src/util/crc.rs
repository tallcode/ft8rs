/// CRC-14 computation and checking.
/// Polynomial: 0x2757 (x^14 + x^13 + x^10 + x^9 + x^8 + x^6 + x^4 + x^2 + x + 1)

const POLY: u16 = 0x2757;

pub fn compute_crc14(msg77: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    // padded with 3 zeros + 16 zero-bits for flush
    for bit_idx in 0..96 {
        let next_bit = if bit_idx < 77 { msg77[bit_idx] } else { 0 };
        if (crc & 0x2000) != 0 {
            crc = ((crc << 1) | next_bit as u16) ^ POLY;
        } else {
            crc = (crc << 1) | next_bit as u16;
        }
        crc &= 0x3fff;
    }
    crc
}

/// Check CRC-14 of a 91-bit decoded message (77 message + 14 CRC).
pub fn check_crc14(bits91: &[u8]) -> bool {
    let received_crc = bits_to_int(&bits91[77..91]);
    let computed_crc = compute_crc14(&bits91[..77]);
    received_crc == computed_crc
}

fn bits_to_int(bits: &[u8]) -> u16 {
    let mut val: u16 = 0;
    for &b in bits {
        val = (val << 1) | b as u16;
    }
    val
}
