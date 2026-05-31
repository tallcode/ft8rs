use std::collections::HashSet;

use crate::decode::ft8_a7::{ft8_a7d_with_downsample_cache, ApDecodeResult, ApDownsampleCache};
use crate::decode::ft8_decode::{
    decode_f64_with_sbase, decode_f64_with_sbase_and_residual, DecodeOptions, DecodedMessage,
};
use crate::decode::subtractft8::subtract_ft8_refined;
use crate::stream::time::SlotTimestamp;
use crate::HashCallBook;

const SAMPLE_RATE: u32 = 12000;
const NMAX: usize = 15 * 12_000;
const NZHSYM_STRIDE: usize = 3456;
const AP_MSG_LEN: usize = 37;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeProfile {
    Wsjtx,
    Jtdx,
    Hybrid,
}

impl DecodeProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wsjtx => "wsjtx",
            Self::Jtdx => "jtdx",
            Self::Hybrid => "hybrid",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "wsjtx" => Ok(Self::Wsjtx),
            "jtdx" => Ok(Self::Jtdx),
            "hybrid" => Ok(Self::Hybrid),
            _ => Err(format!(
                "unknown profile '{value}'; expected one of: wsjtx, jtdx, hybrid"
            )),
        }
    }
}

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

    #[cfg(test)]
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

#[allow(non_snake_case)]
#[derive(Clone, Debug)]
pub struct StreamDecodeConfig {
    pub profile: DecodeProfile,
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
    pub swl: bool,
    pub nft8cycles: usize,
    pub nft8swlcycles: usize,
    pub lft8lowth: bool,
    pub nagcc: bool,
    pub lforcesync: bool,
    pub lhound: bool,
    pub ncandthin: usize,
    pub filter: bool,
    pub hide_dupes: bool,
    pub hide_hash: bool,
    pub mycall: Option<String>,
    pub mygrid: Option<String>,
    pub hiscall: Option<String>,
    pub hisgrid: Option<String>,
}

impl Default for StreamDecodeConfig {
    fn default() -> Self {
        Self {
            profile: DecodeProfile::Wsjtx,
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
            swl: false,
            nft8cycles: 1,
            nft8swlcycles: 1,
            lft8lowth: false,
            nagcc: false,
            lforcesync: false,
            lhound: false,
            ncandthin: 100,
            filter: false,
            hide_dupes: false,
            hide_hash: false,
            mycall: None,
            mygrid: None,
            hiscall: None,
            hisgrid: None,
        }
    }
}

impl StreamDecodeConfig {
    pub fn clone_for_profile_wsjt_x(&self) -> Self {
        let mut config = self.clone();
        config.profile = DecodeProfile::Wsjtx;
        config
    }

    pub fn clone_for_profile_jtdx(&self) -> Self {
        let mut config = self.clone();
        config.profile = DecodeProfile::Jtdx;
        config
    }

