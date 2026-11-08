use crate::stream::session::{
    configured_hash_calls, DecodeProfile, StreamDecodeConfig, StreamDecodeProvenance,
    StreamDecodedMessage, StreamDecodedWithProvenance,
};
use std::collections::HashSet;
use std::time::Instant;

use crate::decode::lib_wsjtx_plus::ft8_a7::{
    ft8_a7d_with_downsample_cache, A7Recovery, ApDecodeResult, ApDownsampleCache,
};
use crate::decode::lib_wsjtx_plus::ft8_decode::{
    decode_f64_with_sbase_and_residual, ft8_a8d_result, CqSpectrum, DecodeOptions, DecodedMessage,
    FinalRetry,
};
use crate::decode::lib_wsjtx_plus::subtractft8::subtract_ft8_refined;
use crate::stream::profile::SlotRecovery;
use crate::stream::time::SlotTimestamp;
use crate::HashCallBook;

const SAMPLE_RATE: u32 = 12000;
const NMAX: usize = 15 * 12_000;
const NZHSYM_STRIDE: usize = 3456;
const AP_MSG_LEN: usize = 37;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FixedMsg37 {
    bytes: [u8; AP_MSG_LEN],
}

impl FixedMsg37 {
    fn from_trimmed(value: &str) -> Self {
        let mut bytes = [b' '; AP_MSG_LEN];
        for (idx, byte) in value
            .trim()
            .as_bytes()
            .iter()
            .copied()
            .take(AP_MSG_LEN)
            .enumerate()
        {
            bytes[idx] = byte.to_ascii_uppercase();
        }
        Self { bytes }
    }

    fn trimmed(&self) -> String {
        let end = self
            .bytes
            .iter()
            .rposition(|&byte| byte != b' ')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        String::from_utf8_lossy(&self.bytes[..end]).into_owned()
    }

    fn contains(&self, needle: &str) -> bool {
        String::from_utf8_lossy(&self.bytes).contains(needle)
    }

    fn word_bounds(&self) -> Option<(usize, usize, usize)> {
        let first_space = self.bytes.iter().position(|&byte| byte == b' ')?;
        let second_rel = self.bytes[first_space + 1..]
            .iter()
            .position(|&byte| byte == b' ')?;
        let second_space = first_space + 1 + second_rel;
        Some((first_space, first_space + 1, second_space))
    }

    fn fortran_slice_trimmed(&self, start: usize, end: usize) -> String {
        let end = end.min(AP_MSG_LEN);
        if start >= end {
            return String::new();
        }
        String::from_utf8_lossy(&self.bytes[start..end])
            .trim_end()
            .to_string()
    }
}

/// Info saved from a slot decode for AP decode in the next slot.
#[derive(Clone, Debug)]
struct A7SaveEntry {
    msg0: FixedMsg37,
    dt0: f64,
    f0: f64,
}

#[derive(Default)]
struct A7Slot {
    timestamp: Option<SlotTimestamp>,
    entries: Vec<A7SaveEntry>,
}

#[derive(Default)]
struct OlderA7History {
    current_time: Option<SlotTimestamp>,
    previous_time: Option<SlotTimestamp>,
    older: A7Slot,
}

const MAX_OLDER_A7_CANDIDATES: usize = 8;

enum OlderA7Audio {
    Cached(ApDownsampleCache),
    Residual(Vec<f64>),
}

struct FinalSlotWork {
    retry: Option<FinalRetry>,
    sbase: Vec<f64>,
    older_a7: Option<OlderA7Audio>,
}

/// A7 seed entries collected for a single slot.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct A7Snapshot(pub(crate) Vec<(String, f64, f64)>);

impl A7Snapshot {
    pub fn record(&mut self, row: &StreamDecodedMessage, frequency: f64) {
        let Some(entry) = ft8_a7_save_entry_from_parts(&row.msg, frequency, row.dt) else {
            return;
        };
        let Some(fields) = entry.decode_fields() else {
            return;
        };
        if (fields.call_1 == "CQ" || is_reference_chkcall(&fields.call_1))
            && is_reference_chkcall(&fields.call_2)
            && frequency.is_finite()
            && row.dt.is_finite()
        {
            self.insert(entry.msg0.trimmed(), entry.f0, entry.dt0);
        }
    }

    pub fn merge(&mut self, other: Self) {
        for (msg, freq, dt) in other.0 {
            self.insert(msg, freq, dt);
        }
    }

    fn insert(&mut self, msg: String, freq: f64, dt: f64) {
        if !self
            .0
            .iter()
            .any(|(m, f, t)| m == &msg && (*f - freq).abs() <= 3.0 && (*t - dt).abs() <= 0.3)
        {
            self.0.push((msg, freq, dt));
        }
    }
}

#[derive(Clone, Debug)]
struct A7DecodeFields {
    call_1: String,
    call_2: String,
    grid4: String,
}

impl A7SaveEntry {
    fn decode_fields(&self) -> Option<A7DecodeFields> {
        let (i1, call_2_start, i2) = self.msg0.word_bounds()?;
        let call_1 = self.msg0.fortran_slice_trimmed(0, i1);
        let call_2 = self.msg0.fortran_slice_trimmed(call_2_start, i2);
        let grid4_raw = self.msg0.fortran_slice_trimmed(i2 + 1, i2 + 5);
        let mut grid4 = if grid4_raw.is_empty() {
            String::from("    ")
        } else {
            grid4_raw
        };
        if grid4 == "RR73" || grid4.contains('+') || grid4.contains('-') {
            grid4 = String::from("    ");
        }
        Some(A7DecodeFields {
            call_1,
            call_2,
            grid4,
        })
    }
}

pub struct StreamSlotDecodeState {
    timestamp: Option<SlotTimestamp>,
    a7_started: bool,
    dd0: Vec<f64>,
    seen: HashSet<String>,
    merged: Vec<StreamDecodedMessage>,
    early_results: Vec<DecodedMessage>,
    dd1: Vec<f64>,
    work: Vec<f64>,
    early_subtracted: Vec<bool>,
}

impl StreamSlotDecodeState {
    fn reset_for_slot(&mut self) {
        self.seen.clear();
        self.merged.clear();
        self.early_results.clear();
        self.early_subtracted.clear();
    }
}

