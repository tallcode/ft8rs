use crate::stream::session::StreamDecodedMessage;
use crate::stream::time::SlotTimestamp;

use super::filter::{normalize_message_word, DxTarget};

const MAX_FOCI: usize = 5;
const FOCUS_HALF_WIDTH_HZ: f64 = 25.0;

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
        };
        if seed_frequency > 0.0 {
            store.remember_frequency(seed_frequency, 8, 0, true);
        }
        store
    }

    pub(super) fn should_run_focused(&self, timestamp: &SlotTimestamp) -> bool {
        match self.tx_parity {
            Some(tx_parity) => tx_parity.parity == slot_parity(timestamp),
            None => true,
        }
    }

    pub(super) fn selected_foci(&self) -> Vec<f64> {
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

    pub(super) fn hisgrid(&self) -> Option<&str> {
        self.hisgrid.as_deref()
    }

    pub(super) fn should_emit_target_row(&self, row: &StreamDecodedMessage) -> bool {
        if !self.target.matches_message(&row.msg) {
            return false;
        }
        !self.has_hard_grid_contradiction(row)
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

        let parity = slot_parity(timestamp);
        if role.target_sender {
            self.set_tx_parity(parity, ParityConfidence::Observed);
            self.harvest_grid(&row.msg);
            self.dt = Some(row.dt);
        } else if role.target_recipient && self.tx_parity.is_none() {
            self.set_tx_parity(1 - parity, ParityConfidence::Inferred);
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

    fn set_tx_parity(&mut self, parity: usize, confidence: ParityConfidence) {
        let replace = match self.tx_parity {
            None => true,
            Some(existing) => {
                matches!(
                    (existing.confidence, confidence),
                    (ParityConfidence::Inferred, ParityConfidence::Observed)
                ) || existing.parity == parity
            }
        };
        if replace {
            self.tx_parity = Some(TxParity { parity, confidence });
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
    use super::*;

    fn row(freq: f64, msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq,
            dt: 0.2,
            snr: -10.0,
            msg: msg.to_string(),
            sync: 2.0,
            itone: [0; 79],
        }
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
}
