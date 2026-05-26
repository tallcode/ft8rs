/// FT8 decoder - Rust port of decode.ts
use crate::ft8::constants::{COSTAS, GRAY_MAP};
use crate::util::constants::{C38, N_LDPC, SAMPLE_RATE};
use crate::util::decode174_91::{decode174_91, DecodeResult};
use crate::util::hashcall::HashCallBook;
use crate::util::pack_jt77::{is_stdcall, pack77};
use crate::util::unpack_jt77::unpack77;
use crate::util::{fft_complex, fft_r2c, sync8_fft_size};
use std::rc::Rc;

// ft8b internal timers: [downsample, sync8d_search, symbols+gate, bitmetrics, ldpc, total_fail]
pub(crate) static FT8B_TIMERS: [std::sync::atomic::AtomicU64; 6] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

static SYNC8_DUMP_CALL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FT8B_DUMP_CALL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DECODE_TRACE_CALL_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// sync8 spectral mode - different representations favour different SNR regimes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncMode {
    /// Power spectrum: s = Re2 + Im2 (best for strong signals)
    Power = 0,
    /// Amplitude spectrum: s = sqrt(Re2 + Im2) (better for weak signals, compresses dynamic range)
    Amplitude = 1,
    /// Absolute sum: s = |Re| + |Im| (most robust against impulsive noise)
    AbsSum = 2,
}

pub(crate) const NSPS: usize = 1920;
pub(crate) const NFFT1: usize = 2 * NSPS; // 3840
pub(crate) const NSTEP: usize = NSPS / 4; // 480
pub(crate) const NMAX: usize = 15 * 12_000; // 180000
pub(crate) const NHSYM: usize = NMAX / NSTEP - 3; // 372
pub(crate) const NDOWN: usize = 60;
pub(crate) const NN: usize = 79;

pub(crate) const NFFT1_LONG: usize = 192000;
#[allow(dead_code)]
const SYNC8_DF: f64 = SAMPLE_RATE as f64 / 4096.0; // 12000/4096 = 2.93 Hz/bin
pub(crate) const NFFT2: usize = 3200;
pub(crate) const NP2: usize = 2812;
pub(crate) const COSTAS_BLOCKS: usize = 7;
pub(crate) const COSTAS_SYMBOL_LEN: usize = 32;
pub(crate) const TAPER_SIZE: usize = 101;
pub(crate) const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

pub(crate) const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
pub(crate) const DT2: f64 = 1.0 / FS2;
pub(crate) const DOWNSAMPLE_DF: f64 = SAMPLE_RATE as f64 / NFFT1_LONG as f64;
pub(crate) const DOWNSAMPLE_BAUD: f64 = SAMPLE_RATE as f64 / NSPS as f64;
pub(crate) const DOWNSAMPLE_SCALE: f64 = 0.12909944487358055; // sqrt(3200/192000)

#[derive(Clone)]
pub struct DecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
    /// FT8 itone pattern for signal subtraction (79 tones)
    pub itone: Vec<i32>,
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
    pub nfqso: Option<f64>,
    pub nftx: Option<f64>,
    pub nqso_progress: Option<usize>,
    pub ncontest: Option<usize>,
    pub napwid: Option<f64>,
    pub ft8_ap: Option<bool>,
    pub ap_cq_only: Option<bool>,
    pub nagain: Option<bool>,
    pub nzhsym: Option<usize>,
    /// Sync spectral mode: Power (default), Amplitude (better for weak signals),
    /// AbsSum (robust against impulsive noise).
    pub sync_mode: Option<SyncMode>,
}

#[derive(Clone)]
pub(crate) struct Candidate {
    pub(crate) freq: f64,
    pub(crate) dt: f64,
    pub(crate) sync: f64,
}

struct Ft8bResult {
    msg: String,
    freq: f64,
    dt: f64,
    snr: f64,
    itone: [i32; 79],
}

#[derive(Clone, Copy, Debug)]
struct TimeSearchResult {
    i0: isize,
    ibest: isize,
    sync: f64,
}

#[derive(Clone, Copy, Debug)]
struct FrequencySearchResult {
    delfbest: f64,
    sync: f64,
}

#[derive(Clone, Copy, Debug)]
struct TimeRefineResult {
    ibest: isize,
    offset: isize,
    sync: f64,
}

#[derive(Clone, Debug)]
struct TraceTarget {
    freq: f64,
    dt: Option<f64>,
    label: String,
}

#[derive(Clone)]
struct Ft8bApOptions {
    enabled: bool,
    cq_only: bool,
    nqso_progress: usize,
    ncontest: usize,
    nfqso: f64,
    nftx: f64,
    napwid: f64,
    nzhsym: usize,
    ap_set: Ft8ApSet,
}

#[derive(Clone)]
struct Ft8ApSet {
    apsym: [i8; 58],
    aph10: [i8; 10],
}

pub(crate) struct SyncTemplate {
    pub(crate) re: Vec<f64>,
    pub(crate) im: Vec<f64>,
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
    bmete: Vec<f64>,
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
        bmete: vec![0.0; N_LDPC],
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

pub(crate) fn build_costas_sync_templates() -> &'static SyncTemplate {
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
    let t_start = std::time::Instant::now();
    let sample_rate = options.sample_rate.unwrap_or(SAMPLE_RATE);

    let dd = if sample_rate == SAMPLE_RATE {
        copy_samples_to_decode_window(samples)
    } else {
        resample(samples, sample_rate, SAMPLE_RATE, NMAX)
    };
    let t_dd = t_start.elapsed();
    let (msgs, _, _) = decode_from_f64(dd, options, t_dd, t_start);
    msgs
}

/// Decode directly from f64 samples (used for cleaned residual after subtraction).
pub fn decode_f64(samples: &[f64], options: DecodeOptions) -> Vec<DecodedMessage> {
    let t_start = std::time::Instant::now();
    let len = samples.len().min(NMAX);
    let dd = samples[..len].to_vec();
    let t_dd = t_start.elapsed();
    let (msgs, _, _) = decode_from_f64(dd, options, t_dd, t_start);
    msgs
}

/// Decode directly from f64 samples and return the sbase noise baseline.
pub fn decode_f64_with_sbase(
    samples: &[f64],
    options: DecodeOptions,
) -> (Vec<DecodedMessage>, Vec<f64>) {
    let t_start = std::time::Instant::now();
    let len = samples.len().min(NMAX);
    let dd = samples[..len].to_vec();
    let t_dd = t_start.elapsed();
    let (msgs, sbase, _) = decode_from_f64(dd, options, t_dd, t_start);
    (msgs, sbase)
}

pub fn decode_f64_with_sbase_and_residual(
    samples: &[f64],
    options: DecodeOptions,
) -> (Vec<DecodedMessage>, Vec<f64>, Vec<f64>) {
    let t_start = std::time::Instant::now();
    let len = samples.len().min(NMAX);
    let dd = samples[..len].to_vec();
    let t_dd = t_start.elapsed();
    decode_from_f64(dd, options, t_dd, t_start)
}

/// Decode returning both messages and the sbase noise baseline.
pub fn decode_with_sbase(
    samples: &[f32],
    options: DecodeOptions,
) -> (Vec<DecodedMessage>, Vec<f64>) {
    let t_start = std::time::Instant::now();
    let sample_rate = options.sample_rate.unwrap_or(SAMPLE_RATE);
    let dd = if sample_rate == SAMPLE_RATE {
        copy_samples_to_decode_window(samples)
    } else {
        resample(samples, sample_rate, SAMPLE_RATE, NMAX)
    };
    let t_dd = t_start.elapsed();
    let (msgs, sbase, _) = decode_from_f64(dd, options, t_dd, t_start);
    (msgs, sbase)
}

