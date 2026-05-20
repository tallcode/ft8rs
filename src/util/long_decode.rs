/// Long decode utility – progressive FT8 decoding.
///
/// Implements WSJT-X-style progressive decoding:
///  - Stage 1 (early):  First 11s, decode strong signals (syncmin=1.95)
///  - Stage 2:          Subtract strong signals from full data
///  - Stage 3 (final):  Full 15s decode on cleaned residual (syncmin=1.3)
use std::rc::Rc;

use crate::decode_ft8;
use crate::DecodeFT8Options;
use crate::util::hashcall::HashCallBook;
use crate::util::subtract_ft8::subtract_ft8;

const SEGMENT_DURATION: f64 = 15.0;
const DECODE_SAMPLE_RATE: u32 = 12000;
const SAMPLE_RATE_INTERNAL: usize = 12000;

/// Configuration for progressive long decode.
#[derive(Clone)]
pub struct LongDecodeConfig {
    pub freq_low: f64,
    pub freq_high: f64,
    pub sync_min: f64,
    pub max_candidates: usize,
    pub depth: usize,
    /// Enable progressive decoding (3-stage: early→subtract→final)
    pub progressive: bool,
    /// Enable data smoothing for Amplitude cycle
    pub smoothing: bool,
    /// Enable cross-segment signal association
    pub cross_segment_memory: bool,
    pub mycall: Option<String>,
    pub hiscall: Option<String>,
}

impl Default for LongDecodeConfig {
    fn default() -> Self {
        Self {
            freq_low: 200.0,
            freq_high: 3000.0,
            sync_min: 1.3,
            max_candidates: 500,
            depth: 3,
            progressive: true,
            smoothing: false,
            cross_segment_memory: true,
            mycall: None,
            hiscall: None,
        }
    }
}

/// A previously decoded signal, saved for cross-segment association.
#[derive(Clone, Debug)]
pub struct SavedSignal {
    pub freq: f64,
    pub dt: f64,
    pub msg: String,
    pub snr: f64,
}

/// Per-segment result.
#[derive(Clone, Debug)]
pub struct SegmentResult {
    pub segment: usize,
    pub start_s: f64,
    pub decoded: Vec<String>,
    pub freq: Vec<f64>,
    pub dt: Vec<f64>,
    pub snr: Vec<f64>,
    pub elapsed_ms: u64,
}

/// Full long decode result.
pub struct LongDecodeResult {
    pub segments: Vec<SegmentResult>,
    pub total_elapsed_ms: u64,
}

/// Perform progressive long decode on a multi-segment recording.
pub fn long_decode(
    samples: &[f32],
    sample_rate: u32,
    config: &LongDecodeConfig,
) -> LongDecodeResult {
    let t0 = std::time::Instant::now();
    let dur = samples.len() as f64 / sample_rate as f64;
    let n_segments = (dur / SEGMENT_DURATION).floor() as usize;

    let s12k: Vec<f32> = if sample_rate == DECODE_SAMPLE_RATE {
        samples.to_vec()
    } else {
        resample_f32(samples, sample_rate as usize, DECODE_SAMPLE_RATE as usize)
    };

    let sps = SEGMENT_DURATION as usize * SAMPLE_RATE_INTERNAL;
    let book = Rc::new(HashCallBook::new());

    let mut all_results = Vec::with_capacity(n_segments);
    let mut signal_memory: Vec<SavedSignal> = Vec::new();

    for seg in 0..n_segments {
        let seg_start =
            (seg as isize * sps as isize - SAMPLE_RATE_INTERNAL as isize).max(0) as usize;
        let seg_end = ((seg + 1) as isize * sps as isize + SAMPLE_RATE_INTERNAL as isize)
            .min(s12k.len() as isize) as usize;
        let data = &s12k[seg_start..seg_end];
        if data.len() < SAMPLE_RATE_INTERNAL * 10 {
            continue;
        }

        let seg_t0 = std::time::Instant::now();
        let mut seg_msgs: Vec<String> = Vec::new();
        let mut seg_freqs: Vec<f64> = Vec::new();
        let mut seg_dts: Vec<f64> = Vec::new();
        let mut seg_snrs: Vec<f64> = Vec::new();

        if config.progressive {
            progressive_decode(
                data,
                DECODE_SAMPLE_RATE,
                config,
                &book,
                &mut seg_msgs,
                &mut seg_freqs,
                &mut seg_dts,
                &mut seg_snrs,
            );
        } else {
            fallback_decode(data, DECODE_SAMPLE_RATE, config, &book,
                &mut seg_msgs, &mut seg_freqs, &mut seg_dts, &mut seg_snrs);
        }

        for msg in &seg_msgs {
            extract_callsigns(msg, &book);
        }

        if config.cross_segment_memory {
            for i in 0..seg_msgs.len() {
                signal_memory.push(SavedSignal {
                    freq: seg_freqs[i],
                    dt: seg_dts[i],
                    msg: seg_msgs[i].clone(),
                    snr: seg_snrs[i],
                });
            }
        }
        if signal_memory.len() > 200 {
            signal_memory.drain(0..signal_memory.len() - 200);
        }

        let elapsed = seg_t0.elapsed();
        all_results.push(SegmentResult {
            segment: seg,
            start_s: seg as f64 * SEGMENT_DURATION,
            decoded: seg_msgs,
            freq: seg_freqs,
            dt: seg_dts,
            snr: seg_snrs,
            elapsed_ms: elapsed.as_millis() as u64,
        });
    }

    LongDecodeResult {
        segments: all_results,
        total_elapsed_ms: t0.elapsed().as_millis() as u64,
    }
}

