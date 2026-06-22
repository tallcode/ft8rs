#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecoderId(pub &'static str);

impl DecoderId {
    pub const WSJTX: Self = Self("wsjtx");
    pub const JTDX: Self = Self("jtdx");

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Provenance {
    Regular,
    ApMask,
    A7Memory,
    A8List,
    JtdxDeep,
    ImportedMemory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DecodeConfidence {
    ConfirmedMulti,
    ConfirmedRegular,
    ConfirmedAp,
    Assisted,
    Speculative,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodeEvidence {
    pub source: DecoderId,
    pub provenance: Provenance,
    pub message: String,
    pub snr_db: i32,
    pub freq_hz: f64,
    pub dt_sec: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SharedDecode {
    pub message: String,
    pub normalized_message: String,
    pub freq_hz: f64,
    pub dt_sec: f64,
    pub snr_db: i32,
    pub sources: Vec<DecoderId>,
    pub confidence: DecodeConfidence,
    pub evidence: Vec<DecodeEvidence>,
    pub import_eligible: bool,
}

#[derive(Clone, Debug)]
pub struct SharedDecodeCandidate {
    pub source: DecoderId,
    pub provenance: Provenance,
    pub message: String,
    pub freq_hz: f64,
    pub dt_sec: f64,
    pub snr_db: i32,
}

#[derive(Clone, Default, Debug)]
pub struct SharedEvidenceStore {
    rows: Vec<SharedDecode>,
}

impl SharedEvidenceStore {
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    pub fn admit(&mut self, candidate: SharedDecodeCandidate) {
        let normalized_message = normalize_message(&candidate.message);
        let evidence = DecodeEvidence {
            source: candidate.source,
            provenance: candidate.provenance,
            message: candidate.message.clone(),
            snr_db: candidate.snr_db,
            freq_hz: candidate.freq_hz,
            dt_sec: candidate.dt_sec,
        };
        if let Some(existing) = self.rows.iter_mut().find(|row| {
            row.normalized_message == normalized_message
                && (row.freq_hz - candidate.freq_hz).abs() <= 5.0
                && (row.dt_sec - candidate.dt_sec).abs() <= 0.3
        }) {
            if !existing.sources.contains(&candidate.source) {
                existing.sources.push(candidate.source);
                existing.sources.sort();
            }
            existing.evidence.push(evidence);
            existing.confidence = confidence_for(&existing.evidence);
            existing.import_eligible = import_eligible(existing.confidence);
            return;
        }

        let evidence_vec = vec![evidence];
        let confidence = confidence_for(&evidence_vec);
        self.rows.push(SharedDecode {
            message: candidate.message,
            normalized_message,
            freq_hz: candidate.freq_hz,
            dt_sec: candidate.dt_sec,
            snr_db: candidate.snr_db,
            sources: vec![candidate.source],
            confidence,
            import_eligible: import_eligible(confidence),
            evidence: evidence_vec,
        });
    }

    pub fn rows(&self) -> &[SharedDecode] {
        &self.rows
    }
}

fn confidence_for(evidence: &[DecodeEvidence]) -> DecodeConfidence {
    // ImportedMemory is derivative (the row only decoded because of a hint), so
    // it never contributes to the quorum or the tier. Rate on the remaining
    // independent evidence; if nothing independent remains, the row is Assisted.
    let independent: Vec<&DecodeEvidence> = evidence
        .iter()
        .filter(|e| e.provenance != Provenance::ImportedMemory)
        .collect();
    if independent.is_empty() {
        return DecodeConfidence::Assisted;
    }

    let mut sources: Vec<DecoderId> = independent.iter().map(|e| e.source).collect();
    sources.sort();
    sources.dedup();

    let has_regular_or_ap_mask = independent
        .iter()
        .any(|e| matches!(e.provenance, Provenance::Regular | Provenance::ApMask));
    if sources.len() >= 2 && has_regular_or_ap_mask {
        return DecodeConfidence::ConfirmedMulti;
    }

    if independent.len() == 1 {
        return match independent[0].provenance {
            Provenance::Regular => DecodeConfidence::ConfirmedRegular,
            Provenance::ApMask => DecodeConfidence::ConfirmedAp,
            _ => DecodeConfidence::Assisted,
        };
    }

    DecodeConfidence::Assisted
}

fn import_eligible(confidence: DecodeConfidence) -> bool {
    // The tier already excludes pure-ImportedMemory rows (they classify as
    // Assisted), so eligibility keys off the tier alone.
    matches!(
        confidence,
        DecodeConfidence::ConfirmedMulti | DecodeConfidence::ConfirmedRegular
    )
}

fn normalize_message(msg: &str) -> String {
    msg.split_whitespace()
        .map(normalize_message_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_message_word(word: &str) -> String {
    let word = word.to_ascii_uppercase();
    if word.starts_with('<') && word.ends_with('>') {
        let inner = &word[1..word.len() - 1];
        if !inner.is_empty() && inner != "..." {
            return inner.to_string();
        }
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(source: DecoderId, provenance: Provenance, msg: &str) -> SharedDecodeCandidate {
        SharedDecodeCandidate {
            source,
            provenance,
            message: msg.to_string(),
            freq_hz: 1205.0,
            dt_sec: 0.6,
            snr_db: -10,
        }
    }

    #[test]
    fn evidence_store_promotes_independent_regular_agreement() {
        let mut store = SharedEvidenceStore::new();
        store.admit(candidate(
            DecoderId::WSJTX,
            Provenance::Regular,
            "EA5/DH0YAH <RK4FF> RR73",
        ));
        store.admit(candidate(
            DecoderId::JTDX,
            Provenance::Regular,
            "EA5/DH0YAH RK4FF RR73",
        ));

        assert_eq!(store.rows().len(), 1);
        assert_eq!(store.rows()[0].confidence, DecodeConfidence::ConfirmedMulti);
        assert!(store.rows()[0].import_eligible);
    }

    #[test]
    fn evidence_store_caps_all_assisted_agreement() {
        let mut store = SharedEvidenceStore::new();
        store.admit(candidate(
            DecoderId::WSJTX,
            Provenance::A7Memory,
            "K1ABC W9XYZ RR73",
        ));
        store.admit(candidate(
            DecoderId::JTDX,
            Provenance::JtdxDeep,
            "K1ABC W9XYZ RR73",
        ));

        assert_eq!(store.rows()[0].confidence, DecodeConfidence::Assisted);
        assert!(!store.rows()[0].import_eligible);
    }

    #[test]
    fn evidence_store_keeps_pure_imported_memory_terminal() {
        // A row whose only evidence is an imported decode never becomes a seed.
        let mut store = SharedEvidenceStore::new();
        store.admit(candidate(
            DecoderId::WSJTX,
            Provenance::ImportedMemory,
            "K1ABC W9XYZ RR73",
        ));

        assert_eq!(store.rows()[0].confidence, DecodeConfidence::Assisted);
        assert!(!store.rows()[0].import_eligible);
    }

    #[test]
    fn evidence_store_rates_on_independent_evidence_ignoring_imported() {
        // ImportedMemory does not contribute to quorum, but a parallel
        // independent Regular decode still stands on its own.
        let mut store = SharedEvidenceStore::new();
        store.admit(candidate(
            DecoderId::WSJTX,
            Provenance::ImportedMemory,
            "K1ABC W9XYZ RR73",
        ));
        store.admit(candidate(
            DecoderId::JTDX,
            Provenance::Regular,
            "K1ABC W9XYZ RR73",
        ));

        assert_eq!(
            store.rows()[0].confidence,
            DecodeConfidence::ConfirmedRegular
        );
        assert!(store.rows()[0].import_eligible);
    }
}
