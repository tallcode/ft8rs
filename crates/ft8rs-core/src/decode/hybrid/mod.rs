//! Hybrid decode runner.
//!
//! Hybrid keeps decoder-private state inside each profile. Cross-decoder
//! knowledge lives in this orchestration layer and is fed back only through
//! session-level adapter methods.

use std::collections::HashMap;
use std::thread;

use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{
    StreamDecodeConfig, StreamDecodeProvenance, StreamDecodeSession, StreamDecodedMessage,
    StreamDecodedWithProvenance, StreamSlotDecodeState,
};
use crate::stream::time::SlotTimestamp;

mod context;
mod evidence;
mod report;
mod shared;
pub use context::{ActiveCallContext, QsoContextHint, QsoContextOpportunityReport};
pub use evidence::{
    DecodeConfidence, DecodeEvidence, DecoderId, Provenance, SharedDecode, SharedDecodeCandidate,
    SharedEvidenceStore,
};
pub use report::{
    divergence_report_from_evidence, HybridDivergenceReport, MessageClass, SnrBucket,
};
use shared::SharedHashCallBook;
pub use shared::{hash_call_opportunity_report, HashCallOpportunityReport};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeSource {
    Wsjtx,
    Jtdx,
    Both,
}

#[derive(Clone, Debug)]
pub struct HybridDecodedMessage {
    pub decode: StreamDecodedMessage,
    pub source: DecodeSource,
}

pub struct HybridStreamDecodeSession {
    wsjtx: StreamDecodeSession,
    jtdx: JtdxStreamDecodeSession,
    shared_hash_book: SharedHashCallBook,
    last_evidence_store: SharedEvidenceStore,
    active_call_context: ActiveCallContext,
}

