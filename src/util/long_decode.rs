/// Long decode utility – WSJT-X style progressive FT8 decoding.
///
/// Architecture (matching WSJT-X ft8_decode.f90):
///  - Stage 1 (nzhsym~41): First 11s, decode strong signals (syncmin=2.0)
///  - Stage 2 (nzhsym~47): Subtract strong signals with lrefinedt dt refinement
///  - Stage 3 (nzhsym=50): Full 15s decode on cleaned residual (syncmin=1.3)
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::decode_ft8;
use crate::DecodeFT8Options;
use crate::util::hashcall::HashCallBook;
use crate::util::subtract_ft8::subtract_ft8_refined;

const SEGMENT_DURATION: f64 = 15.0;
const DECODE_SAMPLE_RATE: u32 = 12000;
const SAMPLE_RATE_INTERNAL: usize = 12000;

/// Timeout: if 60s elapsed and <30% of segments complete, abort.
/// Also: total timeout of 300s (5 minutes).
const WATCHDOG_TIMEOUT_MS: u64 = 300_000;
const EARLY_ABORT_MS: u64 = 60_000;
const EARLY_ABORT_THRESHOLD: f64 = 0.30;

/// Configuration for progressive long decode.
#[derive(Clone)]
pub struct LongDecodeConfig {
    pub freq_low: f64,
    pub freq_high: f64,
    pub sync_min: f64,
    pub max_candidates: usize,
    pub depth: usize,
    pub progressive: bool,
    pub smoothing: bool,
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

#[derive(Clone, Debug)]
pub struct SavedSignal {
    pub freq: f64,
    pub dt: f64,
    pub msg: String,
    pub snr: f64,
}

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

pub struct LongDecodeResult {
    pub segments: Vec<SegmentResult>,
    pub total_elapsed_ms: u64,
}

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

    // Shared global HashCallBook for AP across segments
    let global_book = HashCallBook::new();

    // Watchdog: abort if <30% complete after 60s, or total > 300s
    let completed_count = Arc::new(AtomicUsize::new(0));
    let should_abort = Arc::new(AtomicBool::new(false));
    let total_segments = n_segments;

    let wd_completed = Arc::clone(&completed_count);
    let wd_abort = Arc::clone(&should_abort);
    let watchdog_thread = std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            if wd_abort.load(Ordering::Relaxed) {
                break;
            }
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let completed = wd_completed.load(Ordering::Relaxed);
            let threshold = (total_segments as f64 * EARLY_ABORT_THRESHOLD) as usize;

