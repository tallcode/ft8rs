use std::rc::Rc;
/// FT8 decoder – Rust port of decode.ts

use crate::ft8::constants::{COSTAS, GRAY_MAP};
use crate::util::constants::{N_LDPC, SAMPLE_RATE};
use crate::util::decode174_91::{decode174_91, DecodeResult};
use crate::util::fft::{fft_complex, next_pow2};
use crate::util::hashcall::HashCallBook;
use crate::util::unpack_jt77::unpack77;

/// sync8 spectral mode – different representations favour different SNR regimes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncMode {
    /// Power spectrum: s = Re² + Im² (best for strong signals)
    Power = 0,
    /// Amplitude spectrum: s = sqrt(Re² + Im²) (better for weak signals, compresses dynamic range)
    Amplitude = 1,
    /// Absolute sum: s = |Re| + |Im| (most robust against impulsive noise)
    AbsSum = 2,
}

const NSPS: usize = 1920;
const NFFT1: usize = 2 * NSPS; // 3840
const NSTEP: usize = NSPS / 4; // 480
const NMAX: usize = 15 * 12_000; // 180000
const NHSYM: usize = NMAX / NSTEP - 3; // 372
const NDOWN: usize = 60;
const NN: usize = 79;

const NFFT1_LONG: usize = 192000;
const SYNC8_DF: f64 = SAMPLE_RATE as f64 / 4096.0; // 12000/4096 = 2.93 Hz/bin
const NFFT2: usize = 3200;
const NP2: usize = 2812;
const COSTAS_BLOCKS: usize = 7;
const COSTAS_SYMBOL_LEN: usize = 32;
const TAPER_SIZE: usize = 101;
const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
const MAX_DECODE_PASSES_DEPTH3: usize = 4;

const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
const DT2: f64 = 1.0 / FS2;
const DOWNSAMPLE_DF: f64 = SAMPLE_RATE as f64 / NFFT1_LONG as f64;
const DOWNSAMPLE_BAUD: f64 = SAMPLE_RATE as f64 / NSPS as f64;
const DOWNSAMPLE_SCALE: f64 = 0.12909944487358055; // sqrt(3200/192000)

#[derive(Clone)]
pub struct DecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
}

#[derive(Default)]
pub struct DecodeOptions {
    pub sample_rate: Option<usize>,
    pub freq_low: Option<f64>,
    pub freq_high: Option<f64>,
    pub sync_min: Option<f64>,
    pub depth: Option<usize>,
    pub max_candidates: Option<usize>,
    pub hash_call_book: Option<Rc<HashCallBook>>,
    pub mycall: Option<String>,
    pub hiscall: Option<String>,
    /// Sync spectral mode: Power (default), Amplitude (better for weak signals),
    /// AbsSum (robust against impulsive noise).
    pub sync_mode: Option<SyncMode>,
}


#[derive(Clone)]
struct Candidate {
    freq: f64,
    dt: f64,
    sync: f64,
}

struct Ft8bResult {
    msg: String,
    freq: f64,
    dt: f64,
    snr: f64,
    itone: [i32; 79],
}

struct SyncTemplate {
    re: Vec<f64>,
    im: Vec<f64>,
}

struct FrequencyShiftSyncTemplate {
    delf: f64,
    re: Vec<f64>,
    im: Vec<f64>,
}

struct DecodeWorkspace {
    cd0_re: Vec<f64>,
    cd0_im: Vec<f64>,
    shift_re: Vec<f64>,
    shift_im: Vec<f64>,
    s8: Vec<f64>,
    cs_re: Vec<f64>,
    cs_im: Vec<f64>,
    symb_re: Vec<f64>,
    symb_im: Vec<f64>,
    s2: Vec<f64>,
    bmeta: Vec<f64>,
    bmetb: Vec<f64>,
    bmetc: Vec<f64>,
    bmetd: Vec<f64>,
    llr: Vec<f64>,
    apmask: Vec<i8>,
    ss: Vec<f64>,
}

fn create_decode_workspace() -> DecodeWorkspace {
    DecodeWorkspace {
        cd0_re: vec![0.0; NFFT2],
        cd0_im: vec![0.0; NFFT2],
        shift_re: vec![0.0; NFFT2],
        shift_im: vec![0.0; NFFT2],
        s8: vec![0.0; 8 * NN],
        cs_re: vec![0.0; 8 * NN],
        cs_im: vec![0.0; 8 * NN],
        symb_re: vec![0.0; COSTAS_SYMBOL_LEN],
        symb_im: vec![0.0; COSTAS_SYMBOL_LEN],
        s2: vec![0.0; 1 << 9],
        bmeta: vec![0.0; N_LDPC],
        bmetb: vec![0.0; N_LDPC],
        bmetc: vec![0.0; N_LDPC],
        bmetd: vec![0.0; N_LDPC],
        llr: vec![0.0; N_LDPC],
        apmask: vec![0; N_LDPC],
        ss: vec![0.0; 9],
    }
}

/// Lazy-initialized constants
fn build_taper() -> &'static Vec<f64> {
    static T: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![0.0; TAPER_SIZE];
        let last = TAPER_SIZE - 1;
        for i in 0..TAPER_SIZE {
            t[i] = 0.5 * (1.0 + ((i as f64 * std::f64::consts::PI) / last as f64).cos());
        }
        t
    })
}

fn build_costas_sync_templates() -> &'static SyncTemplate {
    static T: std::sync::OnceLock<SyncTemplate> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        let mut im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
        for i in 0..COSTAS_BLOCKS {
            let mut phi: f64 = 0.0;
            let dphi = (TWO_PI * COSTAS[i] as f64) / COSTAS_SYMBOL_LEN as f64;
            for j in 0..COSTAS_SYMBOL_LEN {
                re[i * COSTAS_SYMBOL_LEN + j] = phi.cos();
                im[i * COSTAS_SYMBOL_LEN + j] = phi.sin();
                phi = (phi + dphi) % TWO_PI;
            }
        }
        SyncTemplate { re, im }
    })
}

