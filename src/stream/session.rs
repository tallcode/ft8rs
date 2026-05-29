use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Instant;

use crate::ft8::ap_decode::{ft8_a7d_with_downsample_cache, ApDecodeResult, ApDownsampleCache};
use crate::ft8::decode::{
    decode_f64_with_sbase, decode_f64_with_sbase_and_residual, DecodeOptions, DecodedMessage,
    SyncMode,
};
use crate::ft8::hashcall::HashCallBook;
use crate::ft8::subtract_ft8::subtract_ft8_refined;
use crate::stream::time::SlotTimestamp;

const SAMPLE_RATE: u32 = 12000;
const NMAX: usize = 15 * 12_000;
const NZHSYM_STRIDE: usize = 3456;

/// Info saved from a slot decode for AP decode in the next slot.
/// Matches WSJT-X ft8_a7_save: dt0, f0, msg0("call_1 call_2")
#[derive(Clone, Debug)]
struct A7SaveEntry {
    msg0: String,
    call_1: String,
    call_2: String,
    grid4: String, // "    " or 4-char grid
    dt0: f64,      // WSJT-X convention: dt = candidate_dt - 0.5
    f0: f64,
    xbase: f64, // noise baseline at this frequency (from sbase)
}

/// WSJT-X uses jseq = mod(nutc/5, 2) to alternate even/odd sequences.
/// AP decode only uses entries from the same parity (even→even, odd→odd).
#[allow(non_snake_case)]
#[derive(Clone, Debug)]
pub struct WsjtxDecodeConfig {
    pub nfa: f64,
    pub nfb: f64,
    pub syncmin: Option<f64>,
    pub ncand: usize,
    pub ndepth: usize,
    pub nfqso: f64,
    pub nftx: f64,
    pub nQSOProgress: usize,
    pub ncontest: usize,
    pub napwid: f64,
    pub lft8apon: bool,
    pub lapcqonly: bool,
    pub nagain: bool,
    pub mycall: Option<String>,
    pub hiscall: Option<String>,
}

pub type StreamDecodeConfig = WsjtxDecodeConfig;

impl Default for WsjtxDecodeConfig {
    fn default() -> Self {
        Self {
            nfa: 200.0,
            nfb: 3000.0,
            syncmin: None,
            ncand: 1000,
            ndepth: 3,
            nfqso: 0.0,
            nftx: 0.0,
            nQSOProgress: 0,
            ncontest: 0,
            napwid: 50.0,
            lft8apon: true,
            lapcqonly: false,
            nagain: false,
            mycall: None,
            hiscall: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct StreamDecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
    pub itone: [i32; 79],
}

pub struct StreamSlotDecodeState {
    dd0: Vec<f64>,
    seen: HashSet<String>,
    merged: Vec<StreamDecodedMessage>,
    early_results: Vec<DecodedMessage>,
    dd1: Vec<f64>,
    early_subtracted: Vec<bool>,
}

pub struct StreamDecodeSession {
    params: WsjtxDecodeConfig,
    book: HashCallBook,
    /// Entries from the previous slot of the SAME parity for AP decode.
    /// Matches WSJT-X ndec(jseq,0).
    prev_even: Vec<A7SaveEntry>,
    prev_odd: Vec<A7SaveEntry>,
    /// Current sequence parity: 0=even, 1=odd. Matches WSJT-X `jseq=mod(nutc/5,2)`.
    jseq: usize,
}

impl StreamDecodeSession {
    pub fn new(params: WsjtxDecodeConfig) -> Self {
        Self {
            params,
            book: HashCallBook::new(),
            prev_even: Vec::new(),
            prev_odd: Vec::new(),
            jseq: 0,
        }
    }

    pub fn decode_slot(&mut self, samples: &[f32]) -> Vec<StreamDecodedMessage> {
        let results = self
            .decode_slot_streaming(samples, |_| Ok(()))
            .expect("in-memory decode callback cannot fail");
        results
    }

    pub fn decode_slot_at(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
    ) -> Vec<StreamDecodedMessage> {
        let results = self
            .decode_slot_streaming_at(timestamp, samples, |_| Ok(()))
            .expect("in-memory decode callback cannot fail");
        results
    }

    pub fn decode_slot_streaming<F>(
        &mut self,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let results = self.ft8_decode_slot(samples, on_decode)?;
        Ok(results)
    }

    pub fn decode_slot_streaming_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let results = self.ft8_decode_slot_at(Some(timestamp), samples, on_decode)?;
        Ok(results)
    }

    pub fn start_slot_decode(&self) -> StreamSlotDecodeState {
        StreamSlotDecodeState {
            dd0: vec![0.0; NMAX],
            seen: HashSet::new(),
            merged: Vec::new(),
            early_results: Vec::new(),
            dd1: vec![0.0; NMAX],
            early_subtracted: Vec::new(),
        }
    }

    pub fn decode_slot_nzhsym41<F>(
        &mut self,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        on_decode: F,
    ) -> Result<(), String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.decode_slot_nzhsym41_at(None, state, samples, on_decode)
    }