fn decode_from_f64(
    mut dd: Vec<f64>,
    options: DecodeOptions,
    t_dd: std::time::Duration,
    t_start: std::time::Instant,
) -> (Vec<DecodedMessage>, Vec<f64>, Vec<f64>) {
    let trace_targets = trace_targets();
    let trace_call = trace_targets
        .as_ref()
        .map(|_| DECODE_TRACE_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1);
    // Truncate to NMAX (15s @ 12kHz = 180000 samples) matching WSJT-X NPTS
    if dd.len() > NMAX {
        dd.truncate(NMAX);
    }
    let nfa = options.freq_low.unwrap_or(200.0);
    let nfb = options.freq_high.unwrap_or(3000.0);
    let depth = options.depth.unwrap_or(3);
    let syncmin = options
        .sync_min
        .unwrap_or_else(|| default_outer_sync_min(depth));
    let max_candidates = options.max_candidates.unwrap_or(1000);
    let book = options.hash_call_book;
    let sync_mode = options.sync_mode.unwrap_or(SyncMode::Power);
    let nfqso = options.nfqso.unwrap_or(0.0);
    let nagain = options.nagain.unwrap_or(false);
    let ncontest = options.ncontest.unwrap_or(0);
    let mycall = options.mycall.clone();
    let hiscall = options.hiscall.clone();
    let ap_options = Ft8bApOptions {
        enabled: options.ft8_ap.unwrap_or(true),
        cq_only: options.ap_cq_only.unwrap_or(false),
        nqso_progress: options.nqso_progress.unwrap_or(0).min(5),
        ncontest,
        nfqso,
        nftx: options.nftx.unwrap_or(0.0),
        napwid: options.napwid.unwrap_or(50.0),
        nzhsym: options.nzhsym.unwrap_or(50),
        ap_set: ft8_ap_set(mycall.as_deref(), hiscall.as_deref(), ncontest),
    };
    let mut residual = dd.clone();

    let mut cx_re = vec![0.0; NFFT1_LONG];
    let mut cx_im = vec![0.0; NFFT1_LONG];
    cx_re[..residual.len().min(NFFT1_LONG)]
        .copy_from_slice(&residual[..residual.len().min(NFFT1_LONG)]);
    fft_complex(&mut cx_re, &mut cx_im, false);
    let t_cx = t_start.elapsed();
    let _workspace = create_decode_workspace();

    let mut decoded: Vec<DecodedMessage> = Vec::new();
    let mut seen_messages = std::collections::HashSet::new();
    let max_passes = if depth == 1 { 2 } else { 3 };

    #[allow(clippy::map_clone)]
    fn count_candidate_frequencies(
        candidates: &[Candidate],
    ) -> std::collections::HashMap<i32, usize> {
        let mut counts = std::collections::HashMap::new();
        for c in candidates {
            *counts.entry(c.freq as i32).or_insert(0) += 1;
        }
        counts
    }

    let mut t_sync8_total = std::time::Duration::ZERO;
    let mut t_decode_total = std::time::Duration::ZERO;
    let mut t_subtract_total = std::time::Duration::ZERO;

    // WSJT-X sync8 refreshes sbase for each pass on the current residual.
    let mut sbase: Vec<f64> = Vec::new();

    for pass_idx in 0..max_passes {
        if pass_idx == 2 && decoded.is_empty() {
            continue;
        }
        let pass_syncmin = syncmin;
        cx_re.fill(0.0);
        cx_im.fill(0.0);

        cx_re[..residual.len()].copy_from_slice(&residual);
        fft_complex(&mut cx_re, &mut cx_im, false);

        let t0 = std::time::Instant::now();
        let (ifa, ifb) = if nagain {
            (nfqso - 20.0, nfqso + 20.0)
        } else {
            (nfa, nfb)
        };

        let (candidates, pass_sbase) = sync8(
            &residual,
            ifa,
            ifb,
            pass_syncmin,
            nfqso,
            max_candidates,
            sync_mode,
        );
        sbase = pass_sbase;
        t_sync8_total += t0.elapsed();
        trace_sync8_targets(
            trace_call,
            trace_targets.as_deref(),
            options.nzhsym.unwrap_or(50),
            pass_idx + 1,
            pass_syncmin,
            &candidates,
        );

        // WSJT-X ft8_decode.f90: pass 1 uses imetric=1, passes 2/3 use imetric=2.
        let pass_imetric = if pass_idx == 0 { 1 } else { 2 };

        let _coarse_frequency_uses = count_candidate_frequencies(&candidates);
        let _coarse_downsample_cache: std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)> =
            std::collections::HashMap::new();
        let mut decoded_in_pass = 0;

        // ── Candidate decoding: sequential with immediate subtraction (matching WSJT-X ft8b) ──
        let t_decode_start = std::time::Instant::now();
        for cand in &candidates {
            let mut cand_ws = create_decode_workspace();
            let mut cand_cache: std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)> =
                std::collections::HashMap::new();
            let mut cand_freq_uses: std::collections::HashMap<i32, usize> =
                std::collections::HashMap::new();
            if let Some(r) = ft8b(
                &residual,
                &cx_re,
                &cx_im,
                cand.freq,
                cand.dt,
                &sbase,
                depth,
                pass_imetric,
                nagain,
                &ap_options,
                &book,
                None,
                &mut cand_ws,
                &mut cand_cache,
                &mut cand_freq_uses,
            ) {
                trace_decode_success(
                    trace_call,
                    trace_targets.as_deref(),
                    options.nzhsym.unwrap_or(50),
                    pass_idx + 1,
                    &r,
                );
                let message_key = normalize_message_key(&r.msg);
                crate::util::subtract_ft8::subtract_ft8(&mut residual, &r.itone, r.freq, r.dt);
                if seen_messages.contains(&message_key) {
                    continue;
                }
                seen_messages.insert(message_key);
                decoded.push(DecodedMessage {
                    freq: r.freq,
                    dt: r.dt - 0.5,
                    snr: r.snr,
                    msg: r.msg.clone(),
                    sync: cand.sync,
                    itone: r.itone.to_vec(),
                });
                decoded_in_pass += 1;
            }
        }
        t_decode_total += t_decode_start.elapsed();
        let _t_sub_start = std::time::Instant::now();
        t_subtract_total += _t_sub_start.elapsed();

        let _ = decoded_in_pass;
    }

    let total = t_start.elapsed();
    let t_dd_us = t_dd.as_micros();
    let t_cx_us = t_cx.as_micros() - t_dd_us;

    // Print ft8b timer breakdown
    let ft8b_down = FT8B_TIMERS[0].swap(0, std::sync::atomic::Ordering::Relaxed);
    let ft8b_sync8d = FT8B_TIMERS[1].swap(0, std::sync::atomic::Ordering::Relaxed);
    let ft8b_symbols = FT8B_TIMERS[2].swap(0, std::sync::atomic::Ordering::Relaxed);
    let ft8b_bmet = FT8B_TIMERS[3].swap(0, std::sync::atomic::Ordering::Relaxed);
    let ft8b_ldpc = FT8B_TIMERS[4].swap(0, std::sync::atomic::Ordering::Relaxed);
    eprintln!("[TIMER] copy={}ms, long_fft={}ms, sync8={}ms, decode={}ms (down={}ms,sync8d={}ms,symbols={}ms,bmet={}ms,ldpc={}ms), sub={}ms, total={}ms",
        t_dd.as_millis(), t_cx_us/1000,
        t_sync8_total.as_millis(), t_decode_total.as_millis(),
        ft8b_down/1000, ft8b_sync8d/1000, ft8b_symbols/1000, ft8b_bmet/1000, ft8b_ldpc/1000,
        t_subtract_total.as_millis(), total.as_millis());

    (decoded, sbase, residual)
}

fn default_outer_sync_min(depth: usize) -> f64 {
    if depth <= 2 {
        2.1
    } else {
        1.3
    }
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
            let v0 = if lo < input.len() {
                input[lo] as f64
            } else {
                0.0
            };
            let v1 = if lo + 1 < input.len() {
                input[lo + 1] as f64
            } else {
                0.0
            };
            v0 * (1.0 - frac) + v1 * frac
        })
        .collect()
}

/// Thread-local reusable buffers for sync8 to avoid allocations.
struct Sync8Buffers {
    s: Vec<f64>,
    savg: Vec<f64>,
    x_re: Vec<f64>,
    x_im: Vec<f64>,
    sync2d: Vec<f64>,
    candidate0: Vec<Candidate>,
}

thread_local! {
    static SYNC8_BUFS: std::cell::RefCell<Sync8Buffers> = const {
        std::cell::RefCell::new(Sync8Buffers {
            s: Vec::new(), savg: Vec::new(), x_re: Vec::new(),
            x_im: Vec::new(), sync2d: Vec::new(), candidate0: Vec::new(),
        })
    };
}

impl Sync8Buffers {
    #[inline]
    fn ensure(&mut self, half_size: usize, width: usize) {
        // s = half_size * NHSYM
        let sz_s = half_size * NHSYM;
        let sz_x = half_size * 2;
        // sync2d can be up to half_size * width (full frequency range)
        let sz_sync2d = width * half_size;
        if self.s.len() < sz_s {
            self.s.resize(sz_s, 0.0);
        }
        if self.savg.len() < half_size {
            self.savg.resize(half_size, 0.0);
        }
        if self.x_re.len() < sz_x {
            self.x_re.resize(sz_x, 0.0);
        }
        if self.x_im.len() < sz_x {
            self.x_im.resize(sz_x, 0.0);
        }
        if self.sync2d.len() < sz_sync2d {
            self.sync2d.resize(sz_sync2d, 0.0);
        }
        // zero buffers
        self.s[..sz_s].fill(0.0);
        self.savg[..half_size].fill(0.0);
    }
}

