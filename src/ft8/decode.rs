/// FT8 decoder - Rust port of decode.ts
use crate::ft8::constants::{COSTAS, GRAY_MAP};
use crate::ft8::decode174_91::{decode174_91, DecodeResult};
use crate::ft8::hashcall::HashCallBook;
use crate::ft8::indexx::indexx_ascending;
use crate::ft8::pack_jt77::{is_stdcall, pack77};
use crate::ft8::protocol::{C38, N_LDPC, SAMPLE_RATE};
use crate::ft8::unpack_jt77::{unpack77, unpack77_with_context, UnpackContext};
use crate::util::{four2a_c2c, four2a_r2c, sync8_fft_size};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[path = "ft8_downsample.rs"]
mod ft8_downsample;
#[path = "ft8b.rs"]
mod ft8b;
#[path = "sync8.rs"]
mod sync8;

pub(crate) use self::ft8b::normalize_bmet;
use self::ft8b::{duration_ms, ft8_ap_set, ft8b, trace_timer, trace_timers_enabled};
use self::sync8::sync8;

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
pub(crate) const NFFT2: usize = 3200;
pub(crate) const NP2: usize = 2812;
pub(crate) const COSTAS_BLOCKS: usize = 7;
pub(crate) const COSTAS_SYMBOL_LEN: usize = 32;
pub(crate) const TAPER_SIZE: usize = 101;
pub(crate) const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
const TWO_PI_F32: f32 = std::f32::consts::PI * 2.0;
const PI_F32: f32 = std::f32::consts::PI;

pub(crate) const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
pub(crate) const DT2: f64 = 1.0 / FS2;
pub(crate) const DOWNSAMPLE_DF: f32 = SAMPLE_RATE as f32 / NFFT1_LONG as f32;
pub(crate) const DOWNSAMPLE_BAUD: f32 = SAMPLE_RATE as f32 / NSPS as f32;
/// WSJT-X ft8_downsample.f90:
/// `fac=1.0/sqrt(float(NFFT1)*NFFT2)` after the unnormalized inverse FFT.
pub(crate) const DOWNSAMPLE_FAC: f64 = 0.000040343576984014362f64;

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

#[allow(non_snake_case)]
#[derive(Default)]
pub struct DecodeOptions {
    pub sample_rate: Option<usize>,
    pub nfa: Option<f64>,
    pub nfb: Option<f64>,
    pub syncmin: Option<f64>,
    pub ndepth: Option<usize>,
    pub ncand: Option<usize>,
    pub hashcallbook: Option<HashCallBook>,
    pub mycall: Option<String>,
    pub hiscall: Option<String>,
    pub nfqso: Option<f64>,
    pub nftx: Option<f64>,
    pub nQSOProgress: Option<usize>,
    pub ncontest: Option<usize>,
    pub napwid: Option<f64>,
    pub lft8apon: Option<bool>,
    pub lapcqonly: Option<bool>,
    pub nagain: Option<bool>,
    pub nzhsym: Option<usize>,
    /// Messages already decoded in an earlier progressive stage for the same
    /// slot. WSJT-X carries these in `allmessages`/`ndecodes` when nzhsym=50.
    pub initial_messages: Vec<String>,
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
    ibest: isize,
}

#[derive(Clone, Copy, Debug)]
struct FrequencySearchResult {
    delfbest: f64,
}