    pub fn decode_slot_nzhsym41_at<F>(
        &mut self,
        timestamp: Option<&SlotTimestamp>,
        state: &mut StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<(), String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        if let Some(timestamp) = timestamp {
            self.jseq = jseq_from_nutc(timestamp.nutc());
        }

        let t_stage = Instant::now();
        state.dd0 = dd0_from_samples(samples);
        let early_dd = dd0_partial_nzhsym(&state.dd0, 41);
        let t_decode = Instant::now();
        let (early_results, _) = if self.params.ndepth == 1 {
            (Vec::new(), Vec::new())
        } else {
            decode_f64_with_sbase(&early_dd, self.ft8_decode_options(41))
        };
        trace_timer(
            "stream.nzhsym41.decode",
            t_decode,
            Some(format!("regular={}", early_results.len())),
        );
        state.early_results = early_results;
        let t_emit = Instant::now();
        for d in &state.early_results {
            push_regular_decode(
                &mut state.seen,
                &mut state.merged,
                &self.book,
                d,
                &mut on_decode,
            )?;
        }
        trace_timer(
            "stream.nzhsym41.emit",
            t_emit,
            Some(format!("merged={}", state.merged.len())),
        );
        trace_timer("stream.nzhsym41.total", t_stage, None);
        Ok(())
    }

    pub fn subtract_slot_nzhsym47(&self, state: &mut StreamSlotDecodeState, samples: &[f32]) {
        let t_stage = Instant::now();
        state.dd0 = dd0_from_samples(samples);
        state.dd1 = dd0_partial_nzhsym(&state.dd0, 47);
        state.early_subtracted = vec![false; state.early_results.len()];
        let lrefinedt = self.params.ndepth > 2;
        let mut subtracted = 0usize;
        for (idx, d) in state.early_results.iter().enumerate() {
            if d.dt < 0.396 {
                let mut itone = [0i32; 79];
                itone.copy_from_slice(&d.itone[..79]);
                subtract_ft8_refined(&mut state.dd1, &itone, d.freq, d.dt + 0.5, lrefinedt);
                state.early_subtracted[idx] = true;
                subtracted += 1;
            }
        }
        trace_timer(
            "stream.nzhsym47.subtract",
            t_stage,
            Some(format!(
                "early={} subtracted={subtracted}",
                state.early_results.len()
            )),
        );
    }

