use crate::stream::session::StreamDecodedMessage;
use crate::stream::time::SlotTimestamp;

use super::filter::{normalize_message_word, DxTarget};
use super::HisgridSource;

const MAX_FOCI: usize = 5;
const FOCUS_HALF_WIDTH_HZ: f64 = 25.0;
/// Upstream `foxgen.f90` lays out the Fox's multi-stream signals at a fixed
/// spacing `fstep = 60 Hz` (`f0 = nfreq + fstep*(n-1)`). We mirror that constant
/// to pre-place the full focus grid in Hound mode.
const FOX_STREAM_SPACING_HZ: f64 = 60.0;
/// Number of distinct observed Fox streams at/above which we treat the target as
/// a multi-stream Fox and switch to the equally-spaced grid (owner's rule: ">= 2").
const FOX_MULTISTREAM_THRESHOLD: usize = 2;

/// Where a frequency candidate came from, so operator-derived intel can be
/// dropped on a mycall change while target-derived intel survives (§6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrequencyOrigin {
    TargetSender,
    MyCall,
    UserPinned,
}

#[derive(Clone, Debug)]
struct FrequencyCandidate {
    freq: f64,
    confidence: u8,
    last_seen_nutc: u32,
    pinned: bool,
    origin: FrequencyOrigin,
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
    hisgrid_source: HisgridSource,
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
            hisgrid_source: if hisgrid.is_some() {
                HisgridSource::User
            } else {
                HisgridSource::None
            },
            dt: None,
            low_band_prior: true,
            hound,
            nfa,
            nfb,
        };
        // A pinned QSO seed only counts when it falls inside the search band;
        // 0 / out-of-band means "no nfqso" and must not seed a focus.
        if seed_frequency >= nfa && seed_frequency <= nfb {
            store.remember_frequency(seed_frequency, 8, 0, true, FrequencyOrigin::UserPinned);
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
        } else if role.contains_mycall && !self.hound {
            3
        } else if listen {
            2
        } else {
            1
        };

        // A row where the target is the *recipient* ("BG7XWF JK1QAY ...") carries
        // the caller's TX frequency, not the target's, so it must not seed a focus
        // (it still informs tx parity above). Only the target's own transmissions
        // and mycall-neighbourhood rows pin where we actually look for the target.
        let frequency_seed_allowed = role.target_sender || (!self.hound && role.contains_mycall);
        if frequency_seed_allowed {
            let origin = if role.target_sender {
                FrequencyOrigin::TargetSender
            } else {
                FrequencyOrigin::MyCall
            };
            self.remember_frequency(row.freq, confidence, timestamp.nutc(), false, origin);
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

    fn remember_frequency(
        &mut self,
        freq: f64,
        confidence: u8,
        nutc: u32,
        pinned: bool,
        origin: FrequencyOrigin,
    ) {
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
                existing.origin = origin;
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
            origin,
        });
    }

    fn age_frequencies(&mut self, nutc: u32) {
        self.frequencies
            .retain(|freq| freq.pinned || slots_between(freq.last_seen_nutc, nutc) <= 16);
    }

    /// Drop mycall-derived intel (for a mycall change), keeping target-derived
    /// candidates, the observed transmit parity, and the harvested grid (§6.5).
    pub(super) fn drop_operator_intel(&mut self) {
        self.frequencies.retain(|candidate| {
            matches!(
                candidate.origin,
                FrequencyOrigin::TargetSender | FrequencyOrigin::UserPinned
            )
        });
        if let Some(parity) = self.tx_parity {
            if parity.confidence == ParityConfidence::Inferred {
                self.tx_parity = None;
            }
        }
    }

    pub(super) fn set_mycall(&mut self, mycall: Option<&str>) {
        self.mycall = mycall.map(DxTarget::new);
    }

    /// Re-point the passband and prune candidates that fall outside it.
    pub(super) fn rebind_band(&mut self, nfa: f64, nfb: f64) {
        self.nfa = nfa;
        self.nfb = nfb;
        self.frequencies
            .retain(|candidate| candidate.freq >= nfa && candidate.freq <= nfb);
    }

    pub(super) fn seed_pinned(&mut self, freq: f64) {
        if freq >= self.nfa && freq <= self.nfb {
            self.remember_frequency(freq, 8, 0, true, FrequencyOrigin::UserPinned);
        }
    }

    fn harvest_grid(&mut self, msg: &str) {
        let words: Vec<String> = msg.split_whitespace().map(normalize_message_word).collect();
        if let Some(grid) = words.iter().rev().find(|word| is_grid4(word)) {
            self.hisgrid = Some(grid.clone());
            self.hisgrid_source = HisgridSource::Harvested;
        }
    }

    /// Read-only snapshot pieces for the GUI DX intel panel (foci, tx parity,
    /// effective grid + its source, dt).
    pub(super) fn snapshot_parts(
        &self,
    ) -> (
        Vec<f64>,
        Option<u8>,
        Option<String>,
        HisgridSource,
        Option<f64>,
    ) {
        (
            self.selected_foci(),
            self.tx_parity.map(|parity| parity.parity as u8),
            self.hisgrid.clone(),
            self.hisgrid_source,
            self.dt,
        )
    }

    fn has_hard_grid_contradiction(&self, row: &StreamDecodedMessage) -> bool {
        // Only a user-supplied grid may suppress a target-sender row. A harvested
        // grid can be poisoned by a ~1/1024 10-bit hash collision (a non-target
        // `<hash>` matching the target in a sender position but carrying a
        // different grid): locking that wrong grid would then suppress the *real*
        // target's rows — a silent miss on the one station we chase. Harvested
        // grids still drive a8d recovery; they just never suppress. (DX.md
        // "Known Limitations / Future Hardening".)
        if self.hisgrid_source != HisgridSource::User {
            return false;
        }
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
    fn non_hound_recipient_row_does_not_seed_focus() {
        // "BG7XWF JK1QAY PM95" decoded at 505 Hz: that 505 is JK1QAY's TX
        // frequency (the caller), not BG7XWF's, so chasing BG7XWF we must not
        // turn it into a focus.
        let mut store = TargetContextStore::new(
            DxTarget::new("BG7XWF"),
            Some("BG5ATV"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();

        store.harvest_listen(&ts, &[row(505.0, "BG7XWF JK1QAY PM95")]);
        assert!(store.selected_foci().is_empty());

        // The target's own CQ still seeds its real TX frequency.
        store.harvest_listen(&ts, &[row(1621.0, "CQ BG7XWF OL99")]);
        assert_eq!(store.selected_foci(), vec![1621.0]);
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
    fn harvested_grid_never_suppresses_the_real_target() {
        // A harvested grid can be poisoned by a hash collision, so it must never
        // suppress a target-sender row (only a user grid may). Here the target
        // sends PM00, and a harvested KO95 (e.g. a collision) must not drop the
        // real PM00 row — otherwise the one station we chase goes silently missing.
        let mut store = TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            1000.0,
            None, // no user grid; grid comes only from harvest
            false,
            200.0,
            3000.0,
        );
        let ts = SlotTimestamp::parse("140630").unwrap();
        store.harvest_listen(&ts, &[row(1000.0, "CQ BG5ATV KO95")]);
        assert_eq!(store.hisgrid(), Some("KO95"));
        // The real target's PM00 row contradicts the harvested KO95, but a
        // harvested grid must not suppress it.
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
}
