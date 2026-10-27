use super::filter::DxTarget;
use super::HisgridSource;
use crate::decode::cooperative::{is_grid4, ParsedMessage};
use crate::stream::session::{
    StreamDecodeProvenance, StreamDecodedMessage, StreamDecodedWithProvenance,
};
use crate::stream::time::SlotTimestamp;

const MAX_FOCI: usize = 5;
const FOCUS_HALF_WIDTH_HZ: f64 = 25.0;
const FOX_STREAM_SPACING_HZ: f64 = 60.0;
const HISTORY_SLOTS: u64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrequencyOrigin {
    TargetSender,
    MyCall,
    Assisted,
}
#[derive(Clone, Debug)]
struct FrequencyCandidate {
    freq: f64,
    confidence: u8,
    last_seen: u64,
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
    last_seen: u64,
}

#[derive(Clone, Debug)]
pub(super) struct TargetContextStore {
    target: DxTarget,
    mycall: Option<DxTarget>,
    frequencies: Vec<FrequencyCandidate>,
    pinned: Option<f64>,
    tx_parity: Option<TxParity>,
    pending_parity: Option<(usize, u64)>,
    user_grid: Option<String>,
    observed_grid: Option<(String, u64)>,
    dt: Option<f64>,
    hound: bool,
    nfa: f64,
    nfb: f64,
    tick: u64,
    last_stamp: Option<SlotTimestamp>,
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
            pinned: None,
            tx_parity: None,
            pending_parity: None,
            user_grid: clean_grid(hisgrid),
            observed_grid: None,
            dt: None,
            hound,
            nfa,
            nfb,
            tick: 0,
            last_stamp: None,
        };
        store.seed_pinned(seed_frequency);
        store
    }
    pub(super) fn begin_slot(&mut self, timestamp: &SlotTimestamp) {
        if let Some(previous) = &self.last_stamp {
            if previous == timestamp {
                return;
            }
            let gap = match (absolute_slot(previous), absolute_slot(timestamp)) {
                (Some(a), Some(b)) if b < a => {
                    // Older file timestamps retain their date prefix when
                    // add_seconds wraps midnight; preserve the UTC slot gap.
                    let old = previous.format();
                    let new = timestamp.format();
                    if old.split_once('_').map(|p| p.0) == new.split_once('_').map(|p| p.0)
                        && previous.nutc() > timestamp.nutc()
                    {
                        u64::from(slots_between(previous.nutc(), timestamp.nutc()).max(1))
                    } else {
                        self.frequencies.clear();
                        self.tx_parity = None;
                        self.pending_parity = None;
                        self.observed_grid = None;
                        self.dt = None;
                        1
                    }
                }
                (Some(a), Some(b)) => (b - a).max(1) as u64,
                _ => u64::from(slots_between(previous.nutc(), timestamp.nutc()).max(1)),
            };
            self.tick = self.tick.saturating_add(gap);
        }
        self.last_stamp = Some(timestamp.clone());
        self.frequencies.retain(|f| {
            self.tick.saturating_sub(f.last_seen)
                <= if f.origin == FrequencyOrigin::Assisted {
                    4
                } else {
                    HISTORY_SLOTS
                }
        });
        if self
            .tx_parity
            .is_some_and(|p| self.tick.saturating_sub(p.last_seen) > HISTORY_SLOTS)
        {
            self.tx_parity = None;
        }
        if self
            .pending_parity
            .is_some_and(|(_, t)| self.tick.saturating_sub(t) > HISTORY_SLOTS)
        {
            self.pending_parity = None;
        }
        if self
            .observed_grid
            .as_ref()
            .is_some_and(|(_, t)| self.tick.saturating_sub(*t) > 64)
        {
            self.observed_grid = None;
        }
    }
    pub(super) fn should_run_focused(&self, timestamp: &SlotTimestamp) -> bool {
        self.tx_parity.is_none_or(|p| {
            p.confidence == ParityConfidence::Inferred || p.parity == slot_parity(timestamp)
        })
    }
    pub(super) fn selected_foci(&self) -> Vec<f64> {
        let mut candidates = self.frequencies.clone();
        candidates.sort_by(|a, b| {
            b.confidence
                .cmp(&a.confidence)
                .then_with(|| b.last_seen.cmp(&a.last_seen))
                .then_with(|| a.freq.total_cmp(&b.freq))
        });
        let mut foci = Vec::new();
        if let Some(pinned) = self.pinned {
            foci.push(pinned);
        }
        for candidate in candidates {
            push_focus(&mut foci, candidate.freq);
        }
        // Actual evidence always gets a slot before extrapolated Fox streams.
        if let Some(grid) = self.fox_multistream_foci() {
            for freq in grid {
                push_focus(&mut foci, freq);
            }
        }
        foci
    }
    fn fox_multistream_foci(&self) -> Option<Vec<f64>> {
        if !self.hound {
            return None;
        }
        let latest = self
            .frequencies
            .iter()
            .filter(|f| f.origin == FrequencyOrigin::TargetSender)
            .map(|f| f.last_seen)
            .max()?;
        let mut observed: Vec<_> = self
            .frequencies
            .iter()
            .filter(|f| f.origin == FrequencyOrigin::TargetSender && f.last_seen == latest)
            .map(|f| f.freq)
            .collect();
        if observed.len() < 2 {
            return None;
        }
        observed.sort_by(f64::total_cmp);
        let base = observed[0];
        if !observed.iter().all(|f| {
            let step = ((f - base) / FOX_STREAM_SPACING_HZ).round();
            (0.0..MAX_FOCI as f64).contains(&step)
                && ((f - base) - step * FOX_STREAM_SPACING_HZ).abs() <= 5.0
        }) {
            return None;
        }
        Some(
            (0..MAX_FOCI)
                .map(|step| base + step as f64 * FOX_STREAM_SPACING_HZ)
                .take_while(|f| f - FOCUS_HALF_WIDTH_HZ <= self.nfb)
                .collect(),
        )
    }
    pub(super) fn hisgrid(&self) -> Option<&str> {
        self.user_grid
            .as_deref()
            .or_else(|| self.observed_grid.as_ref().map(|(g, _)| g.as_str()))
    }
    fn grid_source(&self) -> HisgridSource {
        if self.user_grid.is_some() {
            HisgridSource::User
        } else if self.observed_grid.is_some() {
            HisgridSource::Harvested
        } else {
            HisgridSource::None
        }
    }
    pub(super) fn should_emit_target_row(&self, row: &StreamDecodedMessage) -> bool {
        self.target.matches_message(&row.msg) && !self.has_hard_grid_contradiction(row)
    }
    #[cfg(test)]
    pub(super) fn harvest_listen(
        &mut self,
        timestamp: &SlotTimestamp,
        rows: &[StreamDecodedMessage],
    ) {
        self.begin_slot(timestamp);
        for row in rows {
            self.harvest_row(timestamp, row, true);
        }
    }
    pub(super) fn harvest_tagged(
        &mut self,
        timestamp: &SlotTimestamp,
        rows: &[StreamDecodedWithProvenance],
    ) {
        self.begin_slot(timestamp);
        for row in rows {
            self.harvest_row(
                timestamp,
                &row.decode,
                row.provenance == StreamDecodeProvenance::Regular,
            );
        }
    }
    pub(super) fn harvest_focused(
        &mut self,
        timestamp: &SlotTimestamp,
        rows: &[StreamDecodedMessage],
    ) {
        self.begin_slot(timestamp);
        for row in rows {
            self.harvest_row(timestamp, row, false);
        }
    }
    fn harvest_row(
        &mut self,
        timestamp: &SlotTimestamp,
        row: &StreamDecodedMessage,
        observed: bool,
    ) {
        if self.has_hard_grid_contradiction(row) || !row.dt.is_finite() || row.dt.abs() > 3.5 {
            return;
        }
        let role = MessageRole::from_message(&row.msg, &self.target, self.mycall.as_ref());
        if !role.contains_target && !role.contains_mycall {
            return;
        }
        let parsed = ParsedMessage::parse(&row.msg);
        if parsed.unresolved {
            return;
        }
        if observed && role.target_sender {
            self.set_tx_parity(slot_parity(timestamp), ParityConfidence::Observed);
            if let Some(grid) = parsed.grid {
                self.observed_grid = Some((grid, self.tick));
            } else if let Some((_, seen)) = &mut self.observed_grid {
                *seen = self.tick;
            }
            self.dt = Some(row.dt);
        } else if observed && role.target_recipient && self.tx_parity.is_none() {
            self.set_tx_parity(1 - slot_parity(timestamp), ParityConfidence::Inferred);
        }
        if role.target_sender || (!self.hound && role.contains_mycall) {
            let (confidence, origin) = if !observed {
                (1, FrequencyOrigin::Assisted)
            } else if role.target_sender {
                (10, FrequencyOrigin::TargetSender)
            } else {
                (3, FrequencyOrigin::MyCall)
            };
            self.remember_frequency(row.freq, confidence, origin);
        }
    }
    fn set_tx_parity(&mut self, parity: usize, confidence: ParityConfidence) {
        if confidence == ParityConfidence::Inferred {
            if self.tx_parity.is_none() {
                self.tx_parity = Some(TxParity {
                    parity,
                    confidence,
                    last_seen: self.tick,
                });
            }
            return;
        }
        if let Some(existing) = self.tx_parity {
            if existing.confidence == ParityConfidence::Observed && existing.parity != parity {
                self.tx_parity = None;
                self.pending_parity = Some((parity, self.tick));
                return;
            }
        } else if let Some((pending, when)) = self.pending_parity {
            if pending != parity || when == self.tick {
                self.pending_parity = Some((parity, self.tick));
                return;
            }
        }
        self.tx_parity = Some(TxParity {
            parity,
            confidence,
            last_seen: self.tick,
        });
        self.pending_parity = None;
    }
    fn remember_frequency(&mut self, freq: f64, confidence: u8, origin: FrequencyOrigin) {
        if !freq.is_finite() || freq < self.nfa || freq > self.nfb {
            return;
        }
        if let Some(existing) = self
            .frequencies
            .iter_mut()
            .find(|f| f.origin == origin && (f.freq - freq).abs() <= FOCUS_HALF_WIDTH_HZ)
        {
            existing.freq = freq;
            existing.confidence = confidence;
            existing.last_seen = self.tick;
        } else {
            self.frequencies.push(FrequencyCandidate {
                freq,
                confidence,
                last_seen: self.tick,
                origin,
            });
        }
        if self.frequencies.len() > 20 {
            self.frequencies.sort_by(|a, b| {
                b.confidence
                    .cmp(&a.confidence)
                    .then_with(|| b.last_seen.cmp(&a.last_seen))
            });
            self.frequencies.truncate(20);
        }
    }
    pub(super) fn drop_operator_intel(&mut self) {
        self.frequencies
            .retain(|f| f.origin == FrequencyOrigin::TargetSender);
        if self
            .tx_parity
            .is_some_and(|p| p.confidence == ParityConfidence::Inferred)
        {
            self.tx_parity = None;
        }
    }
    pub(super) fn apply_user_config(
        &mut self,
        mycall: Option<&str>,
        grid: Option<&str>,
        frequency: f64,
        hound: bool,
        nfa: f64,
        nfb: f64,
    ) {
        self.mycall = mycall.map(DxTarget::new);
        let grid = clean_grid(grid);
        if self.user_grid != grid {
            self.observed_grid = None;
        }
        self.user_grid = grid;
        self.hound = hound;
        self.nfa = nfa;
        self.nfb = nfb;
        self.frequencies.retain(|f| f.freq >= nfa && f.freq <= nfb);
        self.seed_pinned(frequency);
    }
    pub(super) fn seed_pinned(&mut self, freq: f64) {
        self.pinned = (freq.is_finite() && freq > 0.0 && freq >= self.nfa && freq <= self.nfb)
            .then_some(freq);
    }
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
            self.tx_parity
                .filter(|p| p.confidence == ParityConfidence::Observed)
                .map(|p| p.parity as u8),
            self.hisgrid().map(str::to_string),
            self.grid_source(),
            self.dt,
        )
    }
    fn has_hard_grid_contradiction(&self, row: &StreamDecodedMessage) -> bool {
        let Some(grid) = self.user_grid.as_deref() else {
            return false;
        };
        let parsed = ParsedMessage::parse(&row.msg);
        parsed
            .sender
            .as_deref()
            .is_some_and(|call| self.target.matches_word(call))
            && parsed.grid.as_deref().is_some_and(|found| found != grid)
    }
}