    pub fn decode_slot_nzhsym50_and_finish<F>(
        &mut self,
        mut state: StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let t_stage = Instant::now();
        state.dd0 = dd0_from_samples(samples);
        let mut full_dd = state.dd0.clone();
        full_dd[50 * NZHSYM_STRIDE..].fill(0.0);
        let mut late_subtracted = 0usize;
        if !self.params.nagain {
            let clean_prefix = (47 * NZHSYM_STRIDE).min(NMAX);
            full_dd[..clean_prefix].copy_from_slice(&state.dd1[..clean_prefix]);
            for (idx, d) in state.early_results.iter().enumerate() {
                if !state.early_subtracted.get(idx).copied().unwrap_or(false) {
                    let mut itone = [0i32; 79];
                    itone.copy_from_slice(&d.itone[..79]);
                    subtract_ft8_refined(&mut full_dd, &itone, d.freq, d.dt + 0.5, true);
                    late_subtracted += 1;
                }
            }
        }
        trace_timer(
            "stream.nzhsym50.prepare",
            t_stage,
            Some(format!("late_subtracted={late_subtracted}")),
        );

        let t_full = Instant::now();
        let mut full_options = self.ft8_decode_options(50);
        full_options.initial_messages = state.early_results.iter().map(|d| d.msg.clone()).collect();
        let (full_results, sbase, full_residual) =
            decode_f64_with_sbase_and_residual(&full_dd, full_options);
        trace_timer(
            "stream.nzhsym50.decode",
            t_full,
            Some(format!("regular={}", full_results.len())),
        );

        // Build current a7 table entries before AP. WSJT-X ft8_a7_save uses
        // these current entries to suppress previous entries already accounted
        // for by a regular decode in this sequence.
        let t_a7_save = Instant::now();
        let all_regular: Vec<&DecodedMessage> = state
            .early_results
            .iter()
            .chain(full_results.iter())
            .collect();
        let mut entries_to_save: Vec<A7SaveEntry> = all_regular
            .iter()
            .copied()
            .filter_map(|d| ft8_a7_save_entry(d, &sbase))
            .collect();
        trace_timer(
            "stream.a7_save",
            t_a7_save,
            Some(format!("entries={}", entries_to_save.len())),
        );

        let t_ap = Instant::now();
        let previous_entries = if self.jseq == 0 {
            &self.prev_even
        } else {
            &self.prev_odd
        };
        let ap_candidates = suppress_previous_a7_entries(previous_entries, &entries_to_save);
        let ap_allowed =
            self.params.lft8apon && self.params.ncontest != 6 && self.params.ncontest != 7;
        let ap_results = if !ap_allowed || ap_candidates.is_empty() {
            Vec::new()
        } else {
            let downsample_cache = ApDownsampleCache::new(&full_residual);
            let mut ap_msgs: Vec<ApDecodeResult> = Vec::new();
            for entry in &ap_candidates {
                let result = decode_a7_with_frequency_retries(&downsample_cache, entry);
                if let Some(r) = result {
                    let norm_r = normal(&r.msg);
                    if !ap_msgs.iter().any(|a| normal(&a.msg) == norm_r) {
                        ap_msgs.push(r);
                    }
                }
            }
            ap_msgs
        };
        trace_timer(
            "stream.ft8_a7d",
            t_ap,
            Some(format!(
                "candidates={} decoded={}",
                ap_candidates.len(),
                ap_results.len()
            )),
        );

        let t_merge = Instant::now();
        for d in &full_results {
            push_regular_decode(
                &mut state.seen,
                &mut state.merged,
                &self.book,
                d,
                &mut on_decode,
            )?;
        }

        for r in &ap_results {
            if let Some(entry) = ft8_a7_save_entry_from_parts(&r.msg, r.freq, r.dt, &sbase) {
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
                on_decode(&decode)?;
                state.merged.push(decode);
            }
        }
        trace_timer(
            "stream.merge",
            t_merge,
            Some(format!("merged={}", state.merged.len())),
        );

        // Matches WSJT-X: ndec(jseq,1) → ndec(jseq,0) at next UTC change.
        if self.jseq == 0 {
            self.prev_even = entries_to_save;
        } else {
            self.prev_odd = entries_to_save;
        }
        self.jseq = 1 - self.jseq;

        trace_timer(
            "stream.nzhsym50.total",
            t_stage,
            Some(format!("merged={}", state.merged.len())),
        );
        Ok(state.merged)
    }

