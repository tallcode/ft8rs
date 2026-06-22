//! Mirrors JTDX `lib/delbraces.f90`.

pub(crate) fn delbraces(msg37: &str) -> String {
    let mut msg = fixed37(msg37);
    let ispc1 = index_space(&msg, 0).unwrap_or(37);
    let ispc2 = index_space(&msg, ispc1.saturating_add(1)).unwrap_or(37);
    let ispc3 = index_space(&msg, ispc2.saturating_add(1)).unwrap_or(37);

    if msg[0] == b'<' && msg[1] != b'.' {
        remove_braces_for_field(&mut msg, 0, ispc1.saturating_sub(1));
    } else {
        let iboc2 = ispc1.saturating_add(1);
        if iboc2 < 37 && msg[iboc2] == b'<' && msg.get(iboc2 + 1) != Some(&b'.') {
            remove_braces_for_field(&mut msg, iboc2, ispc2.saturating_sub(1));
        } else {
            let iboc3 = ispc2.saturating_add(1);
            if iboc3 < 37 && msg[iboc3] == b'<' && msg.get(iboc3 + 1) != Some(&b'.') {
                remove_braces_for_field(&mut msg, iboc3, ispc3.saturating_sub(1));
            }
        }
    }

    String::from_utf8_lossy(&msg).trim_end().to_string()
}

fn remove_braces_for_field(msg: &mut [u8; 37], iboc: usize, ieoc: usize) {
    if ieoc < 37 {
        for i in ieoc..36 {
            msg[i] = msg[i + 1];
        }
        msg[36] = b' ';
    }
    for i in iboc..36 {
        msg[i] = msg[i + 1];
    }
    msg[36] = b' ';
}

fn fixed37(value: &str) -> [u8; 37] {
    let mut out = [b' '; 37];
    for (idx, byte) in value.as_bytes().iter().take(37).enumerate() {
        out[idx] = *byte;
    }
    out
}

fn index_space(msg: &[u8; 37], start: usize) -> Option<usize> {
    msg.iter()
        .enumerate()
        .skip(start)
        .find_map(|(idx, &byte)| (byte == b' ').then_some(idx))
}
