//! Mirrors JTDX `lib/ft8v2/chkcrc14a.f90`.

const CRC14_POLY: u16 = 0x2757;

pub(crate) fn chkcrc14a(decoded91: &[u8]) -> bool {
    let ncrc14 = bits_to_int(&decoded91[77..91]);
    let icrc14 = crc14(&decoded91[..77]);
    ncrc14 == icrc14
}

fn crc14(msg77: &[u8]) -> u16 {
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

fn bits_to_int(bits: &[u8]) -> u16 {
    let mut value = 0u16;
    for &bit in bits {
        value = (value << 1) | bit as u16;
    }
    value
}