    /// Full progressive decode matching WSJT-X flow:
    /// 1. nzhsym=41: early decode on a zero-padded partial buffer.
    /// 2. nzhsym=47: subtract early decodes from the partial buffer and save dd1.
    /// 3. nzhsym=50: decode full buffer with early-cleaned prefix.
    /// 4. AP decode using prev_slot entries of SAME parity (ft8_a7d).
    /// 5. Save current slot entries for next same-parity slot.
    fn ft8_decode_slot<F>(
        &mut self,
        samples: &[f32],
        on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        self.ft8_decode_slot_at(None, samples, on_decode)
    }

    fn ft8_decode_slot_at<F>(
        &mut self,
        timestamp: Option<&SlotTimestamp>,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let mut state = self.start_slot_decode();
        self.decode_slot_nzhsym41_at(timestamp, &mut state, samples, &mut on_decode)?;
        self.subtract_slot_nzhsym47(&mut state, samples);
        self.decode_slot_nzhsym50_and_finish(state, samples, on_decode)
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
            sync_mode: Some(SyncMode::Power),
            nzhsym: Some(nzhsym),
            ..Default::default()
        }
    }
}

fn jseq_from_nutc(nutc: u32) -> usize {
    ((nutc / 5) % 2) as usize
}

fn decode_a7_with_frequency_retries(
    downsample_cache: &ApDownsampleCache,
    entry: &A7SaveEntry,
) -> Option<ApDecodeResult> {
    // Try the saved WSJT-X f0 first. A very weak a7 decode can sit on a
    // half-Hz boundary, so if the exact saved f0 fails, retry the adjacent
    // 0.5 Hz bins used by ft8_a7d's own frequency search.
    for offset in [0.0, 0.5, -0.5] {
        if let Some(result) = ft8_a7d_with_downsample_cache(
            downsample_cache,
            &entry.call_1,
            &entry.call_2,
            &entry.grid4,
            entry.dt0,
            entry.f0 + offset,
            entry.xbase,
        ) {
            return Some(result);
        }
    }
    None
}

fn dd0_from_samples(samples: &[f32]) -> Vec<f64> {
    let mut out = vec![0.0; NMAX];
    let len = samples.len().min(NMAX);
    for i in 0..len {
        out[i] = samples[i] as f64;
    }
    out
}

fn dd0_partial_nzhsym(samples: &[f64], nzhsym: usize) -> Vec<f64> {
    let mut out = vec![0.0; NMAX];
    let n = (nzhsym * NZHSYM_STRIDE).min(NMAX).min(samples.len());
    out[..n].copy_from_slice(&samples[..n]);
    out
}

/// Extract call_1/call_2/grid4/xbase from a decoded message.
/// Matches WSJT-X ft8_a7_save logic.
fn ft8_a7_save_entry(d: &DecodedMessage, sbase: &[f64]) -> Option<A7SaveEntry> {
    ft8_a7_save_entry_from_parts(&d.msg, d.freq, d.dt, sbase)
}

fn ft8_a7_save_entry_from_parts(
    msg: &str,
    freq: f64,
    dt: f64,
    sbase: &[f64],
) -> Option<A7SaveEntry> {
    let words = split77_words(msg);
    if words.len() < 2 {
        return None;
    }

    // Skip messages with / or < (WSJT-X: if(index(msg,'/').ge.1 .or. index(msg,'<').ge.1) go to 999)
    if msg.contains('/') || msg.contains('<') {
        return None;
    }

    if words[0].starts_with("CQ_") {
        return None;
    }

    let (fragment, call_1, call_2) = if words[0] == "CQ" && words.len() >= 3 && words[1].len() <= 2
    {
        (
            format!("CQ {} {}", words[1], words[2]),
            "CQ".to_string(),
            words[1].clone(),
        )
    } else {
        (
            format!("{} {}", words[0], words[1]),
            words[0].clone(),
            words[1].clone(),
        )
    };

    // Extract grid4
    let mut grid4 = String::from("    ");
    if words.len() >= 3 {
        let last = words.last().unwrap();
        if is_grid4(last) {
            grid4 = last.clone();
        }
    }

    // Compute xbase from sbase (matching WSJT-X: 10^(0.1*(sbase(nint(f1/3.125))-40.0)))
    let xbase = {
        let df = crate::ft8::sync8_df();
        let freq_bin = nint_wsjtx_f32(freq / df).max(1) as usize;
        if freq_bin < sbase.len() {
            (10.0f32.powf(0.1 * (sbase[freq_bin] as f32 - 40.0))) as f64
        } else {
            1.0 // fallback
        }
    };

    Some(A7SaveEntry {
        msg0: fragment,
        call_1,
        call_2,
        grid4,
        dt0: dt,
        f0: freq,
        xbase,
    })
}

