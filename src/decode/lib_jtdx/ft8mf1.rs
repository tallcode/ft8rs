//! Mirrors JTDX `lib/ft8mf1.f90`.

use super::tonesd::tonesd_messages;

#[derive(Clone, Debug)]
pub(crate) struct Ft8mf1Result {
    pub(crate) msg37: String,
    pub(crate) msgbits: [u8; 77],
    pub(crate) itone: [i32; 79],
}

pub(crate) fn ft8mf1(s8: &[[f32; 79]; 8], msgd: &str) -> Option<Ft8mf1Result> {
    let msgd = msgd.trim();
    let (c1, c2, c3) = split_message3(msgd)?;
    if c1.len() < 3 || c2.len() < 3 {
        return None;
    }
    let lr73 = c3 == "RR73" || c3 == "73";
    let lgrid = !lr73 && c3.len() == 4 && !c3.starts_with("R+") && !c3.starts_with("R-");
    let lreport = c3.starts_with('+') || c3.starts_with('-');
    let lrreport = c3.starts_with("R+") || c3.starts_with("R-");
    let lrrr = c3 == "RRR";

    let messages = tonesd_messages(c1, c2, if lgrid { c3 } else { "AA00" })?;
    let s8d = data_symbols(s8);
    let ranks = tone_ranks(&s8d);
    let ref0 = (0..58).map(|j| s8d[ranks[j][0]][j]).sum::<f32>();

    let mut ipk = None;
    let mut u1 = 0.0f32;
    let mut u2 = 0.0f32;
    for (k, message) in messages.iter().enumerate() {
        let (psum, refv) = score_message(&s8d, &ranks, &message.idtone, ref0);
        let p = psum / refv.max(1.0e-6);
        if p > u1 {
            u2 = u1;
            u1 = p;
            ipk = Some(k);
        } else if p > u2 {
            u2 = p;
        }
    }
    let ipk = ipk?;
    let selected = &messages[ipk];

    if lgrid {
        if ipk == 75 {
            let (_, _, grid) = split_message3(&selected.msg37)?;
            if grid == "AA00" {
                return None;
            }
        }
        if ipk < 36 || (ipk > 71 && ipk < 75) {
            return None;
        }
    }
    if lreport && ((ipk > 35 && ipk < 72) || ipk == 75) {
        return None;
    }
    if lrreport && (ipk < 36 || ipk == 72 || ipk == 75) {
        return None;
    }
    if lrrr && (ipk < 72 || ipk == 75) {
        return None;
    }
    if lr73 && (ipk < 73 || ipk == 75) {
        return None;
    }

    let qual = 100.0 * (u1 - u2);
    let thresh = (qual + 10.0) * (u1 - 0.6);
    if thresh > 4.0 && qual > 2.6 && u1 > 0.77 {
        Some(Ft8mf1Result {
            msg37: selected.msg37.clone(),
            msgbits: selected.msgbits,
            itone: selected.itone,
        })
    } else {
        None
    }
}

fn split_message3(msg: &str) -> Option<(&str, &str, &str)> {
    let mut parts = msg.split_whitespace();
    Some((parts.next()?, parts.next()?, parts.next()?))
}

fn data_symbols(s8: &[[f32; 79]; 8]) -> [[f32; 58]; 8] {
    let mut out = [[0.0f32; 58]; 8];
    for i in 0..58 {
        let sym = if i < 29 { i + 7 } else { i + 14 };
        for tone in 0..8 {
            out[tone][i] = s8[tone][sym];
        }
    }
    out
}

fn tone_ranks(s8d: &[[f32; 58]; 8]) -> [[usize; 2]; 58] {
    let mut ranks = [[0usize; 2]; 58];
    for j in 0..58 {
        let mut first = 0usize;
        let mut second = 0usize;
        let mut first_v = f32::NEG_INFINITY;
        let mut second_v = f32::NEG_INFINITY;
        for tone in 0..8 {
            let value = s8d[tone][j];
            if value > first_v {
                second = first;
                second_v = first_v;
                first = tone;
                first_v = value;
            } else if value > second_v {
                second = tone;
                second_v = value;
            }
        }
        ranks[j] = [first, second];
    }
    ranks
}

fn score_message(
    s8d: &[[f32; 58]; 8],
    ranks: &[[usize; 2]; 58],
    idtone: &[i32; 58],
    ref0: f32,
) -> (f32, f32) {
    let mut psum = 0.0;
    let mut refv = ref0;
    for j in 0..58 {
        let tone = idtone[j] as usize;
        psum += s8d[tone][j];
        if tone == ranks[j][0] {
            refv = refv - s8d[tone][j] + s8d[ranks[j][1]][j];
        }
    }
    (psum, refv)
}
