/// Core StreamDecoder — WSJT-X style streaming FT8 decode.
/// 
/// Architecture matches ft8_decode.f90:
/// 1. Audio accumulates in buffer, tracked by nzhsym
/// 2. nzhsym=41: Early decode (strong signals, syncmin=2.0)
/// 3. nzhsym=47: Subtract strong signals from residual
/// 4. nzhsym=50: Full decode on cleaned residual
/// 5. AP decode using previous slot results
/// 6. rotate_slot: current → previous for next slot's AP

use rayon::prelude::*;

use crate::stream::buffer::{AudioBuffer, DecodeStage};
use crate::stream::cross_slot::{CrossSlotMemory, SavedDecode};
use crate::stream::ft8b_stream::{ft8b_stream, Ft8bResult};
use crate::stream::subtract::subtract_signal;
use crate::stream::ap_decode::ap_decode;

use crate::ft8::decode::sync8;
use crate::ft8::decode::SyncMode;
use crate::ft8::decode::Candidate;

const SAMPLE_RATE: u32 = 12000;
const NFFT1_LONG: usize = 192000;

/// Configuration for stream decode.
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
            max_candidates: 600,
            depth: 3,
        }
    }
}

/// Decoded message output from stream decoder.
#[derive(Clone, Debug)]
pub struct StreamDecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
    pub itone: [i32; 79],
}

/// Main streaming FT8 decoder.
pub struct StreamDecoder {
    buffer: AudioBuffer,
    memory: CrossSlotMemory,
    config: StreamDecodeConfig,
    all_decoded: Vec<StreamDecodedMessage>,
    seen_messages: std::collections::HashSet<String>,
    /// Precomputed FFT of the full data (updated as audio arrives)
    cx_re: Vec<f64>,
    cx_im: Vec<f64>,
    /// Whether we've already processed each stage
    stage_early_done: bool,
    stage_subtract_done: bool,
    stage_full_done: bool,
    /// Strong signals found in early decode (for subtraction)
    strong_signals: Vec<(f64, f64, [i32; 79])>, // (freq, dt, itone)
}

impl StreamDecoder {
    pub fn new(config: StreamDecodeConfig) -> Self {
        Self {
            buffer: AudioBuffer::new(SAMPLE_RATE),
            memory: CrossSlotMemory::new(),
            config,
            all_decoded: Vec::new(),
            seen_messages: std::collections::HashSet::new(),
            cx_re: vec![0.0; NFFT1_LONG],
            cx_im: vec![0.0; NFFT1_LONG],
            stage_early_done: false,
            stage_subtract_done: false,
            stage_full_done: false,
            strong_signals: Vec::new(),
        }
    }

    /// Push audio samples (f32, 12kHz).
    pub fn push_audio(&mut self, chunk: &[f32]) {
        self.buffer.push(chunk);

        // Update FFT when we have enough data
        if self.buffer.len() > 0 {
            self.update_fft();
        }
    }

    /// Push f64 samples directly.
    pub fn push_audio_f64(&mut self, chunk: &[f64]) {
        self.buffer.push_f64(chunk);
        if self.buffer.len() > 0 {
            self.update_fft();
        }
    }

    /// Process accumulated audio based on current stage.
    /// Returns newly decoded messages.
    pub fn process(&mut self) -> Vec<StreamDecodedMessage> {
        let stage = self.buffer.stage();
        let mut new_decoded = Vec::new();

        match stage {
            DecodeStage::Early if !self.stage_early_done => {
                new_decoded = self.decode_early();
                self.stage_early_done = true;
            }
            DecodeStage::Subtract if !self.stage_subtract_done => {
                new_decoded = self.decode_subtract();
                self.stage_subtract_done = true;
            }
            DecodeStage::Full if !self.stage_full_done => {
                new_decoded = self.decode_full();
                self.stage_full_done = true;
            }
            _ => {}
        }

        self.all_decoded.extend(new_decoded.iter().cloned());
        new_decoded
    }

    /// Finish the current 15s slot. Returns ALL decoded messages.
    /// Prepares for next slot.
    pub fn finish_slot(&mut self) -> Vec<StreamDecodedMessage> {
        // Ensure we've processed all stages
        if self.buffer.stage() == DecodeStage::Full && !self.stage_full_done {
            self.decode_full();
            self.stage_full_done = true;
        }

        // AP decode using previous slot
        let ap_results = self.run_ap_decode();
        for result in ap_results {
            let msg = StreamDecodedMessage {
                freq: result.freq,
                dt: result.dt - 0.5,
                snr: result.snr,
                msg: result.msg,
                sync: result.sync,
                itone: result.itone,
            };
            self.add_decoded_message(&msg);
        }

        // Collect all messages
        let results = self.all_decoded.clone();

        // Rotate slot memory for next slot
        self.memory.rotate_slot();

        // Reset for next slot
        self.buffer.reset();
        self.cx_re.fill(0.0);
        self.cx_im.fill(0.0);
        self.all_decoded.clear();
        self.seen_messages.clear();
        self.stage_early_done = false;
        self.stage_subtract_done = false;
        self.stage_full_done = false;
        self.strong_signals.clear();

        results
    }

    /// Reset completely.
    pub fn reset(&mut self) {
        self.buffer.reset();
        self.memory.reset();
        self.all_decoded.clear();
        self.seen_messages.clear();
        self.cx_re.fill(0.0);
        self.cx_im.fill(0.0);
        self.stage_early_done = false;
        self.stage_subtract_done = false;
        self.stage_full_done = false;
        self.strong_signals.clear();
    }

    // ── Internal methods ──