    pub fn clone_for_profile_jtdx_high_sensitivity(&self) -> Self {
        let mut config = self.clone_for_profile_jtdx();
        config.nft8cycles = 3;
        config.nft8swlcycles = 3;
        config.lft8lowth = true;
        config.nagcc = true;
        config.ncandthin = 100;
        config.filter = false;
        config.hide_dupes = false;
        config.hide_hash = false;
        config.ncand = config.ncand.max(1000);
        config.ndepth = config.ndepth.max(3);
        config.lft8apon = true;
        config.lapcqonly = false;
        config.nagain = false;
        config
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
    params: StreamDecodeConfig,
    book: HashCallBook,
    /// AP memory: a7[jseq][k], where jseq is even/odd UTC sequence parity and
    /// k=0/1 is previous/current for that parity.
    a7: [[Vec<A7SaveEntry>; 2]; 2],
    /// Current sequence parity: 0=even, 1=odd.
    jseq: usize,
}

impl StreamDecodeSession {
    pub fn new(params: StreamDecodeConfig) -> Self {
        Self {
            params,
            book: HashCallBook::new(),
            a7: [[Vec::new(), Vec::new()], [Vec::new(), Vec::new()]],
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
        self.ft8_a7_new_slot(timestamp);

        state.dd0 = dd0_from_samples(samples);
        let early_dd = dd0_partial_nzhsym(&state.dd0, 41);
        let (early_results, _) = if self.params.ndepth == 1 {
            (Vec::new(), Vec::new())
        } else {
            decode_f64_with_sbase(&early_dd, self.ft8_decode_options(41))
        };
        state.early_results = early_results;
        for d in &state.early_results {
            push_regular_decode(
                &mut state.seen,
                &mut state.merged,
                &self.book,
                d,
                &mut on_decode,
            )?;
        }
        Ok(())
    }

    pub fn subtract_slot_nzhsym47(&self, state: &mut StreamSlotDecodeState, samples: &[f32]) {
        state.dd0 = dd0_from_samples(samples);
        state.dd1 = dd0_partial_nzhsym(&state.dd0, 47);
        state.early_subtracted = vec![false; state.early_results.len()];
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

    pub fn decode_slot_nzhsym50_and_finish<F>(
        &mut self,
        mut state: StreamSlotDecodeState,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        state.dd0 = dd0_from_samples(samples);
        let mut full_dd = state.dd0.clone();
        full_dd[50 * NZHSYM_STRIDE..].fill(0.0);
        if !self.params.nagain {
            let clean_prefix = (47 * NZHSYM_STRIDE).min(NMAX);
            full_dd[..clean_prefix].copy_from_slice(&state.dd1[..clean_prefix]);
            for (idx, d) in state.early_results.iter().enumerate() {
                if !state.early_subtracted.get(idx).copied().unwrap_or(false) {
                    let mut itone = [0i32; 79];
                    itone.copy_from_slice(&d.itone[..79]);
                    subtract_ft8_refined(&mut full_dd, &itone, d.freq, d.dt + 0.5, true);
                }
            }
        }

        let mut full_options = self.ft8_decode_options(50);
        full_options.initial_messages = state.early_results.iter().map(|d| d.msg.clone()).collect();
        let (full_results, sbase, full_residual) =
            decode_f64_with_sbase_and_residual(&full_dd, full_options);

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
        let ap_results = if !ap_allowed || ap_candidates.is_empty() {
            Vec::new()
        } else {
            let downsample_cache = ApDownsampleCache::new(&full_residual);
            let mut ap_msgs: Vec<ApDecodeResult> = Vec::new();
            for entry in &ap_candidates {
                let result = decode_a7_from_saved_entry_with_adapter_retries(
                    &downsample_cache,
                    entry,
                    &sbase,
                );
                if let Some(r) = result {
                    let norm_r = normal(&r.msg);
                    if !ap_msgs.iter().any(|a| normal(&a.msg) == norm_r) {
                        ap_msgs.push(r);
                    }
                }
            }
            ap_msgs
        };

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
                on_decode(&decode)?;
                state.merged.push(decode);
            }
        }

        // Save current decodes for the next same-parity slot.
        self.a7[self.jseq][1] = entries_to_save;
        self.jseq = 1 - self.jseq;

        Ok(state.merged)
    }

    fn ft8_a7_new_slot(&mut self, timestamp: Option<&SlotTimestamp>) {
        if let Some(timestamp) = timestamp {
            self.jseq = jseq_from_nutc(timestamp.nutc());
        }
        self.a7[self.jseq][0] = std::mem::take(&mut self.a7[self.jseq][1]);
    }

    /// Full progressive decode flow:
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
            nzhsym: Some(nzhsym),
            ..Default::default()
        }
    }
}

fn jseq_from_nutc(nutc: u32) -> usize {
    ((nutc / 5) % 2) as usize
}

fn decode_a7_from_saved_entry_with_adapter_retries(
    downsample_cache: &ApDownsampleCache,
    entry: &A7SaveEntry,
    sbase: &[f64],
) -> Option<ApDecodeResult> {
    let fields = entry.decode_fields()?;
    let xbase = a7_xbase(entry.f0, sbase);
    // Stream-adapter retry guard. The decoder core ft8_a7d already performs
    // its internal ifr=-5..5 frequency peak. These seed offsets compensate
    // saved-entry frequency quantization at the adapter boundary.
    for offset in [0.0, 0.5, -0.5] {
        if let Some(result) = ft8_a7d_with_downsample_cache(
            downsample_cache,
            &fields.call_1,
            &fields.call_2,
            &fields.grid4,
            entry.dt0,
            entry.f0 + offset,
            xbase,
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

#[cfg(test)]
mod tests {
    use super::{
        ft8_a7_save_entry_from_parts, is_reference_chkcall, jseq_from_nutc, split77_words,
        A7SaveEntry, FixedMsg37, StreamDecodeConfig, StreamDecodeSession,
    };
    use crate::stream::time::SlotTimestamp;

    #[test]
    fn jseq_matches_nutc_parity() {
        assert_eq!(jseq_from_nutc(140300), 0);
        assert_eq!(jseq_from_nutc(140315), 1);
        assert_eq!(jseq_from_nutc(140330), 0);
        assert_eq!(jseq_from_nutc(140345), 1);
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
        session.ft8_a7_new_slot(Some(&even_timestamp));
        assert_eq!(session.jseq, 0);
        assert_eq!(session.a7[0][0].len(), 1);
        assert!(session.a7[0][1].is_empty());
        assert_eq!(session.a7[1][0].len(), 0);
        assert_eq!(session.a7[1][1].len(), 1);

        let odd_timestamp = SlotTimestamp::parse("230208_140345").unwrap();
        session.ft8_a7_new_slot(Some(&odd_timestamp));
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
