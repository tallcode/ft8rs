//! Receive-only alternatives for historic standard-message templates.

pub(super) fn legacy_rr73(bits: &[u8]) -> Option<Vec<u8>> {
    const ACK: u16 = 32403;
    const LEGACY_GRID: u16 = 32373;

    if bits.len() != 77 || bits[58] != 0 || !matches!(&bits[74..], [0, 0, 1] | [0, 1, 0]) {
        return None;
    }
    let grid = bits[59..74]
        .iter()
        .fold(0u16, |value, &bit| (value << 1) | u16::from(bit));
    if grid != ACK {
        return None;
    }
    let mut legacy = bits.to_vec();
    for (i, bit) in legacy[59..74].iter_mut().enumerate() {
        *bit = ((LEGACY_GRID >> (14 - i)) & 1) as u8;
    }
    Some(legacy)
}
