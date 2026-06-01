//! Mirrors JTDX `lib/ft8s.f90`.

use super::ft8v2::packjt77sd::genft8sd;
use super::tone8::Tone8Tables;

#[derive(Clone, Debug)]
pub(crate) struct Ft8sResult {
    pub(crate) msg37: String,
    pub(crate) msgbits: [u8; 77],
    pub(crate) itone: [i32; 79],
}

#[derive(Clone, Debug)]
struct Ft8sMessage {
    msg37: String,
    msgbits: [u8; 77],
    itone: [i32; 79],
    idtone: [i32; 58],
}

pub(crate) fn ft8s(
    s8: &[[f32; 79]; 8],
    srr: f32,
    nft8rxfsens: usize,
    stophint: bool,
    mycall: &str,
    hiscall: &str,
    nlasttx: usize,
    lastrxmsg: Option<&str>,
    tone8_tables: Option<&Tone8Tables>,
) -> Option<Ft8sResult> {
    if stophint || hiscall.trim().len() < 3 || nlasttx == 6 || nlasttx == 0 {
        return None;
    }

    let mut nft8rxfslow = nft8rxfsens.clamp(1, 3);
    let lastrcvdmsg = lastrxmsg.unwrap_or("").trim();
    let mut lmycall = false;
    let mut lhiscall = false;
    let mut lrrr = false;
    let mut lr73 = false;
    let mut lcallingrprt = false;
    let mut lastrrprt = false;
    let mut lastreport = false;
    let mut lgrid = false;

    if !lastrcvdmsg.is_empty() {
        lhiscall = lastrcvdmsg.contains(hiscall.trim());
        lmycall = lastrcvdmsg.starts_with(mycall.trim());
        if lmycall && lhiscall {
            lrrr = lastrcvdmsg.contains(" RRR");
            lr73 = lastrcvdmsg.contains(" RR73") || lastrcvdmsg.contains(" 73");
        }
    } else if nlasttx == 2 {
        lcallingrprt = true;
    }

    if lmycall && lhiscall {
        lastreport = lastrcvdmsg.contains(" +") || lastrcvdmsg.contains(" -");
        lastrrprt = lastrcvdmsg.contains(" R+") || lastrcvdmsg.contains(" R-");
    }
    if lastrxmsg.is_some() && lmycall && lhiscall && !lastreport && !lastrrprt && !lrrr && !lr73 {
        lgrid = true;
    }
    if lastrxmsg.is_some() && !lmycall && lhiscall && nlasttx == 2 {
        lcallingrprt = true;
    }

    let mut messages =
        ft8s_messages_from_tone8(tone8_tables).or_else(|| build_ft8s_messages(mycall, hiscall))?;
    if lgrid {
        let base = format!("{} {} AA00", mycall.trim(), hiscall.trim());
        if messages[52].msg37.trim() == base {
            if let Some(message) = build_message(lastrcvdmsg) {
                messages[52] = message;
            }
        }
    }

    let mut s8_1 = *s8;
    let mut lmatched = [false; 58];
    let mut itonedem = [11i32; 58];
    demod_pass(&mut s8_1, &mut itonedem, None, true);

    let mut nmycall1 = 0usize;
    let mut nbase1 = 0usize;
    for k in 0..19 {
        if messages[0].idtone[k] == itonedem[k] {
            if k < 9 {
                nmycall1 += 1;
            }
            nbase1 += 1;
        }
    }

    let (ilow, ihigh_exclusive) = if nlasttx == 1 {
        (0usize, 26usize)
    } else if lgrid {
        (26usize, 53usize)
    } else if lcallingrprt {
        (26usize, 52usize)
    } else if lastrrprt {
        (26usize, 56usize)
    } else if lr73 || lrrr {
        (53usize, 56usize)
    } else {
        (0usize, 56usize)
    };

    let mut imax = None;
    let mut nmatchditer1 = 0usize;
    let mut ncrcpatyiter1 = 0usize;
    for i in ilow..ihigh_exclusive {
        if lastreport && i >= 26 && i < 53 {
            continue;
        }
        let (nmatch1, ncrcpaty1) = count_matches(&messages[i].idtone, &itonedem, None);
        if nmatch1 > nmatchditer1 {
            imax = Some(i);
            nmatchditer1 = nmatch1;
            ncrcpatyiter1 = ncrcpaty1;
        }
    }

    let imax = imax?;
    if srr > 3.0 && (nmycall1 < 5 || nbase1 < 10) {
        return None;
    }
    if (nlasttx == 1 || lcallingrprt) && nmycall1 == 0 {
        return None;
    }
    if lr73 && (imax == 52 || imax == 53) {
        return None;
    }

    let mut nmatch1 = nmatchditer1;
    let mut ncrcpaty1 = ncrcpatyiter1;
    let idtone = messages[imax].idtone;
    for k in 0..58 {
        if idtone[k] == itonedem[k] {
            lmatched[k] = true;
        }
    }
    if ncrcpaty1 < 8 {
        return None;
    }

    let s8d = data_symbols(s8);
    let ranks = tone_ranks(&s8d);
    let ref0 = ref_sum(&s8d, &ranks, None);
    let ref0paty = ref_sum(&s8d, &ranks, Some(RefPart::Parity));
    let ref0mycl = ref_sum(&s8d, &ranks, Some(RefPart::MyCall));
    let ref0oth = ref0mycl + ref0paty;

    let mut ipk = None;
    let mut u1 = 0.0f32;
    let mut u2 = 0.0f32;
    let mut u1paty = 0.0f32;
    let mut u2paty = 0.0f32;
    let mut u1oth = 0.0f32;
    let mut u2oth = 0.0f32;

    for k in ilow..ihigh_exclusive {
        if lastreport && k >= 26 && k < 53 {
            continue;
        }
        let (psum, refv) = score_message(&s8d, &ranks, &messages[k].idtone, ref0, None);
        let (psumpaty, refpaty) = score_message(
            &s8d,
            &ranks,
            &messages[k].idtone,
            ref0paty,
            Some(RefPart::Parity),
        );
        let (psumoth, refoth) = score_message(
            &s8d,
            &ranks,
            &messages[k].idtone,
            ref0oth,
            Some(RefPart::Other),
        );
        let p = psum / refv.max(1.0e-6);
        let ppaty = psumpaty / refpaty.max(1.0e-6);
        let poth = psumoth / refoth.max(1.0e-6);
        if p > u1 {
            u2 = u1;
            u1 = p;
            u2paty = u1paty;
            u1paty = ppaty;
            u2oth = u1oth;
            u1oth = poth;
            ipk = Some(k);
        } else if p > u2 {
            u2 = p;
            u2paty = ppaty;
            u2oth = poth;
        }
    }

    let ipk = ipk.unwrap_or(usize::MAX);
    if lastrrprt && ipk == 52 {
        return None;
    }
    if lr73 && ipk < 54 {
        return None;
    }
    if lcallingrprt || nlasttx == 1 {
        nft8rxfslow = 1;
    }

    let qual = 100.0 * (u1 - u2);
    let qualp = 100.0 * (u1paty - u2paty);
    let qualo = 100.0 * (u1oth - u2oth);
    let thresh = (qual + 10.0) * (u1 - 0.6);
    let threshp = (qualp + 10.0) * (u1paty - 0.6);
    let thresho = (qualo + 10.0) * (u1oth - 0.6);

    if thresh >= 1.5
        && (!(lcallingrprt || nlasttx == 1) || thresho >= 3.43)
        && thresho >= 2.63
        && threshp >= 2.45
        && (((nft8rxfslow == 1 && thresh > 4.0)
            || (nft8rxfslow == 2 && thresh > 3.55)
            || (nft8rxfslow == 3 && thresh > 3.0))
            && qual > 2.6
            && u1 > 0.77)
    {
        return validate_ft8s(s8, lcallingrprt, nlasttx, &messages[ipk]);
    }

    if imax == ipk
        && (nft8rxfslow > 1 || (nft8rxfslow == 1 && thresh > 2.7))
        && srr < 7.0
        && (ncrcpaty1 > 14 || (nmatch1 > 22 && ncrcpaty1 > 13))
    {
        return validate_ft8s(s8, lcallingrprt, nlasttx, &messages[imax]);
    }

    let ntresh1 = if srr > 7.0 { 29 } else { 26 };
    let ntresh2 = if srr > 7.0 { 41 } else { 38 };
    if imax == ipk && nmatch1 > ntresh1 && ncrcpaty1 > 10 {
        return validate_ft8s(s8, lcallingrprt, nlasttx, &messages[imax]);
    }

    if nmatchditer1 >= 16 {
        let mut nmatch_by_pass = [0usize; 7];
        let mut ncrcpaty_by_pass = [0usize; 7];
        nmatch_by_pass[1] = nmatch1;
        ncrcpaty_by_pass[1] = ncrcpaty1;

        for pass in 2..=6 {
            demod_pass(&mut s8_1, &mut itonedem, Some(&lmatched), true);
            let (nmatch, ncrcpaty) =
                extend_matches(&idtone, &itonedem, &mut lmatched, nmatch1, ncrcpaty1);
            nmatch1 = nmatch;
            ncrcpaty1 = ncrcpaty;
            nmatch_by_pass[pass] = nmatch1;
            ncrcpaty_by_pass[pass] = ncrcpaty1;

            let accept = match pass {
                2 => nmatch1 > ntresh2 && ncrcpaty1 > 19,
                3 => nmatch1 > 44 && ncrcpaty1 > 21,
                4 => {
                    (nft8rxfslow == 3 || (nft8rxfslow == 2 && thresh > 2.2))
                        && nmatch1 > 47
                        && ncrcpaty1 > 23
                }
                5 => {
                    (nft8rxfslow == 3
                        || (nft8rxfslow == 1 && thresh > 3.4)
                        || (nft8rxfslow == 2 && thresh > 3.25))
                        && nmatch1 > 50
                        && (nmatch_by_pass[1] > 21
                            || nmatch_by_pass[2] > 31
                            || nmatch_by_pass[3] > 38
                            || nmatch_by_pass[4] > 46)
                        && ncrcpaty1 > 25
                }
                6 => {
                    let branch61 = ((nft8rxfslow == 2 && thresh > 2.6)
                        || (nft8rxfslow == 3 && thresh > 2.22))
                        && imax == ipk
                        && ncrcpaty1 > 26;
                    let branch62 = nft8rxfslow == 1
                        && nmatch1 > 54
                        && (nmatch_by_pass[1] > 22
                            || nmatch_by_pass[2] > 27
                            || nmatch_by_pass[3] > 35
                            || nmatch_by_pass[2].saturating_sub(nmatch_by_pass[1]) > 9
                            || nmatch_by_pass[3].saturating_sub(nmatch_by_pass[2]) > 10)
                        && ncrcpaty1 > 29
                        && thresh > 3.15;
                    let branch63_sensitivity =
                        nft8rxfslow == 3 || (nft8rxfslow == 3 && thresh > 1.94);
                    let branch63 = ncrcpaty1 > 29
                        && branch63_sensitivity
                        && imax == ipk
                        && ncrcpaty_by_pass[2].saturating_sub(ncrcpaty_by_pass[1]) > 3
                        && ncrcpaty_by_pass[3].saturating_sub(ncrcpaty_by_pass[2]) > 3
                        && ncrcpaty_by_pass[4].saturating_sub(ncrcpaty_by_pass[3]) > 3
                        && ncrcpaty_by_pass[6].saturating_sub(ncrcpaty_by_pass[5]) < 6;

                    branch61 || branch62 || branch63
                }
                _ => false,
            };
            if accept {
                return validate_ft8s(s8, lcallingrprt, nlasttx, &messages[imax]);
            }
            if pass == 2 && srr > 7.0 {
                return None;
            }
        }
    }

    None
}

