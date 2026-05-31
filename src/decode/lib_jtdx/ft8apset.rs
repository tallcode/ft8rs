//! Mirrors JTDX `lib/ft8apset.f90`.

use crate::stream::session::StreamDecodeConfig;

use super::ft8v2::bpdecode174_91::N;
use super::ft8v2::packjt77::pack77;

pub(crate) struct ApMaskPlan {
    pub(crate) message77: [u8; 77],
    pub(crate) apmask: [i8; N],
}

pub(crate) fn build_ap_mask(config: &StreamDecodeConfig, iaptype: i32) -> Option<ApMaskPlan> {
    let mycall = normalized_call(config.mycall.as_deref());
    let hiscall = normalized_call(config.hiscall.as_deref());
    let grid = config
        .hisgrid
        .as_deref()
        .map(str::trim)
        .filter(|grid| grid.len() >= 4)
        .map(|grid| grid[..4].to_ascii_uppercase());

    let (template, ranges, expected_i3): (String, &[(usize, usize)], usize) = match iaptype {
        1 => ("CQ K1ABC AA00".to_string(), &[(0, 29), (74, 77)], 1),
        2 => (
            format!("{} K1ABC AA00", mycall.as_deref()?),
            &[(0, 29), (74, 77)],
            1,
        ),
        3 => (
            format!("{} {} +00", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 58), (74, 77)],
            1,
        ),
        4 => (
            format!("{} {} RRR", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            1,
        ),
        5 => (
            format!("{} {} 73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            1,
        ),
        6 => (
            format!("{} {} RR73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            1,
        ),
        11 | 41 => (
            format!("{} <{}> +00", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 58), (74, 77)],
            1,
        ),
        12 => (
            format!("<{}> {} RRR", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            4,
        ),
        13 => (
            format!("<{}> {} 73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            4,
        ),
        14 => (
            format!("<{}> {} RR73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            4,
        ),
        21 => (
            format!("{} {} +00", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 58), (74, 77)],
            1,
        ),
        22 => (
            hound_special_template(mycall.as_deref()?, hiscall.as_deref()),
            &[(28, 66), (71, 77)],
            0,
        ),
        23 => (
            format!("{} {} RR73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            1,
        ),
        24 => (
            hound_special_template(mycall.as_deref()?, hiscall.as_deref()),
            &[(0, 28), (56, 66), (71, 77)],
            0,
        ),
        31 => {
            if let Some(grid) = grid {
                (format!("CQ {} {grid}", hiscall.as_deref()?), &[(0, 77)], 1)
            } else {
                (
                    format!("CQ {} AA00", hiscall.as_deref()?),
                    &[(0, 58), (74, 77)],
                    1,
                )
            }
        }
        35 => (
            format!("{} {} 73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(29, 77)],
            1,
        ),
        36 => (
            format!("{} {} RR73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(29, 77)],
            1,
        ),
        40 => (
            format!("<{}> {} +00", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 29), (74, 77)],
            1,
        ),
        42 => (
            format!("{} <{}> RRR", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            4,
        ),
        43 => (
            format!("{} <{}> 73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            4,
        ),
        44 => (
            format!("{} <{}> RR73", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 77)],
            4,
        ),
        111 => return Some(hound_cq_type4_mask()),
        _ => return None,
    };

    let packed = pack77(&template);
    if packed.len() != 77 {
        return None;
    }
    if bits_to_usize(&packed[74..77]) != expected_i3 {
        return None;
    }
    let mut message77 = [0u8; 77];
    message77.copy_from_slice(&packed[..77]);
    let mut apmask = [0i8; N];
    for &(start, end) in ranges {
        for i in start..end.min(77) {
            apmask[i] = 1;
        }
    }
    Some(ApMaskPlan { message77, apmask })
}

fn hound_cq_type4_mask() -> ApMaskPlan {
    let mut message77 = [0u8; 77];
    let mut apmask = [0i8; N];
    for i in 71..77 {
        apmask[i] = 1;
    }
    message77[73] = 1;
    message77[74] = 1;
    ApMaskPlan { message77, apmask }
}

fn hound_special_template(mycall: &str, hiscall: Option<&str>) -> String {
    let hiscall = hiscall.unwrap_or(mycall);
    format!("{mycall} RR73; {mycall} <{hiscall}> -16")
}

fn normalized_call(call: Option<&str>) -> Option<String> {
    let call = call?.trim().trim_start_matches('<').trim_end_matches('>');
    if call.len() < 3 {
        return None;
    }
    Some(call.to_ascii_uppercase())
}

fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}
