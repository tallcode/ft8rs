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
    normalize_message(&a.msg) == normalize_message(&b.msg)
        && (a.freq - b.freq).abs() <= 5.0
        && (a.dt - b.dt).abs() <= 0.3
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
            normalize_message(&old.decode.msg) == normalize_message(&row.msg)
                && (old.decode.freq - row.freq).abs() <= self.freq_tolerance
                && (old.decode.dt - row.dt).abs() <= 0.3
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
}
