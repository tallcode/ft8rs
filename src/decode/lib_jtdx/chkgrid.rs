//! Mirrors JTDX `lib/chkgrid.f90`.

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GridCheck {
    #[allow(dead_code)]
    pub(crate) lchkcall: bool,
    pub(crate) lgvalid: bool,
    pub(crate) lwrongcall: bool,
}

pub(crate) fn chkgrid(callsign: &str, grid: &str) -> GridCheck {
    let call = callsign
        .trim()
        .trim_matches(['<', '>'])
        .to_ascii_uppercase();
    let grid = grid.trim().to_ascii_uppercase();
    let lgvalid = is_grid4(&grid);
    let lchkcall = !lgvalid;
    let lwrongcall = lgvalid && obviously_wrong_call_grid(&call, &grid);
    GridCheck {
        lchkcall,
        lgvalid,
        lwrongcall,
    }
}

pub(crate) fn is_grid4(grid: &str) -> bool {
    let bytes = grid.as_bytes();
    bytes.len() == 4
        && (b'A'..=b'R').contains(&bytes[0])
        && (b'A'..=b'R').contains(&bytes[1])
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
}

fn obviously_wrong_call_grid(callsign: &str, grid: &str) -> bool {
    if callsign.starts_with('K') || callsign.starts_with('N') || callsign.starts_with('W') {
        return !matches!(
            &grid[0..2],
            "CM" | "CN" | "DM" | "DN" | "EL" | "EM" | "EN" | "FM" | "FN"
        );
    }
    if callsign.starts_with("JA") || callsign.starts_with("JH") || callsign.starts_with("JR") {
        return !matches!(&grid[0..2], "PM" | "PN" | "QM" | "QN");
    }
    false
}