fn push_focus(foci: &mut Vec<f64>, freq: f64) {
    if foci.len() < MAX_FOCI
        && !foci
            .iter()
            .any(|old| (old - freq).abs() <= FOCUS_HALF_WIDTH_HZ)
    {
        foci.push(freq);
    }
}
fn clean_grid(grid: Option<&str>) -> Option<String> {
    let grid = grid?.trim().to_ascii_uppercase();
    let first = grid.get(..4)?;
    is_grid4(first).then(|| first.to_string())
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
        let parsed = ParsedMessage::parse(msg);
        let target_sender = parsed
            .sender
            .as_deref()
            .is_some_and(|call| target.matches_word(call));
        let target_recipient = parsed
            .recipients
            .iter()
            .any(|call| target.matches_word(call));
        let contains_mycall = mycall.is_some_and(|my| {
            parsed
                .sender
                .iter()
                .chain(parsed.recipients.iter())
                .any(|call| my.matches_word(call))
        });
        Self {
            contains_target: target_sender || target_recipient,
            target_sender,
            target_recipient,
            contains_mycall,
        }
    }
}
fn slot_parity(timestamp: &SlotTimestamp) -> usize {
    ((timestamp.nutc() / 5) % 2) as usize
}
fn slots_between(start: u32, end: u32) -> u32 {
    let start = nutc_to_seconds(start);
    let mut end = nutc_to_seconds(end);
    if end < start {
        end += 86400;
    }
    (end - start) / 15
}
fn nutc_to_seconds(nutc: u32) -> u32 {
    nutc / 10000 * 3600 + (nutc / 100) % 100 * 60 + nutc % 100
}
fn absolute_slot(timestamp: &SlotTimestamp) -> Option<i64> {
    let text = timestamp.format();
    let (date, _) = text.split_once('_')?;
    let (year, rest) = if date.len() == 6 {
        (2000 + date[..2].parse::<i64>().ok()?, &date[2..])
    } else {
        (date[..4].parse::<i64>().ok()?, &date[4..])
    };
    let month = rest[..2].parse::<i64>().ok()?;
    let day = rest[2..].parse::<i64>().ok()?;
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    Some(
        (era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy) * 5760
            + i64::from(nutc_to_seconds(timestamp.nutc()) / 15),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> TargetContextStore {
        TargetContextStore::new(
            DxTarget::new("BG5ATV"),
            Some("K1JT"),
            0.0,
            None,
            false,
            200.0,
            3000.0,
        )
    }
    #[test]
    fn rr73_neither_conflicts_with_nor_overwrites_a_user_grid() {
        let mut s = store();
        s.apply_user_config(Some("K1JT"), Some("PM00"), 0.0, false, 200.0, 3000.0);
        let ts = SlotTimestamp::parse("140600").unwrap();
        let end = row(1000.0, "K1JT BG5ATV RR73");
        assert!(s.should_emit_target_row(&end));
        s.harvest_listen(&ts, &[end, row(1000.0, "CQ BG5ATV PM00")]);
        assert_eq!(s.grid_source(), HisgridSource::User);
        assert_eq!(s.hisgrid(), Some("PM00"));
        let conflict = row(2200.0, "CQ BG5ATV FN42");
        assert!(!s.should_emit_target_row(&conflict));
        s.harvest_listen(&ts, &[conflict]);
        assert_eq!(s.hisgrid(), Some("PM00"));
        assert_eq!(s.selected_foci(), vec![1000.0]);
    }
    #[test]
    fn reconfiguration_replaces_and_clears_user_values() {
        let mut s = store();
        s.apply_user_config(Some("K1JT"), Some("PM00AA"), 1000.0, false, 200.0, 3000.0);
        assert_eq!(s.hisgrid(), Some("PM00"));
        s.apply_user_config(Some("K1JT"), Some("FN42"), 1200.0, false, 200.0, 3000.0);
        assert_eq!(s.hisgrid(), Some("FN42"));
        assert_eq!(s.selected_foci(), vec![1200.0]);
        s.apply_user_config(Some("K1JT"), None, 0.0, false, 200.0, 3000.0);
        assert!(s.hisgrid().is_none());
        assert!(s.selected_foci().is_empty());
    }
    #[test]
    fn observed_parity_can_change_and_expire() {
        let mut s = store();
        s.harvest_listen(
            &SlotTimestamp::parse("140600").unwrap(),
            &[row(1000.0, "CQ BG5ATV PM00")],
        );
        assert!(!s.should_run_focused(&SlotTimestamp::parse("140615").unwrap()));
        s.harvest_listen(
            &SlotTimestamp::parse("140615").unwrap(),
            &[row(1000.0, "CQ BG5ATV PM00")],
        );
        assert!(s.should_run_focused(&SlotTimestamp::parse("140630").unwrap()));
        s.harvest_listen(
            &SlotTimestamp::parse("140645").unwrap(),
            &[row(1000.0, "CQ BG5ATV PM00")],
        );
        assert!(!s.should_run_focused(&SlotTimestamp::parse("140700").unwrap()));
        s.begin_slot(&SlotTimestamp::parse("141200").unwrap());
        assert!(s.should_run_focused(&SlotTimestamp::parse("141200").unwrap()));
    }
    #[test]
    fn assisted_rows_cannot_confirm_grid_or_parity() {
        let mut s = store();
        s.harvest_focused(
            &SlotTimestamp::parse("140600").unwrap(),
            &[row(1000.0, "CQ BG5ATV PM00")],
        );
        assert!(s.hisgrid().is_none());
        assert!(s.tx_parity.is_none());
        assert_eq!(s.selected_foci(), vec![1000.0]);
    }
    #[test]
    fn frequency_tracks_equal_confidence_and_ages_across_days() {
        let mut s = store();
        s.harvest_listen(
            &SlotTimestamp::parse("230208_140600").unwrap(),
            &[row(1000.0, "CQ BG5ATV PM00")],
        );
        s.harvest_listen(
            &SlotTimestamp::parse("230208_140630").unwrap(),
            &[row(1020.0, "CQ BG5ATV PM00")],
        );
        assert_eq!(s.selected_foci(), vec![1020.0]);
        s.begin_slot(&SlotTimestamp::parse("230209_140630").unwrap());
        assert!(s.selected_foci().is_empty());
        assert!(s.tx_parity.is_none());
    }
    #[test]
    fn prefixed_file_midnight_wrap_preserves_recent_frequency() {
        let mut s = store();
        let ts = SlotTimestamp::parse("230208_235945").unwrap();
        s.harvest_listen(&ts, &[row(1000.0, "CQ BG5ATV PM00")]);
        s.begin_slot(&ts.add_seconds(15));
        assert_eq!(s.selected_foci(), vec![1000.0]);
    }
    #[test]
    fn pinned_and_nonconcurrent_frequencies_do_not_prove_fox_grid() {
        let mut s = store();
        s.apply_user_config(Some("K1JT"), None, 300.0, true, 200.0, 3000.0);
        s.harvest_listen(
            &SlotTimestamp::parse("140600").unwrap(),
            &[row(600.0, "K1JT BG5ATV -10")],
        );
        assert_eq!(s.selected_foci(), vec![300.0, 600.0]);
        s.harvest_listen(
            &SlotTimestamp::parse("140630").unwrap(),
            &[row(660.0, "K1JT BG5ATV -10")],
        );
        assert_eq!(s.selected_foci().len(), 3);
        assert!(s.fox_multistream_foci().is_none());
    }

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
        store.harvest_listen(&ts, &[row(360.0, "W9XYZ DX1AAA -12")]);
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
        store.harvest_listen(&ts, &[row(420.0, "W9XYZ DX1AAA -12")]);
        assert_eq!(
            store.selected_foci(),
            vec![360.0, 420.0, 480.0, 540.0, 600.0]
        );

        // The always-on listen later decodes a lower stream we had missed; the
        // anchor drops to it and the whole grid shifts down (dynamic fallback).
        store.harvest_listen(&ts, &[row(300.0, "N0CALL DX1AAA -08")]);
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
        store.harvest_listen(&ts, &[row(960.0, "W9XYZ DX1AAA -12")]);

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
        store.harvest_listen(&ts, &[row(360.0, "W9XYZ DX1AAA -12")]);
        assert_eq!(store.selected_foci(), vec![300.0, 360.0]);
    }
}