            if elapsed_ms > WATCHDOG_TIMEOUT_MS {
                eprintln!("[WATCHDOG] Total timeout exceeded ({}s > {}s). Aborting.",
                    elapsed_ms / 1000, WATCHDOG_TIMEOUT_MS / 1000);
                wd_abort.store(true, Ordering::Relaxed);
                break;
            }
            if elapsed_ms > EARLY_ABORT_MS && completed < threshold {
                eprintln!("[WATCHDOG] Early abort: {}/{} segments completed ({}%) in {}s (< {}% required).",
                    completed, total_segments, (completed as f64 / total_segments as f64 * 100.0),
                    elapsed_ms / 1000, (EARLY_ABORT_THRESHOLD * 100.0) as u64);
                wd_abort.store(true, Ordering::Relaxed);
                break;
            }
        }
    });

    // Process segments SEQUENTIALLY (matching WSJT-X real-time streaming)
    let mut all_results: Vec<SegmentResult> = Vec::with_capacity(n_segments);

    for seg in 0..n_segments {
        if should_abort.load(Ordering::Relaxed) {
            break;
        }

        let seg_t0 = std::time::Instant::now();
        let seg_start =
            (seg as isize * sps as isize - SAMPLE_RATE_INTERNAL as isize).max(0) as usize;
        let seg_end = ((seg + 1) as isize * sps as isize + SAMPLE_RATE_INTERNAL as isize)
            .min(s12k.len() as isize) as usize;
        let data = &s12k[seg_start..seg_end];
        if data.len() < SAMPLE_RATE_INTERNAL * 10 {
            continue;
        }

        let mut seg_msgs: Vec<String> = Vec::new();
        let mut seg_freqs: Vec<f64> = Vec::new();
        let mut seg_dts: Vec<f64> = Vec::new();
        let mut seg_snrs: Vec<f64> = Vec::new();

        if config.progressive {
            progressive_decode(
                data,
                DECODE_SAMPLE_RATE,
                config,
                &global_book,
                &mut seg_msgs,
                &mut seg_freqs,
                &mut seg_dts,
                &mut seg_snrs,
            );
        } else {
            fallback_decode(data, DECODE_SAMPLE_RATE, config,
                &mut seg_msgs, &mut seg_freqs, &mut seg_dts, &mut seg_snrs);
        }

        completed_count.fetch_add(1, Ordering::Relaxed);

        // Extract callsigns from decoded messages into global book for future segments' AP
        for msg in &seg_msgs {
            extract_callsigns(msg, &global_book);
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

    should_abort.store(true, Ordering::Relaxed);
    let _ = watchdog_thread.join();

    LongDecodeResult {
        segments: all_results,
        total_elapsed_ms: t0.elapsed().as_millis() as u64,
    }
}

/// WSJT-X style progressive decode:
/// Stage 1: Early decode (first 11s) with high syncmin → find strong signals
/// Stage 2: Subtract strong signals with lrefinedt dt refinement
/// Stage 3,4: Final decode on cleaned residual with multi-cycle (Power + Amplitude)
fn progressive_decode(
    data: &[f32],
    sr: u32,
    config: &LongDecodeConfig,
    _global_book: &HashCallBook,
    msgs: &mut Vec<String>,
    freqs: &mut Vec<f64>,
    dts: &mut Vec<f64>,
    snrs: &mut Vec<f64>,
) {
    // Stage 1: Early decode – first 11s (nzhsym≈41 style)
    let n_early = 11.0 * sr as f64;
    let early_len = n_early.min(data.len() as f64) as usize;
    if early_len < sr as usize * 8 {
        fallback_decode(data, sr, config, msgs, freqs, dts, snrs);
        return;
    }
    let early_data = &data[..early_len];

    // Early decode with high syncmin to find only strong signals
    // hash_call_book: None enables parallel candidate decoding via rayon
    let early_decoded = decode_ft8(early_data, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some(2.0), // WSJT-X uses syncmin=2.0 for nzhsym=41
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: None, // Enable parallel candidate decoding
        mycall: config.mycall.clone(),
        hiscall: config.hiscall.clone(),
        sync_mode: Some(crate::ft8::decode::SyncMode::Power),
    });

    // Stage 2: Subtract strong signals with lrefinedt dt refinement (WSJT-X nzhsym=47 style)
    let mut residual: Vec<f64> = data.iter().map(|&x| x as f64).collect();
    for sig in &early_decoded {
        let itone_arr: [i32; 79] = {
            let mut arr = [0i32; 79];
            arr.copy_from_slice(&sig.itone[..79]);
            arr
        };
        // Use lrefinedt=true for precise dt refinement during subtraction
        subtract_ft8_refined(&mut residual, &itone_arr, sig.freq, sig.dt + 0.5, true);
    }

    // Stage 3: Final decode on cleaned residual – Power mode first
    // hash_call_book: None enables parallel candidate decoding
    let residual_f32: Vec<f32> = residual.iter().map(|&x| x as f32).collect();
    let final_decoded_power = decode_ft8(&residual_f32, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some(config.sync_min),
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: None, // Enable parallel candidate decoding
        mycall: config.mycall.clone(),
        hiscall: config.hiscall.clone(),
        sync_mode: Some(crate::ft8::decode::SyncMode::Power),
    });

    // Stage 4: Amplitude mode on residual for weak signals (JTDX-style)
    // hash_call_book: None enables parallel candidate decoding
    let final_decoded_amp = decode_ft8(&residual_f32, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some((config.sync_min * 0.85).max(0.8)),
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: None, // Enable parallel candidate decoding
        mycall: config.mycall.clone(),
        hiscall: config.hiscall.clone(),
        sync_mode: Some(crate::ft8::decode::SyncMode::Amplitude),
    });

    // Merge all results
    for d in &early_decoded {
        if !msgs.contains(&d.msg) {
            msgs.push(d.msg.clone());
            freqs.push(d.freq);
            dts.push(d.dt);
            snrs.push(d.snr);
        }
    }
    for d in &final_decoded_power {
        if !msgs.contains(&d.msg) {
            msgs.push(d.msg.clone());
            freqs.push(d.freq);
            dts.push(d.dt);
            snrs.push(d.snr);
        }
    }
    for d in &final_decoded_amp {
        if !msgs.contains(&d.msg) {
            msgs.push(d.msg.clone());
            freqs.push(d.freq);
            dts.push(d.dt);
            snrs.push(d.snr);
        }
    }
}

fn fallback_decode_with_book(
    data: &[f32],
    sr: u32,
    config: &LongDecodeConfig,
    msgs: &mut Vec<String>,
    freqs: &mut Vec<f64>,
    dts: &mut Vec<f64>,
    snrs: &mut Vec<f64>,
) {
    // hash_call_book: None enables parallel candidate decoding via rayon
    let decoded = decode_ft8(data, DecodeFT8Options {
        sample_rate: Some(sr as usize),
        freq_low: Some(config.freq_low),
        freq_high: Some(config.freq_high),
        sync_min: Some(config.sync_min),
        depth: Some(config.depth),
        max_candidates: Some(config.max_candidates),
        hash_call_book: None, // Enable parallel candidate decoding
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

fn fallback_decode(
    data: &[f32],
    sr: u32,
    config: &LongDecodeConfig,
    msgs: &mut Vec<String>,
    freqs: &mut Vec<f64>,
    dts: &mut Vec<f64>,
    snrs: &mut Vec<f64>,
) {
    fallback_decode_with_book(data, sr, config, msgs, freqs, dts, snrs);
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

fn extract_callsigns(msg: &str, book: &HashCallBook) {
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
