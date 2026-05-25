use std::rc::Rc;

use crate::ft8::ap_decode::{ft8_a7d, ApDecodeResult};
use crate::ft8::decode::{decode_f64_with_sbase, DecodeOptions, DecodedMessage, SyncMode};
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
    pub sync_min: f64,
    pub max_candidates: usize,
    pub depth: usize,
}

impl Default for StreamDecodeConfig {
    fn default() -> Self {
        Self {
            freq_low: 200.0,
            freq_high: 3000.0,
            sync_min: 1.3,
            max_candidates: 1000,
            depth: 3,
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
        let (full_results, sbase) =
            decode_f64_with_sbase(&full_dd, self.decode_options(50, Rc::clone(&book)));

        // Build current a7 table entries before AP. WSJT-X ft8_a7_save uses
        // these current entries to suppress previous entries already accounted
        // for by a regular decode in this sequence.
        let all_regular: Vec<&DecodedMessage> =
            early_results.iter().chain(full_results.iter()).collect();
        let entries_to_save: Vec<SlotDecodeEntry> = all_regular
            .iter()
            .copied()
            .filter_map(|d| extract_slot_entry(d, &sbase))
            .collect();

        // ── Stage 4: AP decode using prev_slot entries of SAME parity ──
        let previous_entries = if self.jseq == 0 {
            &self.prev_even
        } else {
            &self.prev_odd
        };
        let ap_candidates = suppress_previous_a7_entries(previous_entries, &entries_to_save);
        let ap_results = if ap_candidates.is_empty() {
            Vec::new()
        } else {
            let mut ap_msgs: Vec<ApDecodeResult> = Vec::new();
            for entry in &ap_candidates {
                let result = ft8_a7d(
                    &raw,
                    &entry.call_1,
                    &entry.call_2,
                    &entry.grid4,
                    entry.dt,
                    entry.freq,
                    entry.xbase,
                );
                if let Some(r) = result {
                    let norm_r = normal(&r.msg);
                    if !ap_msgs.iter().any(|a| normal(&a.msg) == norm_r) {
                        eprintln!(
                            "[AP] HIT: msg='{}' freq={:.1}Hz dt={:.2}",
                            r.msg, r.freq, r.dt
                        );
                        ap_msgs.push(r);
                    }
                } else {
                    eprintln!(
                        "[AP] MISS: {} {} @ {:.1}Hz dt={:.2}",
                        entry.call_1, entry.call_2, entry.freq, entry.dt
                    );
                }
            }
            ap_msgs
        };

        // ── Stage 5: Merge early + full + AP results, dedup ──
        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::new();

        fn collect_book(book: &mut Rc<HashCallBook>, msg: &str) {
            for part in msg.split_whitespace() {
                let p = part.trim();
                if p.len() >= 3
                    && p.chars()
                        .all(|c| c.is_alphanumeric() || c == '/' || c == '<' || c == '>')
                    && p.chars().any(|c| c.is_numeric())
                {
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
            sync_min: Some(self.config.sync_min),
            depth: Some(self.config.depth),
            max_candidates: Some(self.config.max_candidates),
            hash_call_book: Some(book),
            mycall: None,
            hiscall: None,
            sync_mode: Some(SyncMode::Power),
            nzhsym: Some(nzhsym),
            ft8_ap: Some(nzhsym == 50),
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
    let parts: Vec<&str> = d.msg.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    // Skip messages with / or < (WSJT-X: if(index(msg,'/').ge.1 .or. index(msg,'<').ge.1) go to 999)
    if d.msg.contains('/') || d.msg.contains('<') {
        return None;
    }

    let (fragment, call_1, call_2) = if parts[0] == "CQ" && parts.len() >= 3 && parts[1].len() <= 2 {
        (
            format!("CQ {} {}", parts[1], parts[2]),
            "CQ".to_string(),
            parts[1].to_string(),
        )
    } else {
        (
            format!("{} {}", parts[0], parts[1]),
            parts[0].to_string(),
            parts[1].to_string(),
        )
    };

    // CQ_ special format — skip for AP
    if call_1.starts_with("CQ_") {
        return None;
    }

    // Extract grid4
    let mut grid4 = String::from("    ");
    if parts.len() >= 3 {
        let last = parts.last().unwrap();
        if is_grid4(last) {
            grid4 = last.to_string();
        }
    }

    // Compute xbase from sbase (matching WSJT-X: 10^(0.1*(sbase(nint(f1/3.125))-40.0)))
    let xbase = {
        let df = crate::util::sync8_df();
        let freq_bin = (d.freq / df).round() as usize;
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
        dt: d.dt,
        freq: d.freq,
        xbase,
    })
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

fn normal(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.trim().to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}