/// WSJT-X style progressive decode:
/// Stage 1: Early decode (first 11s) with high syncmin → find strong signals
/// Stage 2: Subtract strong signals from full data
/// Stage 3: Final decode on cleaned residual with normal syncmin
fn progressive_decode(
    data: &[f32],
    sr: u32,
    config: &LongDecodeConfig,
    book: &Rc<HashCallBook>,
    msgs: &mut Vec<String>,
    freqs: &mut Vec<f64>,
    dts: &mut Vec<f64>,
    snrs: &mut Vec<f64>,
) {
    let n_early = 11.0 * sr as f64; // First 11 seconds
    let early_len = n_early.min(data.len() as f64) as usize;
    if early_len < sr as usize * 8 {
        fallback_decode(data, sr, config, book, msgs, freqs, dts, snrs);
        return;
    }

    let early_data = &data[..early_len];

    // ── Stage 1: Early decode (high syncmin to find strong signals) ──
    let bk = Rc::clone(book);
    let early_decoded = decode_ft8(early_data, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some(config.sync_min * 1.5), // Higher threshold
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: Some(bk),
        mycall: config.mycall.clone(),
        hiscall: config.hiscall.clone(),
        sync_mode: Some(crate::ft8::decode::SyncMode::Power),
    });

    // ── Stage 2: Subtract strong signals ──
    let mut residual: Vec<f64> = data.iter().map(|&x| x as f64).collect();

    for sig in &early_decoded {
        let itone_arr: [i32; 79] = {
            let mut arr = [0i32; 79];
            arr.copy_from_slice(&sig.itone[..79]);
            arr
        };
        subtract_ft8(&mut residual, &itone_arr, sig.freq, sig.dt);
    }

    // ── Stage 3: Final decode on cleaned residual ──
    let residual_f32: Vec<f32> = residual.iter().map(|&x| x as f32).collect();
    let bk = Rc::clone(book);
    let final_decoded = decode_ft8(&residual_f32, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some(config.sync_min),
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: Some(bk),
        mycall: config.mycall.clone(),
        hiscall: config.hiscall.clone(),
        sync_mode: Some(crate::ft8::decode::SyncMode::Amplitude),
    });

    // Merge: early strong signals + final weak signals
    for d in &early_decoded {
        if !msgs.contains(&d.msg) {
            msgs.push(d.msg.clone());
            freqs.push(d.freq);
            dts.push(d.dt);
            snrs.push(d.snr);
        }
    }
    for d in &final_decoded {
        if !msgs.contains(&d.msg) {
            msgs.push(d.msg.clone());
            freqs.push(d.freq);
            dts.push(d.dt);
            snrs.push(d.snr);
        }
    }
}

fn fallback_decode(
    data: &[f32],
    sr: u32,
    config: &LongDecodeConfig,
    book: &Rc<HashCallBook>,
    msgs: &mut Vec<String>,
    freqs: &mut Vec<f64>,
    dts: &mut Vec<f64>,
    snrs: &mut Vec<f64>,
) {
    let bk = Rc::clone(book);
    let decoded = decode_ft8(data, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some(config.sync_min),
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: Some(bk),
        mycall: config.mycall.clone(),
        hiscall: config.hiscall.clone(),
        sync_mode: Some(crate::ft8::decode::SyncMode::Power),
    });
    for d in &decoded {
        msgs.push(d.msg.clone());
        freqs.push(d.freq);
        dts.push(d.dt);
        snrs.push(d.snr);
    }
}

fn resample_f32(input: &[f32], from_rate: usize, to_rate: usize) -> Vec<f32> {
    if from_rate == to_rate {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_idx = i as f64 * ratio;
        let src_floor = src_idx as usize;
        let src_ceil = (src_floor + 1).min(input.len() - 1);
        let frac = src_idx - src_floor as f64;
        let val = input[src_floor] as f64 * (1.0 - frac) + input[src_ceil] as f64 * frac;
        output.push(val as f32);
    }
    output
}

fn extract_callsigns(msg: &str, book: &Rc<HashCallBook>) {
    let parts: Vec<&str> = msg.split_whitespace().collect();
    for part in parts {
        if part.len() >= 3
            && part.chars().all(|c| c.is_alphanumeric() || c == '/' || c == '<' || c == '>')
            && part.chars().any(|c| c.is_numeric())
        {
            book.save(part);
        }
    }
}