fn split77_words(msg: &str) -> Vec<String> {
    let mut words: Vec<String> = msg
        .split_whitespace()
        .map(|w| w.to_ascii_uppercase())
        .collect();
    if words.len() >= 3 && words[0] == "CQ" {
        let call = words[2].trim_end_matches("/R").trim_end_matches("/P");
        if is_wsjtx_chkcall(call) {
            words[0] = format!("CQ_{}", words[1]);
            words.remove(1);
        }
    }
    words
}

fn nint_wsjtx_f32(x: f64) -> isize {
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
                (cur.f0 - prev.f0).abs() <= 3.0
                    && prev.msg0.contains(&format!(" {}", cur.call_2.trim()))
            })
        })
        .cloned()
        .collect()
}

fn push_regular_decode<F>(
    seen: &mut std::collections::HashSet<String>,
    merged: &mut Vec<StreamDecodedMessage>,
    book: &HashCallBook,
    d: &DecodedMessage,
    on_decode: &mut F,
) -> Result<(), String>
where
    F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
{
    let key = normal(&d.msg);
    if !seen.insert(key) {
        return Ok(());
    }

    let mut itone = [0i32; 79];
    itone.copy_from_slice(&d.itone[..79]);
    let decode = StreamDecodedMessage {
        freq: d.freq,
        dt: d.dt,
        snr: d.snr,
        msg: d.msg.clone(),
        sync: d.sync,
        itone,
    };
    collect_book(book, &decode.msg);
    on_decode(&decode)?;
    merged.push(decode);
    Ok(())
}

fn collect_book(book: &HashCallBook, msg: &str) {
    for part in msg.split_whitespace() {
        let p = part.trim_matches(|c: char| c == ';' || c == ',');
        if is_hashable_callsign_token(p) {
            book.save(p);
        }
    }
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

fn is_wsjtx_chkcall(token: &str) -> bool {
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
    if nbc > 6 || nbc < 3 {
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

fn trace_timers_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("FT8RS_TRACE_TIMERS")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    })
}

fn trace_timer(label: &str, start: Instant, detail: Option<String>) {
    if !trace_timers_enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    match detail {
        Some(detail) => eprintln!("[ft8rs-timer] {label}: {elapsed_ms:.1} ms ({detail})"),
        None => eprintln!("[ft8rs-timer] {label}: {elapsed_ms:.1} ms"),
    }
}

#[cfg(test)]
mod tests {
    use super::{is_wsjtx_chkcall, jseq_from_nutc, split77_words};

    #[test]
    fn jseq_matches_wsjtx_nutc_parity() {
        assert_eq!(jseq_from_nutc(140300), 0);
        assert_eq!(jseq_from_nutc(140315), 1);
        assert_eq!(jseq_from_nutc(140330), 0);
        assert_eq!(jseq_from_nutc(140345), 1);
    }

    #[test]
    fn split77_words_does_not_treat_grid_as_cq_call() {
        assert!(!is_wsjtx_chkcall("KN87"));
        assert_eq!(
            split77_words("CQ D1DX KN87"),
            vec!["CQ".to_string(), "D1DX".to_string(), "KN87".to_string()]
        );
    }

    #[test]
    fn split77_words_keeps_cq_dx_call_rewrite() {
        assert!(is_wsjtx_chkcall("DL8YHR"));
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
    fn chkcall_uses_third_character_digit_when_both_second_and_third_are_digits() {
        assert!(is_wsjtx_chkcall("A12BC"));
    }
}