fn build_frequency_shift_sync_templates() -> &'static Vec<FrequencyShiftSyncTemplate> {
    static T: std::sync::OnceLock<Vec<FrequencyShiftSyncTemplate>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let cs = build_costas_sync_templates();
        let mut templates = Vec::new();
        for ifr in -5..=5 {
            let delf = ifr as f64 * 0.5;
            let dphi = TWO_PI * delf * DT2;
            let mut twk_re = vec![0.0; COSTAS_SYMBOL_LEN];
            let mut twk_im = vec![0.0; COSTAS_SYMBOL_LEN];
            let mut phi: f64 = 0.0;
            for j in 0..COSTAS_SYMBOL_LEN {
                twk_re[j] = phi.cos();
                twk_im[j] = phi.sin();
                phi = (phi + dphi) % TWO_PI;
            }
            let mut re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
            let mut im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
            for i in 0..COSTAS_BLOCKS {
                for j in 0..COSTAS_SYMBOL_LEN {
                    let idx = i * COSTAS_SYMBOL_LEN + j;
                    let cs_re = cs.re[idx];
                    let cs_im = cs.im[idx];
                    re[idx] = twk_re[j] * cs_re - twk_im[j] * cs_im;
                    im[idx] = twk_re[j] * cs_im + twk_im[j] * cs_re;
                }
            }
            templates.push(FrequencyShiftSyncTemplate { delf, re, im });
        }
        templates
    })
}

pub fn decode(samples: &[f32], options: DecodeOptions) -> Vec<DecodedMessage> {
    let sample_rate = options.sample_rate.unwrap_or(SAMPLE_RATE);
    let nfa = options.freq_low.unwrap_or(200.0);
    let nfb = options.freq_high.unwrap_or(3000.0);
    let syncmin = options.sync_min.unwrap_or(0.8);  // was 1.2; lowered to catch weaker signals
    let depth = options.depth.unwrap_or(2);
    let max_candidates = options.max_candidates.unwrap_or(300);
    let book = options.hash_call_book;
    let sync_mode = options.sync_mode.unwrap_or(SyncMode::Power);

    let dd = if sample_rate == SAMPLE_RATE {
        copy_samples_to_decode_window(samples)
    } else {
        resample(samples, sample_rate, SAMPLE_RATE, NMAX)
    };
    let mut residual = dd.clone();

    let mut cx_re = vec![0.0; NFFT1_LONG];
    let mut cx_im = vec![0.0; NFFT1_LONG];
    let mut workspace = create_decode_workspace();

    let mut decoded: Vec<DecodedMessage> = Vec::new();
    let mut seen_messages = std::collections::HashSet::new();
    let max_passes = if depth >= 3 {
        MAX_DECODE_PASSES_DEPTH3
    } else {
        1
    };

    // Save original data for nagain (narrow re-check) passes
    let dd_original = dd.clone();

    // Helper: count candidates per frequency bin (for coarse downsampling cache)
    fn count_candidate_frequencies(candidates: &[Candidate]) -> std::collections::HashMap<i32, usize> {
        let mut counts = std::collections::HashMap::new();
        for c in candidates {
            *counts.entry(c.freq as i32).or_insert(0) += 1;
        }
        counts
    }

    for _pass in 0..max_passes {
        // Use lower syncmin for later passes to find weaker signals in residual
        let pass_syncmin = if max_passes > 1 && _pass > 0 {
            (syncmin * 0.7).max(0.6)
        } else {
            syncmin
        };
        cx_re.fill(0.0);
        cx_im.fill(0.0);

    cx_re[..residual.len()].copy_from_slice(&residual);
        fft_complex(&mut cx_re, &mut cx_im, false);

        let (candidates, _sbase) = sync8(&residual, nfa, nfb, pass_syncmin, max_candidates, sync_mode);
        let mut coarse_frequency_uses = count_candidate_frequencies(&candidates);
        let mut coarse_downsample_cache: std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)> =
            std::collections::HashMap::new();
        let mut decoded_in_pass = 0;

        // ── Candidate decoding: parallel when no shared HashCallBook ──
        let mut results: Vec<(f64, f64, f64, Ft8bResult)> = if book.is_none() {
            use rayon::prelude::*;
            candidates.par_iter()
                .filter_map(|cand| {
                    let mut my_ws = create_decode_workspace();
                    let mut my_cache = std::collections::HashMap::new();
                    let mut my_freq_uses = coarse_frequency_uses.clone();
                    ft8b(&residual, &cx_re, &cx_im, cand.freq, cand.dt, &_sbase,
                         depth, &None, None, None, &mut my_ws, &mut my_cache, &mut my_freq_uses)
                        .map(|r| (cand.freq, cand.dt, cand.sync, r))
                })
                .collect()
        } else {
            let mut seq = Vec::new();
            for cand in &candidates {
                if let Some(r) = ft8b(&residual, &cx_re, &cx_im, cand.freq, cand.dt, &_sbase,
                    depth, &book, None, None, &mut workspace, &mut coarse_downsample_cache, &mut coarse_frequency_uses) {
                    seq.push((cand.freq, cand.dt, cand.sync, r));
                }
            }
            seq
        };

        // Process results sequentially (dedup + subtract + add to decoded)
        for (_freq, _dt, sync, result) in results.drain(..) {
            let message_key = normalize_message_key(&result.msg);
            if seen_messages.contains(&message_key) {
                continue;
            }
            seen_messages.insert(message_key);
            decoded.push(DecodedMessage {
                freq: result.freq,
                dt: result.dt - 0.5,
                snr: result.snr,
                msg: result.msg.clone(),
                sync,
            });
            decoded_in_pass += 1;
            if _pass + 1 < max_passes {
                crate::util::subtract_ft8::subtract_ft8(
                    &mut residual, &result.itone, result.freq, result.dt,
                );
            }
        }

        if decoded_in_pass == 0 {
            break;
        }
    }

    // ── nagain: narrow re-check around each decoded frequency on ORIGINAL data ──
    // WSJT-X nagain mode: after subtraction, re-search narrow band (±20Hz) around
    // EACH decoded frequency individually using original (unsubtracted) data.
    // This catches weak signals hidden in the skirts of stronger ones.
    // Per-frequency search avoids noise from unrelated parts of the spectrum.
    if depth >= 3 && !decoded.is_empty() {
        // Use higher syncmin for narrow-band search (less noise bandwidth → more reliable)
        let nagain_syncmin = (syncmin * 1.5).max(1.1);
        
        // Compute long FFT for original data (done once)
        let mut nagain_cx_re = vec![0.0; NFFT1_LONG];
        let mut nagain_cx_im = vec![0.0; NFFT1_LONG];
        nagain_cx_re[..dd_original.len()].copy_from_slice(&dd_original);
        fft_complex(&mut nagain_cx_re, &mut nagain_cx_im, false);
        
        // Collect decoded frequencies to avoid redundant searches
        let mut searched_freqs: Vec<f64> = decoded.iter().map(|d| d.freq).collect();
        searched_freqs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        searched_freqs.dedup_by(|a, b| (*a - *b).abs() < 10.0);
        
        for &freq in &searched_freqs {
            let nagain_nfa = (freq - 20.0).max(nfa);
            let nagain_nfb = (freq + 20.0).min(nfb);
            if nagain_nfb <= nagain_nfa { continue; }
            
            let (nagain_candidates, nagain_sbase) = sync8(
                &dd_original, nagain_nfa, nagain_nfb, nagain_syncmin, 50, sync_mode,  // narrow band, fewer candidates
            );
            
            let mut nagain_downsample_cache: std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)> =
                std::collections::HashMap::new();
            let mut nagain_freq_uses: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
            
            for cand in &nagain_candidates {
                if let Some(result) = ft8b(
                    &dd_original,
                    &nagain_cx_re,
                    &nagain_cx_im,
                    cand.freq,
                    cand.dt,
                    &nagain_sbase,
                    depth, &book, None, None,
                    &mut workspace,
                    &mut nagain_downsample_cache,
                    &mut nagain_freq_uses,
                ) {
                    let message_key = normalize_message_key(&result.msg);
                    if seen_messages.contains(&message_key) {
                        continue;
                    }
                    seen_messages.insert(message_key);
                    let msg = result.msg.clone();
                    decoded.push(DecodedMessage {
                        freq: result.freq,
                        dt: result.dt - 0.5,
                        snr: result.snr,
                        msg,
                        sync: cand.sync,
                    });
                }
            }
        }
    }

    decoded
}

