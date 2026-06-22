use crate::decode::lib_jtdx::ft8b::{dx_symbol_estimate_snr, DxSymbolField};
use crate::decode::lib_jtdx::ft8v2::encode174_91::encode174_91;
use crate::decode::lib_jtdx::genft8::genft8;

use super::filter::normalize_message;

const DATA_SYMBOLS: [usize; 58] = data_symbols();

#[derive(Clone, Debug)]
pub(super) struct Hypothesis {
    pub msg: String,
    pub itone: [i32; 79],
    pub codeword: [u8; 174],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeepConfidence {
    TwoSlotMatched,
    StackedLlrMatched,
    CrcConfirmedExperimental,
}

#[derive(Clone, Debug)]
pub(super) struct DeepHit {
    pub msg: String,
    pub stat: f32,
    pub margin: f32,
    pub freq: f64,
    pub dt: f64,
    pub snr: Option<f32>,
    pub conf: DeepConfidence,
}

#[derive(Clone, Debug)]
pub(super) struct DeepSearchScore {
    pub idx: usize,
    pub stat: f32,
    pub margin: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct DeepSearchGate {
    pub min_stat: f32,
    pub min_margin: f32,
    pub min_nsync: usize,
    pub min_syncavemax: f32,
    pub top_k: usize,
}

impl Default for DeepSearchGate {
    fn default() -> Self {
        Self {
            min_stat: f32::INFINITY,
            min_margin: f32::INFINITY,
            min_nsync: 0,
            min_syncavemax: 0.0,
            top_k: 8,
        }
    }
}

pub(super) fn build_v1_hypotheses(
    mycall: Option<&str>,
    hiscall: &str,
    hisgrid: Option<&str>,
) -> Vec<Hypothesis> {
    let hiscall = normalize_call(hiscall);
    if hiscall.is_empty() {
        return Vec::new();
    }

    let mut messages = Vec::new();
    if let Some(grid) = normalize_optional_grid(hisgrid) {
        messages.push(format!("CQ {hiscall} {grid}"));
    } else {
        messages.push(format!("CQ {hiscall}"));
    }

    if let Some(mycall) = mycall.map(normalize_call).filter(|call| !call.is_empty()) {
        for report in -24..=0 {
            messages.push(format!("{mycall} {hiscall} {report:+03}"));
            messages.push(format!("{mycall} {hiscall} R{report:+03}"));
        }
        messages.push(format!("{mycall} {hiscall} RR73"));
        messages.push(format!("{mycall} {hiscall} 73"));
    }

    let mut out = Vec::new();
    for msg in messages {
        if let Some(hypothesis) = build_hypothesis(&msg) {
            if !out.iter().any(|existing: &Hypothesis| {
                normalize_message(&existing.msg) == normalize_message(&hypothesis.msg)
            }) {
                out.push(hypothesis);
            }
        }
    }
    out
}

/// Single-slot matched-filter detection (the `TwoSlotMatched` path).
///
/// **Production-dead.** The runtime always passes `DeepSearchGate::default()`
/// (`min_stat`/`min_margin` = `INFINITY`), so the gate check below rejects every
/// finite score and this returns `None` on every real slot. P1 proved the
/// single-slot matched filter cannot separate true from false on real audio (the
/// true/false `stat` ranges overlap), so the gate stays disabled. Kept rather than
/// deleted because the calibration scaffolds still exercise it with finite gates and
/// the deferred field corpus could re-enable it. See PLAN.md decision D5 and the
/// `dx-t1-matched-filter-not-viable` note. Do not "wire it up" without that corpus.
pub(super) fn dx_deep_search(
    field: &DxSymbolField,
    hypotheses: &[Hypothesis],
    gate: DeepSearchGate,
) -> Option<DeepHit> {
    if hypotheses.is_empty()
        || field.nsync < gate.min_nsync
        || field.syncavemax < gate.min_syncavemax
    {
        return None;
    }

    let score = dx_deep_score(field, hypotheses, gate.top_k)?;
    if score.stat < gate.min_stat || score.margin < gate.min_margin {
        return None;
    }

    Some(DeepHit {
        msg: hypotheses[score.idx].msg.clone(),
        stat: score.stat,
        margin: score.margin,
        freq: field.refined_freq,
        dt: field.refined_dt,
        snr: estimate_message_snr(field, &hypotheses[score.idx].msg),
        conf: DeepConfidence::TwoSlotMatched,
    })
}

pub(super) fn estimate_message_snr(field: &DxSymbolField, msg: &str) -> Option<f32> {
    let (_, _, itone) = genft8(msg)?;
    Some(dx_symbol_estimate_snr(field, &itone, 0, false))
}

pub(super) fn dx_deep_score(
    field: &DxSymbolField,
    hypotheses: &[Hypothesis],
    top_k: usize,
) -> Option<DeepSearchScore> {
    if hypotheses.is_empty() {
        return None;
    }

    let mut ranked: Vec<(usize, f32)> = hypotheses
        .iter()
        .enumerate()
        .map(|(idx, hypothesis)| (idx, prefilter(field, &hypothesis.itone)))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut best: Option<(usize, f32)> = None;
    let mut second = f32::NEG_INFINITY;
    for (idx, _) in ranked.into_iter().take(top_k.max(1)) {
        let stat = matched_filter(&field.llr, &hypotheses[idx].codeword);
        match best {
            None => best = Some((idx, stat)),
            Some((_, best_stat)) if stat > best_stat => {
                second = best_stat;
                best = Some((idx, stat));
            }
            _ => second = second.max(stat),
        }
    }

    let (idx, stat) = best?;
    let margin = stat - second;
    Some(DeepSearchScore { idx, stat, margin })
}

pub(super) fn matched_filter(llr: &[f32; 174], codeword: &[u8; 174]) -> f32 {
    (0..174)
        .map(|i| (2.0 * codeword[i] as f32 - 1.0) * llr[i])
        .sum()
}

pub(super) fn prefilter(field: &DxSymbolField, itone: &[i32; 79]) -> f32 {
    let mut score = 0.0f32;
    for &k in &DATA_SYMBOLS {
        let tone = itone[k].clamp(0, 7) as usize;
        let total: f32 = (0..8).map(|idx| field.s8[idx][k]).sum();
        if total > 0.0 {
            score += field.s8[tone][k] / total;
        }
    }
    score
}

fn build_hypothesis(msg: &str) -> Option<Hypothesis> {
    let (msgsent, bits77, itone) = genft8(msg)?;
    if normalize_message(&msgsent) != normalize_message(msg) {
        return None;
    }
    Some(Hypothesis {
        msg: msgsent,
        itone,
        codeword: encode174_91(&bits77),
    })
}

fn normalize_call(call: &str) -> String {
    call.trim().to_ascii_uppercase()
}

fn normalize_optional_grid(grid: Option<&str>) -> Option<String> {
    let grid = grid?.trim().to_ascii_uppercase();
    (grid.len() >= 4).then(|| grid.chars().take(4).collect())
}

const fn data_symbols() -> [usize; 58] {
    let mut out = [0usize; 58];
    let mut idx = 0;
    let mut k = 7;
    while k < 36 {
        out[idx] = k;
        idx += 1;
        k += 1;
    }
    k = 43;
    while k < 72 {
        out[idx] = k;
        idx += 1;
        k += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_hypotheses_cover_cq_and_dx_to_me_only() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let messages: Vec<String> = hypotheses
            .iter()
            .map(|hyp| normalize_message(&hyp.msg))
            .collect();

        assert!(messages.iter().any(|msg| msg == "CQ BG5ATV PM00"));
        assert!(messages.iter().any(|msg| msg == "K1JT BG5ATV -10"));
        assert!(messages.iter().any(|msg| msg == "K1JT BG5ATV R-10"));
        assert!(messages.iter().any(|msg| msg == "K1JT BG5ATV RR73"));
        assert!(!messages.iter().any(|msg| msg.starts_with("OTHER BG5ATV")));
    }

    #[test]
    fn matched_filter_prefers_the_matching_codeword() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let wanted = hypotheses
            .iter()
            .find(|hyp| normalize_message(&hyp.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let other = hypotheses
            .iter()
            .find(|hyp| normalize_message(&hyp.msg) == "K1JT BG5ATV R-10")
            .unwrap();
        let mut llr = [0.0f32; 174];
        for (dst, &bit) in llr.iter_mut().zip(wanted.codeword.iter()) {
            *dst = if bit == 1 { 1.0 } else { -1.0 };
        }

        assert!(matched_filter(&llr, &wanted.codeword) > matched_filter(&llr, &other.codeword));
    }

    #[test]
    fn deep_score_reports_best_hypothesis_and_margin() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let wanted_idx = hypotheses
            .iter()
            .position(|hyp| normalize_message(&hyp.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let mut field = DxSymbolField {
            s8: [[0.0; 79]; 8],
            llr: [0.0; 174],
            ibest: 0,
            refined_freq: 1000.0,
            refined_dt: 0.0,
            syncavemax: 1.0,
            nsync: 8,
        };
        for (dst, &bit) in field
            .llr
            .iter_mut()
            .zip(hypotheses[wanted_idx].codeword.iter())
        {
            *dst = if bit == 1 { 1.0 } else { -1.0 };
        }

        let score = dx_deep_score(&field, &hypotheses, hypotheses.len()).unwrap();

        assert_eq!(score.idx, wanted_idx);
        assert!(score.margin > 0.0);
    }

    #[test]
    fn deep_search_is_wired_but_default_threshold_disabled() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let wanted_idx = hypotheses
            .iter()
            .position(|hyp| normalize_message(&hyp.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let mut field = DxSymbolField {
            s8: [[0.0; 79]; 8],
            llr: [0.0; 174],
            ibest: 0,
            refined_freq: 1000.0,
            refined_dt: 0.0,
            syncavemax: 1.0,
            nsync: 8,
        };
        for (dst, &bit) in field
            .llr
            .iter_mut()
            .zip(hypotheses[wanted_idx].codeword.iter())
        {
            *dst = if bit == 1 { 0.5 } else { -0.5 };
        }
        let score = dx_deep_score(&field, &hypotheses, hypotheses.len()).unwrap();

        assert!(dx_deep_search(&field, &hypotheses, DeepSearchGate::default()).is_none());

        let hit = dx_deep_search(
            &field,
            &hypotheses,
            DeepSearchGate {
                min_stat: score.stat - 0.1,
                min_margin: score.margin - 0.1,
                min_nsync: 8,
                min_syncavemax: 1.0,
                top_k: hypotheses.len(),
            },
        )
        .expect("explicit calibrated gate should allow the target hypothesis");

        assert_eq!(normalize_message(&hit.msg), "K1JT BG5ATV -10");
        assert_eq!(hit.conf, DeepConfidence::TwoSlotMatched);
    }
}
