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
        let mut no_coarse_frequency_uses: std::collections::HashMap<i32, usize> =
            std::collections::HashMap::new();
        let mut no_coarse_downsample_cache: std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)> =
            std::collections::HashMap::new();
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
                &mut no_coarse_downsample_cache,
                &mut no_coarse_frequency_uses,
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
    let jz = 62;
    let fft_size = sync8_fft_size();
    let half_size = fft_size / 2;
    let tstep = NSTEP as f64 / SAMPLE_RATE as f64;
    let df = SAMPLE_RATE as f64 / fft_size as f64;
    let fac = 1.0 / 300.0;
    let width = 2 * jz + 1;
    let nssy = NSPS / NSTEP;
    // WSJT-X sync8.f90: `nfos=NFFT1/NSPS`.
    let nfos = fft_size / NSPS;
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
            four2a_r2c(x_re, x_im);
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

        let ia = nint_wsjtx_f32(nfa / df).max(1) as usize;
        let ib = (nint_wsjtx_f32(nfb / df).max(0) as usize).min(half_size - 14);

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

        // WSJT-X style: normalize red by 40th percentile, then normalize red2 similarly.
        // sync8.f90 uses indexx for both percentile selection and candidate ordering.
        let npctile = nint_wsjtx_f32(0.40 * iz as f64).max(1) as usize;
        {
            let indx = indexx_ascending(&red);
            let base = red[indx[npctile - 1]].max(1e-30);
            for v in red.iter_mut() {
                *v /= base;
            }
        }
        {
            let indx2 = indexx_ascending(&red2);
            let base2 = red2[indx2[npctile - 1]].max(1e-30);
            for v in red2.iter_mut() {
                *v /= base2;
            }
        }

        let order = indexx_ascending(&red);
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

        (candidates, sbase)
    })
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
            // WSJT-X uses single-precision REAL values here. Adjacent 0.04 s
            // time-grid candidates land just outside the strict `tdiff < 0.04`
            // duplicate window; avoid f64 roundoff suppressing that boundary.
            if fdiff.abs() < 4.0 && tdiff < 0.04 - 1e-12 {
                if candidate0[i].sync >= candidate0[j].sync {
                    candidate0[j].sync = 0.0;
                } else {
                    candidate0[i].sync = 0.0;
                }
            }
        }
    }

    let sync_values: Vec<f64> = candidate0.iter().map(|c| c.sync).collect();
    let sorted_idx = indexx_ascending(&sync_values);

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
    _book: &Option<HashCallBook>,
    _sbase_welch: Option<&[f64]>,
    workspace: &mut DecodeWorkspace,
    coarse_downsample_cache: &mut std::collections::HashMap<i32, (Vec<f64>, Vec<f64>)>,
    coarse_frequency_uses: &mut std::collections::HashMap<i32, usize>,
    mut stats: Option<&mut Ft8bStats>,
) -> Option<Ft8bResult> {
    if let Some(stats) = stats.as_deref_mut() {
        stats.calls += 1;
    }
    let collect_stats = stats.is_some();
    let t_downsample = collect_stats.then(Instant::now);
    load_coarse_downsample(
        cx_re,
        cx_im,
        f1,
        workspace,
        coarse_downsample_cache,
        coarse_frequency_uses,
    );
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.downsample, t_downsample);
    }

    let t_align = collect_stats.then(Instant::now);
    let time0 = find_best_time_offset(&workspace.cd0_re, &workspace.cd0_im, xdt);
    let freq0 = find_best_frequency_shift(&workspace.cd0_re, &workspace.cd0_im, time0.ibest);
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.align, t_align);
    }
    f1 += freq0.delfbest;
    let t_downsample = collect_stats.then(Instant::now);
    ft8_downsample(cx_re, cx_im, f1, workspace);
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.downsample, t_downsample);
    }

    let t_align = collect_stats.then(Instant::now);
    let time1 = refine_time_offset(
        &workspace.cd0_re,
        &workspace.cd0_im,
        time0.ibest,
        &mut workspace.ss,
    );
    let ibest = time1.ibest;
    let xdt = (ibest as f64 - 1.0) * DT2;
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.align, t_align);
    }

    let t_symbols = collect_stats.then(Instant::now);
    extract_soft_symbols(ibest, workspace);

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
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.symbols, t_symbols);
    }
    if nsync < min_costas_hits {
        if let Some(stats) = stats.as_deref_mut() {
            stats.sync_rejects += 1;
        }
        return None;
    }

    let t_metrics = collect_stats.then(Instant::now);
    build_bit_metrics(workspace, imetric);
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.metrics, t_metrics);
    }

    // ── xbase: noise baseline at candidate frequency (for xsnr2) ──
    // sbase is built by sync8 with NFFT1=3840 → df=3.125 Hz/bin.
    // WSJT-X ft8b.f90: xbase = 10^(0.1*(sbase[freq_bin]-40))
    // This represents the absolute noise power at f1 in the original spectrum.
    let xbase = {
        let df_sync = SAMPLE_RATE as f64 / NFFT1 as f64; // 3.125 Hz/bin
        let freq_bin = nint_wsjtx_f32(f1 / df_sync).max(0) as usize;
        if freq_bin < _sbase.len() && _sbase[freq_bin] > 0.0 {
            (10.0f32.powf(0.1 * (_sbase[freq_bin] as f32 - 40.0))) as f64
        } else {
            1e-6 // safe fallback: very low noise floor
        }
    };

    let t_ldpc = collect_stats.then(Instant::now);
    let result = try_decode_passes(workspace, depth, f1, ap_options, _book);
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.ldpc, t_ldpc);
    }
    let Some(result) = result else {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
        }
        return None;
    };

    let t_post = collect_stats.then(Instant::now);
    if result.cw.iter().all(|&b| b == 0) {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
            add_elapsed(&mut stats.post, t_post);
        }
        return None;
    }

    let message77 = &result.message91[..77];
    let (_n3v, i3v) = message_type(message77);
    if !is_valid_message_type(message77) {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
            add_elapsed(&mut stats.post, t_post);
        }
        return None;
    }

    let unpack_context = UnpackContext::with_calls(
        _book.as_ref(),
        ap_options.mycall.as_deref(),
        ap_options.hiscall.as_deref(),
    );
    let msg = unpack77_with_context(message77, unpack_context);
    let Some(msg) = msg else {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
            add_elapsed(&mut stats.post, t_post);
        }
        return None;
    };
    if ap_options.ncontest == 0
        && (1..=3).contains(&i3v)
        && (msg.contains("/R") || msg.starts_with("TU; "))
    {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
            add_elapsed(&mut stats.post, t_post);
        }
        return None;
    }
    if msg.trim().is_empty() {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
            add_elapsed(&mut stats.post, t_post);
        }
        return None;
    }

    let tones = get_tones(&result.cw);
    let (xsnr, xsnr2) = compute_snr(&workspace.s8, &tones, xbase);

    // WSJT-X ft8b.f90: when nagain=false (initial decode, not subtract+retry),
    // use xsnr2 (spectrum baseline) instead of xsnr (adjacent-tone).
    // nagain=false is the default case for standalone decode.
    let mut snr = if nagain { xsnr } else { xsnr2 };

    // WSJT-X ft8b.f90: false-positive bail-out
    // if (nsync.le.10 .and. xsnr.lt.-25.0) then nbadcrc=1; return
    if nsync <= 10 && snr < -25.0 {
        if let Some(stats) = stats.as_deref_mut() {
            stats.decode_failures += 1;
            add_elapsed(&mut stats.post, t_post);
        }
        return None;
    }
    if snr < -25.0 {
        snr = -25.0;
    }

    // Compute itone from codeword (same as get_tones but as [i32; 79])
    let mut itone = [0i32; 79];
    for i in 0..79 {
        itone[i] = tones[i] as i32;
    }
    if let Some(stats) = stats.as_deref_mut() {
        add_elapsed(&mut stats.post, t_post);
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

fn find_best_time_offset(cd0_re: &[f64], cd0_im: &[f64], xdt: f64) -> TimeSearchResult {
    let i0_raw = nint_wsjtx_f32((xdt + 0.5) * FS2);
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
    TimeSearchResult { ibest }
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
    FrequencySearchResult { delfbest }
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

        four2a_c2c(&mut workspace.symb_re, &mut workspace.symb_im, -1);
        for tone in 0..8 {
            let re = (workspace.symb_re[tone] as f32 / 1000.0) as f64;
            let im = (workspace.symb_im[tone] as f32 / 1000.0) as f64;
            let idx = tone * NN + k;
            workspace.cs_re[idx] = re;
            workspace.cs_im[idx] = im;
            let s8_re = workspace.symb_re[tone] as f32;
            let s8_im = workspace.symb_im[tone] as f32;
            workspace.s8[idx] = wsjtx_cabs(s8_re, s8_im) as f64;
        }
    }
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
                        workspace.s2[i] = wsjtx_cabs(re as f32, im as f32) as f64;
                    } else if nsym == 2 {
                        let s_re = workspace.cs_re[GRAY_MAP[i2] as usize * NN + ks - 1] as f32
                            + workspace.cs_re[GRAY_MAP[i3] as usize * NN + ks] as f32;
                        let s_im = workspace.cs_im[GRAY_MAP[i2] as usize * NN + ks - 1] as f32
                            + workspace.cs_im[GRAY_MAP[i3] as usize * NN + ks] as f32;
                        workspace.s2[i] = wsjtx_cabs(s_re, s_im) as f64;
                    } else {
                        let s_re = workspace.cs_re[GRAY_MAP[i1] as usize * NN + ks - 1] as f32
                            + workspace.cs_re[GRAY_MAP[i2] as usize * NN + ks] as f32
                            + workspace.cs_re[GRAY_MAP[i3] as usize * NN + ks + 1] as f32;
                        let s_im = workspace.cs_im[GRAY_MAP[i1] as usize * NN + ks - 1] as f32
                            + workspace.cs_im[GRAY_MAP[i2] as usize * NN + ks] as f32
                            + workspace.cs_im[GRAY_MAP[i3] as usize * NN + ks + 1] as f32;
                        workspace.s2[i] = wsjtx_cabs(s_re, s_im) as f64;
                    }
                }
                if imetric == 2 {
                    for i in 0..nt {
                        let v = workspace.s2[i] as f32;
                        workspace.s2[i] = (v * v) as f64;
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

                    let bm = ((max1 as f32) - (max0 as f32)) as f64;
                    if nsym == 1 {
                        workspace.bmeta[idx] = bm;
                        let den = (max1 as f32).max(max0 as f32);
                        workspace.bmetd[idx] = if den > 0.0 {
                            ((bm as f32) / den) as f64
                        } else {
                            0.0
                        };
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
        let temp = [workspace.bmeta[i], workspace.bmetb[i], workspace.bmetc[i]];
        workspace.bmete[i] = maxloc_abs_first(&temp);
    }
    normalize_bmet(&mut workspace.bmeta);
    normalize_bmet(&mut workspace.bmetb);
    normalize_bmet(&mut workspace.bmetc);
    normalize_bmet(&mut workspace.bmetd);
    normalize_bmet(&mut workspace.bmete);
}

fn wsjtx_cabs(re: f32, im: f32) -> f32 {
    (re * re + im * im).sqrt()
}

fn maxloc_abs_first(temp: &[f64]) -> f64 {
    let mut ip = 0usize;
    let mut vmax = temp[0].abs();
    for (i, value) in temp.iter().enumerate().skip(1) {
        let avalue = value.abs();
        if avalue > vmax {
            vmax = avalue;
            ip = i;
        }
    }
    temp[ip]
}

pub(crate) fn normalize_bmet(bmet: &mut [f64]) {
    let n = bmet.len();
    let mut sum = 0.0f32;
    let mut sum2 = 0.0f32;
    for i in 0..n {
        let v = bmet[i] as f32;
        sum += v;
        sum2 += v * v;
    }
    let avg = sum / n as f32;
    let avg2 = sum2 / n as f32;
    let variance = avg2 - avg * avg;
    let sigma = if variance > 0.0 {
        variance.sqrt()
    } else {
        avg2.sqrt()
    };
    if sigma > 0.0 {
        for i in 0..n {
            bmet[i] = ((bmet[i] as f32) / sigma) as f64;
        }
    }
}

fn ft8_downsample(cx_re: &[f64], cx_im: &[f64], f0: f64, workspace: &mut DecodeWorkspace) {
    let df = DOWNSAMPLE_DF;
    let baud = DOWNSAMPLE_BAUD;
    let f0 = f0 as f32;
    let i0 = nint_wsjtx_real(f0 / df).max(0) as usize;
    let ft = f0 + 8.5f32 * baud;
    let it = (nint_wsjtx_real(ft / df).max(0) as usize).min(NFFT1_LONG / 2);
    let fb = f0 - 1.5f32 * baud;
    let ib = 1.max(nint_wsjtx_real(fb / df).max(0) as usize);

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

    let shift = i0 as isize - ib as isize;
    if shift != 0 {
        for i in 0..NFFT2 {
            let src_idx = (i as isize + shift).rem_euclid(NFFT2 as isize) as usize;
            workspace.shift_re[i] = workspace.cd0_re[src_idx];
            workspace.shift_im[i] = workspace.cd0_im[src_idx];
        }
        workspace.cd0_re.copy_from_slice(&workspace.shift_re);
        workspace.cd0_im.copy_from_slice(&workspace.shift_im);
    }

    four2a_c2c(&mut workspace.cd0_re, &mut workspace.cd0_im, 1);

    for i in 0..NFFT2 {
        workspace.cd0_re[i] *= DOWNSAMPLE_FAC;
        workspace.cd0_im[i] *= DOWNSAMPLE_FAC;
    }
}

fn nint_wsjtx_f32(x: f64) -> isize {
    (x as f32).round() as isize
}

fn nint_wsjtx_real(x: f32) -> isize {
    x.round() as isize
}

fn sync8d_isize(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i0: isize,
    sync_re: &[f64],
    sync_im: &[f64],
) -> f64 {
    let mut sync = 0.0f32;
    let stride = 36 * COSTAS_SYMBOL_LEN;

    for i in 0..COSTAS_BLOCKS {
        let base = i * COSTAS_SYMBOL_LEN;
        let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize);

        for _block in 0..3 {
            if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                let i_start = i_start as usize;
                let mut z_re = 0.0f32;
                let mut z_im = 0.0f32;
                for j in 0..COSTAS_SYMBOL_LEN {
                    let s_re = sync_re[base + j] as f32;
                    let s_im = sync_im[base + j] as f32;
                    let d_re = cd0_re[i_start + j] as f32;
                    let d_im = cd0_im[i_start + j] as f32;
                    z_re += d_re * s_re + d_im * s_im;
                    z_im += d_im * s_re - d_re * s_im;
                }
                sync += z_re * z_re + z_im * z_im;
            }
            i_start += stride as isize;
        }
    }

    sync as f64
}

