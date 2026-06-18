use crate::decode::lib_jtdx::ft8b::DxSymbolField;
use crate::stream::session::StreamDecodedMessage;
use crate::stream::time::SlotTimestamp;

use super::deepsearch::{dx_deep_search, DeepConfidence, DeepHit, DeepSearchGate, Hypothesis};
use super::filter::{normalize_message, normalize_message_word, DxTarget};
use super::stack::{PhysicalAdmissionGate, SlotStack, StackDecodePriority, StackKey};

const MAX_FOCI: usize = 5;
const FOCUS_HALF_WIDTH_HZ: f64 = 25.0;
const MAX_DEEP_CRC_DECODE_PER_SLOT: usize = 2;
/// Upstream `foxgen.f90` lays out the Fox's multi-stream signals at a fixed
/// spacing `fstep = 60 Hz` (`f0 = nfreq + fstep*(n-1)`). We mirror that constant
/// to pre-place the full focus grid in Hound mode.
const FOX_STREAM_SPACING_HZ: f64 = 60.0;
/// Number of distinct observed Fox streams at/above which we treat the target as
/// a multi-stream Fox and switch to the equally-spaced grid (owner's rule: ">= 2").
const FOX_MULTISTREAM_THRESHOLD: usize = 2;

#[derive(Clone, Debug)]
struct FrequencyCandidate {
    freq: f64,
    confidence: u8,
    last_seen_nutc: u32,
    pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ParityConfidence {
    Inferred,
    Observed,
}

#[derive(Clone, Copy, Debug)]
struct TxParity {
    parity: usize,
    confidence: ParityConfidence,
    last_seen_nutc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QsoProgressProbe {
    last_message: String,
    inferred_progress: Option<usize>,
    reason: &'static str,
}

#[derive(Clone, Debug)]
struct MatchedObservation {
    msg_norm: String,
    parity: usize,
    freq: f64,
    dt: f64,
    stat: f32,
    margin: f32,
    last_seen_nutc: u32,
}

#[derive(Clone, Copy, Debug)]
struct StackDecodeCandidate {
    idx: usize,
    input_idx: usize,
    priority: StackDecodePriority,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct DeepStackDiagnostics {
    pub crc_candidates: usize,
    pub crc_attempts: usize,
    pub crc_skipped_budget: usize,
}

pub(super) struct DeepFieldInput<'a> {
    pub focus: f64,
    pub field: &'a DxSymbolField,
}

#[derive(Clone, Debug)]
pub(super) struct TargetContextStore {
    target: DxTarget,
    mycall: Option<DxTarget>,
    frequencies: Vec<FrequencyCandidate>,
    tx_parity: Option<TxParity>,
    hisgrid: Option<String>,
    dt: Option<f64>,
    low_band_prior: bool,
    hound: bool,
    nfa: f64,
    nfb: f64,
    deep_stacks: Vec<SlotStack>,
    deep_crc_budget_nutc: Option<u32>,
    deep_crc_budget_used: usize,
    deep_diagnostics: DeepStackDiagnostics,
    qso_progress_probe: Option<QsoProgressProbe>,
    matched_observations: Vec<MatchedObservation>,
}

impl TargetContextStore {
    pub(super) fn new(
        target: DxTarget,
        mycall: Option<&str>,
        seed_frequency: f64,
        hisgrid: Option<&str>,
        hound: bool,
        nfa: f64,
        nfb: f64,
    ) -> Self {
        let mut store = Self {
            target,
            mycall: mycall.map(DxTarget::new),
            frequencies: Vec::new(),
            tx_parity: None,
            hisgrid: hisgrid.map(|grid| grid.trim().to_ascii_uppercase()),
            dt: None,
            low_band_prior: true,
            hound,
            nfa,
            nfb,
            deep_stacks: Vec::new(),
            deep_crc_budget_nutc: None,
            deep_crc_budget_used: 0,
            deep_diagnostics: DeepStackDiagnostics::default(),
            qso_progress_probe: None,
            matched_observations: Vec::new(),
        };
        if seed_frequency > 0.0 {
            store.remember_frequency(seed_frequency, 8, 0, true);
        }
        store
    }

    pub(super) fn should_run_focused(&self, timestamp: &SlotTimestamp) -> bool {
        match self.tx_parity {
            Some(TxParity {
                parity,
                confidence: ParityConfidence::Observed,
                last_seen_nutc,
            }) => {
                if slots_between(last_seen_nutc, timestamp.nutc()) <= 16 {
                    parity == slot_parity(timestamp)
                } else {
                    true
                }
            }
            Some(TxParity {
                confidence: ParityConfidence::Inferred,
                ..
            }) => true,
            None => true,
        }
    }

    pub(super) fn selected_foci(&self) -> Vec<f64> {
        if let Some(grid) = self.fox_multistream_foci() {
            return grid;
        }

        let mut candidates = self.frequencies.clone();
        candidates.sort_by(|a, b| {
            b.confidence
                .cmp(&a.confidence)
                .then_with(|| {
                    low_band_rank(self.low_band_prior, a.freq)
                        .cmp(&low_band_rank(self.low_band_prior, b.freq))
                })
                .then_with(|| a.freq.total_cmp(&b.freq))
        });

        let mut foci: Vec<f64> = Vec::new();
        for candidate in candidates {
            if foci
                .iter()
                .any(|freq| (candidate.freq - *freq).abs() <= FOCUS_HALF_WIDTH_HZ)
            {
                continue;
            }
            foci.push(candidate.freq);
            if foci.len() == MAX_FOCI {
                break;
            }
        }
        foci
    }