pub struct StreamDecodeSession {
    params: StreamDecodeConfig,
    book: HashCallBook,
    regular_hash_calls: HashSet<String>,
    /// AP memory: a7[jseq][k], where jseq is even/odd UTC sequence parity and
    /// k=0/1 is previous/current for that parity.
    a7: [[Vec<A7SaveEntry>; 2]; 2],
    older_a7: Option<Box<[OlderA7History; 2]>>,
    cq_history: [Option<(SlotTimestamp, CqSpectrum)>; 2],
    /// Current sequence parity: 0=even, 1=odd.
    jseq: usize,
    slot_state: Option<StreamSlotDecodeState>,
    search_deadline: Option<Instant>,
}

impl StreamDecodeSession {
    pub fn new(params: StreamDecodeConfig) -> Self {
        let book = HashCallBook::new();
        for call in configured_hash_calls(&params) {
            book.save(&call);
        }
        Self {
            params,
            book,
            regular_hash_calls: HashSet::new(),
            a7: Default::default(),
            older_a7: None,
            cq_history: [None, None],
            jseq: 0,
            slot_state: None,
            search_deadline: None,
        }
    }

    pub(crate) fn new_standalone(params: StreamDecodeConfig) -> Self {
        let mut session = Self::new(params);
        if session.params.wsjtx_plus_candidate_retry {
            session.older_a7 = Some(Box::default());
        }
        session
    }