impl HybridStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        Self {
            wsjtx: StreamDecodeSession::new(config.clone_for_profile_wsjt_x()),
            jtdx: JtdxStreamDecodeSession::new(config.clone_for_profile_jtdx()),
            shared_hash_book: SharedHashCallBook::default(),
            last_evidence_store: SharedEvidenceStore::new(),
            active_call_context: ActiveCallContext::new(4),
        }
    }

    pub fn last_evidence_store(&self) -> &SharedEvidenceStore {
        &self.last_evidence_store
    }

    pub fn qso_context_hints(&self) -> &[QsoContextHint] {
        self.active_call_context.hints()
    }

    /// One-shot slot decode (file mode / tests): run both workers on the full
    /// slot audio, streaming WSJT-X rows live and JTDX-unique rows after the
    /// merge. Monitor mode uses the staged `start_slot`/`decode_slot_nzhsym41`/
    /// `subtract_slot_nzhsym47`/`decode_slot_nzhsym50` path instead, which emits
    /// the same row set (so file-mode output is unchanged) but streams WSJT-X
    /// early decodes before the slot boundary.
    pub fn decode_slot_streaming_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let shared_hash_calls = self.shared_hash_book.safe_calls();
        self.wsjtx.import_hash_calls(&shared_hash_calls);
        self.jtdx.import_hash_calls(&shared_hash_calls);

        let (wsjtx_tagged, jtdx_tagged) = thread::scope(|scope| {
            let jtdx_session = &mut self.jtdx;
            let jtdx_handle = scope.spawn(|| {
                jtdx_session.decode_slot_streaming_with_provenance_at(
                    timestamp,
                    samples,
                    |_| Ok(()),
                )
            });

            let wsjtx_tagged = self.wsjtx.decode_slot_streaming_with_provenance_at(
                timestamp,
                samples,
                &mut on_decode,
            )?;

            let jtdx_tagged = jtdx_handle
                .join()
                .map_err(|_| "hybrid JTDX worker panicked".to_string())??;

            Ok::<_, String>((wsjtx_tagged, jtdx_tagged))
        })?;

        self.finish_slot(&wsjtx_tagged, &jtdx_tagged, on_decode)
    }

    /// Begin a staged (monitor) slot: inject shared knowledge at the boundary
    /// before decode, and return the WSJT-X early-decode state.
    pub fn start_slot(&mut self) -> StreamSlotDecodeState {
        let shared_hash_calls = self.shared_hash_book.safe_calls();
        self.wsjtx.import_hash_calls(&shared_hash_calls);
        self.jtdx.import_hash_calls(&shared_hash_calls);
        self.wsjtx.start_slot_decode()
    }

    /// `nzhsym=41` early decode: stream the WSJT-X early rows immediately (before
    /// the slot boundary) and return how many were emitted, so `nzhsym=50` can
    /// skip re-emitting them. JTDX has no early sub-results, so it does not run
    /// here.
    pub fn decode_slot_nzhsym41<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let early =
            self.wsjtx
                .decode_slot_nzhsym41_with_provenance_at(Some(timestamp), state, samples)?;
        for row in &early {
            on_decode(&row.decode)?;
        }
        Ok(early.len())
    }

    /// `nzhsym=47`: subtract the selected WSJT-X early decodes from the cleaned
    /// prefix (WSJT-X staging only).
    pub fn subtract_slot_nzhsym47(&mut self, state: &mut StreamSlotDecodeState, samples: &[f32]) {
        self.wsjtx.subtract_slot_nzhsym47(state, samples);
    }

    /// `nzhsym=50` final decode: finish the WSJT-X pass (streaming only the rows
    /// not already emitted at `nzhsym=41`, per `early_count`) concurrently with
    /// the JTDX pass, then merge and stream JTDX-unique rows. Same row set as the
    /// one-shot path.
    pub fn decode_slot_nzhsym50<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        state: StreamSlotDecodeState,
        early_count: usize,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let (wsjtx_tagged, jtdx_tagged) = thread::scope(|scope| {
            let jtdx_session = &mut self.jtdx;
            let jtdx_handle = scope.spawn(|| {
                jtdx_session.decode_slot_streaming_with_provenance_at(
                    timestamp,
                    samples,
                    |_| Ok(()),
                )
            });

            // WSJT-X final decode returns ALL rows (including the early ones
            // already streamed at nzhsym=41); emit only the newly-added ones.
            let wsjtx_tagged = self
                .wsjtx
                .decode_slot_nzhsym50_and_finish_with_provenance(state, samples)?;
            for row in wsjtx_tagged.iter().skip(early_count) {
                on_decode(&row.decode)?;
            }

            let jtdx_tagged = jtdx_handle
                .join()
                .map_err(|_| "hybrid JTDX worker panicked".to_string())??;

            Ok::<_, String>((wsjtx_tagged, jtdx_tagged))
        })?;

        self.finish_slot(&wsjtx_tagged, &jtdx_tagged, on_decode)
    }

    /// Shared slot tail: update shared hash book / evidence / active context from
    /// the two workers' tagged rows, then merge and stream JTDX-unique rows.
    /// `on_decode` has already received every WSJT-X row by the time this runs.
    fn finish_slot<F>(
        &mut self,
        wsjtx_tagged: &[StreamDecodedWithProvenance],
        jtdx_tagged: &[StreamDecodedWithProvenance],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let wsjtx_results: Vec<StreamDecodedMessage> =
            wsjtx_tagged.iter().map(|row| row.decode.clone()).collect();
        let jtdx: Vec<StreamDecodedMessage> =
            jtdx_tagged.iter().map(|row| row.decode.clone()).collect();

        self.shared_hash_book
            .import_regular_calls(self.wsjtx.export_regular_hash_calls());
        self.shared_hash_book
            .import_regular_calls(self.jtdx.export_regular_hash_calls());

        self.last_evidence_store =
            build_passive_evidence_store_from_tagged(wsjtx_tagged, jtdx_tagged);
        self.active_call_context
            .update_from_evidence(&self.last_evidence_store);

        let merged_with_source = merge_decodes_with_source(&wsjtx_results, &jtdx);
        for row in &merged_with_source {
            if row.source == DecodeSource::Jtdx {
                on_decode(&row.decode)?;
            }
        }
        Ok(merged_with_source
            .into_iter()
            .map(|row| row.decode)
            .collect())
    }
}