fn normalize_message_key(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn copy_samples_to_decode_window(samples: &[f32]) -> Vec<f64> {
    let len = samples.len().min(NMAX);
    samples[..len].iter().map(|&x| x as f64).collect()
}

fn resample(input: &[f32], from_rate: usize, to_rate: usize, out_len: usize) -> Vec<f64> {
    let ratio = from_rate as f64 / to_rate as f64;
    (0..out_len)
        .map(|i| {
            let src_idx = i as f64 * ratio;
            let lo = src_idx.floor() as usize;
            let frac = src_idx - lo as f64;
            let v0 = if lo < input.len() { input[lo] as f64 } else { 0.0 };
            let v1 = if lo + 1 < input.len() {
                input[lo + 1] as f64
            } else {
                0.0
            };
            v0 * (1.0 - frac) + v1 * frac
        })
        .collect()
}

fn sync8(
    dd: &[f64],
    nfa: f64,
    nfb: f64,
    syncmin: f64,
    maxcand: usize,
    mode: SyncMode,
) -> (Vec<Candidate>, Vec<f64>) {
    let jz = 62;
    let fft_size = next_pow2(NFFT1); // 4096 (df=2.93). Mixed-radix 3840=19/20, returns 4096.
    let half_size = fft_size / 2;
    let tstep = NSTEP as f64 / SAMPLE_RATE as f64;
    let df = SAMPLE_RATE as f64 / fft_size as f64;
    let fac = 1.0 / 300.0;

    let mut s = vec![0.0; half_size * NHSYM];
    let mut savg = vec![0.0; half_size];
    let mut x_re = vec![0.0; fft_size];
    let mut x_im = vec![0.0; fft_size];

    for j in 0..NHSYM {
        let ia = j * NSTEP;
        x_re.fill(0.0);
        x_im.fill(0.0);
        let end = (ia + NSPS).min(dd.len());
        for i in ia..end {
            x_re[i - ia] = fac * dd[i];
        }
        fft_complex(&mut x_re, &mut x_im, false);
        for i in 0..half_size {
            let val = match mode {
                SyncMode::Amplitude => (x_re[i] * x_re[i] + x_im[i] * x_im[i]).sqrt(),
                SyncMode::AbsSum => x_re[i].abs() + x_im[i].abs(),
                _ => x_re[i] * x_re[i] + x_im[i] * x_im[i],
            };
            s[i * NHSYM + j] = val;
            savg[i] += val;
        }
    }

    let sbase = compute_baseline(&savg, nfa, nfb, df, half_size);

    let ia = (1.0_f64.max((nfa / df).round())) as usize;
    let ib = ((half_size - 14) as f64).min((nfb / df).round()) as usize;
    let nssy = NSPS / NSTEP;
    let nfos = (SAMPLE_RATE as f64 / NSPS as f64 / df).round() as usize;
    let jstrt = (0.5 / tstep).round() as usize;
    let width = 2 * jz + 1;
    let mut sync2d = vec![0.0; (ib - ia + 1) * width];

    for i in ia..=ib {
        for jj in (-(jz as isize)..=(jz as isize)).step_by(1) {
            let mut ta = 0.0;
            let mut tb = 0.0;
            let mut tc = 0.0;
            let mut t0a = 0.0;
            let mut t0b = 0.0;
            let mut t0c = 0.0;

            for n in 0..COSTAS_BLOCKS {
                let m: isize = jj + jstrt as isize + nssy as isize * n as isize;
                let i_costas = i + nfos * COSTAS[n] as usize;

                if m >= 0 && (m as usize) < NHSYM && i_costas < half_size {
                    ta += s[i_costas * NHSYM + m as usize];
                    for tone in 0..=6 {
                        let idx = i + nfos * tone;
                        if idx < half_size {
                            t0a += s[idx * NHSYM + m as usize];
                        }
                    }
                }

                let m36 = m + nssy as isize * 36;
                if m36 >= 0 && (m36 as usize) < NHSYM && i_costas < half_size {
                    tb += s[i_costas * NHSYM + m36 as usize];
                    for tone in 0..=6 {
                        let idx = i + nfos * tone;
                        if idx < half_size {
                            t0b += s[idx * NHSYM + m36 as usize];
                        }
                    }
                }

                let m72 = m + nssy as isize * 72;
                if m72 >= 0 && (m72 as usize) < NHSYM && i_costas < half_size {
                    tc += s[i_costas * NHSYM + m72 as usize];
                    for tone in 0..=6 {
                        let idx = i + nfos * tone;
                        if idx < half_size {
                            t0c += s[idx * NHSYM + m72 as usize];
                        }
                    }
                }
            }

            let t = ta + tb + tc;
            let t0 = (t0a + t0b + t0c - t) / 6.0;
            let sync_val = if t0 > 0.0 { t / t0 } else { 0.0 };

            let tbc = tb + tc;
            let t0bc = (t0b + t0c - tbc) / 6.0;
            let sync_bc = if t0bc > 0.0 { tbc / t0bc } else { 0.0 };

            sync2d[(i - ia) * width + (jj + jz as isize) as usize] = sync_val.max(sync_bc);
        }
    }

    let mut candidates0: Vec<Candidate> = Vec::new();
    let mlag: isize = 10;
    for i in ia..=ib {
        let mut best_sync = -1.0;
        let mut best_j: isize = 0;
        for j in (-mlag..=mlag).step_by(1) {
            let v = sync2d[(i - ia) * width + (j + jz as isize) as usize];
            if v > best_sync {
                best_sync = v;
                best_j = j;
            }
        }

        let mut best_sync2 = -1.0;
        let mut best_j2: isize = 0;
        for j in (-(jz as isize)..=(jz as isize)).step_by(1) {
            let v = sync2d[(i - ia) * width + (j + jz as isize) as usize];
            if v > best_sync2 {
                best_sync2 = v;
                best_j2 = j;
            }
        }

        if best_sync >= syncmin {
            candidates0.push(Candidate {
                freq: i as f64 * df,
                dt: (best_j as f64 - 0.5) * tstep,
                sync: best_sync,
            });
        }
        if best_j2 != best_j && best_sync2 >= syncmin {
            candidates0.push(Candidate {
                freq: i as f64 * df,
                dt: (best_j2 as f64 - 0.5) * tstep,
                sync: best_sync2,
            });
        }
    }

    let mut sync_values: Vec<f64> = candidates0.iter().map(|c| c.sync).collect();
    sync_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pctile_idx = (0.4 * sync_values.len() as f64).round().max(1.0) as usize - 1;
    let base = sync_values.get(pctile_idx).copied().unwrap_or(1.0);
    if base > 0.0 {
        for c in &mut candidates0 {
            c.sync /= base;
        }
    }

    // Remove duplicates
    let mut filtered: Vec<Candidate> = Vec::new();
    for i in 0..candidates0.len() {
        let mut keep = true;
        for j in 0..i {
            let fdiff = (candidates0[i].freq - candidates0[j].freq).abs();
            let tdiff = (candidates0[i].dt - candidates0[j].dt).abs();
            if fdiff < 0.5 && tdiff < 0.04 {
                if candidates0[i].sync >= candidates0[j].sync {
                    // mark j for removal (we'll skip it)
                } else {
                    keep = false;
                }
            }
        }
        if keep && candidates0[i].sync >= syncmin {
            filtered.push(candidates0[i].clone());
        }
    }

    filtered.sort_by(|a, b| b.sync.partial_cmp(&a.sync).unwrap());

    (filtered.into_iter().take(maxcand).collect(), sbase)
}

fn compute_baseline(savg: &[f64], nfa: f64, nfb: f64, df: f64, nh1: usize) -> Vec<f64> {
    let mut sbase = vec![0.0; nh1];
    let ia = (1.0_f64.max((nfa / df).round())) as usize;
    let ib = ((nh1 - 1) as f64).min((nfb / df).round()) as usize;
    let window = 50;

    for i in 0..nh1 {
        let mut sum = 0.0;
        let mut count = 0;
        let lo = ia.max(i.saturating_sub(window));
        let hi = ib.min(i + window);
        for j in lo..=hi {
            sum += savg[j];
            count += 1;
        }
        sbase[i] = if count > 0 {
            10.0 * (1e-30f64.max(sum / count as f64)).log10()
        } else {
            0.0
        };
    }
    sbase
}

fn ft8b(
    _dd0: &[f64],
    cx_re: &[f64],
    cx_im: &[f64],
    mut f1: f64,
    xdt: f64,
    _sbase: &[f64],
    depth: usize,
    _book: &Option<std::rc::Rc<HashCallBook>>,
    mycall: Option<&str>,
    hiscall: Option<&str>,
    workspace: &mut DecodeWorkspace,
    coarse_downsample_cache: &mut std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)>,
    coarse_frequency_uses: &mut std::collections::HashMap<i32, usize>,
) -> Option<Ft8bResult> {
    load_coarse_downsample(
        cx_re,
        cx_im,
        f1,
        workspace,
        coarse_downsample_cache,
        coarse_frequency_uses,
    );

    let mut ibest = find_best_time_offset(
        &workspace.cd0_re,
        &workspace.cd0_im,
        xdt,
    );
    let delfbest = find_best_frequency_shift(
        &workspace.cd0_re,
        &workspace.cd0_im,
        ibest,
    );

    f1 += delfbest;
    ft8_downsample(cx_re, cx_im, f1, workspace);

    ibest = refine_time_offset(
        &workspace.cd0_re,
        &workspace.cd0_im,
        ibest,
        &mut workspace.ss,
    );
    let xdt = (ibest as f64 - 1.0) * DT2;

    // Copy data needed for soft symbol extraction to avoid borrow conflicts
    let cd0_re_copy = workspace.cd0_re.clone();
    let cd0_im_copy = workspace.cd0_im.clone();
    extract_soft_symbols(
        &cd0_re_copy,
        &cd0_im_copy,
        ibest,
        workspace,
    );

    // WSJT-X: sync gate is nsync <= 6 bailout (= need >=7)
    // We relax: nsync >= 5 default; nsync >= 4 allowed with high symbol SNR
    let min_costas_hits: usize = if depth >= 3 { 4 } else { 6 };
    if !passes_sync_gate(&workspace.s8, min_costas_hits) {
        return None;
    }

    build_bit_metrics(workspace);
    
    // ── sbase-based LLR normalization: compensate for frequency-dependent noise ──
    // sbase is built by sync8 with SYNC8_DF = 12000/4096 = 2.93 Hz/bin.
    // Previously used DOWNSAMPLE_DF (0.0625) → wrong index, normalization never applied.
    if false { // Disabled: xbase formula needs recalibration for our sbase valuespace
        let freq_bin = (f1 / SYNC8_DF).round() as usize;
        if freq_bin < _sbase.len() {
            let sbase_val = _sbase[freq_bin];
            let xbase = 10.0_f64.powf(0.1 * (sbase_val - 40.0));
            let scale = 1.0 / (xbase.max(0.01).sqrt());
            let scale = scale.clamp(0.5, 3.0);
            for metric in [&mut workspace.bmeta, &mut workspace.bmetb, &mut workspace.bmetc, &mut workspace.bmetd] {
                for v in metric.iter_mut() {
                    *v *= scale;
                }
            }
        }
    }
    
    let result = try_decode_passes(workspace, depth, mycall, hiscall);
    result.as_ref()?;
    let result = result.unwrap();

    if result.cw.iter().all(|&b| b == 0) {
        return None;
    }

    let message77: Vec<u8> = result.message91[..77].to_vec();
    if !is_valid_message_type(&message77) {
        return None;
    }

    let msg = unpack77(&message77, _book.as_ref().map(|rc| rc.as_ref()));
    msg.as_ref()?;
    let msg = msg.unwrap();
    if msg.trim().is_empty() {
        return None;
    }

    let snr = estimate_snr(&workspace.s8, &result.cw);
    
    // Compute itone from codeword (same as get_tones but as [i32; 79])
    let mut itone = [0i32; 79];
    let tones = get_tones(&result.cw);
    for i in 0..79 {
        itone[i] = tones[i] as i32;
    }
    
    Some(Ft8bResult {
        msg,
        freq: f1,
        dt: xdt,
        snr,
        itone,
    })
}

