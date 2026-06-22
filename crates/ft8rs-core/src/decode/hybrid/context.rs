use super::{DecodeConfidence, SharedDecode, SharedEvidenceStore};

#[derive(Clone, Debug, PartialEq)]
pub struct QsoContextHint {
    pub hiscall: String,
    pub nfqso: f64,
    pub source_message: String,
    pub score: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QsoContextOpportunityReport {
    pub slots_with_hints: usize,
    pub total_hints: usize,
    pub max_hints_in_slot: usize,
}

impl QsoContextOpportunityReport {
    pub fn observe_slot(&mut self, hints: &[QsoContextHint]) {
        if hints.is_empty() {
            return;
        }
        self.slots_with_hints += 1;
        self.total_hints += hints.len();
        self.max_hints_in_slot = self.max_hints_in_slot.max(hints.len());
    }
}

#[derive(Clone, Debug)]
pub struct ActiveCallContext {
    max_hints_per_slot: usize,
    hints: Vec<QsoContextHint>,
}

impl ActiveCallContext {
    pub fn new(max_hints_per_slot: usize) -> Self {
        Self {
            max_hints_per_slot,
            hints: Vec::new(),
        }
    }

    pub fn update_from_evidence(&mut self, evidence: &SharedEvidenceStore) {
        for row in evidence.rows() {
            if let Some(hint) = qso_hint_from_shared_decode(row) {
                self.upsert_hint(hint);
            }
        }
        self.hints.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.hiscall.cmp(&b.hiscall))
                .then_with(|| a.nfqso.total_cmp(&b.nfqso))
        });
        self.hints.truncate(self.max_hints_per_slot);
    }

    pub fn hints(&self) -> &[QsoContextHint] {
        &self.hints
    }

    fn upsert_hint(&mut self, hint: QsoContextHint) {
        if let Some(existing) = self
            .hints
            .iter_mut()
            .find(|existing| existing.hiscall == hint.hiscall)
        {
            if hint.score > existing.score {
                *existing = hint;
            }
            return;
        }
        self.hints.push(hint);
    }
}

fn qso_hint_from_shared_decode(row: &SharedDecode) -> Option<QsoContextHint> {
    if !row.import_eligible
        || !matches!(
            row.confidence,
            DecodeConfidence::ConfirmedRegular | DecodeConfidence::ConfirmedMulti
        )
        || row.message.contains("<...")
    {
        return None;
    }

    let words: Vec<&str> = row.message.split_whitespace().collect();
    let (call, score_bonus) = if words.len() >= 3 && words[0].eq_ignore_ascii_case("CQ") {
        (words[1], 5)
    } else if words.len() >= 2 {
        (words[1], 0)
    } else {
        return None;
    };

    if !is_context_call(call) {
        return None;
    }

    Some(QsoContextHint {
        hiscall: call
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_ascii_uppercase(),
        nfqso: row.freq_hz,
        source_message: row.message.clone(),
        score: row.snr_db + score_bonus,
    })
}

fn is_context_call(call: &str) -> bool {
    let call = call.trim().trim_start_matches('<').trim_end_matches('>');
    call.len() >= 3
        && !matches!(
            call.to_ascii_uppercase().as_str(),
            "CQ" | "DE" | "QRZ" | "DX" | "RRR" | "RR73" | "73" | "R" | "TU"
        )
        && call.chars().any(|c| c.is_ascii_digit())
        && call.chars().any(|c| c.is_ascii_alphabetic())
        && call
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::hybrid::{
        DecoderId, Provenance, SharedDecodeCandidate, SharedEvidenceStore,
    };

    fn admit(
        store: &mut SharedEvidenceStore,
        msg: &str,
        freq: f64,
        snr: i32,
        provenance: Provenance,
    ) {
        store.admit(SharedDecodeCandidate {
            source: DecoderId::WSJTX,
            provenance,
            message: msg.to_string(),
            freq_hz: freq,
            dt_sec: 0.5,
            snr_db: snr,
        });
    }

    #[test]
    fn active_call_context_keeps_bounded_best_confirmed_regular_hints() {
        let mut store = SharedEvidenceStore::new();
        admit(
            &mut store,
            "CQ K1ABC FN42",
            1000.0,
            -12,
            Provenance::Regular,
        );
        admit(
            &mut store,
            "W9XYZ N0CALL -10",
            1500.0,
            -3,
            Provenance::Regular,
        );
        admit(
            &mut store,
            "K1ABC W1AW RR73",
            900.0,
            -20,
            Provenance::Regular,
        );

        let mut context = ActiveCallContext::new(2);
        context.update_from_evidence(&store);

        assert_eq!(context.hints().len(), 2);
        assert_eq!(context.hints()[0].hiscall, "N0CALL");
        assert_eq!(context.hints()[1].hiscall, "K1ABC");
    }

    #[test]
    fn active_call_context_ignores_assisted_and_unresolved_hash_rows() {
        let mut store = SharedEvidenceStore::new();
        admit(
            &mut store,
            "K1ABC W9XYZ RR73",
            1200.0,
            -10,
            Provenance::A7Memory,
        );
        admit(
            &mut store,
            "<...> US5VAC -13",
            588.0,
            -13,
            Provenance::Regular,
        );

        let mut context = ActiveCallContext::new(4);
        context.update_from_evidence(&store);

        assert!(context.hints().is_empty());
    }

    #[test]
    fn qso_context_opportunity_report_tracks_bounded_hints_per_slot() {
        let hints = vec![
            QsoContextHint {
                hiscall: "K1ABC".to_string(),
                nfqso: 1000.0,
                source_message: "CQ K1ABC FN42".to_string(),
                score: -7,
            },
            QsoContextHint {
                hiscall: "N0CALL".to_string(),
                nfqso: 1500.0,
                source_message: "W9XYZ N0CALL -10".to_string(),
                score: -3,
            },
        ];
        let mut report = QsoContextOpportunityReport::default();
        report.observe_slot(&[]);
        report.observe_slot(&hints);

        assert_eq!(report.slots_with_hints, 1);
        assert_eq!(report.total_hints, 2);
        assert_eq!(report.max_hints_in_slot, 2);
    }
}
