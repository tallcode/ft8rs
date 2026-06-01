//! Mirrors JTDX `lib/ft8mfcq.f90`.

use super::ft8v2::packjt77sd::genft8sd;

#[derive(Clone, Debug)]
pub(crate) struct Ft8mfcqResult {
    pub(crate) msg37: String,
    pub(crate) msgbits: [u8; 77],
    pub(crate) itone: [i32; 79],
}

#[derive(Clone, Debug)]
struct Ft8mfcqMessage {
    msg37: String,
    msgbits: [u8; 77],
    itone: [i32; 79],
    idtone: [i32; 58],
}

pub(crate) fn ft8mfcq(s8: &[[f32; 79]; 8], msgd: &str) -> Option<Ft8mfcqResult> {
    if msgd.trim().len() < 6 {
        return None;
    }
    let messages = cq25_messages(msgd)?;
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

    if ipk != Some(0) {
        return None;
    }
    let qual = 100.0 * (u1 - u2);
    let thresh = (qual + 10.0) * (u1 - 0.6);
    if thresh > 4.0 && qual > 2.6 && u1 > 0.77 {
        let selected = &messages[0];
        Some(Ft8mfcqResult {
            msg37: selected.msg37.clone(),
            msgbits: selected.msgbits,
            itone: selected.itone,
        })
    } else {
        None
    }
}

fn cq25_messages(msgd: &str) -> Option<Vec<Ft8mfcqMessage>> {
    const MSGCQ25: [&str; 24] = [
        "CQ 2E0DLA IO92",
        "CQ BH3NEB ON81",
        "CQ CG3CGT FN04",
        "CQ DX CT1JA IM59",
        "CQ CU20E",
        "CQ NA CX1OB GF14",
        "CQ DF2AJ JN49",
        "CQ DG4XPZ JN58",
        "CQ EA8XR IL19",
        "CQ F1YE IN94",
        "CQ DX G1KLN IO82",
        "CQ HL2KVF PM38",
        "CQ IU1ZSV JN45",
        "CQ JG1TWO PM96",
        "CQ K2ST EM96",
        "CQ N9TUX EL98",
        "CQ SA NO2FA FM18",
        "CQ OH6GKE KP13",
        "CQ PD0ORM JO24",
        "CQ DX PT7DS HI06",
        "CQ RA3XEP KO84",
        "CQ SM2GSH KP05",
        "CQ UA9OHX NO15",
        "CQ JA W0YH EN12",
    ];
    let mut out = Vec::with_capacity(25);
    out.push(build_message(msgd)?);
    for msg in MSGCQ25 {
        out.push(build_message(msg)?);
    }
    Some(out)
}

fn build_message(msg: &str) -> Option<Ft8mfcqMessage> {
    let (msg37, msgbits, itone) = genft8sd(msg)?;
    let mut idtone = [0i32; 58];
    idtone[..29].copy_from_slice(&itone[7..36]);
    idtone[29..].copy_from_slice(&itone[43..72]);
    Some(Ft8mfcqMessage {
        msg37,
        msgbits,
        itone,
        idtone,
    })
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
