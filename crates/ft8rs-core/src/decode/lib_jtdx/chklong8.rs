//! Mirrors JTDX `lib/chklong8.f90`.

pub(crate) fn chklong8(callsign: &str) -> bool {
    const MASK10: [&[u8; 10]; 9] = [
        b"0011000000",
        b"0011100000",
        b"0011110000",
        b"1011000000",
        b"1011100000",
        b"1011110000",
        b"0010000000",
        b"1010000000",
        b"0110000000",
    ];
    const MASK11: [&[u8; 11]; 7] = [
        b"00111000000",
        b"00111100000",
        b"10111000000",
        b"10111100000",
        b"00100000000",
        b"01100000000",
        b"10100000000",
    ];

    let callsign = callsign.trim().to_ascii_uppercase();
    if callsign.contains('/') || callsign.starts_with("8J") || callsign.starts_with("8N") {
        return false;
    }

    match callsign.len() {
        10 => {
            let mask = digit_mask::<10>(&callsign);
            !MASK10.iter().any(|allowed| **allowed == mask)
        }
        11 => {
            let mask = digit_mask::<11>(&callsign);
            !MASK11.iter().any(|allowed| **allowed == mask)
        }
        _ => false,
    }
}

fn digit_mask<const N: usize>(value: &str) -> [u8; N] {
    let mut mask = [b'0'; N];
    for (idx, byte) in value.as_bytes().iter().take(N).enumerate() {
        if byte.is_ascii_digit() {
            mask[idx] = b'1';
        }
    }
    mask
}