    /// FH/Hound multi-stream foci. Once at least `FOX_MULTISTREAM_THRESHOLD`
    /// distinct Fox-stream frequencies have been observed (in Hound mode every
    /// harvested frequency is a target-as-sender Fox stream), the target is a
    /// multi-stream Fox transmitting an equally-spaced block. Mirror upstream
    /// `foxgen.f90` (`f0 = nfreq + 60*(n-1)`): anchor on the lowest live observed
    /// stream and pre-place the full `MAX_FOCI` grid at 60 Hz, so a Fox that
    /// dynamically grows to 3/4/5 streams is already monitored instead of being
    /// discovered a slot late.
    ///
    /// Dynamic-lowest fallback: the anchor is the running minimum of the live
    /// observed streams, so it re-adjusts downward for free — the always-on
    /// full-band listen decodes any lower stream we had missed, which lowers the
    /// minimum next slot and shifts the whole grid down; stale high anchors age
    /// out the same way. No focus slot is spent on a separate downward probe, so
    /// the upward coverage the owner asked for is preserved.
    fn fox_multistream_foci(&self) -> Option<Vec<f64>> {
        if !self.hound || self.frequencies.len() < FOX_MULTISTREAM_THRESHOLD {
            return None;
        }
        let base = self
            .frequencies
            .iter()
            .map(|candidate| candidate.freq)
            .fold(f64::INFINITY, f64::min);
        if !base.is_finite() {
            return None;
        }
        let mut foci = Vec::with_capacity(MAX_FOCI);
        for step in 0..MAX_FOCI {
            let freq = base + FOX_STREAM_SPACING_HZ * step as f64;
            // Stop once the grid point's whole focus window has left the passband.
            if freq - FOCUS_HALF_WIDTH_HZ > self.nfb {
                break;
            }
            foci.push(freq);
        }
        (!foci.is_empty()).then_some(foci)
    }

    pub(super) fn hisgrid(&self) -> Option<&str> {
        self.hisgrid.as_deref()
    }

    pub(super) fn target_dt(&self) -> Option<f64> {
        self.dt
    }

    pub(super) fn qso_progress(&self) -> Option<usize> {
        self.qso_progress_probe
            .as_ref()
            .and_then(|probe| probe.inferred_progress)
    }

    pub(super) fn should_emit_target_row(&self, row: &StreamDecodedMessage) -> bool {
        if !self.target.matches_message(&row.msg) {
            return false;
        }
        !self.has_hard_grid_contradiction(row)
    }

    #[cfg(test)]
    pub(super) fn observe_deep_field(
        &mut self,
        timestamp: &SlotTimestamp,
        focus: f64,
        field: &DxSymbolField,
        hypotheses: &[Hypothesis],
        matched_gate: DeepSearchGate,
        physical_gate: PhysicalAdmissionGate,
    ) -> Option<DeepHit> {
        self.observe_deep_fields(
            timestamp,
            &[DeepFieldInput { focus, field }],
            hypotheses,
            matched_gate,
            physical_gate,
        )
        .into_iter()
        .next()
    }