fn ft8s_messages_from_tone8(tables: Option<&Tone8Tables>) -> Option<Vec<Ft8sMessage>> {
    let tables = tables?;
    if tables.msg56.len() != 56
        || tables.msgbits56.len() != 56
        || tables.itone56.len() != 56
        || tables.idtone56.len() != 56
    {
        return None;
    }
    let mut out = Vec::with_capacity(56);
    for i in 0..56 {
        out.push(Ft8sMessage {
            msg37: tables.msg56[i].clone(),
            msgbits: tables.msgbits56[i],
            itone: tables.itone56[i],
            idtone: tables.idtone56[i],
        });
    }
    Some(out)
}

fn build_ft8s_messages(mycall: &str, hiscall: &str) -> Option<Vec<Ft8sMessage>> {
    const RPT: [&str; 56] = [
        "-01", "-02", "-03", "-04", "-05", "-06", "-07", "-08", "-09", "-10", "-11", "-12", "-13",
        "-14", "-15", "-16", "-17", "-18", "-19", "-20", "-21", "-22", "-23", "-24", "-25", "-26",
        "R-01", "R-02", "R-03", "R-04", "R-05", "R-06", "R-07", "R-08", "R-09", "R-10", "R-11",
        "R-12", "R-13", "R-14", "R-15", "R-16", "R-17", "R-18", "R-19", "R-20", "R-21", "R-22",
        "R-23", "R-24", "R-25", "R-26", "AA00", "RRR", "RR73", "73",
    ];
    let mut out = Vec::with_capacity(RPT.len());
    for rpt in RPT {
        out.push(build_message(&format!(
            "{} {} {}",
            mycall.trim(),
            hiscall.trim(),
            rpt
        ))?);
    }
    Some(out)
}

