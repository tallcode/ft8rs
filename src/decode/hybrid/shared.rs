use std::collections::{BTreeSet, HashMap, HashSet};

use crate::decode::packjt77::ihashcall;

#[derive(Clone, Debug, Default)]
pub(super) struct SharedHashCallBook {
    calls: BTreeSet<String>,
    ambiguous_calls: BTreeSet<String>,
    conflict_hashes: HashSet<(u8, usize)>,
    hashes: HashMap<(u8, usize), String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashCallOpportunityReport {
    pub unresolved_hash_rows: usize,
    pub rows_resolvable_by_other_decoder: usize,
    pub hash_conflicts: usize,
}

impl HashCallOpportunityReport {
    pub fn merge(&mut self, other: Self) {
        self.unresolved_hash_rows += other.unresolved_hash_rows;
        self.rows_resolvable_by_other_decoder += other.rows_resolvable_by_other_decoder;
        self.hash_conflicts += other.hash_conflicts;
    }
}

impl SharedHashCallBook {
    pub(super) fn import_regular_calls<I, S>(&mut self, calls: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for call in calls {
            self.add_regular_call(call.as_ref());
        }
    }

    pub(super) fn safe_calls(&self) -> Vec<String> {
        self.calls
            .iter()
            .filter(|call| !self.ambiguous_calls.contains(*call))
            .cloned()
            .collect()
    }

    pub(super) fn conflict_count(&self) -> usize {
        self.conflict_hashes.len()
    }

    fn add_regular_call(&mut self, call: &str) {
        let Some(call) = clean_call(call) else {
            return;
        };
        self.calls.insert(call.clone());

        for width in [10u8, 12, 22] {
            let key = (width, ihashcall(&call, usize::from(width)));
            match self.hashes.get(&key) {
                Some(existing) if existing != &call => {
                    self.conflict_hashes.insert(key);
                    self.ambiguous_calls.insert(existing.clone());
                    self.ambiguous_calls.insert(call.clone());
                }
                Some(_) => {}
                None => {
                    self.hashes.insert(key, call.clone());
                }
            }
        }
    }
}

pub fn hash_call_opportunity_report<Row>(
    left_rows: &[Row],
    right_rows: &[Row],
    msg: impl Fn(&Row) -> &str,
    same_signal: impl Fn(&Row, &Row) -> bool,
) -> HashCallOpportunityReport {
    let mut report = HashCallOpportunityReport::default();
    report.merge(one_way_hash_call_opportunity(
        left_rows,
        right_rows,
        &msg,
        &same_signal,
    ));
    report.merge(one_way_hash_call_opportunity(
        right_rows,
        left_rows,
        &msg,
        &same_signal,
    ));
    report
}

fn one_way_hash_call_opportunity<Row>(
    unresolved_side: &[Row],
    resolver_side: &[Row],
    msg: &impl Fn(&Row) -> &str,
    same_signal: &impl Fn(&Row, &Row) -> bool,
) -> HashCallOpportunityReport {
    let mut report = HashCallOpportunityReport::default();
    for row in unresolved_side {
        let words = split_words(msg(row));
        if !words.iter().any(|word| word == "<...>") {
            continue;
        }
        report.unresolved_hash_rows += 1;
        if resolver_side.iter().any(|other| {
            same_signal(row, other)
                && unresolved_can_be_resolved_by(&words, &split_words(msg(other)))
        }) {
            report.rows_resolvable_by_other_decoder += 1;
        }
    }
    report
}

fn unresolved_can_be_resolved_by(unresolved_words: &[String], resolved_words: &[String]) -> bool {
    if unresolved_words.len() != resolved_words.len() {
        return false;
    }
    unresolved_words
        .iter()
        .zip(resolved_words.iter())
        .any(|(a, b)| a == "<...>" && is_full_call_candidate(b))
        && unresolved_words
            .iter()
            .zip(resolved_words.iter())
            .all(|(a, b)| a == b || (a == "<...>" && is_full_call_candidate(b)))
}

fn split_words(msg: &str) -> Vec<String> {
    msg.split_whitespace()
        .map(|word| word.trim_matches(|c: char| c == ';' || c == ','))
        .map(|word| word.to_ascii_uppercase())
        .collect()
}

fn is_full_call_candidate(word: &str) -> bool {
    clean_call(word).is_some()
        && !matches!(
            word,
            "CQ" | "DE" | "QRZ" | "DX" | "RRR" | "RR73" | "73" | "R" | "TU"
        )
}

fn clean_call(call: &str) -> Option<String> {
    let trimmed = call.trim();
    if trimmed.is_empty() || trimmed == "<...>" {
        return None;
    }
    let clean = if trimmed.starts_with('<') && trimmed.ends_with('>') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    if clean.len() < 3 {
        return None;
    }
    Some(clean.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_book_reuses_protocol_hash_for_collision_detection() {
        // The shared book uses the FT8 protocol callsign hash instead of keeping
        // a local copy. Pin known values so collision marking stays aligned with
        // decoder hash-call resolution.
        assert_eq!(
            (
                ihashcall("K1ABC", 10),
                ihashcall("K1ABC", 12),
                ihashcall("K1ABC", 22)
            ),
            (712, 2851, 2920267)
        );
        assert_eq!(
            (
                ihashcall("RK4FF", 10),
                ihashcall("RK4FF", 12),
                ihashcall("RK4FF", 22)
            ),
            (775, 3102, 3177032)
        );
        assert_eq!(
            (
                ihashcall("EA5/DH0YAH", 10),
                ihashcall("EA5/DH0YAH", 12),
                ihashcall("EA5/DH0YAH", 22)
            ),
            (110, 441, 451662)
        );
    }

    #[test]
    fn shared_hash_book_exports_clean_calls_once() {
        let mut book = SharedHashCallBook::default();
        book.import_regular_calls(["k1abc", "<g4abc/p>", "K1ABC"]);

        assert_eq!(
            book.safe_calls(),
            vec!["G4ABC/P".to_string(), "K1ABC".to_string()]
        );
    }

    #[test]
    fn shared_hash_book_suppresses_hash10_collisions() {
        let mut seen: HashMap<usize, String> = HashMap::new();
        let mut collision = None;
        for n in 0..5000 {
            let call = format!("K{n:04}");
            let h = ihashcall(&call, 10);
            if let Some(existing) = seen.insert(h, call.clone()) {
                collision = Some((existing, call));
                break;
            }
        }
        let (a, b) = collision.expect("test generator should find a hash10 collision");

        let mut book = SharedHashCallBook::default();
        book.import_regular_calls([a.as_str(), b.as_str(), "N0CALL"]);

        let safe = book.safe_calls();
        assert!(safe.contains(&"N0CALL".to_string()));
        assert!(!safe.contains(&a));
        assert!(!safe.contains(&b));
    }

    #[test]
    fn hash_opportunity_report_counts_resolvable_same_signal_hash_rows() {
        #[derive(Clone)]
        struct Row {
            freq: f64,
            dt: f64,
            msg: &'static str,
        }
        let left = [Row {
            freq: 588.0,
            dt: 0.5,
            msg: "<...> US5VAC -13",
        }];
        let right = [Row {
            freq: 589.0,
            dt: 0.6,
            msg: "R5AF/O US5VAC -13",
        }];

        let report = hash_call_opportunity_report(
            &left,
            &right,
            |row| row.msg,
            |a, b| (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3,
        );

        assert_eq!(report.unresolved_hash_rows, 1);
        assert_eq!(report.rows_resolvable_by_other_decoder, 1);
    }
}
