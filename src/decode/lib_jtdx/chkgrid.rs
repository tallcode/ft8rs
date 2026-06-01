//! Mirrors JTDX `lib/chkgrid.f90`.

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct GridCheck {
    #[allow(dead_code)]
    pub(crate) lchkcall: bool,
    pub(crate) lgvalid: bool,
    pub(crate) lwrongcall: bool,
}

pub(crate) fn chkgrid(callsign: &str, grid: &str) -> GridCheck {
    let _call = callsign
        .trim()
        .trim_matches(['<', '>'])
        .to_ascii_uppercase();
    let grid = grid.trim().to_ascii_uppercase();
    let grid4 = is_grid4(&grid);
    let lchkcall = grid4 && grid_requires_call_check(&grid);
    let lgvalid = grid4;
    let lwrongcall = false;
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

fn grid_requires_call_check(grid: &str) -> bool {
    let b = grid.as_bytes();
    if b.len() < 4 {
        return false;
    }
    let g1 = b[0];
    let g2 = b[1];
    let g3 = b[2];
    let g4 = b[3];
    let g34 = &grid[2..4];

    if g1.is_ascii_digit() {
        return true;
    }

    match g1 {
        b'A' => in_range(g2, b'A', b'D') || matches!(g2, b'M' | b'N' | b'Q' | b'R'),
        b'B' => in_range(g2, b'A', b'F') || matches!(g2, b'M' | b'N' | b'Q' | b'R'),
        b'C' => {
            in_range(g2, b'A', b'F')
                || (g2 == b'G' && g3 > b'7' && g4 < b'4')
                || (g2 == b'I' && g3 > b'0' && g4 > b'1')
                || in_range(g2, b'J', b'L')
                || matches!(g2, b'Q' | b'R')
        }
        b'D' => {
            in_range(g2, b'A', b'F')
                || in_range(g2, b'H', b'J')
                || (g2 == b'G' && g34 != "52" && g34 != "73")
                || matches!(g2, b'Q' | b'R')
        }
        b'E' => {
            in_range(g2, b'A', b'B')
                || (g2 == b'C' && g34 != "41")
                || (g2 == b'F' && g34 != "96")
                || in_range(g2, b'D', b'E')
                || (g2 == b'G' && g34 != "93")
                || g2 == b'H'
                || g2 == b'Q'
                || (g2 == b'R' && g34 != "60")
        }
        b'F' => in_range(g2, b'A', b'C') || (g2 == b'Q' && g3 < b'3') || g2 == b'R',
        b'G' => {
            in_range(g2, b'A', b'B')
                || in_range(g2, b'L', b'M')
                || (g2 == b'K' && g3 > b'0')
                || (g2 == b'N' && g3 > b'3')
                || (g2 == b'Q' && g4 > b'4')
                || g2 == b'R'
        }
        b'H' => {
            g2 == b'A'
                || (g2 == b'B' && g34 != "22")
                || g2 == b'C'
                || in_range(g2, b'E', b'G')
                || matches!(g2, b'J' | b'L' | b'N' | b'O' | b'R')
                || (g2 == b'K' && g3 < b'7')
                || (g2 == b'M' && g4 < b'6')
                || (g2 == b'Q' && g4 > b'0')
        }
        b'I' => {
            g2 == b'A'
                || in_range(g2, b'C', b'E')
                || (g2 == b'B' && g34 != "59")
                || (g2 == b'F' && g34 != "32")
                || g2 == b'G'
                || (g2 == b'H' && g3 != b'7')
                || (g2 == b'I' && g34 != "22")
                || (g2 == b'O' && g3 < b'3')
                || (g2 == b'P' && g4 > b'7')
                || (g2 == b'Q' && g4 > b'1')
                || g2 == b'R'
        }
        b'J' => {
            g2 == b'C'
                || (g2 == b'A' && g34 != "00")
                || (g2 == b'B' && g34 != "59")
                || (g2 == b'D' && g34 != "15")
                || g2 == b'E'
                || (g2 == b'F' && g3 < b'8')
                || (g2 == b'G' && g3 < b'6')
                || (g2 == b'H' && g3 < b'5')
                || (g2 == b'I' && g3 < b'2')
                || (g2 == b'R' && g4 > b'0')
        }
        b'K' => {
            in_range(g2, b'A', b'B')
                || (g2 == b'C' && g34 != "90")
                || g2 == b'D'
                || (g2 == b'E' && g34 != "83" && g34 != "93")
                || (g2 == b'F' && g4 < b'5')
                || (g2 == b'R' && g4 > b'0')
        }
        b'L' => {
            in_range(g2, b'A', b'D')
                || (g2 == b'E' && g34 != "53" && g34 != "54" && g34 != "63")
                || g2 == b'F'
                || (g2 == b'G' && g4 < b'4')
                || (g2 == b'J' && g3 > b'5')
                || (g2 == b'Q' && g4 < b'5')
                || (g2 == b'R' && g4 > b'1')
        }
        b'M' => {
            in_range(g2, b'A', b'C')
                || (g2 == b'D' && g34 != "66" && g34 != "67" && g34 != "49")
                || (g2 == b'E' && g34 != "40" && g34 != "41" && g34 != "50")
                || (g2 == b'F' && g34 != "81" && g34 != "82")
                || g2 == b'G'
                || (g2 == b'H' && g34 != "10")
                || (g2 == b'I' && g3 < b'5')
                || (g2 == b'J' && g3 < b'6')
                || (g2 == b'K' && g3 < b'5')
                || (g2 == b'R' && g4 > b'1')
        }
        b'N' => {
            in_range(g2, b'A', b'G')
                || (g2 == b'H' && g34 != "87" && g34 != "88")
                || (g2 == b'I' && g3 < b'9' && g34 != "89")
                || (g2 == b'J' && g3 > b'0' && g3 < b'6')
                || (g2 == b'R' && g4 > b'1')
        }
        b'O' => {
            in_range(g2, b'A', b'E')
                || (g2 == b'F' && g3 < b'7')
                || (g2 == b'G' && g3 < b'6')
                || (g2 == b'H' && g34 != "90" && g34 != "92" && g34 != "29" && g34 != "99")
                || g2 == b'R'
        }
        b'P' => {
            in_range(g2, b'A', b'E')
                || (g2 == b'F' && g4 < b'2')
                || (g2 == b'K' && g3 > b'3' && g34 != "90")
                || (g2 == b'Q' && g4 > b'6')
                || g2 == b'R'
        }
        b'Q' => {
            in_range(g2, b'A', b'C')
                || (g2 == b'D' && g34 != "94" && g34 != "95")
                || (g2 == b'E' && g4 < b'6')
                || (g2 == b'F' && g3 > b'6' && g34 != "98")
                || (g2 == b'K' && g3 > b'2' && g34 != "36")
                || (g2 == b'L' && g3 > b'2' && g34 != "64" && g34 != "74")
                || (g2 == b'M' && g3 > b'0' && g34 != "19")
                || (g2 == b'N' && g3 > b'7')
                || (g2 == b'Q' && g4 > b'7')
                || g2 == b'R'
        }
        b'R' => {
            in_range(g2, b'A', b'C')
                || (g2 == b'D' && g34 != "47" && g34 != "29" && g34 != "39")
                || (g2 == b'K' && g4 > b'4' && g34 != "39")
                || in_range(g2, b'L', b'N')
                || (g2 == b'O' && g4 == b'0')
                || (g2 == b'Q' && g4 > b'1')
                || g2 == b'R'
        }
        _ => false,
    }
}

fn in_range(value: u8, lo: u8, hi: u8) -> bool {
    value >= lo && value <= hi
}
