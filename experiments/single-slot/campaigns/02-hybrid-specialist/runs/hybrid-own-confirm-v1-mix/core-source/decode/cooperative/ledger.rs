use super::normalize_message;
use crate::stream::session::{
    StreamDecodeProvenance, StreamDecodedMessage, StreamDecodedWithProvenance,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignalSource {
    Wsjtx,
    Jtdx,
    Recovery,
}

pub(crate) fn same_signal(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3 && same_message(a, b)
}

fn same_message(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    let same_text = normalize_message(&a.msg) == normalize_message(&b.msg);
    // Keep the established semantic matching for fully resolved messages,
    // including equivalent native formats and hash-brace display variants.
    if same_text && !a.msg.contains("<...>") {
        return true;
    }
    let complete_a = complete_native_tones(&a.itone);
    let complete_b = complete_native_tones(&b.itone);
    if same_text {
        // Equal placeholders need not contain the same transmitted hash.
        return !complete_a || !complete_b || a.itone == b.itone;
    }
    // Never re-encode <...>: that would discard the actual transmitted hash.
    if a.itone != b.itone || !complete_a || !complete_b {
        return false;
    }
    let a_words: Vec<_> = a.msg.split_whitespace().collect();
    let b_words: Vec<_> = b.msg.split_whitespace().collect();
    let a_unknown = a_words.contains(&"<...>");
    let b_unknown = b_words.contains(&"<...>");
    if a_words.len() != b_words.len() || a_unknown == b_unknown {
        return false;
    }
    let (unknown, resolved) = if a_unknown {
        (&a_words, &b_words)
    } else {
        (&b_words, &a_words)
    };
    unknown.iter().zip(resolved).all(|(left, right)| {
        if *left == "<...>" {
            right.len() > 2 && right.starts_with('<') && right.ends_with('>')
        } else {
            normalize_message(left) == normalize_message(right)
        }
    })
}

fn complete_native_tones(tones: &[i32; 79]) -> bool {
    const SYNC: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
    tones.iter().all(|tone| (0..=7).contains(tone))
        && [0, 36, 72]
            .into_iter()
            .all(|start| tones[start..start + 7] == SYNC)
}

pub(crate) struct OutputLedger {
    rows: Vec<StreamDecodedWithProvenance>,
    sources: Vec<Vec<SignalSource>>,
    freq_tolerance: f64,
}
impl Default for OutputLedger {
    fn default() -> Self {
        Self::with_frequency_tolerance(5.0)
    }
}

impl OutputLedger {
    pub fn with_frequency_tolerance(freq_tolerance: f64) -> Self {
        Self {
            rows: Vec::new(),
            sources: Vec::new(),
            freq_tolerance,
        }
    }
    /// Source agreement and whether a message was emitted are separate facts.
    pub fn admit(
        &mut self,
        source: SignalSource,
        row: &StreamDecodedMessage,
        provenance: StreamDecodeProvenance,
    ) -> bool {
        if let Some(index) = self.rows.iter().position(|old| {
            (old.decode.freq - row.freq).abs() <= self.freq_tolerance
                && (old.decode.dt - row.dt).abs() <= 0.3
                && same_message(&old.decode, row)
        }) {
            if !self.sources[index].contains(&source) {
                self.sources[index].push(source);
            }
            return false;
        }
        self.rows.push(StreamDecodedWithProvenance {
            decode: row.clone(),
            provenance,
        });
        self.sources.push(vec![source]);
        true
    }

    #[cfg(test)]
    pub fn plain(&self) -> Vec<StreamDecodedMessage> {
        self.rows.iter().map(|r| r.decode.clone()).collect()
    }
    pub fn tagged(&self) -> &[StreamDecodedWithProvenance] {
        &self.rows
    }

    pub fn into_sorted(mut self) -> Vec<StreamDecodedMessage> {
        self.rows.sort_by(|a, b| {
            normalize_message(&a.decode.msg)
                .cmp(&normalize_message(&b.decode.msg))
                .then_with(|| a.decode.freq.total_cmp(&b.decode.freq))
                .then_with(|| a.decode.dt.total_cmp(&b.decode.dt))
        });
        self.rows.into_iter().map(|r| r.decode).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(freq: f64, msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq,
            dt: 0.2,
            snr: -10.0,
            msg: msg.into(),
            sync: 2.0,
            itone: [0; 79],
        }
    }
    #[test]
    fn same_engine_duplicates_do_not_suppress_first_output() {
        let mut ledger = OutputLedger::default();
        let row = row(1000.0, "K1JT BG5ATV RR73");
        assert!(ledger.admit(SignalSource::Jtdx, &row, StreamDecodeProvenance::Regular));
        assert!(!ledger.admit(SignalSource::Jtdx, &row, StreamDecodeProvenance::Regular));
        assert_eq!(ledger.sources[0], [SignalSource::Jtdx]);
        assert!(!ledger.admit(SignalSource::Wsjtx, &row, StreamDecodeProvenance::Unknown));
        assert_eq!(ledger.plain().len(), 1);
    }
    #[test]
    fn retain_distinct_signals_and_unresolved_identity() {
        let mut ledger = OutputLedger::default();
        for r in [
            row(1000.0, "K1JT <...> -10"),
            row(1000.0, "K1JT BG5ATV -10"),
            row(1100.0, "K1JT BG5ATV -10"),
        ] {
            assert!(ledger.admit(SignalSource::Jtdx, &r, StreamDecodeProvenance::Regular));
        }
        assert_eq!(ledger.plain().len(), 3);
    }

    #[test]
    fn identical_native_payload_deduplicates_only_hash_display_expansion() {
        let mut resolved = row(1205.0, "EA5/DH0YAH <RK4FF> RR73");
        resolved.itone = crate::decode::genft8::get_ft8_tones_from_77bits(
            &crate::decode::packjt77::pack77(&resolved.msg),
        );
        let mut unresolved = resolved.clone();
        unresolved.msg = "EA5/DH0YAH <...> RR73".into();
        assert!(same_signal(&resolved, &unresolved));
        let mut ledger = OutputLedger::default();
        assert!(ledger.admit(
            SignalSource::Wsjtx,
            &resolved,
            StreamDecodeProvenance::Unknown
        ));
        assert!(!ledger.admit(
            SignalSource::Recovery,
            &unresolved,
            StreamDecodeProvenance::CooperativeRecovery
        ));
        assert_eq!(ledger.plain()[0].msg, resolved.msg);

        let mut collision = resolved.clone();
        collision.msg = "EA5/DH0YAH <K1JT> RR73".into();
        assert!(!same_signal(&resolved, &collision));
        let mut native_format_variant = resolved.clone();
        native_format_variant.msg = "EA5/DH0YAH RK4FF RR73".into();
        native_format_variant.itone[7] = (native_format_variant.itone[7] + 1) % 8;
        assert!(same_signal(&resolved, &native_format_variant));
        let mut changed = unresolved.clone();
        changed.itone[7] = (changed.itone[7] + 1) % 8;
        assert!(!same_signal(&resolved, &changed));
        let mut same_display = changed.clone();
        same_display.msg = unresolved.msg.clone();
        assert!(!same_signal(&unresolved, &same_display));
        changed = unresolved.clone();
        changed.itone = [0; 79];
        assert!(!same_signal(&resolved, &changed));
        changed = unresolved.clone();
        changed.freq += 6.0;
        assert!(!same_signal(&resolved, &changed));
        changed = unresolved;
        changed.msg = "EA5/DH0YAH <...> 73".into();
        assert!(!same_signal(&resolved, &changed));
    }
}
