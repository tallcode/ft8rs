use std::rc::Rc;

use crate::ft8::ap_decode::{ft8_a7d, ApDecodeResult};
use crate::ft8::decode::{
    decode_f64_with_sbase, decode_f64_with_sbase_and_residual, DecodeOptions, DecodedMessage,
    SyncMode,
};
use crate::util::hashcall::HashCallBook;
use crate::util::subtract_ft8::subtract_ft8_refined;

const SAMPLE_RATE: u32 = 12000;
const NMAX: usize = 15 * 12_000;
const NZHSYM_STRIDE: usize = 3456;

/// Info saved from a slot decode for AP decode in the next slot.
/// Matches WSJT-X ft8_a7_save: dt0, f0, msg0("call_1 call_2")
#[derive(Clone, Debug)]
struct SlotDecodeEntry {
    fragment: String,
    call_1: String,
    call_2: String,
    grid4: String, // "    " or 4-char grid
    dt: f64,       // WSJT-X convention: dt = candidate_dt - 0.5
    freq: f64,
    xbase: f64, // noise baseline at this frequency (from sbase)
}

/// WSJT-X uses jseq = mod(utc/5, 2) to alternate even/odd sequences.
/// AP decode only uses entries from the same parity (even→even, odd→odd).
/// We simulate this by toggling jseq on each decode_slot call.
#[derive(Clone)]
pub struct StreamDecodeConfig {
    pub freq_low: f64,
    pub freq_high: f64,
    pub sync_min: Option<f64>,
    pub max_candidates: usize,
    pub depth: usize,
    pub nfqso: f64,
    pub nftx: f64,
    pub nqso_progress: usize,
    pub ncontest: usize,
    pub napwid: f64,
    pub ft8_ap: bool,
    pub ap_cq_only: bool,
    pub nagain: bool,
    pub mycall: Option<String>,
    pub hiscall: Option<String>,
}

