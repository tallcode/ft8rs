use std::collections::BTreeMap;

use super::{DecoderId, Provenance, SharedDecode, SharedEvidenceStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MessageClass {
    Cq,
    Grid,
    Report,
    RReport,
    Rrr,
    Rr73,
    SeventyThree,
    Hash,
    FreeTextOrOther,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SnrBucket {
    VeryWeak,
    Weak,
    Mid,
    Strong,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HybridDivergenceReport {
    pub total_rows: usize,
    pub shared_rows: usize,
    pub wsjtx_unique_rows: usize,
    pub jtdx_unique_rows: usize,
    pub representation_only_diffs: usize,
    pub unique_by_decoder: BTreeMap<DecoderId, usize>,
    pub unique_by_provenance: BTreeMap<Provenance, usize>,
    pub unique_by_message_class: BTreeMap<MessageClass, usize>,
    pub unique_by_snr_bucket: BTreeMap<SnrBucket, usize>,
}

impl HybridDivergenceReport {
    pub fn merge(&mut self, other: Self) {
        self.total_rows += other.total_rows;
        self.shared_rows += other.shared_rows;
        self.wsjtx_unique_rows += other.wsjtx_unique_rows;
        self.jtdx_unique_rows += other.jtdx_unique_rows;
        self.representation_only_diffs += other.representation_only_diffs;
        merge_counts(&mut self.unique_by_decoder, other.unique_by_decoder);
        merge_counts(&mut self.unique_by_provenance, other.unique_by_provenance);
        merge_counts(
            &mut self.unique_by_message_class,
            other.unique_by_message_class,
        );
        merge_counts(&mut self.unique_by_snr_bucket, other.unique_by_snr_bucket);
    }
}

pub fn divergence_report_from_evidence(store: &SharedEvidenceStore) -> HybridDivergenceReport {
    let mut report = HybridDivergenceReport {
        total_rows: store.rows().len(),
        ..Default::default()
    };

    for row in store.rows() {
        if row.sources.len() >= 2 {
            report.shared_rows += 1;
            if has_representation_only_diff(row) {
                report.representation_only_diffs += 1;
            }
            continue;
        }

        let Some(source) = row.sources.first().copied() else {
            continue;
        };
        if source == DecoderId::WSJTX {
            report.wsjtx_unique_rows += 1;
        } else if source == DecoderId::JTDX {
            report.jtdx_unique_rows += 1;
        }
        increment(&mut report.unique_by_decoder, source);
        increment(
            &mut report.unique_by_message_class,
            classify_message(&row.message),
        );
        increment(&mut report.unique_by_snr_bucket, snr_bucket(row.snr_db));
        for evidence in &row.evidence {
            increment(&mut report.unique_by_provenance, evidence.provenance);
        }
    }

    report
}

fn has_representation_only_diff(row: &SharedDecode) -> bool {
    let mut messages: Vec<String> = row
        .evidence
        .iter()
        .map(|evidence| {
            evidence
                .message
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    messages.sort();
    messages.dedup();
    row.evidence.len() >= 2 && messages.len() >= 2
}

fn classify_message(message: &str) -> MessageClass {
    let words: Vec<&str> = message.split_whitespace().collect();
    if words.iter().any(|word| word.contains("<...>")) {
        return MessageClass::Hash;
    }
    if words
        .first()
        .is_some_and(|word| word.eq_ignore_ascii_case("CQ"))
    {
        return MessageClass::Cq;
    }
    let Some(last) = words.last().copied() else {
        return MessageClass::FreeTextOrOther;
    };
    let last_upper = last.to_ascii_uppercase();
    if last_upper == "RR73" {
        MessageClass::Rr73
    } else if last_upper == "RRR" {
        MessageClass::Rrr
    } else if last_upper == "73" {
        MessageClass::SeventyThree
    } else if is_r_report(&last_upper) {
        MessageClass::RReport
    } else if is_report(&last_upper) {
        MessageClass::Report
    } else if is_grid4(&last_upper) {
        MessageClass::Grid
    } else {
        MessageClass::FreeTextOrOther
    }
}

fn snr_bucket(snr_db: i32) -> SnrBucket {
    if snr_db < -20 {
        SnrBucket::VeryWeak
    } else if snr_db < -15 {
        SnrBucket::Weak
    } else if snr_db < 0 {
        SnrBucket::Mid
    } else {
        SnrBucket::Strong
    }
}

fn is_r_report(value: &str) -> bool {
    value.len() == 4
        && value.starts_with('R')
        && matches!(value.as_bytes()[1], b'+' | b'-')
        && value.as_bytes()[2].is_ascii_digit()
        && value.as_bytes()[3].is_ascii_digit()
}

fn is_report(value: &str) -> bool {
    value.len() == 3
        && matches!(value.as_bytes()[0], b'+' | b'-')
        && value.as_bytes()[1].is_ascii_digit()
        && value.as_bytes()[2].is_ascii_digit()
}

fn is_grid4(value: &str) -> bool {
    value.len() == 4
        && value.as_bytes()[0].is_ascii_uppercase()
        && value.as_bytes()[1].is_ascii_uppercase()
        && value.as_bytes()[2].is_ascii_digit()
        && value.as_bytes()[3].is_ascii_digit()
}

fn increment<K: Ord>(map: &mut BTreeMap<K, usize>, key: K) {
    *map.entry(key).or_insert(0) += 1;
}

fn merge_counts<K: Ord>(dst: &mut BTreeMap<K, usize>, src: BTreeMap<K, usize>) {
    for (key, value) in src {
        *dst.entry(key).or_insert(0) += value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::hybrid::{SharedDecodeCandidate, SharedEvidenceStore};

    fn admit(
        store: &mut SharedEvidenceStore,
        source: DecoderId,
        provenance: Provenance,
        msg: &str,
        freq: f64,
        snr: i32,
    ) {
        store.admit(SharedDecodeCandidate {
            source,
            provenance,
            message: msg.to_string(),
            freq_hz: freq,
            dt_sec: 0.5,
            snr_db: snr,
        });
    }

    #[test]
    fn divergence_report_counts_unique_rows_by_decoder_and_shape() {
        let mut store = SharedEvidenceStore::new();
        admit(
            &mut store,
            DecoderId::WSJTX,
            Provenance::Regular,
            "CQ K1ABC FN42",
            1000.0,
            -12,
        );
        admit(
            &mut store,
            DecoderId::JTDX,
            Provenance::JtdxDeep,
            "W9XYZ N0CALL -20",
            1500.0,
            -21,
        );

        let report = divergence_report_from_evidence(&store);

        assert_eq!(report.total_rows, 2);
        assert_eq!(report.wsjtx_unique_rows, 1);
        assert_eq!(report.jtdx_unique_rows, 1);
        assert_eq!(report.unique_by_message_class[&MessageClass::Cq], 1);
        assert_eq!(report.unique_by_message_class[&MessageClass::Report], 1);
        assert_eq!(report.unique_by_snr_bucket[&SnrBucket::VeryWeak], 1);
    }

    #[test]
    fn divergence_report_keeps_shared_rows_out_of_unique_buckets() {
        let mut store = SharedEvidenceStore::new();
        admit(
            &mut store,
            DecoderId::WSJTX,
            Provenance::Regular,
            "EA5/DH0YAH <RK4FF> RR73",
            1205.0,
            -4,
        );
        admit(
            &mut store,
            DecoderId::JTDX,
            Provenance::Regular,
            "EA5/DH0YAH RK4FF RR73",
            1206.0,
            -4,
        );

        let report = divergence_report_from_evidence(&store);

        assert_eq!(report.total_rows, 1);
        assert_eq!(report.shared_rows, 1);
        assert_eq!(report.wsjtx_unique_rows, 0);
        assert_eq!(report.jtdx_unique_rows, 0);
    }
}