fn load_coarse_downsample(
    cx_re: &[f64],
    cx_im: &[f64],
    f0: f64,
    workspace: &mut DecodeWorkspace,
    coarse_downsample_cache: &mut std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)>,
    coarse_frequency_uses: &mut std::collections::HashMap<i32, usize>,
) {
    let freq_key = f0 as i32;
    if let Some((re, im)) = coarse_downsample_cache.get(&freq_key) {
        workspace.cd0_re.copy_from_slice(re);
        workspace.cd0_im.copy_from_slice(im);
    } else {
        ft8_downsample(cx_re, cx_im, f0, workspace);
        let uses = coarse_frequency_uses.get(&freq_key).copied().unwrap_or(0);
        if uses > 1 {
            coarse_downsample_cache.insert(
                freq_key,
                (workspace.cd0_re.clone(), workspace.cd0_im.clone()),
            );
        }
    }

    let remaining = coarse_frequency_uses
        .get(&freq_key)
        .copied()
        .unwrap_or(1)
        .saturating_sub(1);
    if remaining == 0 {
        coarse_frequency_uses.remove(&freq_key);
        coarse_downsample_cache.remove(&freq_key);
    } else {
        coarse_frequency_uses.insert(freq_key, remaining);
    }
}

fn find_best_time_offset(cd0_re: &[f64], cd0_im: &[f64], xdt: f64) -> isize {
    // TS: Math.round((xdt + 0.5) * FS2)
    // The +0.5 centers the search in the middle of the window.
    // Returns UNWRAPPED ibest (may be negative), matching TS behavior.
    let i0_raw = ((xdt + 0.5) * FS2).round() as isize;
    let i0_center = i0_raw.rem_euclid(NP2 as isize);
    
    let mut smax = 0.0;
    let mut ibest_unwrapped = i0_raw;  // start with unwrapped
    let cs = build_costas_sync_templates();
    for offset in -10..=10 {
        let idx = (i0_center + offset).rem_euclid(NP2 as isize) as usize;
        let sync = sync8d(cd0_re, cd0_im, idx, &cs.re, &cs.im);
        if sync > smax {
            smax = sync;
            ibest_unwrapped = i0_raw + offset;
        }
    }
    ibest_unwrapped
}