pub fn merge_decodes(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> Vec<StreamDecodedMessage> {
    merge_decodes_with_source(wsjtx, jtdx)
        .into_iter()
        .map(|row| row.decode)
        .collect()
}

pub fn hybrid_hash_call_opportunity_report(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> HashCallOpportunityReport {
    let mut report =
        hash_call_opportunity_report(wsjtx, jtdx, |row| row.msg.as_str(), is_same_signal);
    let mut book = SharedHashCallBook::default();
    book.import_regular_calls(extract_full_call_candidates(wsjtx));
    book.import_regular_calls(extract_full_call_candidates(jtdx));
    report.hash_conflicts = book.conflict_count();
    report
}

/// Test-only convenience that treats every row as `Regular` provenance.
///
/// Real callers must use [`build_passive_evidence_store_from_tagged`] with true
/// provenance: assuming `Regular` would let assisted/AP/deep rows masquerade as
/// `ConfirmedRegular` and become `import_eligible`, defeating the confidence
/// gate. Gated to `cfg(test)` so it can never be reached from a production path.
#[cfg(test)]
fn build_passive_evidence_store(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> SharedEvidenceStore {
    let wsjtx_tagged: Vec<StreamDecodedWithProvenance> = wsjtx
        .iter()
        .cloned()
        .map(|decode| StreamDecodedWithProvenance {
            decode,
            provenance: StreamDecodeProvenance::Regular,
        })
        .collect();
    let jtdx_tagged: Vec<StreamDecodedWithProvenance> = jtdx
        .iter()
        .cloned()
        .map(|decode| StreamDecodedWithProvenance {
            decode,
            provenance: StreamDecodeProvenance::Regular,
        })
        .collect();
    build_passive_evidence_store_from_tagged(&wsjtx_tagged, &jtdx_tagged)
}

pub fn build_passive_evidence_store_from_tagged(
    wsjtx: &[StreamDecodedWithProvenance],
    jtdx: &[StreamDecodedWithProvenance],
) -> SharedEvidenceStore {
    let mut store = SharedEvidenceStore::new();
    for row in wsjtx {
        store.admit(shared_decode_candidate(
            DecoderId::WSJTX,
            provenance_from_stream(row.provenance),
            &row.decode,
        ));
    }
    for row in jtdx {
        store.admit(shared_decode_candidate(
            DecoderId::JTDX,
            provenance_from_stream(row.provenance),
            &row.decode,
        ));
    }
    store
}

fn provenance_from_stream(provenance: StreamDecodeProvenance) -> Provenance {
    match provenance {
        StreamDecodeProvenance::Regular => Provenance::Regular,
        StreamDecodeProvenance::ApMask => Provenance::ApMask,
        StreamDecodeProvenance::A7Memory => Provenance::A7Memory,
        StreamDecodeProvenance::A8List => Provenance::A8List,
        StreamDecodeProvenance::JtdxDeep => Provenance::JtdxDeep,
        StreamDecodeProvenance::ImportedMemory => Provenance::ImportedMemory,
    }
}

fn shared_decode_candidate(
    source: DecoderId,
    provenance: Provenance,
    row: &StreamDecodedMessage,
) -> SharedDecodeCandidate {
    SharedDecodeCandidate {
        source,
        provenance,
        message: row.msg.clone(),
        freq_hz: row.freq,
        dt_sec: row.dt,
        snr_db: row.snr.round() as i32,
    }
}

fn merge_decodes_with_source(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> Vec<HybridDecodedMessage> {
    let mut rows: Vec<HybridDecodedMessage> = Vec::new();
    let mut by_msg: HashMap<String, Vec<usize>> = HashMap::new();

    for decode in wsjtx {
        let key = normalized_message(&decode.msg);
        by_msg.entry(key).or_default().push(rows.len());
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Wsjtx,
        });
    }

    for decode in jtdx {
        let key = normalized_message(&decode.msg);
        if let Some(idx) = by_msg.get(&key).and_then(|indices| {
            indices
                .iter()
                .copied()
                .find(|&idx| is_same_signal(&rows[idx].decode, decode))
        }) {
            rows[idx].source = DecodeSource::Both;
            continue;
        }
        by_msg.entry(key).or_default().push(rows.len());
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Jtdx,
        });
    }

    rows
}