    pub(super) fn observe_deep_fields(
        &mut self,
        timestamp: &SlotTimestamp,
        inputs: &[DeepFieldInput<'_>],
        hypotheses: &[Hypothesis],
        matched_gate: DeepSearchGate,
        physical_gate: PhysicalAdmissionGate,
    ) -> Vec<DeepHit> {
        self.age_deep_stacks(timestamp.nutc());
        self.age_matched_observations(timestamp.nutc());

        let mut hits = Vec::new();
        let mut stack_candidates = Vec::new();
        let mut opens = Vec::new();
        let parity = slot_parity(timestamp);

        for (input_idx, input) in inputs.iter().enumerate() {
            let single_matched = dx_deep_search(input.field, hypotheses, matched_gate);
            if let Some(hit) = single_matched
                .as_ref()
                .and_then(|hit| self.observe_matched_hit(timestamp, hit, physical_gate))
            {
                hits.push(hit);
            }

            let key = StackKey {
                parity,
                freq_bin: input.focus.round() as i32,
            };
            if let Some(candidate) = self.best_stack_decode_candidate(
                input_idx,
                key,
                input.focus,
                input.field,
                physical_gate,
            ) {
                stack_candidates.push(candidate);
            } else if physical_gate.passes_floor(input.field) {
                opens.push((key, input.field));
            }
        }

        stack_candidates.sort_by(|a, b| b.priority.cmp_value_order(a.priority));
        let mut commits = Vec::with_capacity(stack_candidates.len());
        for candidate in stack_candidates {
            let input = &inputs[candidate.input_idx];
            self.deep_diagnostics.crc_candidates += 1;
            let allow_crc_decode = self.take_deep_crc_decode_budget(timestamp.nutc());
            if allow_crc_decode {
                self.deep_diagnostics.crc_attempts += 1;
            } else {
                self.deep_diagnostics.crc_skipped_budget += 1;
            }
            let hit = {
                let stack = &self.deep_stacks[candidate.idx];
                stack.decode_combined(
                    input.field,
                    hypotheses,
                    matched_gate,
                    allow_crc_decode,
                    |msg| self.target.matches_message(msg),
                )
            };
            if let Some(hit) = hit {
                hits.push(hit);
            }
            commits.push((candidate.idx, input.field));
        }

        for (idx, field) in commits {
            self.deep_stacks[idx].admit_with_hypotheses(
                field,
                timestamp.nutc(),
                physical_gate,
                hypotheses,
                matched_gate,
            );
        }
        for (key, field) in opens {
            self.deep_stacks
                .push(SlotStack::new(key, field, timestamp.nutc()));
        }

        hits
    }

    fn best_stack_decode_candidate(
        &self,
        input_idx: usize,
        key: StackKey,
        focus: f64,
        field: &DxSymbolField,
        physical_gate: PhysicalAdmissionGate,
    ) -> Option<StackDecodeCandidate> {
        self.deep_stacks
            .iter()
            .enumerate()
            .filter(|(_, stack)| stack.key() == key && stack.can_admit(field, physical_gate))
            .map(|(idx, stack)| StackDecodeCandidate {
                idx,
                input_idx,
                priority: stack.decode_priority(field, focus),
            })
            .max_by(|a, b| a.priority.cmp_value_order(b.priority))
    }

    pub(super) fn harvest_listen(
        &mut self,
        timestamp: &SlotTimestamp,
        rows: &[StreamDecodedMessage],
    ) {
        for row in rows {
            self.harvest_row(timestamp, row, true);
        }
        self.age_frequencies(timestamp.nutc());
    }

    pub(super) fn harvest_focused(
        &mut self,
        timestamp: &SlotTimestamp,
        rows: &[StreamDecodedMessage],
    ) {
        for row in rows {
            self.harvest_row(timestamp, row, false);
        }
        self.age_frequencies(timestamp.nutc());
    }

    fn harvest_row(&mut self, timestamp: &SlotTimestamp, row: &StreamDecodedMessage, listen: bool) {
        let role = MessageRole::from_message(&row.msg, &self.target, self.mycall.as_ref());
        if !role.contains_target && !role.contains_mycall {
            return;
        }

        self.observe_qso_progress_candidate(&row.msg, role);

        let parity = slot_parity(timestamp);
        if role.target_sender {
            self.set_tx_parity(parity, ParityConfidence::Observed, timestamp.nutc());
            self.harvest_grid(&row.msg);
            self.dt = Some(row.dt);
        } else if role.target_recipient && self.tx_parity.is_none() {
            self.set_tx_parity(1 - parity, ParityConfidence::Inferred, timestamp.nutc());
        }

        let confidence = if role.target_sender {
            10
        } else if role.target_recipient && !self.hound {
            5
        } else if role.contains_mycall && !self.hound {
            3
        } else if listen {
            2
        } else {
            1
        };

        let frequency_seed_allowed =
            role.target_sender || (!self.hound && (role.target_recipient || role.contains_mycall));
        if frequency_seed_allowed {
            self.remember_frequency(row.freq, confidence, timestamp.nutc(), false);
        }
    }

    fn set_tx_parity(&mut self, parity: usize, confidence: ParityConfidence, nutc: u32) {
        let replace = match self.tx_parity {
            None => true,
            Some(existing) => {
                matches!(
                    (existing.confidence, confidence),
                    (ParityConfidence::Inferred, ParityConfidence::Observed)
                ) || existing.parity == parity
                    || slots_between(existing.last_seen_nutc, nutc) > 16
            }
        };
        if replace {
            self.tx_parity = Some(TxParity {
                parity,
                confidence,
                last_seen_nutc: nutc,
            });
        }
    }

    fn remember_frequency(&mut self, freq: f64, confidence: u8, nutc: u32, pinned: bool) {
        if !(freq.is_finite() && freq >= self.nfa && freq <= self.nfb) {
            return;
        }
        if let Some(existing) = self
            .frequencies
            .iter_mut()
            .find(|existing| (existing.freq - freq).abs() <= FOCUS_HALF_WIDTH_HZ)
        {
            if confidence > existing.confidence {
                existing.freq = freq;
                existing.confidence = confidence;
            }
            existing.pinned |= pinned;
            existing.last_seen_nutc = nutc;
            return;
        }
        self.frequencies.push(FrequencyCandidate {
            freq,
            confidence,
            last_seen_nutc: nutc,
            pinned,
        });
    }

    fn age_frequencies(&mut self, nutc: u32) {
        self.frequencies
            .retain(|freq| freq.pinned || slots_between(freq.last_seen_nutc, nutc) <= 16);
    }

    fn age_deep_stacks(&mut self, nutc: u32) {
        self.deep_stacks
            .retain(|stack| slots_between(stack.last_seen_nutc(), nutc) <= 16);
    }

    fn age_matched_observations(&mut self, nutc: u32) {
        self.matched_observations
            .retain(|obs| slots_between(obs.last_seen_nutc, nutc) <= 16);
    }

    fn observe_matched_hit(
        &mut self,
        timestamp: &SlotTimestamp,
        hit: &DeepHit,
        gate: PhysicalAdmissionGate,
    ) -> Option<DeepHit> {
        if hit.conf != DeepConfidence::TwoSlotMatched {
            return None;
        }
        let nutc = timestamp.nutc();
        let parity = slot_parity(timestamp);
        let msg_norm = normalize_message(&hit.msg);
        if let Some(prev) = self.matched_observations.iter_mut().find(|prev| {
            prev.msg_norm == msg_norm
                && prev.parity == parity
                && prev.last_seen_nutc != nutc
                && (prev.freq - hit.freq).abs() <= gate.freq_tolerance_hz
                && (prev.dt - hit.dt).abs() <= gate.dt_tolerance_s
        }) {
            prev.last_seen_nutc = nutc;
            prev.freq = hit.freq;
            prev.dt = hit.dt;
            prev.stat = prev.stat.min(hit.stat);
            prev.margin = prev.margin.min(hit.margin);
            return Some(DeepHit {
                msg: hit.msg.clone(),
                stat: prev.stat,
                margin: prev.margin,
                freq: hit.freq,
                dt: hit.dt,
                snr: hit.snr,
                conf: DeepConfidence::TwoSlotMatched,
            });
        }

        self.matched_observations.push(MatchedObservation {
            msg_norm,
            parity,
            freq: hit.freq,
            dt: hit.dt,
            stat: hit.stat,
            margin: hit.margin,
            last_seen_nutc: nutc,
        });
        None
    }

    fn take_deep_crc_decode_budget(&mut self, nutc: u32) -> bool {
        if self.deep_crc_budget_nutc != Some(nutc) {
            self.deep_crc_budget_nutc = Some(nutc);
            self.deep_crc_budget_used = 0;
        }
        if self.deep_crc_budget_used >= MAX_DEEP_CRC_DECODE_PER_SLOT {
            return false;
        }
        self.deep_crc_budget_used += 1;
        true
    }

    #[cfg(test)]
    fn deep_stack_count(&self) -> usize {
        self.deep_stacks.len()
    }

    #[cfg(test)]
    fn deep_crc_budget_used(&self) -> usize {
        self.deep_crc_budget_used
    }

    pub(super) fn deep_diagnostics(&self) -> DeepStackDiagnostics {
        self.deep_diagnostics
    }

    #[cfg(test)]
    fn matched_observation_count(&self) -> usize {
        self.matched_observations.len()
    }

    #[cfg(test)]
    fn observed_qso_progress(&self) -> Option<usize> {
        self.qso_progress()
    }

    fn observe_qso_progress_candidate(&mut self, msg: &str, role: MessageRole) {
        let words: Vec<String> = msg.split_whitespace().map(normalize_message_word).collect();
        self.qso_progress_probe = Some(infer_qso_progress_probe(&words, role));
    }

    fn harvest_grid(&mut self, msg: &str) {
        let words: Vec<String> = msg.split_whitespace().map(normalize_message_word).collect();
        if let Some(grid) = words.iter().rev().find(|word| is_grid4(word)) {
            self.hisgrid = Some(grid.clone());
        }
    }

    fn has_hard_grid_contradiction(&self, row: &StreamDecodedMessage) -> bool {
        let Some(hisgrid) = self.hisgrid.as_deref() else {
            return false;
        };
        let words: Vec<String> = row
            .msg
            .split_whitespace()
            .map(normalize_message_word)
            .collect();
        if !is_target_sender(&words, &self.target) {
            return false;
        }
        words
            .iter()
            .rev()
            .any(|word| is_grid4(word) && word != hisgrid)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MessageRole {
    contains_target: bool,
    target_sender: bool,
    target_recipient: bool,
    contains_mycall: bool,
}

impl MessageRole {
    fn from_message(msg: &str, target: &DxTarget, mycall: Option<&DxTarget>) -> Self {
        let words: Vec<String> = msg.split_whitespace().map(normalize_message_word).collect();
        let contains_target = words.iter().any(|word| target.matches_word(word));
        let contains_mycall =
            mycall.is_some_and(|mycall| words.iter().any(|word| mycall.matches_word(word)));
        let target_sender = is_target_sender(&words, target);
        let target_recipient = is_target_recipient(&words, target);
        Self {
            contains_target,
            target_sender,
            target_recipient,
            contains_mycall,
        }
    }
}

fn is_target_sender(words: &[String], target: &DxTarget) -> bool {
    if words.is_empty() {
        return false;
    }
    if words[0] == "CQ" {
        return words.iter().skip(1).any(|word| target.matches_word(word));
    }
    words.get(1).is_some_and(|word| target.matches_word(word))
}

fn is_target_recipient(words: &[String], target: &DxTarget) -> bool {
    words.first().is_some_and(|word| target.matches_word(word))
}

fn is_grid4(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() == 4
        && bytes[0].is_ascii_alphabetic()
        && bytes[1].is_ascii_alphabetic()
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
}

fn infer_qso_progress_probe(words: &[String], role: MessageRole) -> QsoProgressProbe {
    let last_message = words.join(" ");
    if !role.contains_target || !role.contains_mycall {
        return QsoProgressProbe {
            last_message,
            inferred_progress: None,
            reason: "not-my-target-qso",
        };
    }
    let Some(exchange) = words.get(2).map(String::as_str) else {
        return QsoProgressProbe {
            last_message,
            inferred_progress: None,
            reason: "no-exchange-token",
        };
    };
    let (inferred_progress, reason) = match exchange {
        "RR73" => (Some(5), "rr73"),
        "73" => (Some(5), "73"),
        "RRR" => (Some(4), "rrr"),
        token if is_r_report(token) => (Some(3), "r-report"),
        token if is_signal_report(token) => (Some(2), "signal-report"),
        token if is_grid4(token) => (Some(1), "grid"),
        _ => (None, "unknown-exchange"),
    };
    QsoProgressProbe {
        last_message,
        inferred_progress,
        reason,
    }
}

fn is_signal_report(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() == 3
        && matches!(bytes[0], b'+' | b'-')
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
}

fn is_r_report(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() == 4
        && bytes[0] == b'R'
        && matches!(bytes[1], b'+' | b'-')
        && bytes[2].is_ascii_digit()
        && bytes[3].is_ascii_digit()
}

fn slot_parity(timestamp: &SlotTimestamp) -> usize {
    ((timestamp.nutc() / 5) % 2) as usize
}

fn low_band_rank(low_band_prior: bool, freq: f64) -> u8 {
    if low_band_prior && freq <= 1000.0 {
        0
    } else {
        1
    }
}

fn slots_between(start_nutc: u32, end_nutc: u32) -> u32 {
    let start = nutc_to_seconds(start_nutc);
    let mut end = nutc_to_seconds(end_nutc);
    if end < start {
        end += 24 * 60 * 60;
    }
    (end - start) / 15
}

fn nutc_to_seconds(nutc: u32) -> u32 {
    let h = nutc / 10000;
    let m = (nutc / 100) % 100;
    let s = nutc % 100;
    h * 3600 + m * 60 + s
}

#[cfg(test)]
mod tests {
    use super::super::deepsearch::{build_v1_hypotheses, DeepConfidence};
    use super::*;
    use crate::stream::session::StreamSnrSource;

    fn row(freq: f64, msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq,
            dt: 0.2,
            snr: -10.0,
            snr_source: StreamSnrSource::Decoder,
            deep_confidence: None,
            msg: msg.to_string(),
            sync: 2.0,
            itone: [0; 79],
        }
    }

    fn deep_field(
        freq: f64,
        dt: f64,
        nsync: usize,
        syncavemax: f32,
    ) -> crate::decode::lib_jtdx::ft8b::DxSymbolField {
        crate::decode::lib_jtdx::ft8b::DxSymbolField {
            s8: [[0.0; 79]; 8],
            llr: [0.0; 174],
            ibest: 0,
            refined_freq: freq,
            refined_dt: dt,
            syncavemax,
            nsync,
        }
    }

    fn matched_field(
        hypothesis: &Hypothesis,
        freq: f64,
        dt: f64,
    ) -> crate::decode::lib_jtdx::ft8b::DxSymbolField {
        let mut field = deep_field(freq, dt, 8, 2.0);
        for (dst, &bit) in field.llr.iter_mut().zip(hypothesis.codeword.iter()) {
            *dst = if bit == 1 { 1.0 } else { -1.0 };
        }
        field
    }

    #[test]
    fn target_second_call_is_sender_parity() {
        let target = DxTarget::new("UA3QNA");
        let role = MessageRole::from_message("F1MLZ UA3QNA -04", &target, None);

        assert!(role.target_sender);
        assert!(!role.target_recipient);
    }

    #[test]
    fn target_first_call_is_recipient_parity() {
        let target = DxTarget::new("UA3QNA");
        let role = MessageRole::from_message("UA3QNA F1MLZ R-05", &target, None);

        assert!(!role.target_sender);
        assert!(role.target_recipient);
    }

    #[test]
    fn cq_dx_target_is_sender() {
        let target = DxTarget::new("DL8YHR");
        let role = MessageRole::from_message("CQ DX DL8YHR JO41", &target, None);

        assert!(role.target_sender);
    }

    #[test]
    fn inferred_parity_does_not_hard_skip_opposite_slots() {
        let mut store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            1000.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let recipient_ts = SlotTimestamp::parse("140630").unwrap();
        store.harvest_listen(
            &recipient_ts,
            &[StreamDecodedMessage {
                freq: 1000.0,
                dt: 0.0,
                snr: -5.0,
                snr_source: StreamSnrSource::Decoder,
                deep_confidence: None,
                msg: "UA3QNA F1MLZ R-05".to_string(),
                sync: 0.0,
                itone: [0; 79],
            }],
        );

        assert!(matches!(
            store.tx_parity,
            Some(TxParity {
                confidence: ParityConfidence::Inferred,
                ..
            })
        ));
        assert!(store.should_run_focused(&SlotTimestamp::parse("140630").unwrap()));
        assert!(store.should_run_focused(&SlotTimestamp::parse("140645").unwrap()));

        store.harvest_listen(
            &SlotTimestamp::parse("140700").unwrap(),
            &[StreamDecodedMessage {
                freq: 1000.0,
                dt: 0.0,
                snr: -5.0,
                snr_source: StreamSnrSource::Decoder,
                deep_confidence: None,
                msg: "F1MLZ UA3QNA -04".to_string(),
                sync: 0.0,
                itone: [0; 79],
            }],
        );
        assert!(matches!(
            store.tx_parity,
            Some(TxParity {
                confidence: ParityConfidence::Observed,
                ..
            })
        ));
    }

    #[test]
    fn observed_parity_ages_out_before_it_can_silently_skip_target_slots() {
        let mut store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            1000.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let observed_ts = SlotTimestamp::parse("140700").unwrap();
        store.harvest_listen(
            &observed_ts,
            &[StreamDecodedMessage {
                freq: 1000.0,
                dt: 0.0,
                snr: -5.0,
                snr_source: StreamSnrSource::Decoder,
                deep_confidence: None,
                msg: "F1MLZ UA3QNA -04".to_string(),
                sync: 0.0,
                itone: [0; 79],
            }],
        );

        assert!(store.should_run_focused(&observed_ts));
        assert!(
            !store.should_run_focused(&observed_ts.add_seconds(15)),
            "fresh observed parity should still save focused/deep work on the opposite slot"
        );
        assert!(
            store.should_run_focused(&observed_ts.add_seconds(17 * 15)),
            "stale observed parity must degrade to sensitivity-first probing instead of hard-skipping forever"
        );
    }

    #[test]
    fn frequency_selection_collapses_one_window_and_prefers_confidence() {
        let mut store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();
        store.harvest_listen(&ts, &[row(1152.0, "F1MLZ RA3ABG KO95")]);
        store.harvest_listen(&ts, &[row(1154.0, "F1MLZ UA3QNA -04")]);

        assert_eq!(store.selected_foci(), vec![1154.0]);
    }

    #[test]
    fn focus_selection_order_is_independent_of_harvest_order() {
        fn harvest_in_order(freqs: &[f64]) -> Vec<f64> {
            let mut store = TargetContextStore::new(
                DxTarget::new("UA3QNA"),
                Some("F1MLZ"),
                0.0,
                None,
                false,
                200.0,
                3000.0,
            );
            let ts = SlotTimestamp::parse("140630").unwrap();
            let rows: Vec<_> = freqs
                .iter()
                .map(|&freq| row(freq, "F1MLZ UA3QNA -04"))
                .collect();
            store.harvest_listen(&ts, &rows);
            store.selected_foci()
        }

        let ascending_input = harvest_in_order(&[700.0, 900.0, 1300.0, 1500.0]);
        let mixed_input = harvest_in_order(&[1500.0, 700.0, 1300.0, 900.0]);

        assert_eq!(ascending_input, vec![700.0, 900.0, 1300.0, 1500.0]);
        assert_eq!(mixed_input, ascending_input);
    }

    #[test]
    fn qso_progress_probe_tracks_my_target_exchange_observe_only() {
        let mut store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(1152.0, "F1MLZ UA3QNA -04")]);
        assert_eq!(store.observed_qso_progress(), Some(2));

        store.harvest_listen(&ts, &[row(1152.0, "F1MLZ UA3QNA R-12")]);
        assert_eq!(store.observed_qso_progress(), Some(3));

        store.harvest_listen(&ts, &[row(1152.0, "F1MLZ UA3QNA RR73")]);
        assert_eq!(store.observed_qso_progress(), Some(5));
    }

