//! Mirrors JTDX `lib/ft8sd1.f90`.

use super::ft8v2::packjt77sd::genft8sd;

#[derive(Clone, Debug)]
pub(crate) struct Ft8sd1Result {
    pub(crate) msg37: String,
    pub(crate) msgbits: [u8; 77],
    pub(crate) itone: [i32; 79],
}

#[derive(Clone, Debug)]
struct Ft8sd1Message {
    msg37: String,
    msgbits: [u8; 77],
    itone: [i32; 79],
    idtone: [i32; 58],
}

pub(crate) fn ft8sd1(
    s8: &[[f32; 79]; 8],
    msgd: &str,
    lcq: bool,
    mycall: &str,
) -> Option<Ft8sd1Result> {
    let msgd = msgd.trim();
    let mut lr73 = false;
    let mut lgrid = false;
    let mut direct: Option<Ft8sd1Message> = None;
    let mut msg4: Vec<Ft8sd1Message> = Vec::new();

    if !lcq {
        let (c1, c2, c3) = split_message3(msgd)?;
        if c1.len() < 3 || c2.len() < 3 || c2 == mycall.trim() {
            return None;
        }
        lr73 = c3 == "RR73" || c3 == "73";
        lgrid = !lr73 && c3.len() == 4 && !c3.starts_with("R+") && !c3.starts_with("R-");
        if !lgrid && !lr73 {
            msg4.push(build_message(msgd)?);
            msg4.push(build_message(&format!("{c1} {c2} RRR"))?);
            msg4.push(build_message(&format!("{c1} {c2} RR73"))?);
            msg4.push(build_message(&format!("{c1} {c2} 73"))?);
        }
    }

    if lcq || lgrid || lr73 {
        direct = Some(build_message(msgd)?);
    }

    let mut s8_1 = *s8;
    let mut itonedem = [11i32; 58];
    let mut lmatched = [false; 58];
    demod_pass(&mut s8_1, &mut itonedem, None, true);

    if let Some(message) = direct.as_ref() {
        let (nmatch1, ncrcpaty1) = count_matches(&message.idtone, &itonedem, None);
        if nmatch1 > 29 && ncrcpaty1 > 10 {
            return Some(result_from(message));
        }
        if nmatch1 >= 22 {
            for k in 0..58 {
                lmatched[k] = message.idtone[k] == itonedem[k];
            }
            demod_pass(&mut s8_1, &mut itonedem, Some(&lmatched), false);
            let (nmatch2, ncrcpaty2) =
                extend_matches(&message.idtone, &itonedem, &lmatched, nmatch1, ncrcpaty1);
            if nmatch2 > 41 && ncrcpaty2 > 19 {
                return Some(result_from(message));
            }
        }
        return None;
    }

    let mut imax = None;
    let mut nmatchditer1 = 0usize;
    let mut ncrcpatyiter1 = 0usize;
    for (i, message) in msg4.iter().enumerate() {
        let (nmatch1, ncrcpaty1) = count_matches(&message.idtone, &itonedem, None);
        if nmatch1 > nmatchditer1 {
            imax = Some(i);
            nmatchditer1 = nmatch1;
            ncrcpatyiter1 = ncrcpaty1;
        }
    }
    let imax = imax?;
    if lr73 && imax == 1 {
        return None;
    }
    let message = &msg4[imax];
    if nmatchditer1 > 29 && ncrcpatyiter1 > 10 {
        return Some(result_from(message));
    }
    if nmatchditer1 >= 22 {
        for k in 0..58 {
            lmatched[k] = message.idtone[k] == itonedem[k];
        }
        demod_pass(&mut s8_1, &mut itonedem, Some(&lmatched), false);
        let (nmatch2, ncrcpaty2) = extend_matches(
            &message.idtone,
            &itonedem,
            &lmatched,
            nmatchditer1,
            ncrcpatyiter1,
        );
        if nmatch2 > 41 && ncrcpaty2 > 19 {
            return Some(result_from(message));
        }
    }
    None
}

fn split_message3(msg: &str) -> Option<(&str, &str, &str)> {
    let mut parts = msg.split_whitespace();
    Some((parts.next()?, parts.next()?, parts.next()?))
}

fn build_message(msg: &str) -> Option<Ft8sd1Message> {
    let (msg37, msgbits, itone) = genft8sd(msg)?;
    let mut idtone = [0i32; 58];
    idtone[..29].copy_from_slice(&itone[7..36]);
    idtone[29..].copy_from_slice(&itone[43..72]);
    Some(Ft8sd1Message {
        msg37,
        msgbits,
        itone,
        idtone,
    })
}

fn result_from(message: &Ft8sd1Message) -> Ft8sd1Result {
    Ft8sd1Result {
        msg37: message.msg37.clone(),
        msgbits: message.msgbits,
        itone: message.itone,
    }
}

fn demod_pass(
    s8_1: &mut [[f32; 79]; 8],
    itonedem: &mut [i32; 58],
    lmatched: Option<&[bool; 58]>,
    zero_selected: bool,
) {
    for i in 0..58 {
        if lmatched.is_some_and(|matched| matched[i]) {
            continue;
        }
        let sym = if i < 29 { i + 7 } else { i + 14 };
        let tone = max_tone(s8_1, sym);
        itonedem[i] = tone as i32;
        if zero_selected {
            s8_1[tone][sym] = 0.0;
        }
    }
}

fn max_tone(s8: &[[f32; 79]; 8], sym: usize) -> usize {
    let mut best = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (tone, row) in s8.iter().enumerate() {
        if row[sym] > best_value {
            best_value = row[sym];
            best = tone;
        }
    }
    best
}

fn count_matches(
    idtone: &[i32; 58],
    itonedem: &[i32; 58],
    lmatched: Option<&[bool; 58]>,
) -> (usize, usize) {
    let mut nmatch = 0usize;
    let mut ncrcpaty = 0usize;
    for k in 0..58 {
        if lmatched.is_some_and(|matched| matched[k]) {
            continue;
        }
        if idtone[k] == itonedem[k] {
            nmatch += 1;
            if k >= 25 {
                ncrcpaty += 1;
            }
        }
    }
    (nmatch, ncrcpaty)
}

fn extend_matches(
    idtone: &[i32; 58],
    itonedem: &[i32; 58],
    lmatched: &[bool; 58],
    mut nmatch: usize,
    mut ncrcpaty: usize,
) -> (usize, usize) {
    for k in 0..58 {
        if lmatched[k] {
            continue;
        }
        if idtone[k] == itonedem[k] {
            nmatch += 1;
            if k >= 25 {
                ncrcpaty += 1;
            }
        }
    }
    (nmatch, ncrcpaty)
}