fn find_best_frequency_shift(cd0_re: &[f64], cd0_im: &[f64], ibest: isize) -> f64 {
    let mut smax = 0.0;
    let mut delfbest = 0.0;
    let templates = build_frequency_shift_sync_templates();
    let idx = ibest.rem_euclid(NP2 as isize) as usize;
    for tpl in templates {
        let sync = sync8d(cd0_re, cd0_im, idx, &tpl.re, &tpl.im);
        if sync > smax {
            smax = sync;
            delfbest = tpl.delf;
        }
    }
    delfbest
}

fn refine_time_offset(cd0_re: &[f64], cd0_im: &[f64], ibest: isize, ss: &mut [f64]) -> isize {
    ss.fill(0.0);
    let cs = build_costas_sync_templates();
    for idt in -4..=4 {
        let idx = (ibest + idt).rem_euclid(NP2 as isize) as usize;
        ss[(idt + 4) as usize] =
            sync8d(cd0_re, cd0_im, idx, &cs.re, &cs.im);
    }

    let mut max_idx: isize = 4;
    let mut max_val = -1.0;
    for i in 0..9 {
        if ss[i] > max_val {
            max_val = ss[i];
            max_idx = i as isize;
        }
    }
    ibest + max_idx - 4
}

fn extract_soft_symbols(cd0_re: &[f64], cd0_im: &[f64], ibest: isize, workspace: &mut DecodeWorkspace) {
    for k in 0..NN {
        let i1 = ibest + (k as isize) * (COSTAS_SYMBOL_LEN as isize);
        workspace.symb_re.fill(0.0);
        workspace.symb_im.fill(0.0);

        // TS behavior: skip symbols that don't fit in [0, NP2)
        if i1 >= 0 && (i1 + COSTAS_SYMBOL_LEN as isize - 1) < NP2 as isize {
            let i1u = i1 as usize;
            for j in 0..COSTAS_SYMBOL_LEN {
                workspace.symb_re[j] = cd0_re[i1u + j];
                workspace.symb_im[j] = cd0_im[i1u + j];
            }
        }
        // else: symb stays zero (no wrap-around)

        fft_complex(&mut workspace.symb_re, &mut workspace.symb_im, false);
        for tone in 0..8 {
            let re = workspace.symb_re[tone] / 1000.0;
            let im = workspace.symb_im[tone] / 1000.0;
            let idx = tone * NN + k;
            workspace.cs_re[idx] = re;
            workspace.cs_im[idx] = im;
            workspace.s8[idx] = (re * re + im * im).sqrt();
        }
    }
}