fn is_valid_message_type(message77: &[u8]) -> bool {
    let (n3v, i3v) = message_type(message77);
    if i3v > 5 || (i3v == 0 && n3v > 6) {
        return false;
    }
    if i3v == 0 && n3v == 2 {
        return false;
    }
    true
}

fn message_type(message77: &[u8]) -> (usize, usize) {
    let n3v = ((message77[71] as usize) << 2)
        | ((message77[72] as usize) << 1)
        | (message77[73] as usize);
    let i3v = ((message77[74] as usize) << 2)
        | ((message77[75] as usize) << 1)
        | (message77[76] as usize);
    (n3v, i3v)
}

/// Compute both SNR estimates matching WSJT-X ft8b.f90.
///
/// - `xsnr`: xsig/xnoi - 1 (adjacent-tone noise)
/// - `xsnr2`: xsig/xbase/3e6 - 1 (spectrum baseline)
///
/// WSJT-X uses xsnr2 when nagain=false (initial decode), xsnr when nagain=true
/// (after subtract+retry). xbase is the noise power at f1 from the sync8 baseline.
fn compute_snr(s8: &[f64], itone: &[u8], xbase: f64) -> (f64, f64) {
    let mut xsig = 0.0f32;
    let mut xnoi = 0.0f32;

    for i in 0..79 {
        let tone = itone[i] as usize;
        let sig = s8[tone * NN + i] as f32;
        xsig += sig * sig;
        let ios = (tone + 4) % 7;
        let noi = s8[ios * NN + i] as f32;
        xnoi += noi * noi;
    }

    // xsnr: adjacent-tone noise estimate
    let mut xsnr = 0.001f32;
    let arg = xsig / xnoi.max(1e-30) - 1.0;
    if arg > 0.1 {
        xsnr = arg;
    }
    xsnr = 10.0 * xsnr.log10() - 27.0;

    // xsnr2: spectrum baseline estimate (WSJT-X ft8b.f90, regular decode path)
    let mut xsnr2 = 0.001f32;
    let arg2 = xsig / xbase as f32 / 3.0e6 - 1.0;
    if arg2 > 0.1 {
        xsnr2 = arg2;
    }
    xsnr2 = 10.0 * xsnr2.log10() - 27.0;

    (xsnr as f64, xsnr2 as f64)
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

fn add_elapsed(target: &mut Duration, start: Option<Instant>) {
    if let Some(start) = start {
        *target += start.elapsed();
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
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
    book: &Option<HashCallBook>,
) -> Option<DecodeResult> {
    let maxosd_base = if depth >= 2 { 2 } else { -1 };
    let scalefac = 2.83f32;
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

    let mut npasses = if (ap_options.enabled || ap_options.ncontest == 7) && ap_options.nzhsym >= 50
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
            workspace.llr[i] = (scalefac * metric as f32) as f64;
        }

        workspace.apmask.fill(0);
        if ipass > 5 {
            let apmag = (workspace
                .llr
                .iter()
                .map(|x| x.abs() as f32)
                .fold(0.0f32, f32::max)
                * 1.1f32) as f64;
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
            let acceptable =
                result.nharderrors <= 36 && is_wsjtx_acceptable_codeword(&result, ap_options, book);
            if acceptable {
                return Some(result);
            }
        }
    }

    None
}

fn is_wsjtx_acceptable_codeword(
    result: &DecodeResult,
    ap_options: &Ft8bApOptions,
    book: &Option<HashCallBook>,
) -> bool {
    // WSJT-X ft8b.f90 keeps trying later passes after each of these
    // candidate-codeword rejects (`cycle` inside the ipass loop).
    if result.cw.iter().all(|&b| b == 0) {
        return false;
    }

    let message77 = &result.message91[..77];
    let (_n3v, i3v) = message_type(message77);
    if !is_valid_message_type(message77) {
        return false;
    }

    let unpack_context = UnpackContext::with_calls(
        book.as_ref(),
        ap_options.mycall.as_deref(),
        ap_options.hiscall.as_deref(),
    );
    let Some(msg) = unpack77_with_context(message77, unpack_context) else {
        return false;
    };
    if ap_options.ncontest == 0
        && (1..=3).contains(&i3v)
        && (msg.contains("/R") || msg.starts_with("TU; "))
    {
        return false;
    }

    !msg.trim().is_empty()
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
