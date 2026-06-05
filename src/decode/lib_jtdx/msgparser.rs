//! Mirrors JTDX `lib/msgparser.f90`.

pub(crate) fn msgparser(msg37: &str) -> Option<(String, String)> {
    let spaces = space_positions(msg37);
    if spaces.len() < 5 || spaces[4] + 1 <= 20 {
        return None;
    }

    let ispc1 = spaces[0];
    let ispc2 = spaces[1];
    let ispc3 = spaces[2];
    let ispc4 = spaces[3];
    let ispc5 = spaces[4];

    let call1 = msg37[..ispc1].trim();
    let call2 = msg37[ispc2 + 1..ispc3].trim();
    let mut call3 = msg37[ispc3 + 1..ispc4].trim().to_string();
    if call3.starts_with("<.") {
        call3 = "<...>".to_string();
    } else {
        if call3.starts_with('<') {
            call3.remove(0);
        }
        if let Some(ib) = call3.find('>') {
            if ib >= 3 {
                call3.remove(ib);
            }
        }
    }
    let report = if call3.trim().len() < 11 {
        msg37[ispc4 + 1..ispc5].trim()
    } else {
        msg37[ispc4 + 1..].trim()
    };

    Some((
        format!("{} {} RR73", call1.trim(), call3.trim()),
        format!("{} {} {}", call2.trim(), call3.trim(), report.trim()),
    ))
}

fn space_positions(value: &str) -> Vec<usize> {
    value
        .as_bytes()
        .iter()
        .enumerate()
        .filter_map(|(idx, &byte)| (byte == b' ').then_some(idx))
        .collect()
}