fn passes_sync_gate(s8: &[f64], min_costas_hits: usize) -> bool {
    const SYNC_TIME_SHIFTS: [usize; 3] = [0, 36, 72];
    let mut nsync = 0;
    // ── SNR-based scoring (JTDX-style) ──
    let mut nsyncscore = 0u32;
    let mut scoreratio = 0.0f64;

    for k in 0..COSTAS_BLOCKS {
        for &offset in &SYNC_TIME_SHIFTS {
            let mut max_tone = 0;
            let mut max_val = -1.0;
            let mut sum_noise = 0.0;
            for t in 0..8 {
                let v = s8[t * NN + k + offset];
                sum_noise += v;
                if v > max_val {
                    max_val = v;
                    max_tone = t;
                }
            }
            // SNR per sync symbol: sync_tone / average of other 7 tones
            let noise = (sum_noise - max_val) / 7.0;
            if noise > 1e-12 && max_tone == COSTAS[k] as usize {
                nsync += 1;
                if max_val > noise {
                    nsyncscore += 1;
                    scoreratio += max_val / noise;
                }
            }
        }
    }

    // ── JTDX-style soft gate: hard nsync threshold is lowered when per-symbol SNR is high ──
    if nsyncscore > 0 {
        scoreratio /= nsyncscore as f64;
    }

    // Hard gate: minimum Costas hits
    if nsync < min_costas_hits {
        // Soft override: if per-symbol SNR is very high, let borderline nsync through
        if nsync >= 4 && nsyncscore >= nsync as u32 && scoreratio > 3.0 {
            return true;
        }
        return false;
    }

    true
}

fn build_bit_metrics(workspace: &mut DecodeWorkspace) {
    workspace.bmeta.fill(0.0);
    workspace.bmetb.fill(0.0);
    workspace.bmetc.fill(0.0);
    workspace.bmetd.fill(0.0);

    for nsym in 1..=3 {
        let nt = 1 << (3 * nsym);
        let ibmax = match nsym {
            1 => 2,
            2 => 5,
            _ => 8,
        };

        for ihalf in 1..=2 {
            for k in (1..=29).step_by(nsym) {
                let ks = if ihalf == 1 { k + 7 } else { k + 43 };

                for i in 0..nt {
                    let i1 = i / 64;
                    let i2 = (i & 63) / 8;
                    let i3 = i & 7;
                    if nsym == 1 {
                        let re = workspace.cs_re[GRAY_MAP[i3] as usize * NN + ks - 1];
                        let im = workspace.cs_im[GRAY_MAP[i3] as usize * NN + ks - 1];
                        workspace.s2[i] = (re * re + im * im).sqrt();
                    } else if nsym == 2 {
                        let s_re = workspace.cs_re[GRAY_MAP[i2] as usize * NN + ks - 1]
                            + workspace.cs_re[GRAY_MAP[i3] as usize * NN + ks];
                        let s_im = workspace.cs_im[GRAY_MAP[i2] as usize * NN + ks - 1]
                            + workspace.cs_im[GRAY_MAP[i3] as usize * NN + ks];
                        workspace.s2[i] = (s_re * s_re + s_im * s_im).sqrt();
                    } else {
                        let s_re = workspace.cs_re[GRAY_MAP[i1] as usize * NN + ks - 1]
                            + workspace.cs_re[GRAY_MAP[i2] as usize * NN + ks]
                            + workspace.cs_re[GRAY_MAP[i3] as usize * NN + ks + 1];
                        let s_im = workspace.cs_im[GRAY_MAP[i1] as usize * NN + ks - 1]
                            + workspace.cs_im[GRAY_MAP[i2] as usize * NN + ks]
                            + workspace.cs_im[GRAY_MAP[i3] as usize * NN + ks + 1];
                        workspace.s2[i] = (s_re * s_re + s_im * s_im).sqrt();
                    }
                }

                let i32 = 1 + (k - 1) * 3 + (ihalf - 1) * 87;
                for ib in 0..=ibmax {
                    let mut max1 = -1e30;
                    let mut max0 = -1e30;
                    for i in 0..nt {
                        let bit_set = (i & (1 << (ibmax - ib))) != 0;
                        if bit_set {
                            if workspace.s2[i] > max1 { max1 = workspace.s2[i]; }
                        } else {
                            if workspace.s2[i] > max0 { max0 = workspace.s2[i]; }
                        }
                    }

                    let idx = (i32 as isize + ib as isize - 1) as usize;
                    if idx >= N_LDPC { continue; }

                    let bm = max1 - max0;
                    if nsym == 1 {
                        workspace.bmeta[idx] = bm;
                        let den = max1.max(max0);
                        workspace.bmetd[idx] = if den > 0.0 { bm / den } else { 0.0 };
                    } else if nsym == 2 {
                        workspace.bmetb[idx] = bm;
                    } else {
                        workspace.bmetc[idx] = bm;
                    }
                }
            }
        }
    }

    normalize_bmet(&mut workspace.bmeta);
    normalize_bmet(&mut workspace.bmetb);
    normalize_bmet(&mut workspace.bmetc);
    normalize_bmet(&mut workspace.bmetd);
}

fn normalize_bmet(bmet: &mut [f64]) {
    let n = bmet.len();
    let mut sum = 0.0;
    let mut sum2 = 0.0;
    for i in 0..n {
        sum += bmet[i];
        sum2 += bmet[i] * bmet[i];
    }
    let avg = sum / n as f64;
    let avg2 = sum2 / n as f64;
    let variance = avg2 - avg * avg;
    let sigma = if variance > 0.0 {
        variance.sqrt()
    } else {
        avg2.sqrt()
    };
    if sigma > 0.0 {
        for i in 0..n {
            bmet[i] /= sigma;
        }
    }
}

