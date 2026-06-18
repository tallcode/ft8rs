use std::collections::VecDeque;

use crate::decode::lib_jtdx::ft8b::DxSymbolField;
use crate::decode::lib_jtdx::ft8v2::bpdecode174_91::{bpdecode174_91, N};
use crate::decode::lib_jtdx::ft8v2::osd174_91::osd174_91;
use crate::decode::lib_jtdx::ft8v2::packjt77::unpack77;
use crate::decode::lib_jtdx::genft8::genft8;

use super::deepsearch::{
    estimate_message_snr, matched_filter, DeepConfidence, DeepHit, DeepSearchGate, Hypothesis,
};
use super::filter::normalize_message;

const MIN_STACK_LLR_COSINE_FOR_CRC_OUTPUT: f32 = 0.20;
const MIN_SLOT_CODEWORD_SUPPORT_FOR_CRC_OUTPUT: f32 = 0.0;
const MIN_SUPPORTED_SLOT_NUMERATOR_FOR_CRC_OUTPUT: usize = 2;
const MIN_SUPPORTED_SLOT_DENOMINATOR_FOR_CRC_OUTPUT: usize = 3;
const MAX_STACK_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) struct StackKey {
    pub parity: usize,
    pub freq_bin: i32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct PhysicalAdmissionGate {
    pub freq_tolerance_hz: f64,
    pub dt_tolerance_s: f64,
    pub min_nsync: usize,
    pub min_syncavemax: f32,
}

impl Default for PhysicalAdmissionGate {
    fn default() -> Self {
        Self {
            freq_tolerance_hz: 3.0,
            dt_tolerance_s: 0.3,
            min_nsync: 4,
            min_syncavemax: 0.0,
        }
    }
}

impl PhysicalAdmissionGate {
    pub(super) fn passes_floor(&self, field: &DxSymbolField) -> bool {
        field.nsync >= self.min_nsync && field.syncavemax >= self.min_syncavemax
    }
}

#[derive(Clone, Debug)]
pub(super) struct SlotStack {
    key: StackKey,
    anchor_freq: f64,
    anchor_dt: f64,
    sum_llr: [f32; N],
    slot_llrs: VecDeque<[f32; N]>,
    last_nutc: u32,
    best_hyp: Option<usize>,
    flip_run: u8,
}

impl SlotStack {
    pub(super) fn new(key: StackKey, field: &DxSymbolField, nutc: u32) -> Self {
        let mut stack = Self {
            key,
            anchor_freq: field.refined_freq,
            anchor_dt: field.refined_dt,
            sum_llr: [0.0; N],
            slot_llrs: VecDeque::with_capacity(MAX_STACK_DEPTH),
            last_nutc: nutc,
            best_hyp: None,
            flip_run: 0,
        };
        stack.accumulate_unchecked(field, nutc);
        stack
    }

    pub(super) fn key(&self) -> StackKey {
        self.key
    }

    pub(super) fn depth(&self) -> u32 {
        self.slot_llrs.len() as u32
    }

    pub(super) fn last_seen_nutc(&self) -> u32 {
        self.last_nutc
    }

    pub(super) fn decode_priority(&self, field: &DxSymbolField, focus: f64) -> StackDecodePriority {
        StackDecodePriority {
            depth: self.depth() + 1,
            syncavemax: field.syncavemax,
            nsync: field.nsync,
            distance_to_focus: (self.anchor_freq - focus).abs(),
        }
    }

    pub(super) fn can_admit(&self, field: &DxSymbolField, gate: PhysicalAdmissionGate) -> bool {
        (field.refined_freq - self.anchor_freq).abs() <= gate.freq_tolerance_hz
            && (field.refined_dt - self.anchor_dt).abs() <= gate.dt_tolerance_s
            && gate.passes_floor(field)
    }

    pub(super) fn combined_llr(&self, field: &DxSymbolField) -> [f32; N] {
        let mut out = self.sum_llr;
        for (dst, src) in out.iter_mut().zip(field.llr.iter().copied()) {
            *dst += src;
        }
        out
    }

    pub(super) fn admit_with_hypotheses(
        &mut self,
        field: &DxSymbolField,
        nutc: u32,
        gate: PhysicalAdmissionGate,
        hypotheses: &[Hypothesis],
        matched_gate: DeepSearchGate,
    ) -> bool {
        if !self.can_admit(field, gate) {
            return false;
        }
        if let Some(hyp_idx) = confident_best_hypothesis(&field.llr, hypotheses, matched_gate) {
            if self.should_reset_for_hypothesis(hyp_idx) {
                self.reset_unchecked(field, nutc, Some(hyp_idx));
                return true;
            }
        }
        self.accumulate_unchecked(field, nutc);
        true
    }

    pub(super) fn decode_combined<F>(
        &self,
        field: &DxSymbolField,
        hypotheses: &[Hypothesis],
        matched_gate: DeepSearchGate,
        allow_crc_decode: bool,
        target_matches: F,
    ) -> Option<DeepHit>
    where
        F: Fn(&str) -> bool,
    {
        let allow_crc_decode = allow_crc_decode
            && self.llr_cosine_with_sum(field) >= MIN_STACK_LLR_COSINE_FOR_CRC_OUTPUT;
        let current_crc_msg = allow_crc_decode
            .then(|| decode_crc_target_msg(&field.llr, 1, &target_matches))
            .flatten();
        let llr = self.combined_llr(field);
        let mut support_llrs: Vec<&[f32; N]> = self.slot_llrs.iter().collect();
        support_llrs.push(&field.llr);
        decode_llr(LlrDecodeRequest {
            llr: &llr,
            depth: self.depth() + 1,
            freq: self.anchor_freq,
            dt: self.anchor_dt,
            field,
            hypotheses,
            matched_gate,
            allow_crc_decode,
            current_crc_msg,
            support_llrs: &support_llrs,
            target_matches,
        })
    }

    fn llr_cosine_with_sum(&self, field: &DxSymbolField) -> f32 {
        let mut dot = 0.0f32;
        let mut sum_energy = 0.0f32;
        let mut field_energy = 0.0f32;
        for (&acc, &cur) in self.sum_llr.iter().zip(field.llr.iter()) {
            dot += acc * cur;
            sum_energy += acc * acc;
            field_energy += cur * cur;
        }
        if sum_energy <= 0.0 || field_energy <= 0.0 {
            return 0.0;
        }
        dot / (sum_energy.sqrt() * field_energy.sqrt())
    }

    fn accumulate_unchecked(&mut self, field: &DxSymbolField, nutc: u32) {
        if self.slot_llrs.len() == MAX_STACK_DEPTH {
            if let Some(oldest) = self.slot_llrs.pop_front() {
                for (dst, old) in self.sum_llr.iter_mut().zip(oldest) {
                    *dst -= old;
                }
            }
        }
        for (dst, src) in self.sum_llr.iter_mut().zip(field.llr.iter().copied()) {
            *dst += src;
        }
        self.slot_llrs.push_back(field.llr);
        self.last_nutc = nutc;
    }

    fn reset_unchecked(&mut self, field: &DxSymbolField, nutc: u32, best_hyp: Option<usize>) {
        self.sum_llr = [0.0; N];
        self.slot_llrs.clear();
        self.best_hyp = best_hyp;
        self.flip_run = 0;
        self.accumulate_unchecked(field, nutc);
    }

    fn should_reset_for_hypothesis(&mut self, hyp_idx: usize) -> bool {
        match self.best_hyp {
            None => {
                self.best_hyp = Some(hyp_idx);
                self.flip_run = 0;
                false
            }
            Some(existing) if existing == hyp_idx => {
                self.flip_run = 0;
                false
            }
            Some(_) => {
                self.flip_run = self.flip_run.saturating_add(1);
                self.flip_run >= 2
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StackDecodePriority {
    pub depth: u32,
    pub syncavemax: f32,
    pub nsync: usize,
    pub distance_to_focus: f64,
}

impl StackDecodePriority {
    pub(super) fn cmp_value_order(self, other: Self) -> std::cmp::Ordering {
        self.depth
            .cmp(&other.depth)
            .then_with(|| self.syncavemax.total_cmp(&other.syncavemax))
            .then_with(|| self.nsync.cmp(&other.nsync))
            .then_with(|| other.distance_to_focus.total_cmp(&self.distance_to_focus))
    }
}

struct LlrDecodeRequest<'a, F>
where
    F: Fn(&str) -> bool,
{
    llr: &'a [f32; N],
    depth: u32,
    freq: f64,
    dt: f64,
    field: &'a DxSymbolField,
    hypotheses: &'a [Hypothesis],
    matched_gate: DeepSearchGate,
    allow_crc_decode: bool,
    current_crc_msg: Option<String>,
    support_llrs: &'a [&'a [f32; N]],
    target_matches: F,
}

struct CrcDecodeRequest<'a, F>
where
    F: Fn(&str) -> bool,
{
    llr: &'a [f32; N],
    depth: u32,
    freq: f64,
    dt: f64,
    field: &'a DxSymbolField,
    current_crc_msg: Option<&'a str>,
    support_llrs: &'a [&'a [f32; N]],
    target_matches: F,
}

fn decode_llr<F>(request: LlrDecodeRequest<'_, F>) -> Option<DeepHit>
where
    F: Fn(&str) -> bool,
{
    let matched = decode_matched_llr(
        request.llr,
        request.freq,
        request.dt,
        request.field,
        request.hypotheses,
        request.matched_gate,
    );
    let crc = request
        .allow_crc_decode
        .then(|| {
            decode_crc_llr(CrcDecodeRequest {
                llr: request.llr,
                depth: request.depth,
                freq: request.freq,
                dt: request.dt,
                field: request.field,
                current_crc_msg: request.current_crc_msg.as_deref(),
                support_llrs: request.support_llrs,
                target_matches: request.target_matches,
            })
        })
        .flatten();
    crc.or(matched)
}

fn decode_matched_llr(
    llr: &[f32; N],
    freq: f64,
    dt: f64,
    field: &DxSymbolField,
    hypotheses: &[Hypothesis],
    gate: DeepSearchGate,
) -> Option<DeepHit> {
    if hypotheses.is_empty() {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    let mut second = f32::NEG_INFINITY;
    for (idx, hypothesis) in hypotheses.iter().enumerate() {
        let stat = matched_filter(llr, &hypothesis.codeword);
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
    if stat < gate.min_stat || margin < gate.min_margin {
        return None;
    }
    Some(DeepHit {
        msg: hypotheses[idx].msg.clone(),
        stat,
        margin,
        freq,
        dt,
        snr: estimate_message_snr(field, &hypotheses[idx].msg),
        conf: DeepConfidence::StackedLlrMatched,
    })
}

fn confident_best_hypothesis(
    llr: &[f32; N],
    hypotheses: &[Hypothesis],
    gate: DeepSearchGate,
) -> Option<usize> {
    if hypotheses.is_empty() {
        return None;
    }
    let mut best: Option<(usize, f32)> = None;
    let mut second = f32::NEG_INFINITY;
    for (idx, hypothesis) in hypotheses.iter().enumerate() {
        let stat = matched_filter(llr, &hypothesis.codeword);
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
    (stat >= gate.min_stat && margin >= gate.min_margin).then_some(idx)
}

fn decode_crc_llr<F>(request: CrcDecodeRequest<'_, F>) -> Option<DeepHit>
where
    F: Fn(&str) -> bool,
{
    let msg = decode_crc_target_msg(request.llr, request.depth, &request.target_matches)?;
    if request
        .current_crc_msg
        .is_some_and(|current| normalize_message(current) != normalize_message(&msg))
    {
        return None;
    }
    if !crc_message_has_slot_support(&msg, request.support_llrs) {
        return None;
    }
    let snr = estimate_message_snr(request.field, &msg);
    Some(DeepHit {
        msg,
        stat: 0.0,
        margin: 0.0,
        freq: request.freq,
        dt: request.dt,
        snr,
        conf: DeepConfidence::CrcConfirmedExperimental,
    })
}

fn decode_crc_target_msg<F>(llr: &[f32; N], depth: u32, target_matches: &F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    let apmask = [0i8; N];
    let decoded = bpdecode174_91(llr, &apmask, 30).or_else(|| {
        if depth >= 2 {
            osd174_91(llr, &apmask, 5)
        } else {
            None
        }
    })?;
    let msg = unpack77(&decoded.message77, None)?;
    target_matches(&msg).then_some(msg)
}

fn crc_message_has_slot_support(msg: &str, support_llrs: &[&[f32; N]]) -> bool {
    let (_, bits77, _) = match genft8(msg) {
        Some(packed) => packed,
        None => return false,
    };
    let codeword = crate::decode::lib_jtdx::ft8v2::encode174_91::encode174_91(&bits77);
    if support_llrs.is_empty() {
        return false;
    }

    let mut total_support = 0.0f32;
    let mut supported_slots = 0usize;
    for llr in support_llrs {
        let support = matched_filter(llr, &codeword);
        total_support += support;
        if support >= MIN_SLOT_CODEWORD_SUPPORT_FOR_CRC_OUTPUT {
            supported_slots += 1;
        }
    }

    total_support > 0.0
        && supported_slots * MIN_SUPPORTED_SLOT_DENOMINATOR_FOR_CRC_OUTPUT
            >= support_llrs.len() * MIN_SUPPORTED_SLOT_NUMERATOR_FOR_CRC_OUTPUT
}

#[cfg(test)]
mod tests {
    use crate::decode::dx::deepsearch::build_v1_hypotheses;
    use crate::decode::lib_jtdx::ft8v2::encode174_91::encode174_91;
    use crate::decode::lib_jtdx::genft8::genft8;

    use super::*;

    fn field(freq: f64, dt: f64, nsync: usize, syncavemax: f32, bit: f32) -> DxSymbolField {
        DxSymbolField {
            s8: [[0.0; 79]; 8],
            llr: [bit; N],
            ibest: 0,
            refined_freq: freq,
            refined_dt: dt,
            syncavemax,
            nsync,
        }
    }

    #[test]
    fn physical_admission_is_the_accumulation_gate() {
        let first = field(1000.0, 0.2, 5, 1.0, 1.0);
        let mut stack = SlotStack::new(
            StackKey {
                parity: 0,
                freq_bin: 1000,
            },
            &first,
            140300,
        );

        let weak_but_physical = field(1001.0, 0.25, 4, 0.1, 1.0);
        assert!(stack.admit_with_hypotheses(
            &weak_but_physical,
            140330,
            PhysicalAdmissionGate {
                min_nsync: 4,
                min_syncavemax: 0.0,
                ..PhysicalAdmissionGate::default()
            },
            &[],
            DeepSearchGate::default(),
        ));
        assert_eq!(stack.depth(), 2);

        let wrong_signal = field(1010.0, 0.25, 10, 10.0, 1.0);
        assert!(!stack.admit_with_hypotheses(
            &wrong_signal,
            140400,
            PhysicalAdmissionGate::default(),
            &[],
            DeepSearchGate::default(),
        ));
        assert_eq!(stack.depth(), 2);
    }

    #[test]
    fn combined_llr_uses_current_observation_without_committing_it() {
        let first = field(1000.0, 0.2, 8, 2.0, 1.0);
        let current = field(1000.5, 0.25, 8, 2.0, 2.0);
        let stack = SlotStack::new(
            StackKey {
                parity: 1,
                freq_bin: 1000,
            },
            &first,
            140300,
        );

        let combined = stack.combined_llr(&current);
        assert_eq!(combined[0], 3.0);
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn sustained_confident_hypothesis_flip_resets_stack() {
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let msg_a = hypotheses
            .iter()
            .find(|hyp| hyp.msg == "K1JT BG5ATV -10")
            .unwrap();
        let msg_b = hypotheses
            .iter()
            .find(|hyp| hyp.msg == "K1JT BG5ATV R-10")
            .unwrap();
        let matched_gate = DeepSearchGate {
            min_stat: 100.0,
            min_margin: 1.0,
            min_nsync: 0,
            min_syncavemax: 0.0,
            top_k: hypotheses.len(),
        };
        let physical_gate = PhysicalAdmissionGate {
            min_nsync: 1,
            ..PhysicalAdmissionGate::default()
        };
        let field_a = hypothesis_field(msg_a, 1000.0, 0.2);
        let field_b = hypothesis_field(msg_b, 1000.0, 0.2);
        let mut stack = SlotStack::new(
            StackKey {
                parity: 0,
                freq_bin: 1000,
            },
            &field_a,
            140300,
        );

        assert!(stack.admit_with_hypotheses(
            &field_a,
            140330,
            physical_gate,
            &hypotheses,
            matched_gate,
        ));
        assert_eq!(stack.depth(), 2);

        assert!(stack.admit_with_hypotheses(
            &field_b,
            140400,
            physical_gate,
            &hypotheses,
            matched_gate,
        ));
        assert_eq!(stack.depth(), 3);

        assert!(stack.admit_with_hypotheses(
            &field_b,
            140430,
            physical_gate,
            &hypotheses,
            matched_gate,
        ));
        assert_eq!(stack.depth(), 1);
    }

    #[test]
    fn stacked_llr_depth_trend_recovers_after_noise_cancels() {
        let codeword = codeword_for("K1JT BG5ATV -10");
        let slots: Vec<[f32; N]> = (0..8)
            .map(|phase| noisy_llr(&codeword, phase, 0.35, 0.9))
            .collect();

        assert!(decode_llr(LlrDecodeRequest {
            llr: &slots[0],
            depth: 1,
            freq: 1000.0,
            dt: 0.2,
            field: &field(1000.0, 0.2, 8, 2.0, 0.0),
            hypotheses: &[],
            matched_gate: DeepSearchGate::default(),
            allow_crc_decode: true,
            current_crc_msg: None,
            support_llrs: &[&slots[0]],
            target_matches: |msg| { msg.contains("BG5ATV") },
        })
        .is_none());

        let support_by_depth: Vec<f32> = [1usize, 2, 4, 8]
            .into_iter()
            .map(|depth| matched_filter(&sum_llr_prefix(&slots, depth), &codeword))
            .collect();
        assert!(
            support_by_depth
                .windows(2)
                .all(|window| window[1] > window[0]),
            "stack target support should increase with depth: {support_by_depth:?}"
        );

        let sum4 = sum_llr_prefix(&slots, 4);

        let hit = decode_llr(LlrDecodeRequest {
            llr: &sum4,
            depth: 4,
            freq: 1000.0,
            dt: 0.2,
            field: &field(1000.0, 0.2, 8, 2.0, 0.0),
            hypotheses: &[],
            matched_gate: DeepSearchGate::default(),
            allow_crc_decode: true,
            current_crc_msg: None,
            support_llrs: &[&slots[0], &slots[1], &slots[2], &slots[3]],
            target_matches: |msg| msg.contains("BG5ATV"),
        })
        .expect("summed LLR should recover the repeated target message");
        assert_eq!(hit.conf, DeepConfidence::CrcConfirmedExperimental);
        assert_eq!(hit.msg, "K1JT BG5ATV -10");
    }

    #[test]
    fn crc_output_uses_statistical_slot_support_for_decoded_codeword() {
        let codeword = codeword_for("K1JT BG5ATV -10");
        let supporting = llr_for_codeword(&codeword, 0.8);
        let weak_outlier = llr_for_codeword(&codeword, -0.1);
        let strong_contradicting = llr_for_codeword(&codeword, -2.0);

        assert!(crc_message_has_slot_support(
            "K1JT BG5ATV -10",
            &[&supporting]
        ));
        assert!(crc_message_has_slot_support(
            "K1JT BG5ATV -10",
            &[&supporting, &supporting, &weak_outlier]
        ));
        assert!(!crc_message_has_slot_support(
            "K1JT BG5ATV -10",
            &[&supporting, &strong_contradicting]
        ));
        assert!(!crc_message_has_slot_support(
            "K1JT BG5ATV -10",
            &[&supporting, &weak_outlier, &weak_outlier]
        ));
    }

    #[test]
    fn stack_depth_cap_removes_oldest_llr_from_sum() {
        let first = field(1000.0, 0.2, 8, 2.0, 1.0);
        let mut stack = SlotStack::new(
            StackKey {
                parity: 0,
                freq_bin: 1000,
            },
            &first,
            140300,
        );
        for idx in 0..MAX_STACK_DEPTH {
            let next = field(1000.0, 0.2, 8, 2.0, 2.0 + idx as f32);
            assert!(stack.admit_with_hypotheses(
                &next,
                140330 + idx as u32 * 30,
                PhysicalAdmissionGate::default(),
                &[],
                DeepSearchGate::default(),
            ));
        }

        assert_eq!(stack.depth(), MAX_STACK_DEPTH as u32);
        // The original 1.0 slot was evicted, leaving values 2.0..=9.0.
        assert_eq!(
            stack.sum_llr[0],
            (2..=9).map(|value| value as f32).sum::<f32>()
        );
    }

    fn codeword_for(msg: &str) -> [u8; N] {
        let (_, bits77, _) = genft8(msg).expect("test message must pack");
        encode174_91(&bits77)
    }

    fn llr_for_codeword(codeword: &[u8; N], magnitude: f32) -> [f32; N] {
        let mut llr = [0.0f32; N];
        for (dst, &bit) in llr.iter_mut().zip(codeword.iter()) {
            *dst = if bit == 1 { magnitude } else { -magnitude };
        }
        llr
    }

    fn noisy_llr(codeword: &[u8; N], phase: usize, signal: f32, noise: f32) -> [f32; N] {
        let mut llr = [0.0f32; N];
        for (idx, dst) in llr.iter_mut().enumerate() {
            let expected = if codeword[idx] == 1 { signal } else { -signal };
            let noise_sign = if (idx + phase).is_multiple_of(4) {
                -noise
            } else {
                noise / 3.0
            };
            *dst = expected + noise_sign;
        }
        llr
    }

    fn sum_llr_prefix(slots: &[[f32; N]], depth: usize) -> [f32; N] {
        let mut sum = [0.0f32; N];
        for slot in slots.iter().take(depth) {
            for (dst, value) in sum.iter_mut().zip(slot.iter().copied()) {
                *dst += value;
            }
        }
        sum
    }

    fn hypothesis_field(hypothesis: &Hypothesis, freq: f64, dt: f64) -> DxSymbolField {
        let mut field = field(freq, dt, 8, 2.0, 0.0);
        for (dst, &bit) in field.llr.iter_mut().zip(hypothesis.codeword.iter()) {
            *dst = if bit == 1 { 1.0 } else { -1.0 };
        }
        field
    }
}