    pub(crate) fn decode_slot_streaming_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: impl FnMut(&StreamDecodedMessage) -> Result<(), String>,
    ) -> Result<Vec<StreamDecodedMessage>, String> {
        self.decode_slot_streaming_tagged_at(timestamp, samples, |row| on_decode(&row.decode))
    }

    pub(crate) fn decode_slot_streaming_tagged_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    ) -> Result<Vec<StreamDecodedMessage>, String> {
        let mut state = self
            .slot_state
            .take()
            .unwrap_or_else(|| self.start_slot_decode());
        state.reset_for_slot();
        let result = (|| {
            self.decode_slot_nzhsym41_tagged(timestamp, &mut state, samples, &mut on_decode)?;
            self.subtract_slot_nzhsym47(&mut state, samples);
            self.decode_slot_nzhsym50_and_finish_state(
                &mut state,
                samples,
                SlotRecovery::default(),
                on_decode,
            )
        })();
        self.slot_state = Some(state);
        result
    }

    pub fn import_hash_calls(&mut self, calls: &[String]) {
        for call in calls {
            self.book.save(call);
        }
    }

    pub(crate) fn restore_a7(&mut self, timestamp: &SlotTimestamp, snapshot: A7Snapshot) {
        let jseq = jseq_from_nutc(timestamp.nutc());
        self.a7[jseq][1] = Self::a7_entries(snapshot);
        if let Some(history) = &mut self.older_a7 {
            history[jseq].current_time = Some(timestamp.add_seconds(-30));
        }
    }

    pub(crate) fn add_a7_rows<'a>(
        &mut self,
        timestamp: &SlotTimestamp,
        rows: impl Iterator<Item = &'a StreamDecodedMessage>,
    ) {
        let bank = &mut self.a7[jseq_from_nutc(timestamp.nutc())][1];
        let mut snapshot = A7Snapshot(
            bank.iter()
                .map(|e| (e.msg0.trimmed(), e.f0, e.dt0))
                .collect(),
        );
        for row in rows {
            snapshot.record(row, row.freq);
        }
        *bank = Self::a7_entries(snapshot);
    }

    fn a7_entries(snapshot: A7Snapshot) -> Vec<A7SaveEntry> {
        snapshot
            .0
            .into_iter()
            .map(|(msg, f0, dt0)| A7SaveEntry {
                msg0: FixedMsg37::from_trimmed(&msg),
                f0,
                dt0,
            })
            .collect()
    }

    pub(crate) fn with_search_deadline(mut self, deadline: Option<Instant>) -> Self {
        self.search_deadline = deadline;
        self
    }

    pub fn export_regular_hash_calls(&self) -> Vec<String> {
        let mut calls: Vec<String> = self.regular_hash_calls.iter().cloned().collect();
        calls.sort();
        calls
    }

    pub fn start_slot_decode(&self) -> StreamSlotDecodeState {
        StreamSlotDecodeState {
            timestamp: None,
            a7_started: false,
            dd0: vec![0.0; NMAX],
            seen: HashSet::new(),
            merged: Vec::new(),
            early_results: Vec::new(),
            dd1: vec![0.0; NMAX],
            work: vec![0.0; NMAX],
            early_subtracted: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn decode_slot_nzhsym41_at(
        &mut self,
        timestamp: Option<&SlotTimestamp>,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: impl FnMut(&StreamDecodedMessage) -> Result<(), String>,
    ) -> Result<usize, String> {
        self.decode_slot_nzhsym41_tagged(
            timestamp.expect("replay timestamp"),
            state,
            samples,
            |row| on_decode(&row.decode),
        )
    }

    pub(crate) fn decode_slot_nzhsym41_tagged(
        &mut self,
        timestamp: &SlotTimestamp,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    ) -> Result<usize, String> {
        state.timestamp = Some(timestamp.clone());

        fill_dd0_from_samples(samples, &mut state.dd0);
        dd0_partial_nzhsym_into(&state.dd0, 41, &mut state.work);
        state.a7_started =
            self.older_a7.is_none() || state.work.iter().any(|&sample| sample != 0.0);
        self.ft8_a7_new_slot(Some(timestamp), state.a7_started);
        if !state.a7_started {
            return Ok(0);
        }
        if self.params.ndepth != 1 {
            let options = self.ft8_decode_options(41);
            let (early_results, _, _) =
                decode_f64_with_sbase_and_residual(&state.work, options, None, |d| {
                    push_regular_decode(
                        &mut state.seen,
                        &mut state.merged,
                        &self.book,
                        &mut self.regular_hash_calls,
                        StreamDecodeProvenance::from_wsjtx_pass(d.ap_type),
                        d,
                        &mut on_decode,
                    )
                })?;
            self.a7[self.jseq][1] = early_results.iter().filter_map(ft8_a7_save_entry).collect();
            state.early_results = early_results;
        }
        Ok(state.merged.len())
    }

    pub fn subtract_slot_nzhsym47(&self, state: &mut StreamSlotDecodeState, samples: &[f32]) {
        fill_dd0_from_samples(samples, &mut state.dd0);
        dd0_partial_nzhsym_into(&state.dd0, 47, &mut state.dd1);
        state.early_subtracted.clear();
        state
            .early_subtracted
            .resize(state.early_results.len(), false);
        let lrefinedt = self.params.ndepth > 2;
        for (idx, d) in state.early_results.iter().enumerate() {
            if d.dt < 0.396 {
                let mut itone = [0i32; 79];
                itone.copy_from_slice(&d.itone[..79]);
                subtract_ft8_refined(&mut state.dd1, &itone, d.freq, d.dt + 0.5, lrefinedt);
                state.early_subtracted[idx] = true;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn decode_slot_nzhsym50_and_finish(
        &mut self,
        state: StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: impl FnMut(&StreamDecodedMessage) -> Result<(), String>,
    ) -> Result<Vec<StreamDecodedMessage>, String> {
        self.decode_slot_nzhsym50_tagged(state, samples, SlotRecovery::default(), |row| {
            on_decode(&row.decode)
        })
    }

    pub(crate) fn decode_slot_nzhsym50_tagged(
        &mut self,
        mut state: StreamSlotDecodeState,
        samples: &[f32],
        recovery: SlotRecovery<'_>,
        on_decode: impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    ) -> Result<Vec<StreamDecodedMessage>, String> {
        self.decode_slot_nzhsym50_and_finish_state(&mut state, samples, recovery, on_decode)
    }

    /// Signals the end of baseline and compatibility callbacks with `None`.
    pub(crate) fn decode_hybrid_slot_tagged(
        &mut self,
        timestamp: &SlotTimestamp,
        staged: Option<StreamSlotDecodeState>,
        samples: &[f32],
        recovery: SlotRecovery<'_>,
        mut on_event: impl FnMut(Option<&StreamDecodedWithProvenance>) -> Result<(), String>,
    ) -> Result<(), String> {
        let is_file = staged.is_none();
        let mut state = staged
            .or_else(|| self.slot_state.take())
            .unwrap_or_else(|| self.start_slot_decode());
        let result = (|| {
            let mut emit = |row: &StreamDecodedWithProvenance| on_event(Some(row));
            if is_file {
                state.reset_for_slot();
                self.decode_slot_nzhsym41_tagged(timestamp, &mut state, samples, &mut emit)?;
                self.subtract_slot_nzhsym47(&mut state, samples);
            }
            let deadline = recovery.deadline;
            let pending = self.decode_slot_nzhsym50_base(&mut state, samples, recovery, emit)?;
            // Allow Hybrid to start downstream work before the retry search.
            on_event(None)?;
            if let Some(FinalSlotWork {
                retry: Some(mut retry),
                sbase,
                ..
            }) = pending
            {
                Self::finish_candidate_retry(&mut state, &mut retry, &sbase, deadline, |row| {
                    on_event(Some(row))
                })?;
            }
            Ok(())
        })();
        if is_file {
            self.slot_state = Some(state);
        }
        result
    }

    fn decode_slot_nzhsym50_and_finish_state<F>(
        &mut self,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        recovery: SlotRecovery<'_>,
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    {
        let deadline = recovery.deadline;
        let pending = self.decode_slot_nzhsym50_base(state, samples, recovery, &mut on_decode)?;
        if let Some(FinalSlotWork {
            retry,
            sbase,
            older_a7,
        }) = pending
        {
            if let Some(mut retry) = retry {
                Self::finish_candidate_retry(state, &mut retry, &sbase, deadline, &mut on_decode)?;
                if let Some(timestamp) = &state.timestamp {
                    let bank = &mut self.cq_history[jseq_from_nutc(timestamp.nutc())];
                    let previous = bank
                        .as_ref()
                        .filter(|(time, _)| time.add_seconds(30) == *timestamp)
                        .map(|(_, spectrum)| spectrum);
                    let base = state.merged.clone();
                    let timestamp = timestamp.clone();
                    *bank = retry
                        .recover_cq(previous, &sbase, &base, deadline, |result| {
                            push_candidate_retry(state, result, &mut on_decode)
                        })?
                        .map(|spectrum| (timestamp, spectrum));
                }
            }
            if let Some(audio) = older_a7 {
                self.recover_older_a7(
                    state,
                    audio,
                    &sbase,
                    deadline.or(self.search_deadline),
                    &mut on_decode,
                )?;
            }
        }
        Ok(std::mem::take(&mut state.merged))
    }

    fn decode_slot_nzhsym50_base(
        &mut self,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        recovery: SlotRecovery<'_>,
        mut on_decode: impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    ) -> Result<Option<FinalSlotWork>, String> {
        let deadline = recovery.deadline.or(self.search_deadline);
        let within_budget = || deadline.is_none_or(|deadline| Instant::now() < deadline);
        fill_dd0_from_samples(samples, &mut state.dd0);
        dd0_partial_nzhsym_into(&state.dd0, 50, &mut state.work);
        if !self.params.nagain {
            let clean_prefix = (47 * NZHSYM_STRIDE).min(NMAX);
            state.work[..clean_prefix].copy_from_slice(&state.dd1[..clean_prefix]);
            for (idx, d) in state.early_results.iter().enumerate() {
                if !state.early_subtracted.get(idx).copied().unwrap_or(false) {
                    let mut itone = [0i32; 79];
                    itone.copy_from_slice(&d.itone[..79]);
                    subtract_ft8_refined(&mut state.work, &itone, d.freq, d.dt + 0.5, true);
                }
            }
        }

        if !state.a7_started {
            // A staged slot can have audio at the final pass after a muted prefix.
            if state.work.iter().all(|&sample| sample == 0.0) {
                self.jseq = 1 - self.jseq;
                return Ok(None);
            }
            self.ft8_a7_new_slot(state.timestamp.as_ref(), true);
        }

        let mut full_options = self.ft8_decode_options(50);
        full_options.initial_messages = state.early_results.iter().map(|d| d.msg.clone()).collect();
        let mut retry = None;
        let retry_enabled = self.params.profile == DecodeProfile::WsjtxPlus
            && self.params.wsjtx_plus_candidate_retry
            && within_budget()
            && self.params.ndepth >= 3
            && self.params.ncontest == 0
            && !self.params.nagain
            && samples.len() >= 50 * NZHSYM_STRIDE;
        let (full_results, sbase, full_residual) = decode_f64_with_sbase_and_residual(
            &state.work,
            full_options,
            retry_enabled.then_some(&mut retry),
            |d| {
                push_regular_decode(
                    &mut state.seen,
                    &mut state.merged,
                    &self.book,
                    &mut self.regular_hash_calls,
                    StreamDecodeProvenance::from_wsjtx_pass(d.ap_type),
                    d,
                    &mut on_decode,
                )
            },
        )?;
        #[cfg(test)]
        super::validation::rr73_replay::record_terminal_residual(&full_residual);

        // Build current a7 table entries before AP. Current entries suppress
        // previous entries already accounted for by a regular decode in this
        // sequence.
        let all_regular: Vec<&DecodedMessage> = state
            .early_results
            .iter()
            .chain(full_results.iter())
            .collect();
        let mut entries_to_save: Vec<A7SaveEntry> = all_regular
            .iter()
            .copied()
            .filter_map(ft8_a7_save_entry)
            .collect();

        let previous_entries = &self.a7[self.jseq][0];
        let ap_candidates = suppress_previous_a7_entries(previous_entries, &entries_to_save);
        let ap_allowed =
            self.params.lft8apon && self.params.ncontest != 6 && self.params.ncontest != 7;
        let cache = (ap_allowed && !ap_candidates.is_empty())
            .then(|| ApDownsampleCache::new(&full_residual));
        let mut ap_results: Vec<ApDecodeResult> = Vec::new();
        let mut missed = Vec::new();
        if let Some(cache) = &cache {
            for entry in &ap_candidates {
                let (result, recovery) = decode_a7_from_saved_entry_with_adapter_retries(
                    cache,
                    entry,
                    &sbase,
                    self.params.wsjtx_plus_compatibility && within_budget(),
                );
                if let Some(r) = result {
                    let norm_r = normal(&r.msg);
                    if !ap_results.iter().any(|a| normal(&a.msg) == norm_r) {
                        if let Some(entry) = ft8_a7_save_entry_from_parts(&r.msg, r.freq, r.dt) {
                            entries_to_save.push(entry);
                        }
                        let key = normal(&r.msg);
                        if state.seen.insert(key) {
                            let decode = StreamDecodedMessage {
                                freq: r.freq,
                                dt: r.dt,
                                snr: r.snr,
                                msg: r.msg.clone(),
                                sync: 0.0,
                                itone: [0i32; 79],
                            };
                            collect_book(&self.book, &decode.msg);
                            emit_decode(
                                &mut state.merged,
                                decode,
                                StreamDecodeProvenance::A7Memory,
                                &mut on_decode,
                            )?;
                        }
                        ap_results.push(r);
                    }
                } else if !recovery.is_empty() {
                    missed.push(recovery);
                }
            }
        }
        let a8_allowed = self.wsjtx_a8_allowed(&state.early_results, &full_results, &ap_results);
        let (a8_result, a8_recovery) = if a8_allowed {
            let mycall = self.params.mycall.as_deref().unwrap_or("");
            let hiscall = self.params.hiscall.as_deref().unwrap_or("");
            let standard = crate::decode::packjt77::is_stdcall_suffix;
            ft8_a8d_result(
                &full_residual,
                mycall,
                hiscall,
                self.params.hisgrid.as_deref().unwrap_or(""),
                self.params.nfqso,
                self.params.wsjtx_plus_compatibility
                    && within_budget()
                    && standard(mycall)
                    && standard(hiscall),
            )
        } else {
            (None, None)
        };

        if let Some(r) = &a8_result {
            if let Some(entry) = ft8_a7_save_entry_from_parts(&r.msg, self.params.nfqso, r.dt) {
                entries_to_save.push(entry);
            }
            let key = normal(&r.msg);
            if state.seen.insert(key) {
                let decode = StreamDecodedMessage {
                    freq: r.freq,
                    dt: r.dt,
                    snr: r.snr,
                    msg: r.msg.clone(),
                    sync: 10.0,
                    itone: r.itone,
                };
                collect_book(&self.book, &decode.msg);
                emit_decode(
                    &mut state.merged,
                    decode,
                    StreamDecodeProvenance::A8List,
                    &mut on_decode,
                )?;
            }
        }

        // Save current decodes for the next same-parity slot.
        self.a7[self.jseq][1] = entries_to_save;
        if let Some(checkpoint) = recovery.checkpoint {
            checkpoint(A7Snapshot(
                self.a7[self.jseq][1]
                    .iter()
                    .map(|entry| (entry.msg0.trimmed(), entry.f0, entry.dt0))
                    .collect(),
            ));
        }
        self.jseq = 1 - self.jseq;

        // Supplementary outputs are emitted after the baseline callbacks and
        // never saved into the hash book, A7 memory, or subtraction input.
        for attempts in missed.into_iter().take_while(|_| within_budget()) {
            if let Some(r) = attempts
                .into_iter()
                .take_while(|_| within_budget())
                .find_map(A7Recovery::recover)
            {
                push_compatibility_decode(
                    state,
                    StreamDecodedMessage {
                        freq: r.freq,
                        dt: r.dt,
                        snr: r.snr,
                        msg: r.msg,
                        sync: 0.0,
                        itone: [0; 79],
                    },
                    &mut on_decode,
                )?;
            }
        }
        if let Some(r) = a8_recovery
            .filter(|_| within_budget())
            .and_then(|recovery| recovery.recover())
        {
            push_compatibility_decode(
                state,
                StreamDecodedMessage {
                    freq: r.freq,
                    dt: r.dt,
                    snr: r.snr,
                    msg: r.msg,
                    sync: 10.0,
                    itone: r.itone,
                },
                &mut on_decode,
            )?;
        }
        if let Some(target) = self.fine_rr73_target().filter(|_| within_budget()) {
            let already_received = state.merged.iter().any(|row| {
                row.msg.ends_with(" RR73")
                    && row
                        .msg
                        .split_whitespace()
                        .nth(1)
                        .is_some_and(|sender| sender.eq_ignore_ascii_case(target))
            });
            if !already_received {
                if let Some(row) = super::ft8_decode::recover_rr73(
                    &full_residual,
                    self.params.nfqso,
                    self.params.nfa..=self.params.nfb,
                    target,
                    deadline,
                    || {},
                ) {
                    push_compatibility_decode(state, row, &mut on_decode)?;
                }
            }
        }
        let older_a7 = (ap_allowed
            && within_budget()
            && state
                .timestamp
                .as_ref()
                .is_some_and(|timestamp| !self.older_a7_entries(timestamp).is_empty()))
        .then_some(match cache {
            Some(cache) => OlderA7Audio::Cached(cache),
            None => OlderA7Audio::Residual(full_residual),
        });
        Ok(
            (retry.is_some() || older_a7.is_some()).then_some(FinalSlotWork {
                retry,
                sbase,
                older_a7,
            }),
        )
    }

    fn older_a7_entries(&self, timestamp: &SlotTimestamp) -> &[A7SaveEntry] {
        let Some(history) = &self.older_a7 else {
            return &[];
        };
        let older = &history[jseq_from_nutc(timestamp.nutc())].older;
        if older
            .timestamp
            .as_ref()
            .is_some_and(|time| time.add_seconds(60) == *timestamp)
        {
            &older.entries
        } else {
            &[]
        }
    }

    fn recover_older_a7(
        &self,
        state: &mut StreamSlotDecodeState,
        audio: OlderA7Audio,
        sbase: &[f64],
        deadline: Option<Instant>,
        on_decode: &mut impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    ) -> Result<(), String> {
        let within_budget = || deadline.is_none_or(|deadline| Instant::now() < deadline);
        let Some(timestamp) = state.timestamp.as_ref().filter(|_| within_budget()) else {
            return Ok(());
        };
        let recent = &self.a7[jseq_from_nutc(timestamp.nutc())][0];
        let current: Vec<_> = state
            .merged
            .iter()
            .filter_map(|row| ft8_a7_save_entry_from_parts(&row.msg, row.freq, row.dt))
            .collect();
        let mut candidates = Vec::new();
        for entry in self
            .older_a7_entries(timestamp)
            .iter()
            .take_while(|_| within_budget())
        {
            if !recent
                .iter()
                .chain(&current)
                .chain(candidates.iter().copied())
                .any(|other| same_a7_hint(entry, other))
            {
                candidates.push(entry);
            }
        }
        if candidates.is_empty() || !within_budget() {
            return Ok(());
        }
        let cache = match audio {
            OlderA7Audio::Cached(cache) => cache,
            OlderA7Audio::Residual(residual) => ApDownsampleCache::new(&residual),
        };
        let mut ranked: Vec<_> = candidates
            .into_iter()
            .take_while(|_| within_budget())
            .filter_map(|entry| {
                let noise = a7_xbase(entry.f0, sbase);
                let score = cache.older_a7_sync(entry.f0, entry.dt0) / noise;
                (score.is_finite() && score > 0.0).then_some((score, entry, noise))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (_, entry, noise) in ranked
            .into_iter()
            .take(MAX_OLDER_A7_CANDIDATES)
            .take_while(|_| within_budget())
        {
            let Some(fields) = entry.decode_fields() else {
                continue;
            };
            let decode = |freq, noise| {
                ft8_a7d_with_downsample_cache(
                    &cache,
                    &fields.call_1,
                    &fields.call_2,
                    &fields.grid4,
                    entry.dt0,
                    freq,
                    noise,
                    false,
                )
                .0
            };
            let Some(row) = decode(entry.f0, noise) else {
                continue;
            };
            let key = normal(&row.msg);
            // A quarter-Hz offset changes the grid of half-Hz alignments.
            let shifted = entry.f0 + 0.25;
            let confirmed = !state.seen.contains(&key)
                && within_budget()
                && decode(shifted, a7_xbase(shifted, sbase)).is_some_and(|other| {
                    other.msg == row.msg
                        && (other.freq - row.freq).abs() <= 1.0
                        && (other.dt - row.dt).abs() <= 0.04
                });
            if !confirmed {
                continue;
            }
            state.seen.insert(key);
            emit_decode(
                &mut state.merged,
                StreamDecodedMessage {
                    freq: row.freq,
                    dt: row.dt,
                    snr: row.snr,
                    msg: row.msg,
                    sync: 0.0,
                    itone: [0; 79],
                },
                StreamDecodeProvenance::CandidateRetry,
                on_decode,
            )?;
        }
        Ok(())
    }

    fn finish_candidate_retry(
        state: &mut StreamSlotDecodeState,
        retry: &mut FinalRetry,
        sbase: &[f64],
        deadline: Option<Instant>,
        mut on_decode: impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
    ) -> Result<(), String> {
        retry.sort_peaks();
        for result in retry.recover_local(sbase, &state.merged, deadline) {
            push_candidate_retry(state, result, &mut on_decode)?;
        }
        let base = state.merged.clone();
        retry.recover_adaptive(sbase, &base, deadline, |result| {
            push_candidate_retry(state, result, &mut on_decode)
        })
    }

    fn fine_rr73_target(&self) -> Option<&str> {
        #[cfg(test)]
        if !super::validation::rr73_replay::fine_recovery_enabled() {
            return None;
        }
        let has_operator = self
            .params
            .mycall
            .as_deref()
            .is_some_and(|call| !call.trim().is_empty());
        if !self.params.wsjtx_plus_compatibility
            || has_operator
            || self.params.ncontest != 0
            || !self.params.lft8apon
            || self.params.nagain
        {
            return None;
        }
        self.params.hiscall.as_deref().map(str::trim)
    }

    fn wsjtx_a8_allowed(
        &self,
        early_results: &[DecodedMessage],
        full_results: &[DecodedMessage],
        ap_results: &[ApDecodeResult],
    ) -> bool {
        if !self.params.lft8apon || self.params.ncontest == 6 || self.params.ncontest == 7 {
            return false;
        }
        let hiscall = self.params.hiscall.as_deref().unwrap_or("").trim();
        let hisgrid = self.params.hisgrid.as_deref().unwrap_or("").trim();
        if hiscall.len() < 3 || hisgrid.len() < 4 {
            return false;
        }
        let nfqso = self.params.nfqso;
        // Mirrors WSJT-X `ltry_a8`: callback processing disables a8 when any
        // decode has already landed within 3 Hz of the QSO frequency.
        let ltry_a8 = early_results
            .iter()
            .chain(full_results.iter())
            .all(|d| (d.freq - nfqso).abs() >= 3.0)
            && ap_results.iter().all(|d| (d.freq - nfqso).abs() >= 3.0);
        // Mirrors WSJT-X `la8`: a7 AP decode containing `hiscall12` suppresses
        // the later a8 list decode.
        let la8 = ap_results.iter().all(|d| !d.msg.contains(hiscall));
        la8 && ltry_a8
    }

    fn ft8_a7_new_slot(&mut self, timestamp: Option<&SlotTimestamp>, has_audio: bool) {
        if let Some(timestamp) = timestamp {
            self.jseq = jseq_from_nutc(timestamp.nutc());
        }
        if has_audio {
            let [previous, current] = &mut self.a7[self.jseq];
            let older = std::mem::replace(previous, std::mem::take(current));
            if let Some(history) = &mut self.older_a7 {
                let history = &mut history[self.jseq];
                history.older = A7Slot {
                    timestamp: history.previous_time.take(),
                    entries: older,
                };
                history.previous_time =
                    std::mem::replace(&mut history.current_time, timestamp.cloned());
            }
        }
    }

    fn ft8_decode_options(&self, nzhsym: usize) -> DecodeOptions {
        DecodeOptions {
            sample_rate: Some(SAMPLE_RATE as usize),
            nfa: Some(self.params.nfa),
            nfb: Some(self.params.nfb),
            syncmin: self.params.syncmin,
            ndepth: Some(self.params.ndepth),
            ncand: Some(self.params.ncand),
            hashcallbook: Some(self.book.clone_book()),
            mycall: self.params.mycall.clone(),
            hiscall: self.params.hiscall.clone(),
            nfqso: Some(self.params.nfqso),
            nftx: Some(self.params.nftx),
            nQSOProgress: Some(self.params.nQSOProgress),
            ncontest: Some(self.params.ncontest),
            napwid: Some(self.params.napwid),
            lft8apon: Some(self.params.lft8apon && nzhsym == 50),
            lapcqonly: Some(self.params.lapcqonly),
            nagain: Some(self.params.nagain),
            nzhsym: Some(nzhsym),
            search_deadline: self.search_deadline,
            ..Default::default()
        }
    }
}

fn jseq_from_nutc(nutc: u32) -> usize {
    ((nutc / 5) % 2) as usize
}

fn push_candidate_retry(
    state: &mut StreamSlotDecodeState,
    result: DecodedMessage,
    on_decode: &mut impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
) -> Result<(), String> {
    let decode = stream_decode(&result);
    // The same codeword can be displayed with a different hash spelling.
    if state.merged.iter().any(|row| {
        row.itone == decode.itone
            && (row.freq - decode.freq).abs() <= 5.0
            && (row.dt - decode.dt).abs() <= 0.3
    }) {
        return Ok(());
    }
    if state.seen.insert(normal(&decode.msg)) {
        emit_decode(
            &mut state.merged,
            decode,
            StreamDecodeProvenance::CandidateRetry,
            on_decode,
        )?;
    }
    Ok(())
}

fn push_compatibility_decode(
    state: &mut StreamSlotDecodeState,
    decode: StreamDecodedMessage,
    on_decode: &mut impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
) -> Result<(), String> {
    if state.seen.insert(normal(&decode.msg)) {
        emit_decode(
            &mut state.merged,
            decode,
            StreamDecodeProvenance::PlusCompatibility,
            on_decode,
        )?;
    }
    Ok(())
}

fn decode_a7_from_saved_entry_with_adapter_retries(
    downsample_cache: &ApDownsampleCache,
    entry: &A7SaveEntry,
    sbase: &[f64],
    compatibility: bool,
) -> (Option<ApDecodeResult>, Vec<A7Recovery>) {
    let Some(fields) = entry.decode_fields() else {
        return (None, Vec::new());
    };
    let standard = crate::decode::packjt77::is_stdcall;
    let keep_recovery = compatibility
        && standard(&fields.call_2)
        && (standard(&fields.call_1) || (fields.call_1 == "CQ" && is_grid4(&fields.grid4)));

    let xbase = a7_xbase(entry.f0, sbase);
    // Stream-adapter retry guard. The decoder core ft8_a7d already performs
    // its internal ifr=-5..5 frequency peak. These seed offsets compensate
    // saved-entry frequency quantization at the adapter boundary.
    let mut missed = Vec::new();
    for offset in [0.0, 0.5, -0.5] {
        let (result, recovery) = ft8_a7d_with_downsample_cache(
            downsample_cache,
            &fields.call_1,
            &fields.call_2,
            &fields.grid4,
            entry.dt0,
            entry.f0 + offset,
            xbase,
            keep_recovery,
        );
        if result.is_some() {
            return (result, Vec::new());
        }
        missed.extend(recovery);
    }
    (None, missed)
}

fn fill_dd0_from_samples(samples: &[f32], out: &mut [f64]) {
    out.fill(0.0);
    let len = samples.len().min(NMAX);
    for i in 0..len {
        out[i] = samples[i] as f64;
    }
}

fn dd0_partial_nzhsym_into(samples: &[f64], nzhsym: usize, out: &mut [f64]) {
    let n = (nzhsym * NZHSYM_STRIDE).min(NMAX).min(samples.len());
    out[..n].copy_from_slice(&samples[..n]);
    out[n..].fill(0.0);
}

fn ft8_a7_save_entry(d: &DecodedMessage) -> Option<A7SaveEntry> {
    ft8_a7_save_entry_from_parts(&d.msg, d.freq, d.dt)
}

fn ft8_a7_save_entry_from_parts(msg: &str, freq: f64, dt: f64) -> Option<A7SaveEntry> {
    let words = split77_words(msg);
    if words.len() < 2 {
        return None;
    }

    // Skip compound/hash forms for AP memory.
    if msg.contains('/') || msg.contains('<') {
        return None;
    }

    if words[0].starts_with("CQ_") {
        return None;
    }

    let fragment = if words[0] == "CQ" && words.len() >= 3 && words[1].len() <= 2 {
        format!("CQ {} {}", words[1], words[2])
    } else {
        format!("{} {}", words[0], words[1])
    };

    let msg0 = if words.len() >= 3 && is_grid4(words.last().unwrap()) {
        format!("{} {}", fragment, words.last().unwrap())
    } else {
        fragment
    };

    Some(A7SaveEntry {
        msg0: FixedMsg37::from_trimmed(&msg0),
        dt0: dt,
        f0: freq,
    })
}

fn a7_xbase(f1: f64, sbase: &[f64]) -> f64 {
    let df = crate::decode::sync8_df();
    let freq_bin = nint_reference_f32(f1 / df).max(1) as usize;
    if freq_bin < sbase.len() {
        (10.0f32.powf(0.1 * (sbase[freq_bin] as f32 - 40.0))) as f64
    } else {
        1.0
    }
}

fn split77_words(msg: &str) -> Vec<String> {
    let mut words: Vec<String> = msg
        .split_whitespace()
        .map(|w| w.to_ascii_uppercase())
        .collect();
    if words.len() >= 3 && words[0] == "CQ" {
        let call = words[2].trim_end_matches("/R").trim_end_matches("/P");
        if is_reference_chkcall(call) {
            words[0] = format!("CQ_{}", words[1]);
            words.remove(1);
        }
    }
    words
}

fn nint_reference_f32(x: f64) -> isize {
    (x as f32).round() as isize
}

fn suppress_previous_a7_entries(
    previous: &[A7SaveEntry],
    current: &[A7SaveEntry],
) -> Vec<A7SaveEntry> {
    previous
        .iter()
        .filter(|prev| {
            !current.iter().any(|cur| {
                let Some(cur_fields) = cur.decode_fields() else {
                    return false;
                };
                (cur.f0 - prev.f0).abs() <= 3.0
                    && prev
                        .msg0
                        .contains(&format!(" {}", cur_fields.call_2.trim()))
            })
        })
        .cloned()
        .collect()
}

fn same_a7_hint(a: &A7SaveEntry, b: &A7SaveEntry) -> bool {
    (a.f0 - b.f0).abs() <= 3.0
        && (a.dt0 - b.dt0).abs() <= 0.3
        && a.msg0.word_bounds().zip(b.msg0.word_bounds()).is_some_and(
            |((_, _, a_end), (_, _, b_end))| a.msg0.bytes[..a_end] == b.msg0.bytes[..b_end],
        )
}

#[allow(clippy::too_many_arguments)]
fn push_regular_decode<F>(
    seen: &mut std::collections::HashSet<String>,
    merged: &mut Vec<StreamDecodedMessage>,
    book: &HashCallBook,
    regular_hash_calls: &mut HashSet<String>,
    provenance: StreamDecodeProvenance,
    d: &DecodedMessage,
    on_decode: &mut F,
) -> Result<(), String>
where
    F: FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
{
    let key = normal(&d.msg);
    if !seen.insert(key) {
        return Ok(());
    }

    let decode = stream_decode(d);
    for call in collect_book(book, &decode.msg) {
        regular_hash_calls.insert(call);
    }
    emit_decode(merged, decode, provenance, on_decode)
}

fn stream_decode(d: &DecodedMessage) -> StreamDecodedMessage {
    let mut itone = [0i32; 79];
    itone.copy_from_slice(&d.itone[..79]);
    StreamDecodedMessage {
        freq: d.freq,
        dt: d.dt,
        snr: d.snr,
        msg: d.msg.clone(),
        sync: d.sync,
        itone,
    }
}

fn emit_decode(
    merged: &mut Vec<StreamDecodedMessage>,
    decode: StreamDecodedMessage,
    provenance: StreamDecodeProvenance,
    on_decode: &mut impl FnMut(&StreamDecodedWithProvenance) -> Result<(), String>,
) -> Result<(), String> {
    let row = StreamDecodedWithProvenance { decode, provenance };
    on_decode(&row)?;
    merged.push(row.decode);
    Ok(())
}

fn collect_book(book: &HashCallBook, msg: &str) -> Vec<String> {
    let mut saved = Vec::new();
    for part in msg.split_whitespace() {
        let p = part.trim_matches(|c: char| c == ';' || c == ',');
        if is_hashable_callsign_token(p) {
            book.save(p);
            saved.push(
                p.trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_ascii_uppercase(),
            );
        }
    }
    saved
}

fn is_grid4(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 4
        && bytes[0] >= b'A'
        && bytes[0] <= b'R'
        && bytes[1] >= b'A'
        && bytes[1] <= b'R'
        && bytes[2] >= b'0'
        && bytes[2] <= b'9'
        && bytes[3] >= b'0'
        && bytes[3] <= b'9'
}

fn is_hashable_callsign_token(token: &str) -> bool {
    let t = token.trim();
    if t.len() < 3 || t == "<...>" || t.eq_ignore_ascii_case("CQ") {
        return false;
    }
    if matches!(
        t.to_ascii_uppercase().as_str(),
        "DE" | "QRZ" | "DX" | "RRR" | "RR73" | "73" | "R" | "TU"
    ) {
        return false;
    }
    let bare = t.trim_start_matches('<').trim_end_matches('>');
    if is_grid4(bare) {
        return false;
    }
    bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '/')
        && bare.chars().any(|c| c.is_ascii_alphabetic())
        && bare.chars().any(|c| c.is_ascii_digit())
}

fn is_reference_chkcall(token: &str) -> bool {
    let w = token.trim().to_ascii_uppercase();
    if w.is_empty()
        || w.len() > 11
        || w.contains('.')
        || w.contains('+')
        || w.contains('-')
        || w.contains('?')
    {
        return false;
    }
    if w.len() > 6 && !w.contains('/') {
        return false;
    }

    let base = if let Some(i0) = w.find('/') {
        let left = &w[..i0];
        let right = &w[i0 + 1..];
        if left.len().max(right.len()) > 6 || left.is_empty() || right.is_empty() {
            return false;
        }
        if left.len() <= right.len() {
            right
        } else {
            left
        }
    } else {
        w.as_str()
    };

    let bytes = base.as_bytes();
    let nbc = bytes.len();
    if !(3..=6).contains(&nbc) {
        return false;
    }
    if !bytes[0].is_ascii_uppercase() && !bytes[1].is_ascii_uppercase() {
        return false;
    }
    if bytes[0] == b'Q' && !base.starts_with("QU1RK") {
        return false;
    }

    let mut digit_pos = None;
    if bytes[1].is_ascii_digit() {
        digit_pos = Some(1usize);
    }
    if bytes[2].is_ascii_digit() {
        digit_pos = Some(2usize);
    }
    let Some(digit_pos) = digit_pos else {
        return false;
    };
    if digit_pos + 1 == nbc {
        return false;
    }
    bytes[digit_pos + 1..]
        .iter()
        .all(|b| b.is_ascii_uppercase())
        && (1..=3).contains(&(nbc - digit_pos - 1))
}

fn normal(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.trim().to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        ft8_a7_save_entry_from_parts, is_reference_chkcall, jseq_from_nutc, same_a7_hint,
        split77_words, A7SaveEntry, FixedMsg37, StreamDecodeConfig, StreamDecodeSession,
    };
    use crate::stream::time::SlotTimestamp;

    #[test]
    fn hash_and_ap_memory_do_not_cross_session_boundaries() {
        let mut first = StreamDecodeSession::new(StreamDecodeConfig::default());
        let second = StreamDecodeSession::new(StreamDecodeConfig::default());
        let original = crate::StreamDecodeSession::new(StreamDecodeConfig::default());
        first.import_hash_calls(&["K1ABC".into()]);
        assert_eq!(first.book.get_calls(), ["K1ABC"]);
        assert!(second.book.get_calls().is_empty());
        first.a7[0][1].push(test_a7_entry("CQ K1ABC FN42"));
        assert!(second.a7.iter().flatten().all(Vec::is_empty));
        assert!(original.export_regular_hash_calls().is_empty());
        // Hints remain hints; no imported result is promoted to regular evidence.
        assert!(first.export_regular_hash_calls().is_empty());
        first.book.clear();
        assert!(first.book.get_calls().is_empty());
    }

    #[test]
    fn jseq_matches_nutc_parity() {
        assert_eq!(jseq_from_nutc(140300), 0);
        assert_eq!(jseq_from_nutc(140315), 1);
        assert_eq!(jseq_from_nutc(140330), 0);
        assert_eq!(jseq_from_nutc(140345), 1);
    }

    #[test]
    fn high_sensitivity_preserves_explicit_agc_compensation_selection() {
        for nagcc in [false, true] {
            let config = StreamDecodeConfig {
                nagcc,
                ..Default::default()
            };
            let jtdx = config.clone_for_profile_jtdx_high_sensitivity();
            assert_eq!(jtdx.nagcc, nagcc);
            assert!(jtdx.lft8lowth && jtdx.lft8subpass);
        }
    }

    #[test]
    fn jtdx_defaults_to_one_partition_and_preserves_explicit_auto() {
        assert_eq!(StreamDecodeConfig::default().jtdx_threads, 1);
        let automatic = StreamDecodeConfig {
            jtdx_threads: 0,
            ..Default::default()
        };
        assert_eq!(
            automatic
                .clone_for_profile_jtdx_high_sensitivity()
                .jtdx_threads,
            0
        );
    }

    #[test]
    fn split77_words_does_not_treat_grid_as_cq_call() {
        assert!(!is_reference_chkcall("KN87"));
        assert_eq!(
            split77_words("CQ D1DX KN87"),
            vec!["CQ".to_string(), "D1DX".to_string(), "KN87".to_string()]
        );
    }

    #[test]
    fn split77_words_keeps_cq_dx_call_rewrite() {
        assert!(is_reference_chkcall("DL8YHR"));
        assert_eq!(
            split77_words("CQ DX DL8YHR JO41"),
            vec![
                "CQ_DX".to_string(),
                "DL8YHR".to_string(),
                "JO41".to_string()
            ]
        );
    }

    #[test]
    fn a7_save_entry_matches_cq_grid_and_skip_rules() {
        let cq = ft8_a7_save_entry_from_parts("CQ D1DX KN87", 1500.0, 0.2).unwrap();
        assert_eq!(cq.msg0.trimmed(), "CQ D1DX KN87");
        let cq_fields = cq.decode_fields().unwrap();
        assert_eq!(cq_fields.call_1, "CQ");
        assert_eq!(cq_fields.call_2, "D1DX");
        assert_eq!(cq_fields.grid4, "KN87");

        let report = ft8_a7_save_entry_from_parts("K1ABC W9XYZ -07", 1500.0, 0.2).unwrap();
        assert_eq!(report.msg0.trimmed(), "K1ABC W9XYZ");
        let report_fields = report.decode_fields().unwrap();
        assert_eq!(report_fields.call_1, "K1ABC");
        assert_eq!(report_fields.call_2, "W9XYZ");
        assert_eq!(report_fields.grid4, "    ");

        assert!(ft8_a7_save_entry_from_parts("CQ DX DL8YHR JO41", 1500.0, 0.2).is_none());
        assert!(ft8_a7_save_entry_from_parts("EA5/DH0YAH RK4FF RR73", 1500.0, 0.2).is_none());
        assert!(ft8_a7_save_entry_from_parts("<RK4FF> EA5/DH0YAH 73", 1500.0, 0.2).is_none());
    }

    #[test]
    fn older_a7_hint_matches_the_pair_and_signal_position() {
        let recent = ft8_a7_save_entry_from_parts("K1ABC W9XYZ -07", 1500.0, 0.2).unwrap();
        for (msg, freq, dt, duplicate) in [
            ("K1ABC W9XYZ EN50", 1502.0, 0.3, true),
            ("K1ABC W9XYZ EN50", 1504.0, 0.3, false),
            ("K1ABC W9XYZ EN50", 1502.0, 0.6, false),
            ("W1AW W9XYZ EN50", 1502.0, 0.3, false),
        ] {
            let older = ft8_a7_save_entry_from_parts(msg, freq, dt).unwrap();
            assert_eq!(same_a7_hint(&recent, &older), duplicate);
        }
    }

    #[test]
    fn msg37_storage_is_fixed_width_uppercase_and_blank_padded() {
        let msg37 = FixedMsg37::from_trimmed("k1abc w9xyz rr73");

        assert_eq!(msg37.trimmed(), "K1ABC W9XYZ RR73");
        assert_eq!(msg37.bytes.len(), 37);
        assert!(msg37.bytes[msg37.trimmed().len()..]
            .iter()
            .all(|&byte| byte == b' '));
    }

    #[test]
    fn chkcall_uses_third_character_digit_when_both_second_and_third_are_digits() {
        assert!(is_reference_chkcall("A12BC"));
    }

    #[test]
    fn a7_memory_moves_current_to_previous_by_jseq_at_new_slot() {
        let mut session = StreamDecodeSession::new(StreamDecodeConfig::default());
        session.a7[0][1].push(test_a7_entry("K1ABC W9XYZ"));
        session.a7[1][1].push(test_a7_entry("N0CALL W1AW"));

        let even_timestamp = SlotTimestamp::parse("230208_140330").unwrap();
        session.ft8_a7_new_slot(Some(&even_timestamp), true);
        assert_eq!(session.jseq, 0);
        assert_eq!(session.a7[0][0].len(), 1);
        assert!(session.a7[0][1].is_empty());
        assert_eq!(session.a7[1][0].len(), 0);
        assert_eq!(session.a7[1][1].len(), 1);

        let odd_timestamp = SlotTimestamp::parse("230208_140345").unwrap();
        session.ft8_a7_new_slot(Some(&odd_timestamp), true);
        assert_eq!(session.jseq, 1);
        assert_eq!(session.a7[1][0].len(), 1);
        assert!(session.a7[1][1].is_empty());
        assert_eq!(session.a7[0][0].len(), 1);
    }

    fn test_a7_entry(msg0: &str) -> A7SaveEntry {
        A7SaveEntry {
            msg0: FixedMsg37::from_trimmed(msg0),
            dt0: 0.0,
            f0: 1500.0,
        }
    }
}