fn ft8_downsample(cx_re: &[f64], cx_im: &[f64], f0: f64, workspace: &mut DecodeWorkspace) {
    let df = DOWNSAMPLE_DF;
    let baud = DOWNSAMPLE_BAUD;
    let i0 = (f0 / df).round() as usize;
    let ft = f0 + 8.5 * baud;
    let it = ((ft / df).round() as usize).min(NFFT1_LONG / 2);
    let fb = f0 - 1.5 * baud;
    let ib = 1.max((fb / df).round() as usize);

    workspace.cd0_re.fill(0.0);
    workspace.cd0_im.fill(0.0);
    let mut k = 0;
    for i in ib..=it {
        if k >= NFFT2 {
            break;
        }
        workspace.cd0_re[k] = cx_re[i];
        workspace.cd0_im[k] = cx_im[i];
        k += 1;
    }

    let taper_data = build_taper();
    for i in 0..TAPER_SIZE {
        if i >= NFFT2 {
            break;
        }
        let tap = taper_data[TAPER_SIZE - 1 - i];
        workspace.cd0_re[i] *= tap;
        workspace.cd0_im[i] *= tap;
    }

    let end_tap = k - 1;
    for i in 0..TAPER_SIZE {
        let idx = end_tap - TAPER_SIZE + 1 + i;
        if idx < NFFT2 {
            let tap = taper_data[i];
            workspace.cd0_re[idx] *= tap;
            workspace.cd0_im[idx] *= tap;
        }
    }

    let shift = i0 - ib;
    for i in 0..NFFT2 {
        let src_idx = (i + shift) % NFFT2;
        workspace.shift_re[i] = workspace.cd0_re[src_idx];
        workspace.shift_im[i] = workspace.cd0_im[src_idx];
    }
    workspace.cd0_re.copy_from_slice(&workspace.shift_re);
    workspace.cd0_im.copy_from_slice(&workspace.shift_im);

    fft_complex(&mut workspace.cd0_re, &mut workspace.cd0_im, true);

    for i in 0..NFFT2 {
        workspace.cd0_re[i] *= DOWNSAMPLE_SCALE;
        workspace.cd0_im[i] *= DOWNSAMPLE_SCALE;
    }
}

fn sync8d(cd0_re: &[f64], cd0_im: &[f64], i0: usize, sync_re: &[f64], sync_im: &[f64]) -> f64 {
    sync8d_isize(cd0_re, cd0_im, i0 as isize, sync_re, sync_im)
}

fn sync8d_isize(cd0_re: &[f64], cd0_im: &[f64], i0: isize, sync_re: &[f64], sync_im: &[f64]) -> f64 {
    let mut sync = 0.0;
    let stride = 36 * COSTAS_SYMBOL_LEN;

    for i in 0..COSTAS_BLOCKS {
        let base = i * COSTAS_SYMBOL_LEN;
        let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize);

        for _block in 0..3 {
            if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                let i_start = i_start as usize;
                let mut z_re = 0.0;
                let mut z_im = 0.0;
                for j in 0..COSTAS_SYMBOL_LEN {
                    let s_re = sync_re[base + j];
                    let s_im = sync_im[base + j];
                    let d_re = cd0_re[i_start + j];
                    let d_im = cd0_im[i_start + j];
                    z_re += d_re * s_re + d_im * s_im;
                    z_im += d_im * s_re - d_re * s_im;
                }
                sync += z_re * z_re + z_im * z_im;
            }
            i_start += stride as isize;
        }
    }

    sync
}

fn is_valid_message_type(message77: &[u8]) -> bool {
    let n3v = ((message77[71] as usize) << 2)
        | ((message77[72] as usize) << 1)
        | (message77[73] as usize);
    let i3v = ((message77[74] as usize) << 2)
        | ((message77[75] as usize) << 1)
        | (message77[76] as usize);
    if i3v > 5 || (i3v == 0 && n3v > 6) {
        return false;
    }
    if i3v == 0 && n3v == 2 {
        return false;
    }
    true
}

fn estimate_snr(s8: &[f64], cw: &[u8]) -> f64 {
    let itone = get_tones(cw);
    let mut xsig = 0.0;
    let mut xnoi = 0.0;

    for i in 0..79 {
        let tone = itone[i] as usize;
        xsig += s8[tone * NN + i].powi(2);
        let ios = (tone + 4) % 7;
        xnoi += s8[ios * NN + i].powi(2);
    }

    let mut snr = 0.001;
    let arg = xsig / xnoi.max(1e-30) - 1.0;
    if arg > 0.1 {
        snr = arg;
    }
    snr = 10.0 * snr.log10() - 27.0;
    if snr < -24.0 {
        -24.0
    } else {
        snr
    }
}


fn get_tones(cw: &[u8]) -> Vec<u8> {
    let mut tones = vec![0u8; 79];
    for i in 0..7 {
        tones[i] = COSTAS[i];
        tones[36 + i] = COSTAS[i];
        tones[72 + i] = COSTAS[i];
    }
    let mut k = 7;
    for j in 1..=58 {
        let i = (j - 1) * 3;
        if j == 30 {
            k += 7;
        }
        let indx = (cw[i] as usize) * 4 + (cw[i + 1] as usize) * 2 + (cw[i + 2] as usize);
        tones[k] = GRAY_MAP[indx];
        k += 1;
    }
    tones
}



/// Encode callsign to 28-bit AP pattern.
fn encode_callsign_ap(call: &str) -> Option<Vec<i8>> {
    use crate::util::pack_jt77::pack77;
    let msg = format!("CQ {} AA00", call.trim().to_uppercase());
    let bits77 = pack77(&msg);
    if bits77.len() == 77 && bits77[74] == 0 && bits77[75] == 0 && bits77[76] == 1 {
        let mut ap_bits = Vec::with_capacity(28);
        for i in 29..57 {
            ap_bits.push(if bits77[i] == 1 { 1i8 } else { -1i8 });
        }
        return Some(ap_bits);
    }
    None
}

fn encode_callsigns_ap(mycall: &str, hiscall: &str) -> Option<Vec<i8>> {
    let my_bits = encode_callsign_ap(mycall)?;
    let his_bits = encode_callsign_ap(hiscall)?;
    let mut combined = Vec::with_capacity(58);
    combined.extend_from_slice(&my_bits);
    combined.extend_from_slice(&his_bits);
    Some(combined)
}