#[derive(Clone, Copy, Debug)]
struct TimeRefineResult {
    ibest: isize,
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
    mycall: Option<String>,
    hiscall: Option<String>,
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

#[derive(Default)]
struct Ft8bStats {
    calls: usize,
    sync_rejects: usize,
    decode_failures: usize,
    downsample: Duration,
    align: Duration,
    symbols: Duration,
    metrics: Duration,
    ldpc: Duration,
    post: Duration,
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
            let x = (i as f32 * PI_F32) / last as f32;
            t[i] = (0.5f32 * (1.0f32 + x.cos())) as f64;
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
            let mut phi = 0.0f32;
            let dphi = TWO_PI_F32 * COSTAS[i] as f32 / COSTAS_SYMBOL_LEN as f32;
            for j in 0..COSTAS_SYMBOL_LEN {
                re[i * COSTAS_SYMBOL_LEN + j] = phi.cos() as f64;
                im[i * COSTAS_SYMBOL_LEN + j] = phi.sin() as f64;
                phi = (phi + dphi) % TWO_PI_F32;
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
            let dphi = TWO_PI_F32 * delf as f32 * DT2 as f32;
            let mut twk_re = vec![0.0; COSTAS_SYMBOL_LEN];
            let mut twk_im = vec![0.0; COSTAS_SYMBOL_LEN];
            let mut phi = 0.0f32;
            for j in 0..COSTAS_SYMBOL_LEN {
                twk_re[j] = phi.cos() as f64;
                twk_im[j] = phi.sin() as f64;
                phi = (phi + dphi) % TWO_PI_F32;
            }
            let mut re = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
            let mut im = vec![0.0; COSTAS_BLOCKS * COSTAS_SYMBOL_LEN];
            for i in 0..COSTAS_BLOCKS {
                for j in 0..COSTAS_SYMBOL_LEN {
                    let idx = i * COSTAS_SYMBOL_LEN + j;
                    let twk_re = twk_re[j] as f32;
                    let twk_im = twk_im[j] as f32;
                    let cs_re = cs.re[idx] as f32;
                    let cs_im = cs.im[idx] as f32;
                    re[idx] = (twk_re * cs_re - twk_im * cs_im) as f64;
                    im[idx] = (twk_re * cs_im + twk_im * cs_re) as f64;
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
    _t_dd: std::time::Duration,
    _t_start: std::time::Instant,
) -> (Vec<DecodedMessage>, Vec<f64>, Vec<f64>) {
    let t_decode_total = Instant::now();
    // Truncate to NMAX (15s @ 12kHz = 180000 samples) matching WSJT-X NPTS
    if dd.len() > NMAX {
        dd.truncate(NMAX);
    }
    let nfa = options.nfa.unwrap_or(200.0);
    let nfb = options.nfb.unwrap_or(3000.0);
    let ndepth = options.ndepth.unwrap_or(3);
    let syncmin = options
        .syncmin
        .unwrap_or_else(|| default_outer_sync_min(ndepth));
    let ncand = options.ncand.unwrap_or(1000);
    let book = options.hashcallbook;
    let sync_mode = options.sync_mode.unwrap_or(SyncMode::Power);
    let nfqso = options.nfqso.unwrap_or(0.0);
    let nagain = options.nagain.unwrap_or(false);
    let ncontest = options.ncontest.unwrap_or(0);
    let mycall = options.mycall.clone();
    let hiscall = options.hiscall.clone();
    let nzhsym = options.nzhsym.unwrap_or(50);
    let ap_options = Ft8bApOptions {
        enabled: options.lft8apon.unwrap_or(true),
        cq_only: options.lapcqonly.unwrap_or(false),
        nqso_progress: options.nQSOProgress.unwrap_or(0).min(5),
        ncontest,
        nfqso,
        nftx: options.nftx.unwrap_or(0.0),
        napwid: options.napwid.unwrap_or(50.0),
        nzhsym,
        ap_set: ft8_ap_set(mycall.as_deref(), hiscall.as_deref(), ncontest),
        mycall,
        hiscall,
    };
    let mut residual = dd.clone();

    let mut cx_re = vec![0.0; NFFT1_LONG];
    let mut cx_im = vec![0.0; NFFT1_LONG];

    let mut decoded: Vec<DecodedMessage> = Vec::new();
    let mut seen_messages: std::collections::HashSet<String> = options
        .initial_messages
        .iter()
        .map(|msg| normalize_message_key(msg))
        .collect();
    let mut ndecodes_total = seen_messages.len();
    let max_passes = if ndepth == 1 { 2 } else { 3 };

    // WSJT-X sync8 refreshes sbase for each pass on the current residual.
    let mut sbase: Vec<f64> = Vec::new();
    for pass_idx in 0..max_passes {
        if pass_idx == 2 && ndecodes_total == 0 {
            trace_timer(
                "decode.pass.skip",
                t_decode_total,
                Some(format!("nzhsym={nzhsym} pass={}", pass_idx + 1)),
            );
            continue;
        }
        let t_pass = Instant::now();
        let pass_syncmin = syncmin;
        cx_re.fill(0.0);
        cx_im.fill(0.0);

        let t_fft = Instant::now();
        cx_re[..residual.len()].copy_from_slice(&residual);
        four2a_r2c(&mut cx_re, &mut cx_im);
        trace_timer(
            "decode.pass.fft",
            t_fft,
            Some(format!("nzhsym={nzhsym} pass={}", pass_idx + 1)),
        );

        let (ifa, ifb) = if nagain {
            (nfqso - 20.0, nfqso + 20.0)
        } else {
            (nfa, nfb)
        };

        let t_sync = Instant::now();
        let (candidates, pass_sbase) =
            sync8(&residual, ifa, ifb, pass_syncmin, nfqso, ncand, sync_mode);
        sbase = pass_sbase;
        trace_timer(
            "decode.pass.sync8",
            t_sync,
            Some(format!(
                "nzhsym={nzhsym} pass={} candidates={}",
                pass_idx + 1,
                candidates.len()
            )),
        );

        // WSJT-X ft8_decode.f90: pass 1 uses imetric=1, passes 2/3 use imetric=2.
        let pass_imetric = if pass_idx == 0 { 1 } else { 2 };

        let mut cand_ws = create_decode_workspace();
        let mut ft8b_stats = trace_timers_enabled().then(Ft8bStats::default);
        // Candidate decoding is sequential so each accepted signal updates the residual
        // before later candidates are evaluated.
        let t_candidates = Instant::now();
        let decoded_before = decoded.len();
        let mut accepted = 0usize;
        let mut duplicates = 0usize;
        for cand in &candidates {
            if let Some(r) = ft8b(
                &residual,
                &cx_re,
                &cx_im,
                cand.freq,
                cand.dt,
                &sbase,
                ndepth,
                pass_imetric,
                nagain,
                &ap_options,
                &book,
                None,
                &mut cand_ws,
                ft8b_stats.as_mut(),
            ) {
                let message_key = normalize_message_key(&r.msg);
                crate::ft8::subtract_ft8::subtract_ft8(&mut residual, &r.itone, r.freq, r.dt);
                if seen_messages.contains(&message_key) {
                    duplicates += 1;
                    continue;
                }
                seen_messages.insert(message_key);
                ndecodes_total += 1;
                accepted += 1;
                decoded.push(DecodedMessage {
                    freq: r.freq,
                    dt: r.dt - 0.5,
                    snr: r.snr,
                    msg: r.msg.clone(),
                    sync: cand.sync,
                    itone: r.itone.to_vec(),
                });
            }
        }
        trace_timer(
            "decode.pass.candidates",
            t_candidates,
            Some(format!(
                "nzhsym={nzhsym} pass={} accepted={accepted} duplicates={duplicates} total_decoded={}",
                pass_idx + 1,
                decoded.len()
            )),
        );
        if let Some(stats) = &ft8b_stats {
            trace_timer(
                "decode.pass.ft8b",
                t_candidates,
                Some(format!(
                    "nzhsym={nzhsym} pass={} calls={} sync_rejects={} decode_failures={} downsample={:.1}ms align={:.1}ms symbols={:.1}ms metrics={:.1}ms ldpc={:.1}ms post={:.1}ms",
                    pass_idx + 1,
                    stats.calls,
                    stats.sync_rejects,
                    stats.decode_failures,
                    duration_ms(stats.downsample),
                    duration_ms(stats.align),
                    duration_ms(stats.symbols),
                    duration_ms(stats.metrics),
                    duration_ms(stats.ldpc),
                    duration_ms(stats.post),
                )),
            );
        }
        trace_timer(
            "decode.pass.total",
            t_pass,
            Some(format!(
                "nzhsym={nzhsym} pass={} new_decodes={}",
                pass_idx + 1,
                decoded.len() - decoded_before
            )),
        );
    }

    trace_timer(
        "decode.total",
        t_decode_total,
        Some(format!("nzhsym={nzhsym} decoded={}", decoded.len())),
    );
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

pub(crate) fn compute_baseline(savg: &[f64], nfa: f64, nfb: f64, df: f64, nh1: usize) -> Vec<f64> {
    // WSJT-X stores sbase(1:NH1), with FFT bin 0/DC omitted. Keep index 0
    // unused so callers can use nint(f/df) directly, matching Fortran.
    let mut sbase = vec![0.0; nh1 + 1];
    let ia = nint_wsjtx_f32(nfa / df).max(1) as usize;
    let ib = (nint_wsjtx_f32(nfb / df).max(0) as usize).min(nh1);

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
    let indx = indexx_ascending(slice);
    let idx = ((slice.len() as f64 * 0.01 * k as f64).round() as usize)
        .min(slice.len())
        .max(1)
        - 1;
    slice[indx[idx]]
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
fn nuttall_window(n: usize) -> Vec<f64> {
    let mut w = vec![0.0; n];
    let nf = n as f64;
    let a0 = 0.3635819;
    let a1 = -0.4891775;
    let a2 = 0.1365995;
    let a3 = -0.0106411;
    for i in 0..n {
        let x = 2.0 * std::f64::consts::PI * i as f64 / nf;
        w[i] = a0 + a1 * x.cos() + a2 * (2.0 * x).cos() + a3 * (3.0 * x).cos();
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
        four2a_r2c(&mut x_re, &mut x_im);
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

fn nint_wsjtx_f32(x: f64) -> isize {
    (x as f32).round() as isize
}

fn nint_wsjtx_real(x: f32) -> isize {
    x.round() as isize
}

#[cfg(test)]
mod tests {
    use super::ft8b::{apply_wsjt_ap_mask, is_acceptable_unpacked_message, M73, MCQ, MRR73, MRRR};
    use super::sync8::finalize_sync8_candidates;
    use super::*;

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

    #[test]
    fn non_contest_quirk_gate_matches_wsjtx() {
        assert!(is_acceptable_unpacked_message(
            "CQ 001 IZ7MMG 549 2025",
            3,
            0
        ));
        assert!(!is_acceptable_unpacked_message(
            "TU; K1ABC W9XYZ 549 CA",
            3,
            0
        ));
        assert!(!is_acceptable_unpacked_message("K1ABC/R W9XYZ 73", 1, 0));
    }

    fn test_ap_options() -> Ft8bApOptions {
        Ft8bApOptions {
            enabled: true,
            cq_only: false,
            nqso_progress: 0,
            ncontest: 0,
            nfqso: 1500.0,
            nftx: 1500.0,
            napwid: 100.0,
            nzhsym: 50,
            ap_set: Ft8ApSet {
                apsym: [1; 58],
                aph10: [1; 10],
            },
            mycall: Some("K1ABC".to_string()),
            hiscall: Some("W9XYZ".to_string()),
        }
    }

    fn assert_ap_sign(workspace: &DecodeWorkspace, idx_1based: usize, sign: i8, apmag: f64) {
        let idx = idx_1based - 1;
        assert_eq!(workspace.apmask[idx], 1, "AP mask bit {idx_1based}");
        assert_eq!(
            workspace.llr[idx],
            if sign > 0 { apmag } else { -apmag },
            "AP LLR bit {idx_1based}"
        );
    }

    #[test]
    fn cq_ap_mask_matches_wsjtx_bit_positions() {
        let ap = test_ap_options();
        let apmag = 7.0;
        let mut workspace = create_decode_workspace();

        assert!(apply_wsjt_ap_mask(&mut workspace, &ap, 1, apmag, 1500.0));

        for (offset, bit) in MCQ.iter().enumerate() {
            let sign = if *bit == 0 { -1 } else { 1 };
            assert_ap_sign(&workspace, offset + 1, sign, apmag);
        }
        assert_ap_sign(&workspace, 75, -1, apmag);
        assert_ap_sign(&workspace, 76, -1, apmag);
        assert_ap_sign(&workspace, 77, 1, apmag);

        let masked = workspace.apmask.iter().filter(|&&bit| bit == 1).count();
        assert_eq!(masked, 32);
    }

    #[test]
    fn standard_ap_masks_match_wsjtx_mycall_and_dxcall_positions() {
        let ap = test_ap_options();
        let apmag = 6.0;

        let mut mycall_workspace = create_decode_workspace();
        assert!(apply_wsjt_ap_mask(
            &mut mycall_workspace,
            &ap,
            2,
            apmag,
            1500.0
        ));
        for idx in 1..=29 {
            assert_ap_sign(&mycall_workspace, idx, 1, apmag);
        }
        assert_ap_sign(&mycall_workspace, 75, -1, apmag);
        assert_ap_sign(&mycall_workspace, 76, -1, apmag);
        assert_ap_sign(&mycall_workspace, 77, 1, apmag);
        let masked = mycall_workspace
            .apmask
            .iter()
            .filter(|&&bit| bit == 1)
            .count();
        assert_eq!(masked, 32);

        let mut dxcall_workspace = create_decode_workspace();
        assert!(apply_wsjt_ap_mask(
            &mut dxcall_workspace,
            &ap,
            3,
            apmag,
            1500.0
        ));
        for idx in 1..=58 {
            assert_ap_sign(&dxcall_workspace, idx, 1, apmag);
        }
        assert_ap_sign(&dxcall_workspace, 75, -1, apmag);
        assert_ap_sign(&dxcall_workspace, 76, -1, apmag);
        assert_ap_sign(&dxcall_workspace, 77, 1, apmag);
        let masked = dxcall_workspace
            .apmask
            .iter()
            .filter(|&&bit| bit == 1)
            .count();
        assert_eq!(masked, 61);
    }

    #[test]
    fn tail_ap_masks_match_wsjtx_rrr_73_rr73_bits() {
        for (iaptype, tail) in [(4, MRRR), (5, M73), (6, MRR73)] {
            let ap = test_ap_options();
            let apmag = 5.5;
            let mut workspace = create_decode_workspace();

            assert!(apply_wsjt_ap_mask(
                &mut workspace,
                &ap,
                iaptype,
                apmag,
                1500.0
            ));

            for idx in 1..=58 {
                assert_ap_sign(&workspace, idx, 1, apmag);
            }
            for (offset, bit) in tail.iter().enumerate() {
                let sign = if *bit == 0 { -1 } else { 1 };
                assert_ap_sign(&workspace, 59 + offset, sign, apmag);
            }
            let masked = workspace.apmask.iter().filter(|&&bit| bit == 1).count();
            assert_eq!(masked, 77);
        }
    }

    #[test]
    fn ap_mask_gates_match_wsjtx_contest_and_call_requirements() {
        let apmag = 4.0;

        let mut fox_contest = test_ap_options();
        fox_contest.ncontest = 6;
        let mut workspace = create_decode_workspace();
        assert!(!apply_wsjt_ap_mask(
            &mut workspace,
            &fox_contest,
            1,
            apmag,
            1500.0
        ));
        assert!(workspace.apmask.iter().all(|&bit| bit == 0));

        let mut field_day = test_ap_options();
        field_day.ncontest = 7;
        let mut workspace = create_decode_workspace();
        assert!(!apply_wsjt_ap_mask(
            &mut workspace,
            &field_day,
            1,
            apmag,
            951.0
        ));
        assert!(workspace.apmask.iter().all(|&bit| bit == 0));

        let mut missing_dx = test_ap_options();
        missing_dx.ap_set.apsym[29] = 99;
        let mut workspace = create_decode_workspace();
        assert!(!apply_wsjt_ap_mask(
            &mut workspace,
            &missing_dx,
            3,
            apmag,
            1500.0
        ));
        assert!(workspace.apmask.iter().all(|&bit| bit == 0));
    }

    #[test]
    fn ap_mask_matrix_covers_wsjtx_contest_iaptype_shape() {
        let cases = [
            (0, 1, Some(32)),
            (0, 2, Some(32)),
            (0, 3, Some(61)),
            (0, 4, Some(77)),
            (0, 5, Some(77)),
            (0, 6, Some(77)),
            (1, 1, Some(32)),
            (1, 2, Some(32)),
            (1, 3, Some(61)),
            (1, 4, Some(77)),
            (1, 5, Some(77)),
            (1, 6, Some(77)),
            (2, 1, Some(32)),
            (2, 2, Some(34)),
            (2, 3, Some(61)),
            (2, 4, Some(77)),
            (2, 5, Some(77)),
            (2, 6, Some(77)),
            (3, 1, Some(32)),
            (3, 2, Some(31)),
            (3, 3, Some(62)),
            (3, 4, Some(77)),
            (3, 5, Some(77)),
            (3, 6, Some(77)),
            (4, 1, Some(32)),
            (4, 2, Some(31)),
            (4, 3, Some(59)),
            (4, 4, Some(77)),
            (4, 5, Some(77)),
            (4, 6, Some(77)),
            (5, 1, Some(32)),
            (5, 2, Some(32)),
            (5, 3, Some(61)),
            (5, 4, Some(77)),
            (5, 5, Some(77)),
            (5, 6, Some(77)),
            (6, 1, None),
            (6, 2, None),
            (6, 3, None),
            (6, 4, None),
            (6, 5, None),
            (6, 6, None),
            (7, 1, Some(32)),
            (7, 2, Some(44)),
            (7, 3, Some(61)),
            (7, 4, Some(44)),
            (7, 5, None),
            (7, 6, Some(77)),
            (8, 1, Some(32)),
            (8, 2, Some(32)),
            (8, 3, Some(61)),
            (8, 4, Some(77)),
            (8, 5, Some(77)),
            (8, 6, Some(77)),
        ];

        for (ncontest, iaptype, expected_count) in cases {
            let mut ap = test_ap_options();
            ap.ncontest = ncontest;
            let mut workspace = create_decode_workspace();
            let f1 = if ncontest == 7 { 900.0 } else { 1500.0 };
            let accepted = apply_wsjt_ap_mask(&mut workspace, &ap, iaptype, 3.0, f1);
            assert_eq!(
                accepted,
                expected_count.is_some(),
                "ncontest={ncontest} iaptype={iaptype}"
            );
            let count = workspace.apmask.iter().filter(|&&bit| bit == 1).count();
            assert_eq!(
                count,
                expected_count.unwrap_or(0),
                "ncontest={ncontest} iaptype={iaptype}"
            );
        }
    }
}