fn normalized_message(msg: &str) -> String {
    msg.split_whitespace()
        .map(normalized_message_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_message_word(word: &str) -> String {
    let word = word.to_ascii_uppercase();
    if word.starts_with('<') && word.ends_with('>') {
        let inner = &word[1..word.len() - 1];
        if !inner.is_empty() && inner != "..." {
            return inner.to_string();
        }
    }
    word
}

fn is_same_signal(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3
}

fn extract_full_call_candidates(rows: &[StreamDecodedMessage]) -> Vec<String> {
    let mut calls = Vec::new();
    for row in rows {
        for word in row.msg.split_whitespace() {
            let token = word.trim_matches(|c: char| c == ';' || c == ',');
            if is_shared_full_call_candidate(token) {
                calls.push(
                    token
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    calls
}

fn is_shared_full_call_candidate(token: &str) -> bool {
    let token = token.trim();
    if token.len() < 3 || token == "<...>" || token.eq_ignore_ascii_case("CQ") {
        return false;
    }
    if matches!(
        token.to_ascii_uppercase().as_str(),
        "DE" | "QRZ" | "DX" | "RRR" | "RR73" | "73" | "R" | "TU"
    ) {
        return false;
    }
    let bare = token.trim_start_matches('<').trim_end_matches('>');
    bare.chars().any(|c| c.is_ascii_digit())
        && bare.chars().any(|c| c.is_ascii_alphabetic())
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(freq: f64, dt: f64, msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq,
            dt,
            snr: 0.0,
            msg: msg.to_string(),
            sync: 0.0,
            itone: [0; 79],
        }
    }

    fn tagged(
        freq: f64,
        dt: f64,
        msg: &str,
        provenance: StreamDecodeProvenance,
    ) -> StreamDecodedWithProvenance {
        StreamDecodedWithProvenance {
            decode: decoded(freq, dt, msg),
            provenance,
        }
    }

    #[test]
    fn merge_decodes_deduplicates_hash_brace_display_variants() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <RK4FF> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes_with_source(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, DecodeSource::Both);
        assert_eq!(merged[0].decode.msg, "EA5/DH0YAH <RK4FF> RR73");
    }

    #[test]
    fn merge_decodes_keeps_unresolved_hash_marker_distinct() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <...> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes_with_source(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn hash_call_opportunity_report_detects_cross_decoder_resolution() {
        let wsjtx = [decoded(588.0, 0.5, "<...> US5VAC -13")];
        let jtdx = [decoded(589.0, 0.6, "R5AF/O US5VAC -13")];

        let report = hybrid_hash_call_opportunity_report(&wsjtx, &jtdx);

        assert_eq!(report.unresolved_hash_rows, 1);
        assert_eq!(report.rows_resolvable_by_other_decoder, 1);
        assert_eq!(report.hash_conflicts, 0);
    }

    #[test]
    fn passive_evidence_store_merges_agreeing_decoder_rows_without_affecting_output_shape() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <RK4FF> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes(&wsjtx, &jtdx);
        let evidence = build_passive_evidence_store(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 1);
        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(
            evidence.rows()[0].confidence,
            DecodeConfidence::ConfirmedMulti
        );
        assert!(evidence.rows()[0].import_eligible);
    }

    #[test]
    fn passive_evidence_store_does_not_import_assisted_agreement() {
        let wsjtx = [tagged(
            1205.0,
            0.6,
            "EA5/DH0YAH RK4FF RR73",
            StreamDecodeProvenance::A7Memory,
        )];
        let jtdx = [tagged(
            1206.0,
            0.8,
            "EA5/DH0YAH RK4FF RR73",
            StreamDecodeProvenance::JtdxDeep,
        )];

        let evidence = build_passive_evidence_store_from_tagged(&wsjtx, &jtdx);

        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(evidence.rows()[0].confidence, DecodeConfidence::Assisted);
        assert!(!evidence.rows()[0].import_eligible);
    }

    #[test]
    fn passive_evidence_store_does_not_import_a8_list_decodes() {
        let wsjtx = [tagged(
            1000.0,
            0.5,
            "K1JT BG5ATV PM00",
            StreamDecodeProvenance::A8List,
        )];
        let jtdx: [StreamDecodedWithProvenance; 0] = [];

        let evidence = build_passive_evidence_store_from_tagged(&wsjtx, &jtdx);

        assert_eq!(evidence.rows().len(), 1);
        assert_eq!(evidence.rows()[0].confidence, DecodeConfidence::Assisted);
        assert!(!evidence.rows()[0].import_eligible);
    }
}