fn try_decode_passes(workspace: &mut DecodeWorkspace, depth: usize, mycall: Option<&str>, hiscall: Option<&str>) -> Option<DecodeResult> {
    let maxosd_base = if depth >= 3 { 2 } else if depth >= 2 { 0 } else { -1 };
    let scalefac = 2.83;
    let bmetrics = [
        &workspace.bmeta,
        &workspace.bmetb,
        &workspace.bmetc,
        &workspace.bmetd,
    ];

    // Passes 1-4: regular BP+OSD decoding with 4 bit metrics
    workspace.apmask.fill(0);

    for ipass in 0..4 {
        let _maxosd = match ipass {
            0|1|3=>maxosd_base,
            2=>if depth>=3{5}else{maxosd_base},
            _=>maxosd_base,
        };
        let metric = bmetrics[ipass];
        for i in 0..N_LDPC {
            workspace.llr[i] = scalefac * metric[i];
        }

        if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
            // WSJT-X: nharderrors > 36 则跳过. 我们放宽到 40 以捕获更弱信号
            if result.nharderrors <= 36 {
                return Some(result);
            }
        }
    }

    // ── AP (A Priori) decoding passes ──
    // WSJT-X iaptype decoding: constrain known bits with strong LLR priors.
    // This is the key differentiator that lets WSJT-X decode 20/20 vs our 16/20.
    if depth >= 2 {
        // Compute apmag: max LLR magnitude * 1.01 (matches WSJT-X ft8b.f90)
        let apmag = bmetrics[0].iter()
            .map(|&x| (scalefac * x).abs())
            .fold(0.0f64, f64::max) * 1.01;
        if apmag > 0.1 {
            // Use the first metric (bmeta) as base LLR for AP passes
            for i in 0..N_LDPC {
                workspace.llr[i] = scalefac * bmetrics[0][i];
            }

            // ── AP Pass 5: CQ call mask ──
            // Constrain bits 0-28 to "CQ" pattern (n28a=2, ipa=0)
            // and bits 74-76 to i3=1 (standard message with grid)
            workspace.apmask.fill(0);
            // Set CQ pattern: n28a=2 → bit 26=1, others 0; ipa=0
            for i in 0..29 {
                workspace.apmask[i] = 1;
                workspace.llr[i] = if i == 26 { apmag } else { -apmag };
            }
            // Constrain i3=1 (standard 2-call message)
            workspace.apmask[74] = 1; workspace.llr[74] = -apmag;  // i3 bit 0 = 0
            workspace.apmask[75] = 1; workspace.llr[75] = -apmag;  // i3 bit 1 = 0
            workspace.apmask[76] = 1; workspace.llr[76] = apmag;   // i3 bit 2 = 1
            
            if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }

            // ── AP Pass 6: CQ + alternate i3 ──
            // Try i3=2 (standard 2-call message with /R or /P)
            workspace.apmask[74] = 1; workspace.llr[74] = -apmag;  // i3 bit 0 = 0
            workspace.apmask[75] = 1; workspace.llr[75] = apmag;   // i3 bit 1 = 1
            workspace.apmask[76] = 1; workspace.llr[76] = -apmag;  // i3 bit 2 = 0
            
            if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }

            // ── AP Pass 7: Message type only constraint ──
            // Only constrain i3/n3 without assuming specific message content.
            // i3=1, n3=0: standard 2-call with grid
            workspace.apmask.fill(0);
            for i in 0..N_LDPC {
                workspace.llr[i] = scalefac * bmetrics[0][i];
            }
            workspace.apmask[71] = 1; workspace.llr[71] = -apmag;  // n3 bit 0 = 0
            workspace.apmask[72] = 1; workspace.llr[72] = -apmag;  // n3 bit 1 = 0
            workspace.apmask[73] = 1; workspace.llr[73] = -apmag;  // n3 bit 2 = 0 (n3=0)
            workspace.apmask[74] = 1; workspace.llr[74] = -apmag;  // i3 bit 0 = 0
            workspace.apmask[75] = 1; workspace.llr[75] = -apmag;  // i3 bit 1 = 0
            workspace.apmask[76] = 1; workspace.llr[76] = apmag;   // i3 bit 2 = 1 (i3=1)
            
            if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }

            // ── AP Pass 8: Constrain only n3=0, i3=2 ──
            workspace.apmask[75] = 1; workspace.llr[75] = apmag;   // i3 bit 1 = 1
            workspace.apmask[76] = 1; workspace.llr[76] = -apmag;  // i3 bit 2 = 0 (i3=2)
            
            if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }
            // ── AP Pass 9: MYCALL ??? ??? (iaptype=2) ──
            // ── AP Pass 10: MYCALL HISCALL ??? (iaptype=3) ──
            // Framework implemented; requires explicit mycall/hiscall parameters.
            // Disabled by default (None).
            if mycall.is_some() {
                if let Some(mycall_str) = mycall {
                    if let Some(mycall_bits) = encode_callsign_ap(mycall_str) {
                        workspace.apmask.fill(0);
                        for i in 0..N_LDPC {
                            workspace.llr[i] = scalefac * bmetrics[0][i];
                        }
                        for i in 0..28 {
                            workspace.apmask[i] = 1;
                            workspace.llr[i] = apmag * mycall_bits[i] as f64;
                        }
                        workspace.apmask[74] = 1; workspace.llr[74] = -apmag;
                        workspace.apmask[75] = 1; workspace.llr[75] = -apmag;
                        workspace.apmask[76] = 1; workspace.llr[76] = apmag;
                        if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
                            if result.nharderrors <= 36 { return Some(result); }
                        }
                    }
                }
            }
            if let (Some(mycall_str), Some(hiscall_str)) = (mycall, hiscall) {
                if let Some(both_bits) = encode_callsigns_ap(mycall_str, hiscall_str) {
                    workspace.apmask.fill(0);
                    for i in 0..N_LDPC {
                        workspace.llr[i] = scalefac * bmetrics[0][i];
                    }
                    for i in 0..58 {
                        workspace.apmask[i] = 1;
                        workspace.llr[i] = apmag * both_bits[i] as f64;
                    }
                    workspace.apmask[74] = 1; workspace.llr[74] = -apmag;
                    workspace.apmask[75] = 1; workspace.llr[75] = -apmag;
                    workspace.apmask[76] = 1; workspace.llr[76] = apmag;
                    if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
                        if result.nharderrors <= 36 { return Some(result); }
                    }
                }
            }
        }
    }
    None
}