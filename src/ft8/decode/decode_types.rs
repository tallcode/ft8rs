use crate::ft8::protocol::{N_LDPC, SAMPLE_RATE};
use std::time::Duration;

pub(super) const NSPS: usize = 1920;
pub(super) const NFFT1: usize = 2 * NSPS; // 3840
pub(super) const NSTEP: usize = NSPS / 4; // 480
pub(super) const NMAX: usize = 15 * 12_000; // 180000
pub(super) const NHSYM: usize = NMAX / NSTEP - 3; // 372
pub(super) const NDOWN: usize = 60;
pub(crate) const NN: usize = 79;

pub(crate) const NFFT1_LONG: usize = 192000;
pub(crate) const NFFT2: usize = 3200;
pub(crate) const NP2: usize = 2812;
pub(crate) const COSTAS_BLOCKS: usize = 7;
pub(crate) const COSTAS_SYMBOL_LEN: usize = 32;
pub(crate) const TAPER_SIZE: usize = 101;
pub(crate) const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
pub(super) const TWO_PI_F32: f32 = std::f32::consts::PI * 2.0;
pub(super) const PI_F32: f32 = std::f32::consts::PI;

pub(crate) const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
pub(crate) const DT2: f64 = 1.0 / FS2;
pub(crate) const DOWNSAMPLE_DF: f32 = SAMPLE_RATE as f32 / NFFT1_LONG as f32;
pub(crate) const DOWNSAMPLE_BAUD: f32 = SAMPLE_RATE as f32 / NSPS as f32;
/// WSJT-X ft8_downsample.f90:
/// `fac=1.0/sqrt(float(NFFT1)*NFFT2)` after the unnormalized inverse FFT.
/// `float(NFFT1)` makes this a default-REAL calculation in WSJT-X, so keep the
/// scale as f32 before storing the downsampled symbols back into f64 buffers.
pub(crate) const DOWNSAMPLE_FAC: f32 = 0.00004034357698401436f32;

#[derive(Clone)]
pub(super) struct Candidate {
    pub(super) freq: f64,
    pub(super) dt: f64,
    pub(super) sync: f64,
}

pub(super) struct Ft8bResult {
    pub(super) msg: String,
    pub(super) freq: f64,
    pub(super) dt: f64,
    pub(super) snr: f64,
    pub(super) itone: [i32; 79],
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TimeSearchResult {
    pub(super) ibest: isize,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FrequencySearchResult {
    pub(super) delfbest: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TimeRefineResult {
    pub(super) ibest: isize,
}

#[derive(Clone)]
pub(super) struct Ft8bApOptions {
    pub(super) enabled: bool,
    pub(super) cq_only: bool,
    pub(super) nqso_progress: usize,
    pub(super) ncontest: usize,
    pub(super) nfqso: f64,
    pub(super) nftx: f64,
    pub(super) napwid: f64,
    pub(super) nzhsym: usize,
    pub(super) ap_set: Ft8ApSet,
    pub(super) mycall: Option<String>,
    pub(super) hiscall: Option<String>,
}

#[derive(Clone)]
pub(super) struct Ft8ApSet {
    pub(super) apsym: [i8; 58],
    pub(super) aph10: [i8; 10],
}

pub(crate) struct SyncTemplate {
    pub(crate) re: Vec<f64>,
    pub(crate) im: Vec<f64>,
}

pub(super) struct FrequencyShiftSyncTemplate {
    pub(super) delf: f64,
    pub(super) re: Vec<f64>,
    pub(super) im: Vec<f64>,
}

pub(super) struct DecodeWorkspace {
    pub(super) cd0_re: Vec<f64>,
    pub(super) cd0_im: Vec<f64>,
    pub(super) shift_re: Vec<f64>,
    pub(super) shift_im: Vec<f64>,
    pub(super) s8: Vec<f64>,
    pub(super) cs_re: Vec<f64>,
    pub(super) cs_im: Vec<f64>,
    pub(super) symb_re: Vec<f64>,
    pub(super) symb_im: Vec<f64>,
    pub(super) s2: Vec<f64>,
    pub(super) bmeta: Vec<f64>,
    pub(super) bmetb: Vec<f64>,
    pub(super) bmetc: Vec<f64>,
    pub(super) bmetd: Vec<f64>,
    pub(super) bmete: Vec<f64>,
    pub(super) llr: Vec<f64>,
    pub(super) apmask: Vec<i8>,
    pub(super) ss: Vec<f64>,
}

#[derive(Default)]
pub(super) struct Ft8bStats {
    pub(super) calls: usize,
    pub(super) sync_rejects: usize,
    pub(super) decode_failures: usize,
    pub(super) downsample: Duration,
    pub(super) align: Duration,
    pub(super) symbols: Duration,
    pub(super) metrics: Duration,
    pub(super) ldpc: Duration,
    pub(super) post: Duration,
}

pub(super) fn create_decode_workspace() -> DecodeWorkspace {
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