pub(crate) fn sync8(
    dd: &[f64],
    nfa: f64,
    nfb: f64,
    syncmin: f64,
    nfqso: f64,
    maxcand: usize,
    mode: SyncMode,
) -> (Vec<Candidate>, Vec<f64>) {
    let dump_limit = sync8_dump_limit();
    let dump_call = dump_limit
        .map(|_| SYNC8_DUMP_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1);
    let jz = 62;
    let fft_size = sync8_fft_size();
    let half_size = fft_size / 2;
    let tstep = NSTEP as f64 / SAMPLE_RATE as f64;
    let df = SAMPLE_RATE as f64 / fft_size as f64;
    let fac = 1.0 / 300.0;
    let width = 2 * jz + 1;
    let nssy = NSPS / NSTEP;
    let nfos = (SAMPLE_RATE as f64 / NSPS as f64 / df).round() as usize;
    // WSJT-X sync8.f90 uses implicit-integer jstrt. Assigning 0.5/tstep
    // therefore truncates 12.5 to 12 rather than rounding to 13.
    let jstrt = (0.5 / tstep) as usize;

    SYNC8_BUFS.with_borrow_mut(|b| {
        b.ensure(half_size, width);
        let s = &mut b.s[..half_size * NHSYM];
        let savg = &mut b.savg[..half_size];
        let x_re = &mut b.x_re[..fft_size];
        let x_im = &mut b.x_im[..fft_size];
        let sync2d = &mut b.sync2d[..];
        let candidate0 = &mut b.candidate0;
        candidate0.clear();
        savg.fill(0.0);

        for j in 0..NHSYM {
            let ia = j * NSTEP;
            x_re[..fft_size].fill(0.0);
            x_im[..fft_size].fill(0.0);
            let end = (ia + NSPS).min(dd.len());
            for i in ia..end {
                x_re[i - ia] = fac * dd[i];
            }
            fft_r2c(x_re, x_im);
            let row_offset = j;
            for i in 0..half_size {
                let val = match mode {
                    SyncMode::Amplitude => (x_re[i] * x_re[i] + x_im[i] * x_im[i]).sqrt(),
                    SyncMode::AbsSum => x_re[i].abs() + x_im[i].abs(),
                    _ => x_re[i] * x_re[i] + x_im[i] * x_im[i],
                };
                s[i * NHSYM + row_offset] = val;
                savg[i] += val;
            }
        }

        let sbase = get_spectrum_baseline(dd, nfa, nfb);

        let ia = (1.0_f64.max((nfa / df).round())) as usize;
        let ib = ((half_size - 14) as f64).min((nfb / df).round()) as usize;

        // Zero sync2d region we'll use
        let sync2d_len = (ib - ia + 1) * width;
        sync2d[..sync2d_len].fill(0.0);

        // WSJT-X sync8.f90:150: scale spectrum to reference level
        let mut s_max = 0.0;
        for i in 0..half_size {
            for j in 0..NHSYM {
                if s[i * NHSYM + j] > s_max {
                    s_max = s[i * NHSYM + j];
                }
            }
        }
        if s_max > 1e-30 {
            let fac = 20.0 / s_max;
            for v in s.iter_mut() {
                *v *= fac;
            }
            for v in savg.iter_mut() {
                *v *= fac;
            }
        }

        for i in ia..=ib {
            for jj in (-(jz as isize)..=(jz as isize)).step_by(1) {
                let mut ta = 0.0;
                let mut tb = 0.0;
                let mut tc = 0.0;
                let mut t0a = 0.0;
                let mut t0b = 0.0;
                let mut t0c = 0.0;

                for n in 0..COSTAS_BLOCKS {
                    // WSJT-X sync8.f90 keeps `m` as a 1-based time-bin index
                    // into s(:,m). Convert only at the Rust access boundary.
                    let m: isize = jj + jstrt as isize + nssy as isize * n as isize;
                    let m0 = m - 1;
                    let i_costas = i + nfos * COSTAS[n] as usize;

                    if m >= 1 && m <= NHSYM as isize && i_costas < half_size {
                        ta += s[i_costas * NHSYM + m0 as usize];
                        for tone in 0..=6 {
                            let idx = i + nfos * tone;
                            if idx < half_size {
                                t0a += s[idx * NHSYM + m0 as usize];
                            }
                        }
                    }

                    let m36 = m + nssy as isize * 36;
                    let m36_0 = m36 - 1;
                    if m36 >= 1 && m36 <= NHSYM as isize && i_costas < half_size {
                        tb += s[i_costas * NHSYM + m36_0 as usize];
                        for tone in 0..=6 {
                            let idx = i + nfos * tone;
                            if idx < half_size {
                                t0b += s[idx * NHSYM + m36_0 as usize];
                            }
                        }
                    }

                    let m72 = m + nssy as isize * 72;
                    let m72_0 = m72 - 1;
                    if m72 >= 1 && m72 <= NHSYM as isize && i_costas < half_size {
                        tc += s[i_costas * NHSYM + m72_0 as usize];
                        for tone in 0..=6 {
                            let idx = i + nfos * tone;
                            if idx < half_size {
                                t0c += s[idx * NHSYM + m72_0 as usize];
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

        // Red + red2 per frequency bin (matching WSJT-X)
        let mlag: isize = 13;
        let mlag2: isize = jz as isize;
        let iz = ib - ia + 1;
        let mut red = vec![0.0f64; iz];
        let mut red2 = vec![0.0f64; iz];
        let mut jpeak = vec![0isize; iz];
        let mut jpeak2 = vec![0isize; iz];
        for i in ia..=ib {
            let idx = i - ia;
            let mut best = -1.0;
            for j in (-mlag..=mlag).step_by(1) {
                let v = sync2d[idx * width + (j + jz as isize) as usize];
                if v > best {
                    best = v;
                    jpeak[idx] = j;
                }
            }
            red[idx] = best;
            let mut best2 = -1.0;
            for j in (-mlag2..=mlag2).step_by(1) {
                let v = sync2d[idx * width + (j + jz as isize) as usize];
                if v > best2 {
                    best2 = v;
                    jpeak2[idx] = j;
                }
            }
            red2[idx] = best2;
        }

        // WSJT-X style: normalize red by 40th percentile, then normalize red2 similarly
        let npctile = ((0.40 * iz as f64).round() as usize).max(1) - 1;
        {
            let mut red_copy = red.clone();
            red_copy.select_nth_unstable_by(npctile, |a, b| a.partial_cmp(b).unwrap());
            let base = red_copy[npctile].max(1e-30);
            for v in red.iter_mut() {
                *v /= base;
            }
        }
        {
            let mut red2_copy = red2.clone();
            red2_copy.select_nth_unstable_by(npctile, |a, b| a.partial_cmp(b).unwrap());
            let base2 = red2_copy[npctile].max(1e-30);
            for v in red2.iter_mut() {
                *v /= base2;
            }
        }

        let mut order: Vec<usize> = (0..iz).collect();
        order.sort_by(|&a, &b| red[a].partial_cmp(&red[b]).unwrap());
        if let (Some(call_id), Some(limit)) = (dump_call, dump_limit) {
            dump_sync8_precandidates(
                call_id, limit, nfa, nfb, syncmin, nfqso, ia, ib, df, tstep, jstrt, nssy, nfos,
                &order, &red, &red2, &jpeak, &jpeak2,
            );
        }
        let maxprecand = 1000usize;
        for &idx in order.iter().rev().take(iz.min(maxprecand)) {
            if candidate0.len() >= maxprecand {
                break;
            }
            if red[idx] >= syncmin && red[idx].is_finite() {
                let i = ia + idx;
                candidate0.push(Candidate {
                    freq: i as f64 * df,
                    dt: (jpeak[idx] as f64 - 0.5) * tstep,
                    sync: red[idx],
                });
            }
            if jpeak2[idx] == jpeak[idx] || candidate0.len() >= maxprecand {
                continue;
            }
            if red2[idx] >= syncmin && red2[idx].is_finite() {
                let i = ia + idx;
                candidate0.push(Candidate {
                    freq: i as f64 * df,
                    dt: (jpeak2[idx] as f64 - 0.5) * tstep,
                    sync: red2[idx],
                });
            }
        }

        let candidates = finalize_sync8_candidates(candidate0, syncmin, nfqso, maxcand);
        if let (Some(call_id), Some(limit)) = (dump_call, dump_limit) {
            dump_sync8_final_candidates(call_id, limit, &candidates);
        }

        (candidates, sbase)
    })
}

fn sync8_dump_limit() -> Option<usize> {
    let raw = std::env::var("FT8RS_DUMP_SYNC8").ok()?;
    if raw.is_empty() || raw == "0" {
        return None;
    }
    if raw == "1" {
        return Some(25);
    }
    raw.parse::<usize>().ok().filter(|&n| n > 0).or(Some(25))
}

#[allow(clippy::too_many_arguments)]
fn dump_sync8_precandidates(
    call_id: u64,
    limit: usize,
    nfa: f64,
    nfb: f64,
    syncmin: f64,
    nfqso: f64,
    ia: usize,
    ib: usize,
    df: f64,
    tstep: f64,
    jstrt: usize,
    nssy: usize,
    nfos: usize,
    order: &[usize],
    red: &[f64],
    red2: &[f64],
    jpeak: &[isize],
    jpeak2: &[isize],
) {
    eprintln!(
        "[SYNC8_DUMP] call={} nfa={:.3} nfb={:.3} syncmin={:.3} nfqso={:.3} ia={} ib={} df={:.9} tstep={:.9} jstrt={} nssy={} nfos={}",
        call_id, nfa, nfb, syncmin, nfqso, ia, ib, df, tstep, jstrt, nssy, nfos
    );

    let mut rank = 0usize;
    for &idx in order.iter().rev() {
        if rank >= limit {
            break;
        }
        let bin = ia + idx;
        let freq = bin as f64 * df;
        if red[idx] >= syncmin && red[idx].is_finite() {
            rank += 1;
            eprintln!(
                "[SYNC8_PRE] call={} rank={} kind=red bin={} freq={:.6} xdt={:.9} sync={:.9} jpeak={} jpeak2={} red={:.9} red2={:.9}",
                call_id,
                rank,
                bin,
                freq,
                (jpeak[idx] as f64 - 0.5) * tstep,
                red[idx],
                jpeak[idx],
                jpeak2[idx],
                red[idx],
                red2[idx]
            );
        }
        if rank >= limit {
            break;
        }
        if jpeak2[idx] != jpeak[idx] && red2[idx] >= syncmin && red2[idx].is_finite() {
            rank += 1;
            eprintln!(
                "[SYNC8_PRE] call={} rank={} kind=red2 bin={} freq={:.6} xdt={:.9} sync={:.9} jpeak={} jpeak2={} red={:.9} red2={:.9}",
                call_id,
                rank,
                bin,
                freq,
                (jpeak2[idx] as f64 - 0.5) * tstep,
                red2[idx],
                jpeak[idx],
                jpeak2[idx],
                red[idx],
                red2[idx]
            );
        }
    }
}

fn dump_sync8_final_candidates(call_id: u64, limit: usize, candidates: &[Candidate]) {
    for (rank, c) in candidates.iter().take(limit).enumerate() {
        eprintln!(
            "[SYNC8_FINAL] call={} rank={} freq={:.6} xdt={:.9} sync={:.9}",
            call_id,
            rank + 1,
            c.freq,
            c.dt,
            c.sync
        );
    }
}

fn trace_targets() -> Option<&'static [TraceTarget]> {
    static TARGETS: std::sync::OnceLock<Option<Vec<TraceTarget>>> = std::sync::OnceLock::new();
    TARGETS
        .get_or_init(|| parse_trace_targets())
        .as_ref()
        .map(Vec::as_slice)
}

fn parse_trace_targets() -> Option<Vec<TraceTarget>> {
    let raw = std::env::var("FT8RS_TRACE_TARGETS").ok()?;
    let mut targets = Vec::new();
    for item in raw.split(';') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let mut parts = item.splitn(3, ':');
        let Some(freq_raw) = parts.next() else {
            continue;
        };
        let Ok(freq) = freq_raw.trim().parse::<f64>() else {
            continue;
        };
        let dt = parts.next().and_then(|v| {
            let v = v.trim();
            if v.is_empty() {
                None
            } else {
                v.parse::<f64>().ok()
            }
        });
        let label = parts
            .next()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or("target")
            .to_string();
        targets.push(TraceTarget { freq, dt, label });
    }
    if targets.is_empty() {
        None
    } else {
        Some(targets)
    }
}

fn trace_freq_tolerance() -> f64 {
    std::env::var("FT8RS_TRACE_FREQ_TOL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(8.0)
}

fn trace_dt_tolerance() -> f64 {
    std::env::var("FT8RS_TRACE_DT_TOL")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(0.8)
}

fn target_matches_freq(target: &TraceTarget, freq: f64) -> bool {
    (target.freq - freq).abs() <= trace_freq_tolerance()
}

fn target_matches_internal_dt(target: &TraceTarget, xdt: f64) -> bool {
    let Some(dt) = target.dt else {
        return true;
    };
    let tol = trace_dt_tolerance();
    (xdt - dt).abs() <= tol || (xdt - (dt + 0.5)).abs() <= tol
}

fn trace_sync8_targets(
    call_id: Option<u64>,
    targets: Option<&[TraceTarget]>,
    nzhsym: usize,
    pass: usize,
    syncmin: f64,
    candidates: &[Candidate],
) {
    let Some(targets) = targets else {
        return;
    };
    for target in targets {
        let mut printed = 0usize;
        for (rank, c) in candidates.iter().enumerate() {
            if !target_matches_freq(target, c.freq) || !target_matches_internal_dt(target, c.dt) {
                continue;
            }
            printed += 1;
            eprintln!(
                "[TRACE_SYNC8] decode_call={} nzhsym={} pass={} target=\"{}\" rank={} freq={:.3} xdt={:.3} display_dt~{:.3} sync={:.6} syncmin={:.3}",
                call_id.unwrap_or(0),
                nzhsym,
                pass,
                target.label,
                rank + 1,
                c.freq,
                c.dt,
                c.dt - 0.5,
                c.sync,
                syncmin
            );
            if printed >= 8 {
                break;
            }
        }
        if printed == 0 {
            eprintln!(
                "[TRACE_SYNC8_MISS] decode_call={} nzhsym={} pass={} target=\"{}\" freq={:.3} dt={}",
                call_id.unwrap_or(0),
                nzhsym,
                pass,
                target.label,
                target.freq,
                target
                    .dt
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_else(|| "*".to_string())
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn trace_ft8b_time_targets(
    targets: Option<&[TraceTarget]>,
    f1_candidate: f64,
    xdt_candidate: f64,
    depth: usize,
    imetric: usize,
    time0: TimeSearchResult,
    freq0: FrequencySearchResult,
    time1: TimeRefineResult,
    f1_refined: f64,
    xdt_refined: f64,
    nsync: usize,
    min_costas_hits: usize,
    sync_gate_ok: bool,
) {
    let Some(targets) = targets else {
        return;
    };
    for target in targets {
        if !target_matches_freq(target, f1_candidate)
            || !target_matches_internal_dt(target, xdt_candidate)
        {
            continue;
        }
        eprintln!(
            "[TRACE_FT8B] target=\"{}\" f1_in={:.3} xdt_in={:.3} display_in~{:.3} depth={} imetric={} i0={} ibest0={} sync0={:.6} delfbest={:.3} fsync={:.6} ibest1={} dt_offset={} sync1={:.6} f1_refined={:.3} xdt_refined={:.3} display_refined={:.3} nsync={} gate_min={} gate_ok={}",
            target.label,
            f1_candidate,
            xdt_candidate,
            xdt_candidate - 0.5,
            depth,
            imetric,
            time0.i0,
            time0.ibest,
            time0.sync,
            freq0.delfbest,
            freq0.sync,
            time1.ibest,
            time1.offset,
            time1.sync,
            f1_refined,
            xdt_refined,
            xdt_refined - 0.5,
            nsync,
            min_costas_hits,
            sync_gate_ok
        );
    }
}

fn trace_decode_success(
    call_id: Option<u64>,
    targets: Option<&[TraceTarget]>,
    nzhsym: usize,
    pass: usize,
    result: &Ft8bResult,
) {
    let Some(targets) = targets else {
        return;
    };
    for target in targets {
        if !target_matches_freq(target, result.freq) {
            continue;
        }
        eprintln!(
            "[TRACE_DECODE] decode_call={} nzhsym={} pass={} target=\"{}\" freq={:.3} xdt={:.3} display_dt={:.3} snr={:.1} msg=\"{}\"",
            call_id.unwrap_or(0),
            nzhsym,
            pass,
            target.label,
            result.freq,
            result.dt,
            result.dt - 0.5,
            result.snr,
            result.msg
        );
    }
}

fn next_ft8b_dump_call() -> Option<u64> {
    let limit = ft8b_dump_limit()?;
    let call = FT8B_DUMP_CALL_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if call <= limit as u64 {
        Some(call)
    } else {
        None
    }
}

fn ft8b_dump_limit() -> Option<usize> {
    let raw = std::env::var("FT8RS_DUMP_FT8B").ok()?;
    if raw.is_empty() || raw == "0" {
        return None;
    }
    if raw == "1" {
        return Some(50);
    }
    raw.parse::<usize>().ok().filter(|&n| n > 0).or(Some(50))
}

#[allow(clippy::too_many_arguments)]
fn dump_ft8b_time_chain(
    call_id: Option<u64>,
    f1_candidate: f64,
    xdt_candidate: f64,
    depth: usize,
    imetric: usize,
    time0: TimeSearchResult,
    freq0: FrequencySearchResult,
    time1: TimeRefineResult,
    f1_refined: f64,
    xdt_refined: f64,
    nsync: usize,
    min_costas_hits: usize,
    sync_gate_ok: bool,
) {
    let Some(call_id) = call_id else {
        return;
    };
    eprintln!(
        "[FT8B_TIME] call={} f1_in={:.6} xdt_in={:.9} depth={} imetric={} i0={} ibest0={} sync0={:.9} delfbest={:.3} fsync={:.9} ibest1={} dt_offset={} sync1={:.9} f1_refined={:.6} xdt_refined={:.9} nsync={} gate_min={} gate_ok={}",
        call_id,
        f1_candidate,
        xdt_candidate,
        depth,
        imetric,
        time0.i0,
        time0.ibest,
        time0.sync,
        freq0.delfbest,
        freq0.sync,
        time1.ibest,
        time1.offset,
        time1.sync,
        f1_refined,
        xdt_refined,
        nsync,
        min_costas_hits,
        sync_gate_ok
    );
}

fn dump_ft8b_message(call_id: Option<u64>, msg: &str, snr: f64) {
    let Some(call_id) = call_id else {
        return;
    };
    eprintln!("[FT8B_MSG] call={} snr={:.3} msg={}", call_id, snr, msg);
}

fn finalize_sync8_candidates(
    candidate0: &mut [Candidate],
    syncmin: f64,
    nfqso: f64,
    maxcand: usize,
) -> Vec<Candidate> {
    for i in 0..candidate0.len() {
        for j in 0..i {
            let fdiff = candidate0[i].freq.abs() - candidate0[j].freq.abs();
            let tdiff = (candidate0[i].dt - candidate0[j].dt).abs();
            if fdiff.abs() < 4.0 && tdiff < 0.04 {
                if candidate0[i].sync >= candidate0[j].sync {
                    candidate0[j].sync = 0.0;
                } else {
                    candidate0[i].sync = 0.0;
                }
            }
        }
    }

    let mut sorted_idx: Vec<usize> = (0..candidate0.len()).collect();
    sorted_idx.sort_by(|&a, &b| candidate0[a].sync.partial_cmp(&candidate0[b].sync).unwrap());

    let mut candidates = Vec::with_capacity(maxcand);
    for c in candidate0.iter_mut() {
        if (c.freq - nfqso).abs() <= 10.0 && c.sync >= syncmin {
            candidates.push(c.clone());
            c.sync = 0.0;
            if candidates.len() >= maxcand {
                return candidates;
            }
        }
    }

    for &idx in sorted_idx.iter().rev() {
        let c = &candidate0[idx];
        if c.sync >= syncmin {
            candidates.push(Candidate {
                freq: c.freq.abs(),
                dt: c.dt,
                sync: c.sync,
            });
            if candidates.len() >= maxcand {
                break;
            }
        }
    }
    candidates
}

pub(crate) fn compute_baseline(savg: &[f64], nfa: f64, nfb: f64, df: f64, nh1: usize) -> Vec<f64> {
    // WSJT-X stores sbase(1:NH1), with FFT bin 0/DC omitted. Keep index 0
    // unused so callers can use nint(f/df) directly, matching Fortran.
    let mut sbase = vec![0.0; nh1 + 1];
    let ia = (1.0_f64.max((nfa / df).round())) as usize;
    let ib = (nh1 as f64).min((nfb / df).round()) as usize;

    let db_range = (ib - ia + 1).max(1);
    let mut sdb = vec![0.0; nh1 + 1];
    for i in ia..=ib {
        sdb[i] = 10.0 * savg[i].max(1e-30).log10();
    }

    let nseg: usize = 10;
    let nlen = db_range / nseg;
    if nlen < 1 {
        let window = 50;
        for i in 1..=nh1 {
            let lo = ia.max(i.saturating_sub(window));
            let hi = ib.min(i + window);
            let mut sum = 0.0;
            let mut count = 0;
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
        return sbase;
    }

    let npct: usize = 10;
    let mut env_x: Vec<f64> = Vec::new();
    let mut env_y: Vec<f64> = Vec::new();
    let i0 = db_range / 2;

    for n in 0..nseg {
        let ja = ia + n * nlen;
        let jb = (ja + nlen - 1).min(ib);
        if ja > ib || ja >= sdb.len() {
            break;
        }
        let slice = &sdb[ja..=jb.min(nh1)];
        let pval = percentile(slice, npct);
        for i in ja..=jb.min(nh1) {
            if sdb[i] <= pval {
                let x = (i as isize - i0 as isize) as f64;
                if env_x.len() < 1000 {
                    env_x.push(x);
                    env_y.push(sdb[i]);
                } else {
                    env_x[999] = x;
                    env_y[999] = sdb[i];
                }
            }
        }
    }

    // WSJT-X baseline.f90 uses nterms=5, i.e. five coefficients a(1:5)
    // and a degree-4 polynomial. Rust polyfit() takes the degree.
    let a = polyfit(&env_x, &env_y, 4);

    for i in ia..=ib.min(nh1) {
        let t = (i as isize - i0 as isize) as f64;
        sbase[i] = evpoly(&a, t) + 0.65;
    }

    sbase
}

fn percentile(slice: &[f64], k: usize) -> f64 {
    if slice.is_empty() {
        return 0.0;
    }
    let mut tmp = slice.to_vec();
    tmp.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((tmp.len() as f64 * 0.01 * k as f64).round() as usize)
        .min(tmp.len())
        .max(1)
        - 1;
    tmp[idx]
}

fn polyfit(x: &[f64], y: &[f64], d: usize) -> Vec<f64> {
    let n = x.len().min(y.len());
    if n <= d {
        return vec![0.0; d + 1];
    }
    let m = d + 1;
    let mut a = vec![vec![0.0; m]; m];
    let mut b = vec![0.0; m];

    for i in 0..n {
        for j in 0..m {
            let xj = x[i].powi(j as i32);
            for k2 in 0..m {
                a[j][k2] += xj * x[i].powi(k2 as i32);
            }
            b[j] += xj * y[i];
        }
    }

    for col in 0..m {
        let mut max_val = a[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..m {
            if a[row][col].abs() > max_val {
                max_val = a[row][col].abs();
                max_row = row;
            }
        }
        if max_val < 1e-30 {
            break;
        }
        if max_row != col {
            a.swap(col, max_row);
            b.swap(col, max_row);
        }
        for row in (col + 1)..m {
            let factor = a[row][col] / a[col][col];
            for k2 in col..m {
                a[row][k2] -= factor * a[col][k2];
            }
            b[row] -= factor * b[col];
        }
    }

    let mut coeffs = vec![0.0; m];
    for i in (0..m).rev() {
        let mut sum = 0.0;
        for j in (i + 1)..m {
            sum += a[i][j] * coeffs[j];
        }
        if a[i][i].abs() >= 1e-30 {
            coeffs[i] = (b[i] - sum) / a[i][i];
        }
    }
    coeffs
}

fn evpoly(a: &[f64], t: f64) -> f64 {
    let mut result = 0.0;
    for i in (0..a.len()).rev() {
        result = result * t + a[i];
    }
    result
}

/// Nuttall 4-term window (matching WSJT-X nuttal_window.f90).
#[allow(dead_code)]
fn nuttall_window(n: usize) -> Vec<f64> {
    let mut w = vec![0.0; n];
    let nf = n as f64;
    let a0 = 0.355768;
    let a1 = 0.487396;
    let a2 = 0.144232;
    let a3 = 0.012604;
    for i in 0..n {
        let x = 2.0 * std::f64::consts::PI * i as f64 / nf;
        w[i] = a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos();
    }
    w
}

/// WSJT-X get_spectrum_baseline: Welch method with Nuttall window.
fn get_spectrum_baseline(dd: &[f64], mut nfa: f64, mut nfb: f64) -> Vec<f64> {
    let nfft = NFFT1;
    let nh1 = nfft / 2;
    let nst = nh1;
    let nf = 93usize;
    let window = nuttall_window(nfft);
    let wsum: f64 = window.iter().sum();
    let wscale = NSPS as f64 * 2.0 / 300.0 / wsum;
    let mut savg = vec![0.0; nh1 + 1];
    for j in 0..nf {
        let ia = j * nst;
        let ib = ia + nfft;
        if ib > NMAX {
            break;
        }
        let mut x_re = vec![0.0; nfft];
        let mut x_im = vec![0.0; nfft];
        for i in 0..nfft {
            let sample = dd.get(ia + i).copied().unwrap_or(0.0);
            x_re[i] = sample * window[i] * wscale;
        }
        fft_r2c(&mut x_re, &mut x_im);
        for i in 1..=nh1 {
            savg[i] += x_re[i] * x_re[i] + x_im[i] * x_im[i];
        }
    }
    let nwin = nfb - nfa;
    if nfa < 100.0 {
        nfa = 100.0;
        if nwin < 100.0 {
            nfb = nfa + nwin;
        }
    }
    if nfb > 4910.0 {
        nfb = 4910.0;
        if nwin < 100.0 {
            nfa = nfb - nwin;
        }
    }
    let df = SAMPLE_RATE as f64 / nfft as f64;
    compute_baseline(&savg, nfa, nfb, df, nh1)
}

fn ft8b(
    _dd0: &[f64],
    cx_re: &[f64],
    cx_im: &[f64],
    mut f1: f64,
    xdt: f64,
    _sbase: &[f64],
    depth: usize,
    imetric: usize,
    nagain: bool,
    ap_options: &Ft8bApOptions,
    _book: &Option<std::rc::Rc<HashCallBook>>,
    _sbase_welch: Option<&[f64]>,
    workspace: &mut DecodeWorkspace,
    coarse_downsample_cache: &mut std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)>,
    coarse_frequency_uses: &mut std::collections::HashMap<i32, usize>,
) -> Option<Ft8bResult> {
    let t0 = std::time::Instant::now();
    let ft8b_dump_call = next_ft8b_dump_call();
    let f1_candidate = f1;
    let xdt_candidate = xdt;

    load_coarse_downsample(
        cx_re,
        cx_im,
        f1,
        workspace,
        coarse_downsample_cache,
        coarse_frequency_uses,
    );
    let t1 = t0.elapsed();

    let time0 = find_best_time_offset(&workspace.cd0_re, &workspace.cd0_im, xdt);
    let freq0 = find_best_frequency_shift(&workspace.cd0_re, &workspace.cd0_im, time0.ibest);
    f1 += freq0.delfbest;
    ft8_downsample(cx_re, cx_im, f1, workspace);
    let t2 = t0.elapsed();

    let time1 = refine_time_offset(
        &workspace.cd0_re,
        &workspace.cd0_im,
        time0.ibest,
        &mut workspace.ss,
    );
    let ibest = time1.ibest;
    let xdt = (ibest as f64 - 1.0) * DT2;

    extract_soft_symbols(ibest, workspace);
    let t3 = t0.elapsed();

    // WSJT-X ft8b.f90: syncmin=6, imetric=2 => 7, depth<=2 => 8,
    // and bailout is nsync <= syncmin.
    let min_costas_hits: usize = if depth <= 2 {
        9
    } else if imetric == 2 {
        8
    } else {
        7
    };
    let nsync = compute_nsync(&workspace.s8);
    let sync_gate_ok = passes_sync_gate_strict(&workspace.s8, min_costas_hits);
    dump_ft8b_time_chain(
        ft8b_dump_call,
        f1_candidate,
        xdt_candidate,
        depth,
        imetric,
        time0,
        freq0,
        time1,
        f1,
        xdt,
        nsync,
        min_costas_hits,
        sync_gate_ok,
    );
    trace_ft8b_time_targets(
        trace_targets(),
        f1_candidate,
        xdt_candidate,
        depth,
        imetric,
        time0,
        freq0,
        time1,
        f1,
        xdt,
        nsync,
        min_costas_hits,
        sync_gate_ok,
    );
    if !sync_gate_ok {
        // Accumulate timers before returning
        FT8B_TIMERS[0].fetch_add(t1.as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
        FT8B_TIMERS[1].fetch_add(
            (t2 - t1).as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        FT8B_TIMERS[2].fetch_add(
            (t3 - t2).as_micros() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        return None;
    }

    build_bit_metrics(workspace, imetric);
    let _t4 = t0.elapsed();

    // ── xbase: noise baseline at candidate frequency (for xsnr2) ──
    // sbase is built by sync8 with NFFT1=3840 → df=3.125 Hz/bin.
    // WSJT-X ft8b.f90: xbase = 10^(0.1*(sbase[freq_bin]-40))
    // This represents the absolute noise power at f1 in the original spectrum.
    let xbase = {
        let df_sync = SAMPLE_RATE as f64 / NFFT1 as f64; // 3.125 Hz/bin
        let freq_bin = (f1 / df_sync).round() as usize;
        if freq_bin < _sbase.len() && _sbase[freq_bin] > 0.0 {
            10.0_f64.powf(0.1 * (_sbase[freq_bin] - 40.0))
        } else {
            1e-6 // safe fallback: very low noise floor
        }
    };

    let result = try_decode_passes(workspace, depth, f1, ap_options);
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

    let (xsnr, xsnr2) = compute_snr(&workspace.s8, &result.cw, xbase);

    // WSJT-X ft8b.f90: when nagain=false (initial decode, not subtract+retry),
    // use xsnr2 (spectrum baseline) instead of xsnr (adjacent-tone).
    // nagain=false is the default case for standalone decode.
    let mut snr = if nagain { xsnr } else { xsnr2 };

    // WSJT-X ft8b.f90: false-positive bail-out
    // if (nsync.le.10 .and. xsnr.lt.-25.0) then nbadcrc=1; return
    if nsync <= 10 && snr < -25.0 {
        return None;
    }
    if snr < -25.0 {
        snr = -25.0;
    }

    // Compute itone from codeword (same as get_tones but as [i32; 79])
    let mut itone = [0i32; 79];
    let tones = get_tones(&result.cw);
    for i in 0..79 {
        itone[i] = tones[i] as i32;
    }

    dump_ft8b_message(ft8b_dump_call, &msg, snr);

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

fn find_best_time_offset(cd0_re: &[f64], cd0_im: &[f64], xdt: f64) -> TimeSearchResult {
    let i0_raw = ((xdt + 0.5) * FS2).round() as isize;
    let mut smax = 0.0;
    let mut ibest = i0_raw;
    let cs = build_costas_sync_templates();
    for offset in -10..=10 {
        let idx = i0_raw + offset;
        let sync = sync8d_isize(cd0_re, cd0_im, idx, &cs.re, &cs.im);
        if sync > smax {
            smax = sync;
            ibest = idx;
        }
    }
    TimeSearchResult {
        i0: i0_raw,
        ibest,
        sync: smax,
    }
}

fn find_best_frequency_shift(
    cd0_re: &[f64],
    cd0_im: &[f64],
    ibest: isize,
) -> FrequencySearchResult {
    let mut smax = 0.0;
    let mut delfbest = 0.0;
    let templates = build_frequency_shift_sync_templates();
    for tpl in templates {
        let sync = sync8d_isize(cd0_re, cd0_im, ibest, &tpl.re, &tpl.im);
        if sync > smax {
            smax = sync;
            delfbest = tpl.delf;
        }
    }
    FrequencySearchResult {
        delfbest,
        sync: smax,
    }
}

fn refine_time_offset(
    cd0_re: &[f64],
    cd0_im: &[f64],
    ibest: isize,
    ss: &mut [f64],
) -> TimeRefineResult {
    ss.fill(0.0);
    let cs = build_costas_sync_templates();
    for idt in -4..=4 {
        ss[(idt + 4) as usize] = sync8d_isize(cd0_re, cd0_im, ibest + idt, &cs.re, &cs.im);
    }

    let mut max_idx: isize = 4;
    let mut max_val = -1.0;
    for i in 0..9 {
        if ss[i] > max_val {
            max_val = ss[i];
            max_idx = i as isize;
        }
    }
    TimeRefineResult {
        ibest: ibest + max_idx - 4,
        offset: max_idx - 4,
        sync: max_val,
    }
}

fn extract_soft_symbols(ibest: isize, workspace: &mut DecodeWorkspace) {
    let cd0_re = &workspace.cd0_re;
    let cd0_im = &workspace.cd0_im;
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

fn passes_sync_gate_strict(s8: &[f64], min_costas_hits: usize) -> bool {
    const SYNC_TIME_SHIFTS: [usize; 3] = [0, 36, 72];
    let mut nsync = 0;

    for k in 0..COSTAS_BLOCKS {
        for &offset in &SYNC_TIME_SHIFTS {
            let mut max_tone = 0;
            let mut max_val = -1.0;
            for t in 0..8 {
                let v = s8[t * NN + k + offset];
                if v > max_val {
                    max_val = v;
                    max_tone = t;
                }
            }
            if max_tone == COSTAS[k] as usize {
                nsync += 1;
            }
        }
    }

    nsync >= min_costas_hits
}

/// Compute nsync count matching WSJT-X ft8b.f90: count of correct Costas tones.
/// Returns 0-21 (3 blocks × 7 tones).
fn compute_nsync(s8: &[f64]) -> usize {
    const SYNC_TIME_SHIFTS: [usize; 3] = [0, 36, 72];
    let mut nsync = 0;

    for k in 0..COSTAS_BLOCKS {
        for &offset in &SYNC_TIME_SHIFTS {
            let mut max_tone = 0;
            let mut max_val = -1.0;
            for t in 0..8 {
                let v = s8[t * NN + k + offset];
                if v > max_val {
                    max_val = v;
                    max_tone = t;
                }
            }
            if max_tone == COSTAS[k] as usize {
                nsync += 1;
            }
        }
    }

    nsync
}

fn build_bit_metrics(workspace: &mut DecodeWorkspace, imetric: usize) {
    workspace.bmeta.fill(0.0);
    workspace.bmetb.fill(0.0);
    workspace.bmetc.fill(0.0);
    workspace.bmetd.fill(0.0);
    workspace.bmete.fill(0.0);

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
                if imetric == 2 {
                    for i in 0..nt {
                        workspace.s2[i] *= workspace.s2[i];
                    }
                }

                let i32 = 1 + (k - 1) * 3 + (ihalf - 1) * 87;
                for ib in 0..=ibmax {
                    let mut max1 = -1e30;
                    let mut max0 = -1e30;
                    for i in 0..nt {
                        let bit_set = (i & (1 << (ibmax - ib))) != 0;
                        if bit_set {
                            if workspace.s2[i] > max1 {
                                max1 = workspace.s2[i];
                            }
                        } else {
                            if workspace.s2[i] > max0 {
                                max0 = workspace.s2[i];
                            }
                        }
                    }

                    let idx = (i32 as isize + ib as isize - 1) as usize;
                    if idx >= N_LDPC {
                        continue;
                    }

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

    for i in 0..N_LDPC {
        let vals = [workspace.bmeta[i], workspace.bmetb[i], workspace.bmetc[i]];
        workspace.bmete[i] = *vals
            .iter()
            .max_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap())
            .unwrap();
    }
    normalize_bmet(&mut workspace.bmeta);
    normalize_bmet(&mut workspace.bmetb);
    normalize_bmet(&mut workspace.bmetc);
    normalize_bmet(&mut workspace.bmetd);
    normalize_bmet(&mut workspace.bmete);
}

pub(crate) fn normalize_bmet(bmet: &mut [f64]) {
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
    if shift != 0 {
        for i in 0..NFFT2 {
            let src_idx = (i + shift) % NFFT2;
            workspace.shift_re[i] = workspace.cd0_re[src_idx];
            workspace.shift_im[i] = workspace.cd0_im[src_idx];
        }
        workspace.cd0_re.copy_from_slice(&workspace.shift_re);
        workspace.cd0_im.copy_from_slice(&workspace.shift_im);
    }

    fft_complex(&mut workspace.cd0_re, &mut workspace.cd0_im, true);

    for i in 0..NFFT2 {
        workspace.cd0_re[i] *= DOWNSAMPLE_SCALE;
        workspace.cd0_im[i] *= DOWNSAMPLE_SCALE;
    }
}

fn sync8d_isize(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i0: isize,
    sync_re: &[f64],
    sync_im: &[f64],
) -> f64 {
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

/// Compute both SNR estimates matching WSJT-X ft8b.f90.
///
/// - `xsnr`: xsig/xnoi - 1 (adjacent-tone noise)
/// - `xsnr2`: xsig/xbase/3e6 - 1 (spectrum baseline)
///
/// WSJT-X uses xsnr2 when nagain=false (initial decode), xsnr when nagain=true
/// (after subtract+retry). xbase is the noise power at f1 from the sync8 baseline.
fn compute_snr(s8: &[f64], cw: &[u8], xbase: f64) -> (f64, f64) {
    let itone = get_tones(cw);
    let mut xsig = 0.0;
    let mut xnoi = 0.0;

    for i in 0..79 {
        let tone = itone[i] as usize;
        xsig += s8[tone * NN + i].powi(2);
        let ios = (tone + 4) % 7;
        xnoi += s8[ios * NN + i].powi(2);
    }

    // xsnr: adjacent-tone noise estimate
    let mut xsnr = 0.001;
    let arg = xsig / xnoi.max(1e-30) - 1.0;
    if arg > 0.1 {
        xsnr = arg;
    }
    xsnr = 10.0 * xsnr.log10() - 27.0;

    // xsnr2: spectrum baseline estimate (WSJT-X ft8b.f90, regular decode path)
    // WSJT-X: xsnr2_arg = xsig / (xbase * 3e6) - 1
    // This regular path stores s8 from csymb/1000, so xsig is 1e6x smaller.
    // Compensate with 3 instead of 3e6. AP ft8_a7d keeps s8 unscaled.
    let mut xsnr2 = 0.001;
    let arg2 = xsig / xbase / 3.0 - 1.0;
    if arg2 > 0.1 {
        xsnr2 = arg2;
    }
    xsnr2 = 10.0 * xsnr2.log10() - 27.0;

    (xsnr, xsnr2)
}

/// Legacy SNR estimate (adjacent-tone only, for compatibility).
#[allow(dead_code)]
fn estimate_snr(s8: &[f64], cw: &[u8]) -> f64 {
    let (xsnr, _) = compute_snr(s8, cw, 1e-6); // dummy xbase, unused
    xsnr
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

const MCQ: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0,
];
const MCQRU: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0,
];
const MCQFD: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 0,
];
const MCQTEST: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 0,
];
const MCQWW: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1, 1, 0, 1, 1, 1, 1, 0,
];
const MRRR: [i8; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1];
const M73: [i8; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 1];
const MRR73: [i8; 19] = [0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 1, 0, 1, 0, 0, 1];

fn ft8_ap_set(mycall: Option<&str>, hiscall: Option<&str>, ncontest: usize) -> Ft8ApSet {
    let mut apsym = [0i8; 58];
    apsym[0] = 99;
    apsym[29] = 99;
    let mut aph10 = [0i8; 10];
    aph10[0] = 99;

    let Some(mycall_raw) = mycall.map(str::trim).filter(|s| s.len() >= 3) else {
        return Ft8ApSet { apsym, aph10 };
    };
    let mycall = mycall_raw.to_ascii_uppercase();

    let hiscall_trimmed = hiscall.map(str::trim).unwrap_or("").to_ascii_uppercase();
    let no_hiscall = hiscall_trimmed.len() < 3;
    let hiscall_for_pack = if no_hiscall {
        "KA1ABC"
    } else {
        hiscall_trimmed.as_str()
    };

    if !no_hiscall {
        let n10 = ihashcall_bits(hiscall_for_pack, 10);
        for (i, slot) in aph10.iter_mut().enumerate() {
            let bit = ((n10 >> (9 - i)) & 1) as i8;
            *slot = 2 * bit - 1;
        }
    }

    let msg = if is_stdcall(&mycall) {
        format!("{} {} RRR", mycall, hiscall_for_pack)
    } else {
        format!("<{}> {} RRR", mycall, hiscall_for_pack)
    };
    let bits = pack77(&msg);
    if bits.len() != 77 {
        return Ft8ApSet { apsym, aph10 };
    }

    let i3 = ((bits[74] as usize) << 2) | ((bits[75] as usize) << 1) | bits[76] as usize;
    let unpacked = unpack77(&bits, None);
    if ncontest == 7 && (i3 != 1 || unpacked.is_none()) {
        return Ft8ApSet { apsym, aph10 };
    }
    if ncontest <= 5 && (i3 != 1 || unpacked.as_deref() != Some(msg.as_str())) {
        return Ft8ApSet { apsym, aph10 };
    }

    for i in 0..58 {
        apsym[i] = 2 * bits[i] as i8 - 1;
    }
    if no_hiscall {
        apsym[29] = 99;
        aph10[0] = 99;
    }

    Ft8ApSet { apsym, aph10 }
}

fn ihashcall_bits(call: &str, m: usize) -> usize {
    let mut n8: u64 = 0;
    let mut count = 0;
    for c in call.chars() {
        if count >= 11 {
            break;
        }
        let uc = c.to_ascii_uppercase();
        let j = C38.iter().position(|&x| x == uc as u8).unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
        count += 1;
    }
    while count < 11 {
        let j = C38.iter().position(|&x| x == b' ').unwrap_or(0) as u64;
        n8 = 38 * n8 + j;
        count += 1;
    }
    const MAGIC: u64 = 47055833459;
    let prod = MAGIC.wrapping_mul(n8);
    ((prod >> (64 - m as u32)) & ((1u64 << m as u32) - 1)) as usize
}

fn try_decode_passes(
    workspace: &mut DecodeWorkspace,
    depth: usize,
    f1: f64,
    ap_options: &Ft8bApOptions,
) -> Option<DecodeResult> {
    let maxosd_base = if depth >= 2 { 2 } else { -1 };
    let scalefac = 2.83;
    // Passes 1-5: regular WSJT-X BP+OSD decoding with 5 bit metrics.
    workspace.apmask.fill(0);

    let nappasses = [2usize, 2, 2, 4, 4, 3];
    let naptypes = [
        [1usize, 2, 0, 0],
        [2usize, 3, 0, 0],
        [2usize, 3, 0, 0],
        [3usize, 4, 5, 6],
        [3usize, 4, 5, 6],
        [3usize, 1, 2, 0],
    ];

    let mut npasses = if (ap_options.enabled || ap_options.ncontest == 7)
        && ap_options.nzhsym >= 50
        && depth >= 2
    {
        if ap_options.cq_only {
            7
        } else {
            5 + 2 * nappasses[ap_options.nqso_progress]
        }
    } else {
        5
    };
    if ap_options.ncontest == 6 {
        npasses = 5;
    }

    for ipass in 1..=npasses {
        for i in 0..N_LDPC {
            let metric = match ipass {
                1 => workspace.bmeta[i],
                2 => workspace.bmetb[i],
                3 => workspace.bmetc[i],
                4 => workspace.bmetd[i],
                5 => workspace.bmete[i],
                _ if (ipass - 5) % 2 == 1 => workspace.bmeta[i],
                _ => workspace.bmetc[i],
            };
            workspace.llr[i] = scalefac * metric;
        }

        workspace.apmask.fill(0);
        if ipass > 5 {
            let apmag = workspace.llr.iter().map(|x| x.abs()).fold(0.0f64, f64::max) * 1.1;
            let iaptype = if ap_options.cq_only {
                1
            } else {
                naptypes[ap_options.nqso_progress][(ipass - 6) / 2]
            };

            if iaptype == 0 || !apply_wsjt_ap_mask(workspace, ap_options, iaptype, apmag, f1) {
                continue;
            }
        }

        if let Some(result) = decode174_91(&workspace.llr, &workspace.apmask, maxosd_base) {
            if result.nharderrors <= 36 {
                return Some(result);
            }
        }
    }

    None
}

fn apply_wsjt_ap_mask(
    workspace: &mut DecodeWorkspace,
    ap: &Ft8bApOptions,
    iaptype: usize,
    apmag: f64,
    f1: f64,
) -> bool {
    if ap.ncontest == 6 {
        return false;
    }
    if ap.ncontest == 7 && f1 > 950.0 {
        return false;
    }
    if ap.ncontest <= 5
        && iaptype >= 3
        && (ap.nfqso - f1).abs() > ap.napwid
        && (ap.nftx - f1).abs() > ap.napwid
    {
        return false;
    }
    if iaptype >= 2 && ap.ap_set.apsym[0] > 1 {
        return false;
    }
    if ap.ncontest == 7 && iaptype >= 2 && ap.ap_set.aph10[0] > 1 {
        return false;
    }
    if iaptype >= 3 && ap.ap_set.apsym[29] > 1 {
        return false;
    }

    match iaptype {
        1 => apply_cq_ap_mask(workspace, ap.ncontest, apmag),
        2 => apply_mycall_ap_mask(workspace, ap, apmag),
        3 => apply_mycall_dxcall_ap_mask(workspace, ap, apmag),
        4 | 5 | 6 => apply_tail_ap_mask(workspace, ap, iaptype, apmag),
        _ => false,
    }
}

fn apply_cq_ap_mask(workspace: &mut DecodeWorkspace, ncontest: usize, apmag: f64) -> bool {
    let pattern = match ncontest {
        0 | 7 => &MCQ,
        1 | 2 | 8 => &MCQTEST,
        3 => &MCQFD,
        4 => &MCQRU,
        5 => &MCQWW,
        _ => return false,
    };
    set_bits_from_zero_one(workspace, 1, pattern, apmag);
    set_i3_001(workspace, apmag);
    true
}

fn apply_mycall_ap_mask(workspace: &mut DecodeWorkspace, ap: &Ft8bApOptions, apmag: f64) -> bool {
    match ap.ncontest {
        0 | 1 | 5 | 8 => {
            set_signs(workspace, 1, &ap.ap_set.apsym[..29], apmag);
            set_i3_001(workspace, apmag);
            true
        }
        2 => {
            set_signs(workspace, 1, &ap.ap_set.apsym[..28], apmag);
            set_sign(workspace, 72, -1, apmag);
            set_sign(workspace, 73, 1, apmag);
            set_sign(workspace, 74, -1, apmag);
            set_range_sign(workspace, 75, 77, -1, apmag);
            true
        }
        3 => {
            set_signs(workspace, 1, &ap.ap_set.apsym[..28], apmag);
            set_range_sign(workspace, 75, 77, -1, apmag);
            true
        }
        4 => {
            set_signs(workspace, 2, &ap.ap_set.apsym[..28], apmag);
            set_sign(workspace, 75, -1, apmag);
            set_range_sign(workspace, 76, 77, 1, apmag);
            true
        }
        7 => {
            set_signs(workspace, 29, &ap.ap_set.apsym[..28], apmag);
            set_signs(workspace, 57, &ap.ap_set.aph10, apmag);
            set_range_sign(workspace, 72, 73, -1, apmag);
            set_sign(workspace, 74, 1, apmag);
            set_range_sign(workspace, 75, 77, -1, apmag);
            true
        }
        _ => false,
    }
}

fn apply_mycall_dxcall_ap_mask(
    workspace: &mut DecodeWorkspace,
    ap: &Ft8bApOptions,
    apmag: f64,
) -> bool {
    match ap.ncontest {
        0 | 1 | 2 | 5 | 7 | 8 => {
            set_signs(workspace, 1, &ap.ap_set.apsym, apmag);
            set_i3_001(workspace, apmag);
            true
        }
        3 => {
            set_signs(workspace, 1, &ap.ap_set.apsym[..28], apmag);
            set_signs(workspace, 29, &ap.ap_set.apsym[29..57], apmag);
            set_mask_range(workspace, 72, 74);
            set_range_sign(workspace, 75, 77, -1, apmag);
            true
        }
        4 => {
            set_signs(workspace, 2, &ap.ap_set.apsym[..28], apmag);
            set_signs(workspace, 30, &ap.ap_set.apsym[29..57], apmag);
            set_sign(workspace, 75, -1, apmag);
            set_range_sign(workspace, 76, 77, 1, apmag);
            true
        }
        _ => false,
    }
}

fn apply_tail_ap_mask(
    workspace: &mut DecodeWorkspace,
    ap: &Ft8bApOptions,
    iaptype: usize,
    apmag: f64,
) -> bool {
    if iaptype == 5 && ap.ncontest == 7 {
        return false;
    }
    if ap.ncontest <= 5 || ap.ncontest == 8 || (ap.ncontest == 7 && iaptype == 6) {
        set_mask_range(workspace, 1, 77);
        set_signs(workspace, 1, &ap.ap_set.apsym, apmag);
        let tail = match iaptype {
            4 => &MRRR,
            5 => &M73,
            _ => &MRR73,
        };
        set_bits_from_zero_one(workspace, 59, tail, apmag);
        return true;
    }
    if ap.ncontest == 7 && iaptype == 4 {
        set_signs(workspace, 1, &ap.ap_set.apsym[..28], apmag);
        set_signs(workspace, 57, &ap.ap_set.aph10, apmag);
        set_range_sign(workspace, 72, 73, -1, apmag);
        set_sign(workspace, 74, 1, apmag);
        set_range_sign(workspace, 75, 77, -1, apmag);
        return true;
    }
    false
}

fn set_bits_from_zero_one(
    workspace: &mut DecodeWorkspace,
    start_1based: usize,
    bits: &[i8],
    apmag: f64,
) {
    for (offset, &bit) in bits.iter().enumerate() {
        set_sign(
            workspace,
            start_1based + offset,
            if bit == 0 { -1 } else { 1 },
            apmag,
        );
    }
}

fn set_signs(workspace: &mut DecodeWorkspace, start_1based: usize, signs: &[i8], apmag: f64) {
    for (offset, &sign) in signs.iter().enumerate() {
        set_sign(workspace, start_1based + offset, sign, apmag);
    }
}

fn set_range_sign(
    workspace: &mut DecodeWorkspace,
    start_1based: usize,
    end_1based: usize,
    sign: i8,
    apmag: f64,
) {
    for idx in start_1based..=end_1based {
        set_sign(workspace, idx, sign, apmag);
    }
}

fn set_mask_range(workspace: &mut DecodeWorkspace, start_1based: usize, end_1based: usize) {
    for idx in start_1based..=end_1based {
        if idx <= N_LDPC {
            workspace.apmask[idx - 1] = 1;
        }
    }
}

fn set_sign(workspace: &mut DecodeWorkspace, idx_1based: usize, sign: i8, apmag: f64) {
    if idx_1based == 0 || idx_1based > N_LDPC {
        return;
    }
    let idx = idx_1based - 1;
    workspace.apmask[idx] = 1;
    workspace.llr[idx] = if sign > 0 { apmag } else { -apmag };
}

fn set_i3_001(workspace: &mut DecodeWorkspace, apmag: f64) {
    set_sign(workspace, 75, -1, apmag);
    set_sign(workspace, 76, -1, apmag);
    set_sign(workspace, 77, 1, apmag);
}

#[cfg(test)]
mod tests {
    use super::{default_outer_sync_min, finalize_sync8_candidates, Candidate};

    #[test]
    fn default_outer_sync_min_matches_wsjtx_depth_gate() {
        assert_eq!(default_outer_sync_min(1), 2.1);
        assert_eq!(default_outer_sync_min(2), 2.1);
        assert_eq!(default_outer_sync_min(3), 1.3);
    }

    #[test]
    fn sync8_candidate_order_matches_wsjtx_priority_rules() {
        let mut candidate0 = vec![
            Candidate {
                freq: 100.0,
                dt: 0.00,
                sync: 4.5,
            },
            Candidate {
                freq: 102.5,
                dt: 0.02,
                sync: 5.0,
            },
            Candidate {
                freq: 210.0,
                dt: -0.01,
                sync: 6.0,
            },
            Candidate {
                freq: 300.0,
                dt: 0.10,
                sync: 8.0,
            },
            Candidate {
                freq: 400.0,
                dt: 0.20,
                sync: 3.0,
            },
        ];

        let ordered = finalize_sync8_candidates(&mut candidate0, 4.0, 212.0, 3);

        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].freq, 210.0);
        assert_eq!(ordered[1].freq, 300.0);
        assert_eq!(ordered[2].freq, 102.5);
    }
}
