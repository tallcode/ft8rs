//! CRC14 checker for FT8 decoded 91-bit words.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/chkcrc14a.f90`

use crate::decode::get_crc14::compute_crc14;

pub(crate) fn check_crc14(bits91: &[u8]) -> bool {
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
