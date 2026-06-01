//! Mirrors JTDX `lib/ft8sd.f90`.

use super::ft8v2::packjt77sd::genft8sd;

#[derive(Clone, Debug)]
pub(crate) struct Ft8sdResult {
    pub(crate) msg37: String,
    pub(crate) msgbits: [u8; 77],
    pub(crate) itone: [i32; 79],
}

#[derive(Clone, Debug)]
struct Ft8sdMessage {
    msg37: String,
    msgbits: [u8; 77],
    itone: [i32; 79],
    idtone: [i32; 58],
}

pub(crate) fn ft8sd(
    s8: &[[f32; 79]; 8],
    srr: f32,
    msgd: &str,
    lcq: bool,
    mycall: &str,
) -> Option<Ft8sdResult> {
    let msgd = msgd.trim();
    if msgd.contains(" RR73") || msgd.contains(" 73") {
        return None;
    }
    if !(msgd.starts_with("CQ ")
        || msgd.contains('-')
        || msgd.contains('+')
        || msgd.contains(" RRR"))
    {
        return None;
    }

    let mut direct: Option<Ft8sdMessage> = None;
    let mut msg4: Vec<Ft8sdMessage> = Vec::new();
    if lcq {
        direct = Some(build_message(msgd)?);
    } else {
        let (c1, c2) = split_message2(msgd)?;
        if c1.len() < 3 || c2.len() < 3 || c2 == mycall.trim() {
            return None;
        }
        msg4.push(build_message(msgd)?);
        msg4.push(build_message(&format!("{c1} {c2} RRR"))?);
        msg4.push(build_message(&format!("{c1} {c2} RR73"))?);
        msg4.push(build_message(&format!("{c1} {c2} 73"))?);
    }

    let mut s8_1 = *s8;
    let mut itonedem = [11i32; 58];
    let mut lmatched = [false; 58];
    demod_pass(&mut s8_1, &mut itonedem, None, true);

    let mut nmatch1;
    let mut ncrcpaty1;
    let selected: &Ft8sdMessage;
    if let Some(message) = direct.as_ref() {
        let counted = count_matches(&message.idtone, &itonedem, None);
        nmatch1 = counted.0;
        ncrcpaty1 = counted.1;
        for k in 0..58 {
            if message.idtone[k] == itonedem[k] {
                lmatched[k] = true;
            }
        }
        if nmatch1 > 26 {
            return Some(result_from(message));
        }
        selected = message;
    } else {
        let mut imax = None;
        let mut nmatchditer1 = 0usize;
        let mut nbaseiter1 = 0usize;
        let mut ncrcpatyiter1 = 0usize;
        for (i, message) in msg4.iter().enumerate() {
            let mut nmatch = 0usize;
            let mut nbase = 0usize;
            let mut ncrcpaty = 0usize;
            for k in 0..58 {
                if message.idtone[k] == itonedem[k] {
                    nmatch += 1;
                    if k < 22 {
                        nbase += 1;
                    }
                    if k >= 25 {
                        ncrcpaty += 1;
                    }
                }
            }
            if nmatch > nmatchditer1 {
                imax = Some(i);
                nmatchditer1 = nmatch;
                nbaseiter1 = nbase;
                ncrcpatyiter1 = ncrcpaty;
            }
        }
        let imax = imax?;
        if srr > 3.0 && nbaseiter1 < 12 {
            return None;
        }
        selected = &msg4[imax];
        nmatch1 = nmatchditer1;
        ncrcpaty1 = ncrcpatyiter1;
        if nmatch1 > 26 && ncrcpaty1 > 10 {
            return Some(result_from(selected));
        }
        for k in 0..58 {
            if selected.idtone[k] == itonedem[k] {
                lmatched[k] = true;
            }
        }
    }

    if nmatch1 >= 16 {
        let mut history = [0usize; 6];
        history[0] = nmatch1;
        for pass in 2..=6 {
            demod_pass(&mut s8_1, &mut itonedem, Some(&lmatched), true);
            let counted = extend_matches(
                &selected.idtone,
                &itonedem,
                &mut lmatched,
                nmatch1,
                ncrcpaty1,
            );
            nmatch1 = counted.0;
            ncrcpaty1 = counted.1;
            history[pass - 1] = nmatch1;

            let accept = match pass {
                2 => nmatch1 > 38 && ncrcpaty1 > 19,
                3 => nmatch1 > 44 && ncrcpaty1 > 21,
                4 => nmatch1 > 47 && ncrcpaty1 > 23,
                5 => {
                    nmatch1 > 50
                        && (history[0] > 21
                            || history[1] > 31
                            || history[2] > 38
                            || history[3] > 46)
                        && ncrcpaty1 > 25
                }
                6 => {
                    nmatch1 > 54
                        && (history[0] > 22
                            || history[1] > 27
                            || history[2] > 35
                            || history[1].saturating_sub(history[0]) > 9
                            || history[2].saturating_sub(history[1]) > 10)
                        && ncrcpaty1 > 29
                }
                _ => false,
            };
            if accept {
                return Some(result_from(selected));
            }
        }
    }

    None
}

fn split_message2(msg: &str) -> Option<(&str, &str)> {
    let mut parts = msg.split_whitespace();
    Some((parts.next()?, parts.next()?))
}

fn build_message(msg: &str) -> Option<Ft8sdMessage> {
    let (msg37, msgbits, itone) = genft8sd(msg)?;
    let mut idtone = [0i32; 58];
    idtone[..29].copy_from_slice(&itone[7..36]);
    idtone[29..].copy_from_slice(&itone[43..72]);
    Some(Ft8sdMessage {
        msg37,
        msgbits,
        itone,
        idtone,
    })
}

fn result_from(message: &Ft8sdMessage) -> Ft8sdResult {
    Ft8sdResult {
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
    lmatched: &mut [bool; 58],
    mut nmatch: usize,
    mut ncrcpaty: usize,
) -> (usize, usize) {
    for k in 0..58 {
        if lmatched[k] {
            continue;
        }
        if idtone[k] == itonedem[k] {
            nmatch += 1;
            lmatched[k] = true;
            if k >= 25 {
                ncrcpaty += 1;
            }
        }
    }
    (nmatch, ncrcpaty)
}