    #[test]
    fn qso_progress_probe_ignores_target_working_someone_else() {
        let mut store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(1152.0, "RA3ABG UA3QNA -04")]);

        assert_eq!(store.observed_qso_progress(), None);
    }

    #[test]
    fn hard_grid_contradiction_suppresses_only_target_sender_rows() {
        let store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            Some("PM00"),
            false,
            200.0,
            3000.0,
        );

        assert!(!store.should_emit_target_row(&row(1000.0, "CQ BG5ATV FN42")));
        assert!(store.should_emit_target_row(&row(1000.0, "BG5ATV K1JT FN42")));
        assert!(store.should_emit_target_row(&row(1000.0, "CQ BG5ATV PM00")));
    }

    #[test]
    fn out_of_passband_seed_is_ignored() {
        let store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            3500.0,
            None,
            false,
            200.0,
            3000.0,
        );

        assert!(store.selected_foci().is_empty());
    }

    #[test]
    fn in_passband_user_seed_is_pinned() {
        let mut store = TargetContextStore::new(
            DxTarget::new("UA3QNA"),
            Some("F1MLZ"),
            1152.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let later = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&later, &[]);

        assert_eq!(store.selected_foci(), vec![1152.0]);
    }

    #[test]
    fn hound_mode_does_not_focus_hunter_rows() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            true,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(2100.0, "DX1AAA MY1AAA R-05")]);
        assert!(store.selected_foci().is_empty());

        store.harvest_listen(&ts, &[row(600.0, "MY1AAA DX1AAA -10")]);
        assert_eq!(store.selected_foci(), vec![600.0]);
    }

    #[test]
    fn frequency_aging_survives_midnight_rollover() {
        assert_eq!(slots_between(235945, 0), 1);
        assert_eq!(slots_between(235945, 15), 2);
    }

    #[test]
    fn fox_multistream_pre_places_full_60hz_grid_from_lowest() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            true,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        // One observed Fox stream is not enough — stays a single focus.
        store.harvest_listen(&ts, &[row(300.0, "MY1AAA DX1AAA -10")]);
        assert_eq!(store.selected_foci(), vec![300.0]);

        // A second distinct stream proves a multi-stream Fox: lay the full grid
        // at 60 Hz from the lowest, so streams 3/4/5 are covered before they appear.
        store.harvest_listen(&ts, &[row(360.0, "OTHER DX1AAA -12")]);
        assert_eq!(
            store.selected_foci(),
            vec![300.0, 360.0, 420.0, 480.0, 540.0]
        );
    }

    #[test]
    fn fox_multistream_anchor_re_adjusts_to_dynamic_lowest() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            true,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(360.0, "MY1AAA DX1AAA -10")]);
        store.harvest_listen(&ts, &[row(420.0, "OTHER DX1AAA -12")]);
        assert_eq!(
            store.selected_foci(),
            vec![360.0, 420.0, 480.0, 540.0, 600.0]
        );

        // The always-on listen later decodes a lower stream we had missed; the
        // anchor drops to it and the whole grid shifts down (dynamic fallback).
        store.harvest_listen(&ts, &[row(300.0, "THIRD DX1AAA -08")]);
        assert_eq!(
            store.selected_foci(),
            vec![300.0, 360.0, 420.0, 480.0, 540.0]
        );
    }

    #[test]
    fn fox_multistream_grid_is_clipped_to_passband_top() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            true,
            200.0,
            1000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(900.0, "MY1AAA DX1AAA -10")]);
        store.harvest_listen(&ts, &[row(960.0, "OTHER DX1AAA -12")]);

        // 900, 960, 1020(window 995..1045 still overlaps the 1000 Hz top);
        // 1080's window 1055..1105 is fully past the passband, so it is dropped.
        assert_eq!(store.selected_foci(), vec![900.0, 960.0, 1020.0]);
    }

    #[test]
    fn multistream_grid_is_hound_only() {
        // Same two streams but not in Hound mode: keep the plain harvested foci,
        // never the Fox grid (a non-FH target does not transmit a spaced block).
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(300.0, "MY1AAA DX1AAA -10")]);
        store.harvest_listen(&ts, &[row(360.0, "OTHER DX1AAA -12")]);
        assert_eq!(store.selected_foci(), vec![300.0, 360.0]);
    }

    #[test]
    fn deep_stack_ignores_fields_below_physical_floor() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();
        let gate = PhysicalAdmissionGate {
            min_nsync: 4,
            min_syncavemax: 0.5,
            ..PhysicalAdmissionGate::default()
        };

        let field = deep_field(1000.0, 0.2, 3, 1.0);
        let hit =
            store.observe_deep_field(&ts, 1000.0, &field, &[], DeepSearchGate::default(), gate);

        assert!(hit.is_none());
        assert_eq!(store.deep_stack_count(), 0);
    }

    #[test]
    fn deep_stack_allows_multiple_physical_stacks_per_frequency_bucket() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();
        let gate = PhysicalAdmissionGate {
            freq_tolerance_hz: 3.0,
            dt_tolerance_s: 0.1,
            min_nsync: 4,
            min_syncavemax: 0.0,
        };

        let first = deep_field(1000.1, 0.2, 5, 1.0);
        let same_bucket_different_dt = deep_field(1000.4, 0.6, 5, 1.0);
        store.observe_deep_field(&ts, 1000.0, &first, &[], DeepSearchGate::default(), gate);
        store.observe_deep_field(
            &ts,
            1000.0,
            &same_bucket_different_dt,
            &[],
            DeepSearchGate::default(),
            gate,
        );

        assert_eq!(store.deep_stack_count(), 2);
    }

    #[test]
    fn deep_crc_decode_budget_is_slot_bounded() {
        let mut store = TargetContextStore::new(
            DxTarget::new("DX1AAA"),
            Some("MY1AAA"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );

        assert!(store.take_deep_crc_decode_budget(140630));
        assert!(store.take_deep_crc_decode_budget(140630));
        assert!(!store.take_deep_crc_decode_budget(140630));
        assert_eq!(store.deep_crc_budget_used(), 2);

        assert!(store.take_deep_crc_decode_budget(140645));
        assert_eq!(store.deep_crc_budget_used(), 1);
    }

    #[test]
    fn stack_decode_candidate_prefers_deeper_stack_before_spending_crc_budget() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let key = StackKey {
            parity: slot_parity(&SlotTimestamp::parse("140630").unwrap()),
            freq_bin: 1000,
        };
        let gate = PhysicalAdmissionGate {
            freq_tolerance_hz: 3.0,
            dt_tolerance_s: 0.3,
            min_nsync: 1,
            min_syncavemax: 0.0,
        };
        let shallow = deep_field(1001.0, 0.2, 12, 9.0);
        let deep = deep_field(999.0, 0.2, 4, 1.0);
        store
            .deep_stacks
            .push(SlotStack::new(key, &shallow, 140600));
        store.deep_stacks.push(SlotStack::new(key, &deep, 140545));
        assert!(store.deep_stacks[1].admit_with_hypotheses(
            &deep,
            140600,
            gate,
            &[],
            DeepSearchGate::default(),
        ));

        let current = deep_field(1000.0, 0.2, 8, 2.0);
        let candidate = store
            .best_stack_decode_candidate(0, key, 1000.0, &current, gate)
            .expect("both stacks should be admissible");

        assert_eq!(candidate.idx, 1);
        assert_eq!(store.deep_stacks[candidate.idx].depth(), 2);
        assert_eq!(store.deep_crc_budget_used(), 0);
    }

    #[test]
    fn deep_stack_diagnostics_count_budget_skips() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let timestamp = SlotTimestamp::parse("140630").unwrap();
        let key_parity = slot_parity(&timestamp);
        let gate = PhysicalAdmissionGate {
            freq_tolerance_hz: 3.0,
            dt_tolerance_s: 0.3,
            min_nsync: 1,
            min_syncavemax: 0.0,
        };
        let prior_a = deep_field(990.0, 0.2, 8, 2.0);
        let prior_b = deep_field(1000.0, 0.2, 8, 2.0);
        let prior_c = deep_field(1010.0, 0.2, 8, 2.0);
        store.deep_stacks.push(SlotStack::new(
            StackKey {
                parity: key_parity,
                freq_bin: 990,
            },
            &prior_a,
            140600,
        ));
        store.deep_stacks.push(SlotStack::new(
            StackKey {
                parity: key_parity,
                freq_bin: 1000,
            },
            &prior_b,
            140600,
        ));
        store.deep_stacks.push(SlotStack::new(
            StackKey {
                parity: key_parity,
                freq_bin: 1010,
            },
            &prior_c,
            140600,
        ));

        let current_a = deep_field(990.0, 0.2, 8, 2.0);
        let current_b = deep_field(1000.0, 0.2, 8, 2.0);
        let current_c = deep_field(1010.0, 0.2, 8, 2.0);
        let inputs = [
            DeepFieldInput {
                focus: 990.0,
                field: &current_a,
            },
            DeepFieldInput {
                focus: 1000.0,
                field: &current_b,
            },
            DeepFieldInput {
                focus: 1010.0,
                field: &current_c,
            },
        ];

        let _ =
            store.observe_deep_fields(&timestamp, &inputs, &[], DeepSearchGate::default(), gate);

        assert_eq!(
            store.deep_diagnostics(),
            DeepStackDiagnostics {
                crc_candidates: 3,
                crc_attempts: 2,
                crc_skipped_budget: 1,
            }
        );
    }

    #[test]
    fn matched_filter_hit_requires_two_slots_before_returning() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let wanted = hypotheses
            .iter()
            .find(|hypothesis| normalize_message(&hypothesis.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let field = matched_field(wanted, 1000.0, 0.2);
        let matched_gate = DeepSearchGate {
            min_stat: 100.0,
            min_margin: 1.0,
            min_nsync: 1,
            min_syncavemax: 0.0,
            top_k: hypotheses.len(),
        };
        let no_stack_gate = PhysicalAdmissionGate {
            min_nsync: 99,
            ..PhysicalAdmissionGate::default()
        };

        let first = store.observe_deep_field(
            &SlotTimestamp::parse("140630").unwrap(),
            1000.0,
            &field,
            &hypotheses,
            matched_gate,
            no_stack_gate,
        );
        assert!(first.is_none());
        assert_eq!(store.matched_observation_count(), 1);

        let second = store.observe_deep_field(
            &SlotTimestamp::parse("140700").unwrap(),
            1000.0,
            &field,
            &hypotheses,
            matched_gate,
            no_stack_gate,
        );

        let hit = second.expect("second same-parity matched observation should corroborate");
        assert_eq!(hit.conf, DeepConfidence::TwoSlotMatched);
        assert_eq!(normalize_message(&hit.msg), "K1JT BG5ATV -10");
    }

    #[test]
    fn matched_filter_corroboration_requires_same_parity_and_physical_match() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let wanted = hypotheses
            .iter()
            .find(|hypothesis| normalize_message(&hypothesis.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let matched_gate = DeepSearchGate {
            min_stat: 100.0,
            min_margin: 1.0,
            min_nsync: 1,
            min_syncavemax: 0.0,
            top_k: hypotheses.len(),
        };
        let no_stack_gate = PhysicalAdmissionGate {
            freq_tolerance_hz: 3.0,
            dt_tolerance_s: 0.3,
            min_nsync: 99,
            ..PhysicalAdmissionGate::default()
        };

        let first = matched_field(wanted, 1000.0, 0.2);
        assert!(store
            .observe_deep_field(
                &SlotTimestamp::parse("140630").unwrap(),
                1000.0,
                &first,
                &hypotheses,
                matched_gate,
                no_stack_gate,
            )
            .is_none());

        let opposite_parity = matched_field(wanted, 1000.0, 0.2);
        assert!(store
            .observe_deep_field(
                &SlotTimestamp::parse("140645").unwrap(),
                1000.0,
                &opposite_parity,
                &hypotheses,
                matched_gate,
                no_stack_gate,
            )
            .is_none());

        let too_far = matched_field(wanted, 1010.0, 0.2);
        assert!(store
            .observe_deep_field(
                &SlotTimestamp::parse("140700").unwrap(),
                1010.0,
                &too_far,
                &hypotheses,
                matched_gate,
                no_stack_gate,
            )
            .is_none());

        let compatible = matched_field(wanted, 1001.0, 0.25);
        let hit = store
            .observe_deep_field(
                &SlotTimestamp::parse("140730").unwrap(),
                1001.0,
                &compatible,
                &hypotheses,
                matched_gate,
                no_stack_gate,
            )
            .expect("same-parity physically compatible matched observation should corroborate");

        assert_eq!(hit.conf, DeepConfidence::TwoSlotMatched);
        assert_eq!(normalize_message(&hit.msg), "K1JT BG5ATV -10");
    }

    #[test]
    fn matched_filter_corroboration_requires_same_normalized_message() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let first_msg = hypotheses
            .iter()
            .find(|hypothesis| normalize_message(&hypothesis.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let changed_msg = hypotheses
            .iter()
            .find(|hypothesis| normalize_message(&hypothesis.msg) == "K1JT BG5ATV R-10")
            .unwrap();
        let matched_gate = DeepSearchGate {
            min_stat: 100.0,
            min_margin: 1.0,
            min_nsync: 1,
            min_syncavemax: 0.0,
            top_k: hypotheses.len(),
        };
        let no_stack_gate = PhysicalAdmissionGate {
            freq_tolerance_hz: 3.0,
            dt_tolerance_s: 0.3,
            min_nsync: 99,
            ..PhysicalAdmissionGate::default()
        };

        let first = matched_field(first_msg, 1000.0, 0.2);
        assert!(store
            .observe_deep_field(
                &SlotTimestamp::parse("140630").unwrap(),
                1000.0,
                &first,
                &hypotheses,
                matched_gate,
                no_stack_gate,
            )
            .is_none());

        let changed = matched_field(changed_msg, 1001.0, 0.25);
        assert!(store
            .observe_deep_field(
                &SlotTimestamp::parse("140700").unwrap(),
                1001.0,
                &changed,
                &hypotheses,
                matched_gate,
                no_stack_gate,
            )
            .is_none());
        assert_eq!(store.matched_observation_count(), 2);
    }

    #[test]
    fn t2_stack_returns_crc_confirmed_hit_on_second_slot() {
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let hypotheses = build_v1_hypotheses(Some("K1JT"), "BG5ATV", Some("PM00"));
        let wanted = hypotheses
            .iter()
            .find(|hypothesis| normalize_message(&hypothesis.msg) == "K1JT BG5ATV -10")
            .unwrap();
        let field = matched_field(wanted, 1000.0, 0.2);
        let physical_gate = PhysicalAdmissionGate {
            min_nsync: 1,
            min_syncavemax: 0.0,
            ..PhysicalAdmissionGate::default()
        };

        let first = store.observe_deep_field(
            &SlotTimestamp::parse("140630").unwrap(),
            1000.0,
            &field,
            &[],
            DeepSearchGate::default(),
            physical_gate,
        );
        assert!(first.is_none());
        assert_eq!(store.deep_stack_count(), 1);

        let second = store.observe_deep_field(
            &SlotTimestamp::parse("140700").unwrap(),
            1000.0,
            &field,
            &[],
            DeepSearchGate::default(),
            physical_gate,
        );

        let hit = second.expect("second physical observation should decode the summed LLR");
        assert_eq!(hit.conf, DeepConfidence::CrcConfirmedExperimental);
        assert_eq!(normalize_message(&hit.msg), "K1JT BG5ATV -10");
    }
}