fn build_message(msg: &str) -> Option<Ft8sMessage> {
    let (msg37, msgbits, itone) = genft8sd(msg)?;
    let mut idtone = [0i32; 58];
    idtone[..29].copy_from_slice(&itone[7..36]);
    idtone[29..].copy_from_slice(&itone[43..72]);
    Some(Ft8sMessage {
        msg37,
        msgbits,
        itone,
        idtone,
    })
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
        let sym = data_symbol_index(i);
        let tone = max_tone(s8_1, sym);
        itonedem[i] = tone as i32;
        if zero_selected {
            s8_1[tone][sym] = 0.0;
        }
    }
}

fn data_symbol_index(i: usize) -> usize {
    if i < 29 {
        i + 7
    } else {
        i + 14
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

fn data_symbols(s8: &[[f32; 79]; 8]) -> [[f32; 58]; 8] {
    let mut out = [[0.0f32; 58]; 8];
    for i in 0..58 {
        let sym = data_symbol_index(i);
        for tone in 0..8 {
            out[tone][i] = s8[tone][sym];
        }
    }
    out
}

#[derive(Clone, Copy)]
enum RefPart {
    Parity,
    MyCall,
    Other,
}

fn in_part(j: usize, part: Option<RefPart>) -> bool {
    match part {
        None => true,
        Some(RefPart::Parity) => j >= 26,
        Some(RefPart::MyCall) => j < 9,
        Some(RefPart::Other) => j < 9 || j >= 26,
    }
}

fn tone_ranks(s8d: &[[f32; 58]; 8]) -> [[usize; 4]; 58] {
    let mut ranks = [[0usize; 4]; 58];
    for j in 0..58 {
        let mut used = [false; 8];
        for r in 0..4 {
            let mut best = 0usize;
            let mut best_value = f32::NEG_INFINITY;
            for tone in 0..8 {
                if !used[tone] && s8d[tone][j] > best_value {
                    best_value = s8d[tone][j];
                    best = tone;
                }
            }
            ranks[j][r] = best;
            used[best] = true;
        }
    }
    ranks
}

fn ref_sum(s8d: &[[f32; 58]; 8], ranks: &[[usize; 4]; 58], part: Option<RefPart>) -> f32 {
    let mut sum = 0.0;
    for j in 0..58 {
        if in_part(j, part) {
            sum += s8d[ranks[j][0]][j];
        }
    }
    sum
}

fn score_message(
    s8d: &[[f32; 58]; 8],
    ranks: &[[usize; 4]; 58],
    idtone: &[i32; 58],
    ref0: f32,
    part: Option<RefPart>,
) -> (f32, f32) {
    let mut psum = 0.0;
    let mut refv = ref0;
    for j in 0..58 {
        if !in_part(j, part) {
            continue;
        }
        let tone = idtone[j] as usize;
        psum += s8d[tone][j];
        for r in 0..3 {
            if tone == ranks[j][r] {
                let stmp = s8d[ranks[j][r + 1]][j] - s8d[tone][j];
                refv += stmp;
                break;
            }
        }
    }
    (psum, refv)
}

fn validate_ft8s(
    s8: &[[f32; 79]; 8],
    lcallingrprt: bool,
    nlasttx: usize,
    message: &Ft8sMessage,
) -> Option<Ft8sResult> {
    let mut snr = 0.0f32;
    let mut snrbase = 0.0f32;
    let mut snrmycall = 0.0f32;
    let mut snrpaty = 0.0f32;
    for i in 0..79 {
        let tone = message.itone[i] as usize;
        let xsig = s8[tone][i];
        let xnoi = (sum_symbol(s8, i) - xsig) / 7.0;
        let snr1 = xsig / (xnoi + 1.0e-6);
        snr += snr1;
        if i > 6 && i < 33 {
            snrbase += snr1;
            if i < 16 {
                snrmycall += snr1;
            }
        }
        if (i > 42 && i < 72) || (i > 32 && i < 36) {
            snrpaty += snr1;
        }
    }
    let snrdata = snrbase + snrpaty;
    let snrsync = (snr - snrdata) / 21.0;
    let snrother = (snrmycall + snrpaty) / 48.0;
    let snrpaty = snrpaty / 32.0;
    if lcallingrprt || nlasttx == 1 {
        let soratio = snrsync / snrother.max(1.0e-6);
        if soratio > 1.29 {
            return None;
        }
    }
    let spratio = snrsync / snrpaty.max(1.0e-6);
    if !(0.6..=1.25).contains(&spratio) {
        return None;
    }

    let mut xsync = [0.0f32; 21];
    for i in 0..7 {
        xsync[i] = s8[message.itone[i] as usize][i];
        let k2 = i + 36;
        xsync[i + 7] = s8[message.itone[k2] as usize][k2];
        let k3 = i + 72;
        xsync[i + 14] = s8[message.itone[k3] as usize][k3];
    }
    let mut xdata = [0.0f32; 58];
    for i in 0..58 {
        let sym = data_symbol_index(i);
        xdata[i] = s8[message.itone[sym] as usize][sym];
    }
    let mut xnoise = [0.0f32; 79];
    for i in 0..79 {
        let tone = ((message.itone[i] + 4).rem_euclid(8)) as usize;
        xnoise[i] = s8[tone][i];
    }

    let mut xmsync = median3_series_21(&xsync);
    xmsync[19] = xmsync[17];
    xmsync[20] = xmsync[18];
    let mut xmdata = median3_series_58(&xdata);
    xmdata[56] = xmdata[54];
    xmdata[57] = xmdata[55];
    let mut xmnoise = median3_series_79(&xnoise);
    xmnoise[77] = xmnoise[75];
    xmnoise[78] = xmnoise[76];

    let ssync = xmsync.iter().sum::<f32>() / 21.0;
    let spaty = xmdata[26..58].iter().sum::<f32>() / 32.0;
    let spnoise =
        (xmnoise[33..36].iter().sum::<f32>() + xmnoise[43..72].iter().sum::<f32>()) / 32.0;
    let spother = (xmdata[..9].iter().sum::<f32>() + xmdata[26..58].iter().sum::<f32>()) / 41.0;
    let spratiom = ssync / (spaty + 1.0e-6);
    let spnratiom = spaty / (spnoise + 1.0e-6);
    let sporatiom = ssync / (spother + 1.0e-6);
    if spnratiom > 2.3 {
        if lcallingrprt || nlasttx == 1 {
            if sporatiom > 1.35 {
                return None;
            }
        } else if spratiom > 1.35 {
            return None;
        }
    }

    Some(Ft8sResult {
        msg37: message.msg37.clone(),
        msgbits: message.msgbits,
        itone: message.itone,
    })
}

fn sum_symbol(s8: &[[f32; 79]; 8], sym: usize) -> f32 {
    s8.iter().map(|row| row[sym]).sum()
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
    if (a > b && a < c) || (a < b && a > c) {
        a
    } else if (b > a && b < c) || (b < a && b > c) {
        b
    } else if (c > a && c < b) || (c < a && c > b) {
        c
    } else {
        a
    }
}

fn median3_series_21(input: &[f32; 21]) -> [f32; 21] {
    let mut out = [0.0f32; 21];
    for i in 0..19 {
        out[i] = median3(input[i], input[i + 1], input[i + 2]);
    }
    out
}

fn median3_series_58(input: &[f32; 58]) -> [f32; 58] {
    let mut out = [0.0f32; 58];
    for i in 0..56 {
        out[i] = median3(input[i], input[i + 1], input[i + 2]);
    }
    out
}

fn median3_series_79(input: &[f32; 79]) -> [f32; 79] {
    let mut out = [0.0f32; 79];
    for i in 0..77 {
        out[i] = median3(input[i], input[i + 1], input[i + 2]);
    }
    out
}
