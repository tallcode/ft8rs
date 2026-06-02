//! Mirrors JTDX `lib/ft8apset.f90`.

use crate::stream::session::StreamDecodeConfig;

use super::ft8_mod1::{NAPTYPES, NDXNSAPTYPES, NHAPTYPES, NMYCNSAPTYPES};
use super::ft8v2::bpdecode174_91::N;
use super::ft8v2::packjt77::pack77;

#[derive(Clone)]
pub(crate) struct ApMaskPlan {
    pub(crate) message77: [u8; 77],
    pub(crate) apmask: [i8; N],
}

#[derive(Clone, Default)]
pub(crate) struct Ft8ApSet {
    plans: Vec<(i32, Option<ApMaskPlan>)>,
}

impl Ft8ApSet {
    pub(crate) fn get(&self, iaptype: i32) -> Option<ApMaskPlan> {
        self.plans
            .iter()
            .find_map(|(key, plan)| (*key == iaptype).then(|| plan.clone()))
            .flatten()
    }
}

pub(crate) fn ft8apset(config: &StreamDecodeConfig) -> Ft8ApSet {
    let mut plans = Vec::new();
    for iaptype in ap_types_for_config(config) {
        if !plans.iter().any(|(key, _)| *key == iaptype) {
            plans.push((iaptype, build_ap_mask(config, iaptype)));
        }
    }
    Ft8ApSet { plans }
}

fn ap_types_for_config(config: &StreamDecodeConfig) -> Vec<i32> {
    let mycall = normalized_call(config.mycall.as_deref());
    let hiscall = normalized_call(config.hiscall.as_deref());
    let lnohiscall = hiscall.is_none();
    let lmycallstd =
        mycall.is_some() && !is_nonstandard_call(config.mycall.as_deref().unwrap_or(""));
    let lhiscallstd = !lnohiscall && !is_nonstandard_call(config.hiscall.as_deref().unwrap_or(""));
    let table = if config.lhound {
        &NHAPTYPES
    } else if lmycallstd && (lhiscallstd || lnohiscall) {
        &NAPTYPES
    } else if lmycallstd && !lhiscallstd && !lnohiscall {
        &NDXNSAPTYPES
    } else if !lmycallstd && !lhiscallstd && !lnohiscall {
        &NDXNSAPTYPES
    } else {
        &NMYCNSAPTYPES
    };
    table
        .iter()
        .flat_map(|row| row.iter().copied())
        .filter(|&iaptype| iaptype != 0)
        .collect()
}

fn build_ap_mask(config: &StreamDecodeConfig, iaptype: i32) -> Option<ApMaskPlan> {
    let mycall = normalized_call(config.mycall.as_deref());
    let hiscall = normalized_call(config.hiscall.as_deref());
    let mybcall = base_call(config.mycall.as_deref());
    let hisbcall = base_call(config.hiscall.as_deref());
    let mycall_raw = config.mycall.as_deref().unwrap_or("");
    let hiscall_raw = config.hiscall.as_deref().unwrap_or("");
    let lmycallstd = mycall.is_some() && !is_nonstandard_call(mycall_raw);
    let lhiscallstd = hiscall.is_some() && !is_nonstandard_call(hiscall_raw);
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
        11 => (
            format!("{} <{}> +00", mycall.as_deref()?, hiscall.as_deref()?),
            &[(0, 58), (74, 77)],
            1,
        ),
        41 => (
            format!("<{}> {} +00", mycall.as_deref()?, hiscall.as_deref()?),
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
            format!("{} {} -15", mybcall.as_deref()?, hisbcall.as_deref()?),
            &[(0, 58), (74, 77)],
            1,
        ),
        22 => (
            hound_special_template(mycall.as_deref()?, hiscall.as_deref()),
            &[(28, 66), (71, 77)],
            0,
        ),
        23 => (
            format!("{} {} RR73", mybcall.as_deref()?, hisbcall.as_deref()?),
            &[(0, 77)],
            1,
        ),
        24 => (
            hound_special_template(mycall.as_deref()?, hiscall.as_deref()),
            &[(0, 28), (56, 66), (71, 77)],
            0,
        ),
        31 => {
            if lhiscallstd {
                if let Some(grid) = grid {
                    (format!("CQ {} {grid}", hiscall.as_deref()?), &[(0, 77)], 1)
                } else {
                    (
                        format!("CQ {} AA00", hiscall.as_deref()?),
                        &[(0, 58), (74, 77)],
                        1,
                    )
                }
            } else {
                (format!("CQ {}", hiscall.as_deref()?), &[(0, 77)], 4)
            }
        }
        35 => {
            if lhiscallstd {
                let first = if lmycallstd {
                    mycall.as_deref()?
                } else {
                    hiscall.as_deref()?
                };
                (
                    format!("{} {} 73", first, hiscall.as_deref()?),
                    &[(29, 77)],
                    1,
                )
            } else {
                (
                    format!("<W9XYZ> {} 73", hiscall.as_deref()?),
                    &[(13, 77)],
                    4,
                )
            }
        }
        36 => {
            if config.lhound
                && (lhiscallstd
                    || (!lhiscallstd && hiscall_raw.trim().len() > 2 && hiscall_raw.contains('/')))
            {
                (
                    format!("{} {} RR73", mybcall.as_deref()?, hisbcall.as_deref()?),
                    &[(29, 77)],
                    1,
                )
            } else if lhiscallstd {
                let first = if lmycallstd {
                    mycall.as_deref()?
                } else {
                    hiscall.as_deref()?
                };
                (
                    format!("{} {} RR73", first, hiscall.as_deref()?),
                    &[(29, 77)],
                    1,
                )
            } else {
                (
                    format!("<W9XYZ> {} RR73", hiscall.as_deref()?),
                    &[(13, 77)],
                    4,
                )
            }
        }
        40 => (
            format!(
                "<{}> {} -15",
                mycall.as_deref()?,
                hiscall.as_deref().unwrap_or("ZZ1ZZZ")
            ),
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

fn base_call(call: Option<&str>) -> Option<String> {
    let call = call?.trim().trim_start_matches('<').trim_end_matches('>');
    let base = if let Some((left, right)) = call.split_once('/') {
        if left.len() >= right.len() {
            left
        } else {
            right
        }
    } else {
        call
    };
    if base.len() < 3 {
        return None;
    }
    Some(base.to_ascii_uppercase())
}

fn is_nonstandard_call(call: &str) -> bool {
    let call = call.trim();
    if call.is_empty() {
        return false;
    }
    call.starts_with('<')
        || call.ends_with('>')
        || call.contains('/')
        || call.len() > 6
        || !call
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}