    /// Early decode (nzhsym=41): find strong signals only.
    fn decode_early(&mut self) -> Vec<StreamDecodedMessage> {
        let samples = self.buffer.samples_f32();
        let syncmin = DecodeStage::Early.sync_min();

        let (candidates, sbase) = sync8(
            &samples.iter().map(|&x| x as f64).collect::<Vec<_>>(),
            self.config.freq_low,
            self.config.freq_high,
            syncmin,
            self.config.max_candidates,
            SyncMode::Power,
        );

        let results = self.decode_candidates_parallel(&candidates, &sbase);

        for result in &results {
            self.add_decoded_message(result);
            // Track strong signals for subtraction
            self.strong_signals.push((
                result.freq,
                result.dt,
                result.itone,
            ));
        }

        results
    }

    /// Subtract + decode (nzhsym=47): subtract strong signals, decode residual.
    fn decode_subtract(&mut self) -> Vec<StreamDecodedMessage> {
        let mut residual: Vec<f64> = self.buffer.samples().to_vec();

        // Subtract strong signals with DT refinement
        for (freq, dt, itone) in &self.strong_signals {
            subtract_signal(&mut residual, itone, *freq, *dt + 0.5, true);
        }

        // Decode on residual
        let syncmin = DecodeStage::Full.sync_min();
        let (candidates, sbase) = sync8(
            &residual,
            self.config.freq_low,
            self.config.freq_high,
            syncmin,
            self.config.max_candidates,
            SyncMode::Power,
        );

        let results = self.decode_candidates_parallel(&candidates, &sbase);

        for result in &results {
            self.add_decoded_message(result);
        }

        results
    }

    /// Full decode (nzhsym=50): complete decode on residual (Power + Amplitude).
    fn decode_full(&mut self) -> Vec<StreamDecodedMessage> {
        let mut residual: Vec<f64> = self.buffer.samples().to_vec();

        // Subtract any unsubtracted signals
        for (freq, dt, itone) in &self.strong_signals {
            subtract_signal(&mut residual, itone, *freq, *dt + 0.5, true);
        }

        let mut results = Vec::new();

        // Power mode
        let syncmin = self.config.sync_min;
        let (candidates_p, sbase_p) = sync8(
            &residual,
            self.config.freq_low,
            self.config.freq_high,
            syncmin,
            self.config.max_candidates,
            SyncMode::Power,
        );
        for result in self.decode_candidates_parallel(&candidates_p, &sbase_p) {
            results.push(result);
        }

        // Amplitude mode for weak signals
        let (candidates_a, sbase_a) = sync8(
            &residual,
            self.config.freq_low,
            self.config.freq_high,
            (syncmin * 0.85).max(0.8),
            self.config.max_candidates,
            SyncMode::Amplitude,
        );
        for result in self.decode_candidates_parallel(&candidates_a, &sbase_a) {
            results.push(result);
        }

        for result in &results {
            self.add_decoded_message(result);
        }

        results
    }

    /// Parallel candidate decoding — NO shared state, fully parallel.
    fn decode_candidates_parallel(
        &self,
        candidates: &[Candidate],
        sbase: &[f64],
    ) -> Vec<StreamDecodedMessage> {
        let dd_vec: Vec<f64> = self.buffer.samples().to_vec();
        let cx_re = self.cx_re.clone();
        let cx_im = self.cx_im.clone();
        let depth = self.config.depth;

        candidates.par_iter()
            .filter_map(|cand| {
                ft8b_stream(
                    &dd_vec,
                    &cx_re,
                    &cx_im,
                    cand.freq,
                    cand.dt,
                    sbase,
                    depth,
                    cand.sync,
                )
                .map(|r| StreamDecodedMessage {
                    freq: r.freq,
                    dt: r.dt - 0.5,
                    snr: r.snr,
                    msg: r.msg,
                    sync: r.sync,
                    itone: r.itone,
                })
            })
            .collect()
    }

    /// AP decode using previous slot results.
    fn run_ap_decode(&self) -> Vec<Ft8bResult> {
        let prev = self.memory.get_previous_slot();
        if prev.is_empty() {
            return Vec::new();
        }

        let dd_vec: Vec<f64> = self.buffer.samples().to_vec();
        let cx_re = self.cx_re.clone();
        let cx_im = self.cx_im.clone();

        // Compute sbase for AP
        let sbase = self.compute_sbase(&dd_vec);

        ap_decode(
            &dd_vec,
            &cx_re,
            &cx_im,
            &sbase,
            prev,
            self.config.depth,
        )
    }

    fn compute_sbase(&self, dd: &[f64]) -> Vec<f64> {
        use crate::ft8::decode::compute_baseline;
        let mut savg = vec![0.0; 960];
        let df: f64 = 12000.0 / 4096.0;
        for i in 0..960 {
            savg[i] = 1e-20;
        }
        compute_baseline(&savg, 200.0, 3000.0, df, 960)
    }

    /// Add a decoded message, deduplicating.
    fn add_decoded_message(&mut self, msg: &StreamDecodedMessage) {
        let key = normalize_message(&msg.msg);
        if self.seen_messages.contains(&key) {
            return;
        }
        self.seen_messages.insert(key);
        self.all_decoded.push(msg.clone());

        // Also save to cross-slot memory
        self.memory.save(SavedDecode {
            freq: msg.freq,
            dt: msg.dt,
            msg: msg.msg.clone(),
            itone: msg.itone,
            snr: msg.snr,
            sync: msg.sync,
            subtracted: false,
        });
    }

    /// Update the precomputed FFT.
    fn update_fft(&mut self) {
        use crate::util::fft::fft_complex;

        let samples = self.buffer.samples();
        self.cx_re.fill(0.0);
        self.cx_im.fill(0.0);
        self.cx_re[..samples.len()].copy_from_slice(samples);
        fft_complex(&mut self.cx_re, &mut self.cx_im, false);
    }
}

fn normalize_message(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.trim().to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}