impl Default for StreamDecodeConfig {
    fn default() -> Self {
        Self {
            freq_low: 200.0,
            freq_high: 3000.0,
            sync_min: None,
            max_candidates: 1000,
            depth: 3,
            nfqso: 0.0,
            nftx: 0.0,
            nqso_progress: 0,
            ncontest: 0,
            napwid: 50.0,
            ft8_ap: true,
            ap_cq_only: false,
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

pub struct StreamDecoder {
    config: StreamDecodeConfig,
    book: Rc<HashCallBook>,
    /// Entries from the previous slot of the SAME parity for AP decode.
    /// Matches WSJT-X ndec(jseq,0).
    prev_even: Vec<SlotDecodeEntry>,
    prev_odd: Vec<SlotDecodeEntry>,
    /// Current sequence parity: 0=even, 1=odd (simulates jseq = mod(utc/5, 2))
    jseq: usize,
}

impl StreamDecoder {
    pub fn new(config: StreamDecodeConfig) -> Self {
        Self {
            config,
            book: Rc::new(HashCallBook::new()),
            prev_even: Vec::new(),
            prev_odd: Vec::new(),
            jseq: 0,
        }
    }

    pub fn decode_slot(&mut self, samples: &[f32]) -> Vec<StreamDecodedMessage> {
        let results = self.progressive_decode_slot(samples);
        // Toggle parity for next slot (simulates UTC progression mod 5)
        self.jseq = 1 - self.jseq;
        results
    }

    /// Full progressive decode matching WSJT-X flow:
    /// 1. nzhsym=41: early decode on a zero-padded partial buffer.
    /// 2. nzhsym=47: subtract early decodes from the partial buffer and save dd1.
    /// 3. nzhsym=50: decode full buffer with early-cleaned prefix.
    /// 4. AP decode using prev_slot entries of SAME parity (ft8_a7d).
    /// 5. Save current slot entries for next same-parity slot.
    fn progressive_decode_slot(&mut self, samples: &[f32]) -> Vec<StreamDecodedMessage> {
        let raw = samples_to_window(samples);
        let book = Rc::clone(&self.book);

        // ── Stage 1: nzhsym=41 early decode ──
        let early_dd = partial_window(&raw, 41);
        let (early_results, _) =
            decode_f64_with_sbase(&early_dd, self.decode_options(41, Rc::clone(&book)));

        // ── Stage 2: nzhsym=47 early subtraction only ──
        let mut dd1 = partial_window(&raw, 47);
        let mut early_subtracted = vec![false; early_results.len()];
        let lrefinedt = self.config.depth > 2;
        for (idx, d) in early_results.iter().enumerate() {
            if d.dt < 0.396 {
                let mut itone = [0i32; 79];
                itone.copy_from_slice(&d.itone[..79]);
                subtract_ft8_refined(&mut dd1, &itone, d.freq, d.dt + 0.5, lrefinedt);
                early_subtracted[idx] = true;
            }
        }

        // ── Stage 3: nzhsym=50 full decode with early-cleaned prefix ──
        let mut full_dd = raw.clone();
        let clean_prefix = (47 * NZHSYM_STRIDE).min(NMAX);
        full_dd[..clean_prefix].copy_from_slice(&dd1[..clean_prefix]);
        for (idx, d) in early_results.iter().enumerate() {
            if !early_subtracted[idx] {
                let mut itone = [0i32; 79];
                itone.copy_from_slice(&d.itone[..79]);
                subtract_ft8_refined(&mut full_dd, &itone, d.freq, d.dt + 0.5, true);
            }
        }
        let (full_results, sbase, full_residual) =
            decode_f64_with_sbase_and_residual(&full_dd, self.decode_options(50, Rc::clone(&book)));

        // Build current a7 table entries before AP. WSJT-X ft8_a7_save uses
        // these current entries to suppress previous entries already accounted
        // for by a regular decode in this sequence.
        let all_regular: Vec<&DecodedMessage> =
            early_results.iter().chain(full_results.iter()).collect();
        let mut entries_to_save: Vec<SlotDecodeEntry> = all_regular
            .iter()
            .copied()
            .filter_map(|d| extract_slot_entry(d, &sbase))
            .collect();
        for entry in &entries_to_save {
            trace_ap_memory("SAVE", self.jseq, entry);
        }

        // ── Stage 4: AP decode using prev_slot entries of SAME parity ──
        let previous_entries = if self.jseq == 0 {
            &self.prev_even
        } else {
            &self.prev_odd
        };
        for entry in previous_entries {
            trace_ap_memory("PREV", self.jseq, entry);
        }
        let ap_candidates = suppress_previous_a7_entries(previous_entries, &entries_to_save);
        for entry in &ap_candidates {
            trace_ap_memory("CAND", self.jseq, entry);
        }
        let ap_results = if ap_candidates.is_empty() {
            Vec::new()
        } else {
            let mut ap_msgs: Vec<ApDecodeResult> = Vec::new();
            for entry in &ap_candidates {
                trace_ap_entry(self.jseq, entry);
                let result = ft8_a7d(
                    &full_residual,
                    &entry.call_1,
                    &entry.call_2,
                    &entry.grid4,
                    entry.dt,
                    entry.freq,
                    entry.xbase,
                );
                if let Some(r) = result {
                    trace_ap_result(self.jseq, entry, &r);
                    let norm_r = normal(&r.msg);
                    if !ap_msgs.iter().any(|a| normal(&a.msg) == norm_r) {
                        ap_msgs.push(r);
                    }
                }
            }
            ap_msgs
        };

        // ── Stage 5: Merge early + full + AP results, dedup ──
        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::new();

        fn collect_book(book: &mut Rc<HashCallBook>, msg: &str) {
            for part in msg.split_whitespace() {
                let p = part.trim_matches(|c: char| c == ';' || c == ',');
                if is_hashable_callsign_token(p) {
                    book.save(p);
                }
            }
        }

        for d in early_results.iter().chain(full_results.iter()) {
            let key = normal(&d.msg);
            if seen.insert(key) {
                let mut itone = [0i32; 79];
                itone.copy_from_slice(&d.itone[..79]);
                merged.push(StreamDecodedMessage {
                    freq: d.freq,
                    dt: d.dt,
                    snr: d.snr,
                    msg: d.msg.clone(),
                    sync: d.sync,
                    itone,
                });
                collect_book(&mut self.book, &d.msg);
            }
        }

        for r in &ap_results {
            if let Some(entry) = extract_slot_entry_from_parts(&r.msg, r.freq, r.dt, &sbase) {
                entries_to_save.push(entry);
            }
            let key = normal(&r.msg);
            if seen.insert(key) {
                merged.push(StreamDecodedMessage {
                    freq: r.freq,
                    dt: r.dt,
                    snr: r.snr,
                    msg: r.msg.clone(),
                    sync: 0.0,
                    itone: [0i32; 79],
                });
                collect_book(&mut self.book, &r.msg);
            }
        }

        // ── Stage 6: Save current slot entries for next same-parity slot ──
        // Matches WSJT-X: ndec(jseq,1) → ndec(jseq,0) at next UTC change
        if self.jseq == 0 {
            self.prev_even = entries_to_save;
        } else {
            self.prev_odd = entries_to_save;
        }

        merged
    }

    fn decode_options(&self, nzhsym: usize, book: Rc<HashCallBook>) -> DecodeOptions {
        DecodeOptions {
            sample_rate: Some(SAMPLE_RATE as usize),
            freq_low: Some(self.config.freq_low),
            freq_high: Some(self.config.freq_high),
            sync_min: self.config.sync_min,
            depth: Some(self.config.depth),
            max_candidates: Some(self.config.max_candidates),
            hash_call_book: Some(book),
            mycall: self.config.mycall.clone(),
            hiscall: self.config.hiscall.clone(),
            nfqso: Some(self.config.nfqso),
            nftx: Some(self.config.nftx),
            nqso_progress: Some(self.config.nqso_progress),
            ncontest: Some(self.config.ncontest),
            napwid: Some(self.config.napwid),
            ft8_ap: Some(self.config.ft8_ap && nzhsym == 50),
            ap_cq_only: Some(self.config.ap_cq_only),
            nagain: Some(self.config.nagain),
            sync_mode: Some(SyncMode::Power),
            nzhsym: Some(nzhsym),
            ..Default::default()
        }
    }
}

fn samples_to_window(samples: &[f32]) -> Vec<f64> {
    let mut out = vec![0.0; NMAX];
    let len = samples.len().min(NMAX);
    for i in 0..len {
        out[i] = samples[i] as f64;
    }
    out
}

fn partial_window(samples: &[f64], nzhsym: usize) -> Vec<f64> {
    let mut out = vec![0.0; NMAX];
    let n = (nzhsym * NZHSYM_STRIDE).min(NMAX).min(samples.len());
    out[..n].copy_from_slice(&samples[..n]);
    out
}

/// Extract call_1/call_2/grid4/xbase from a decoded message.
/// Matches WSJT-X ft8_a7_save logic.
fn extract_slot_entry(d: &DecodedMessage, sbase: &[f64]) -> Option<SlotDecodeEntry> {
    extract_slot_entry_from_parts(&d.msg, d.freq, d.dt, sbase)
}

fn extract_slot_entry_from_parts(
    msg: &str,
    freq: f64,
    dt: f64,
    sbase: &[f64],
) -> Option<SlotDecodeEntry> {
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
        let df = crate::util::sync8_df();
        let freq_bin = (freq / df).round() as usize;
        if freq_bin < sbase.len() && freq_bin > 0 {
            10.0_f64.powf(0.1 * (sbase[freq_bin] - 40.0))
        } else {
            1.0 // fallback
        }
    };

    Some(SlotDecodeEntry {
        fragment,
        call_1,
        call_2,
        grid4,
        dt,
        freq,
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

fn suppress_previous_a7_entries(
    previous: &[SlotDecodeEntry],
    current: &[SlotDecodeEntry],
) -> Vec<SlotDecodeEntry> {
    previous
        .iter()
        .filter(|prev| {
            !current.iter().any(|cur| {
                (cur.freq - prev.freq).abs() <= 3.0
                    && prev.fragment.contains(&format!(" {}", cur.call_2.trim()))
            })
        })
        .cloned()
        .collect()
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

    let digit_pos = if bytes[1].is_ascii_digit() {
        Some(1usize)
    } else if bytes[2].is_ascii_digit() {
        Some(2usize)
    } else {
        None
    };
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

fn trace_ap_entry(jseq: usize, entry: &SlotDecodeEntry) {
    if !trace_entry_matches(entry) {
        return;
    }
    eprintln!(
        "[TRACE_AP_ENTRY] jseq={} fragment=\"{}\" call_1=\"{}\" call_2=\"{}\" grid4=\"{}\" dt={:.3} freq={:.3} xbase={:.6e}",
        jseq,
        entry.fragment,
        entry.call_1,
        entry.call_2,
        entry.grid4.trim(),
        entry.dt,
        entry.freq,
        entry.xbase
    );
}

fn trace_ap_result(jseq: usize, entry: &SlotDecodeEntry, result: &ApDecodeResult) {
    if !trace_entry_matches(entry) && !trace_message_matches(&result.msg, result.freq) {
        return;
    }
    eprintln!(
        "[TRACE_AP_RESULT] jseq={} source=\"{}\" freq={:.3} dt={:.3} snr={:.1} nhard={} msg=\"{}\"",
        jseq, entry.fragment, result.freq, result.dt, result.snr, result.nharderrors, result.msg
    );
}

fn trace_ap_memory(kind: &str, jseq: usize, entry: &SlotDecodeEntry) {
    if !trace_entry_matches(entry) {
        return;
    }
    eprintln!(
        "[TRACE_AP_{}] jseq={} fragment=\"{}\" call_1=\"{}\" call_2=\"{}\" grid4=\"{}\" dt={:.3} freq={:.3} xbase={:.6e}",
        kind,
        jseq,
        entry.fragment,
        entry.call_1,
        entry.call_2,
        entry.grid4.trim(),
        entry.dt,
        entry.freq,
        entry.xbase
    );
}

fn trace_entry_matches(entry: &SlotDecodeEntry) -> bool {
    let Ok(raw) = std::env::var("FT8RS_TRACE_TARGETS") else {
        return false;
    };
    let freq_tol = trace_freq_tol();
    let call_1 = entry.call_1.to_ascii_uppercase();
    let call_2 = entry.call_2.to_ascii_uppercase();
    for item in raw.split(';') {
        let mut parts = item.trim().splitn(3, ':');
        let Some(freq_raw) = parts.next() else {
            continue;
        };
        let Ok(freq) = freq_raw.trim().parse::<f64>() else {
            continue;
        };
        if (freq - entry.freq).abs() > freq_tol {
            continue;
        }
        let label = parts.nth(1).unwrap_or("").to_ascii_uppercase();
        if label.contains(&call_2) && (call_1 == "CQ" || label.contains(&call_1)) {
            return true;
        }
    }
    false
}

fn trace_message_matches(msg: &str, freq: f64) -> bool {
    let Ok(raw) = std::env::var("FT8RS_TRACE_TARGETS") else {
        return false;
    };
    let freq_tol = trace_freq_tol();
    let msg = normal(msg);
    for item in raw.split(';') {
        let mut parts = item.trim().splitn(3, ':');
        let Some(freq_raw) = parts.next() else {
            continue;
        };
        let Ok(target_freq) = freq_raw.trim().parse::<f64>() else {
            continue;
        };
        if (target_freq - freq).abs() > freq_tol {
            continue;
        }
        let label = parts.nth(1).unwrap_or("").to_ascii_uppercase();
        if !label.is_empty() && msg == normal(&label) {
            return true;
        }
    }
    false
}

fn trace_freq_tol() -> f64 {
    std::env::var("FT8RS_TRACE_FREQ_TOL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(8.0)
}

#[cfg(test)]
mod tests {
    use super::{is_wsjtx_chkcall, split77_words};

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
}
