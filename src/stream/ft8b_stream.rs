/// Independent ft8b implementation for streaming decode.
/// Each candidate gets its own local state (matching WSJT-X stack variables).
/// No shared workspace, no HashCallBook in the candidate loop.

use crate::ft8::constants::{COSTAS, GRAY_MAP};
use crate::util::constants::{N_LDPC, SAMPLE_RATE};
use crate::util::decode174_91::{decode174_91, DecodeResult};
use crate::util::fft::fft_complex;

const NDOWN: usize = 60;
const NN: usize = 79;
const NFFT1_LONG: usize = 192000;
const NFFT2: usize = 3200;
const NP2: usize = 2812;
const COSTAS_BLOCKS: usize = 7;
const COSTAS_SYMBOL_LEN: usize = 32;
const TAPER_SIZE: usize = 101;
const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
const DT2: f64 = 1.0 / FS2;
const DOWNSAMPLE_DF: f64 = SAMPLE_RATE as f64 / NFFT1_LONG as f64;
const DOWNSAMPLE_BAUD: f64 = SAMPLE_RATE as f64 / 1920.0; // NSPS
const DOWNSAMPLE_SCALE: f64 = 0.12909944487358055;

/// Local state for one ft8b decode call (equivalent to WSJT-X stack variables).
pub struct Ft8bLocalState {
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

impl Ft8bLocalState {
    pub fn new() -> Self {
        Self {
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
}

/// Result from ft8b_stream decode.
pub struct Ft8bResult {
    pub msg: String,
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub itone: [i32; 79],
    pub sync: f64,
}

/// Thread-local reusable templates (constants, not mutable).
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

fn build_costas_sync_templates() -> &'static crate::ft8::decode::SyncTemplate {
    use crate::ft8::decode::build_costas_sync_templates as build;
    build()
}

/// Main ft8b streaming decode — no shared state, fully parallel-safe.
pub fn ft8b_stream(
    dd0: &[f64],
    cx_re: &[f64],
    cx_im: &[f64],
    mut f1: f64,
    xdt: f64,
    sbase: &[f64],
    depth: usize,
    sync: f64,
) -> Option<Ft8bResult> {
    let mut state = Ft8bLocalState::new();

    // 1. Coarse downsample: mix f1 to baseband
    ft8_downsample(cx_re, cx_im, f1, &mut state);

    // 2. Find best time offset (±10 samples)
    let mut ibest = find_best_time_offset(&state.cd0_re, &state.cd0_im, xdt);

    // 3. Find best frequency shift (±2.5 Hz)
    let delfbest = find_best_frequency_shift(&state.cd0_re, &state.cd0_im, ibest);
    f1 += delfbest;

    // 4. Re-downsample with refined frequency
    ft8_downsample(cx_re, cx_im, f1, &mut state);

    // 5. Refine time offset (±4 samples)
    ibest = refine_time_offset(&state.cd0_re, &state.cd0_im, ibest, &mut state.ss);
    let xdt = (ibest as f64 - 1.0) * DT2;

    // 6. Extract soft symbols
    extract_soft_symbols(ibest, &mut state);

    // 7. Sync gate: need ≥7 Costas hits
    if !passes_sync_gate_strict(&state.s8, 7) {
        return None;
    }

    // 8. Build bit metrics
    build_bit_metrics(&mut state);

    // 9. Try decode passes (BP + OSD + AP)
    let result = try_decode_passes(&mut state, depth)?;

    // 10. Validate codeword
    if result.cw.iter().all(|&b| b == 0) {
        return None;
    }

    let message77: Vec<u8> = result.message91[..77].to_vec();
    if !is_valid_message_type(&message77) {
        return None;
    }

    // 11. Unpack message (no book for streaming — AP is separate)
    let msg = crate::util::unpack_jt77::unpack77(&message77, None);
    let msg = msg?;
    if msg.trim().is_empty() {
        return None;
    }

    // 12. Estimate SNR
    let snr = estimate_snr(&state.s8, &result.cw);

    // 13. Compute itone pattern
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
        sync,
    })
}

fn ft8_downsample(cx_re: &[f64], cx_im: &[f64], f0: f64, state: &mut Ft8bLocalState) {
    let df = DOWNSAMPLE_DF;
    let baud = DOWNSAMPLE_BAUD;
    let i0 = (f0 / df).round() as usize;
    let ft = f0 + 8.5 * baud;
    let it = ((ft / df).round() as usize).min(NFFT1_LONG / 2);
    let fb = f0 - 1.5 * baud;
    let ib = 1.max((fb / df).round() as usize);

    state.cd0_re.fill(0.0);
    state.cd0_im.fill(0.0);
    let mut k = 0;
    for i in ib..=it {
        if k >= NFFT2 {
            break;
        }
        state.cd0_re[k] = cx_re[i];
        state.cd0_im[k] = cx_im[i];
        k += 1;
    }

    let taper_data = build_taper();
    for i in 0..TAPER_SIZE {
        if i >= NFFT2 {
            break;
        }
        let tap = taper_data[TAPER_SIZE - 1 - i];
        state.cd0_re[i] *= tap;
        state.cd0_im[i] *= tap;
    }

    let end_tap = k - 1;
    for i in 0..TAPER_SIZE {
        let idx = end_tap - TAPER_SIZE + 1 + i;
        if idx < NFFT2 {
            let tap = taper_data[i];
            state.cd0_re[idx] *= tap;
            state.cd0_im[idx] *= tap;
        }
    }

    let shift = i0 - ib;
    if shift != 0 {
        for i in 0..NFFT2 {
            let src_idx = (i + shift) % NFFT2;
            state.shift_re[i] = state.cd0_re[src_idx];
            state.shift_im[i] = state.cd0_im[src_idx];
        }
        state.cd0_re.copy_from_slice(&state.shift_re);
        state.cd0_im.copy_from_slice(&state.shift_im);
    }

    fft_complex(&mut state.cd0_re, &mut state.cd0_im, true);

    for i in 0..NFFT2 {
        state.cd0_re[i] *= DOWNSAMPLE_SCALE;
        state.cd0_im[i] *= DOWNSAMPLE_SCALE;
    }
}

fn find_best_time_offset(cd0_re: &[f64], cd0_im: &[f64], xdt: f64) -> isize {
    let i0_raw = ((xdt + 0.5) * FS2).round() as isize;
    let i0_center = i0_raw.rem_euclid(NP2 as isize);

    let mut smax = 0.0;
    let mut ibest_unwrapped = i0_raw;
    let cs = build_costas_sync_templates();
    for offset in -10..=10 {
        let idx = (i0_center + offset).rem_euclid(NP2 as isize) as usize;
        let sync = sync8d(&cd0_re, &cd0_im, idx, &cs.re, &cs.im);
        if sync > smax {
            smax = sync;
            ibest_unwrapped = i0_raw + offset;
        }
    }
    ibest_unwrapped
}

fn find_best_frequency_shift(cd0_re: &[f64], cd0_im: &[f64], ibest: isize) -> f64 {
    // Build frequency shift templates inline
    let cs = build_costas_sync_templates();
    let mut smax = 0.0;
    let mut delfbest = 0.0;
    let idx = ibest.rem_euclid(NP2 as isize) as usize;

    for ifr in -5..=5 {
        let delf = ifr as f64 * 0.5;
        let dphi = TWO_PI * delf * DT2;
        let mut twk_re = [0.0f64; COSTAS_SYMBOL_LEN];
        let mut twk_im = [0.0f64; COSTAS_SYMBOL_LEN];
        let mut phi: f64 = 0.0;
        for j in 0..COSTAS_SYMBOL_LEN {
            twk_re[j] = phi.cos();
            twk_im[j] = phi.sin();
            phi = (phi + dphi) % TWO_PI;
        }

        let mut sync = 0.0;
        for i in 0..COSTAS_BLOCKS {
            let base = i * COSTAS_SYMBOL_LEN;
            let mut i_start = (idx as isize) + (i as isize) * (COSTAS_SYMBOL_LEN as isize);
            for _block in 0..3 {
                if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                    let i_start = i_start as usize;
                    let mut z_re = 0.0;
                    let mut z_im = 0.0;
                    for j in 0..COSTAS_SYMBOL_LEN {
                        let s_re = cs.re[base + j];
                        let s_im = cs.im[base + j];
                        let d_re = cd0_re[i_start + j];
                        let d_im = cd0_im[i_start + j];
                        let tr = twk_re[j];
                        let ti = twk_im[j];
                        // Mix template with twiddle
                        let ts_re = tr * s_re - ti * s_im;
                        let ts_im = tr * s_im + ti * s_re;
                        z_re += d_re * ts_re + d_im * ts_im;
                        z_im += d_im * ts_re - d_re * ts_im;
                    }
                    sync += z_re * z_re + z_im * z_im;
                }
                i_start += 36 * COSTAS_SYMBOL_LEN as isize;
            }
        }
        if sync > smax {
            smax = sync;
            delfbest = delf;
        }
    }
    delfbest
}

fn refine_time_offset(cd0_re: &[f64], cd0_im: &[f64], ibest: isize, ss: &mut [f64]) -> isize {
    ss.fill(0.0);
    let cs = build_costas_sync_templates();
    for idt in -4..=4 {
        let idx = (ibest + idt).rem_euclid(NP2 as isize) as usize;
        ss[(idt + 4) as usize] = sync8d(cd0_re, cd0_im, idx, &cs.re, &cs.im);
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

fn extract_soft_symbols(ibest: isize, state: &mut Ft8bLocalState) {
    let cd0_re = &state.cd0_re;
    let cd0_im = &state.cd0_im;
    for k in 0..NN {
        let i1 = ibest + (k as isize) * (COSTAS_SYMBOL_LEN as isize);
        state.symb_re.fill(0.0);
        state.symb_im.fill(0.0);

        if i1 >= 0 && (i1 + COSTAS_SYMBOL_LEN as isize - 1) < NP2 as isize {
            let i1u = i1 as usize;
            for j in 0..COSTAS_SYMBOL_LEN {
                state.symb_re[j] = cd0_re[i1u + j];
                state.symb_im[j] = cd0_im[i1u + j];
            }
        }

        fft_complex(&mut state.symb_re, &mut state.symb_im, false);
        for tone in 0..8 {
            let re = state.symb_re[tone] / 1000.0;
            let im = state.symb_im[tone] / 1000.0;
            let idx = tone * NN + k;
            state.cs_re[idx] = re;
            state.cs_im[idx] = im;
            state.s8[idx] = (re * re + im * im).sqrt();
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

fn build_bit_metrics(state: &mut Ft8bLocalState) {
    state.bmeta.fill(0.0);
    state.bmetb.fill(0.0);
    state.bmetc.fill(0.0);
    state.bmetd.fill(0.0);

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
                        let re = state.cs_re[GRAY_MAP[i3] as usize * NN + ks - 1];
                        let im = state.cs_im[GRAY_MAP[i3] as usize * NN + ks - 1];
                        state.s2[i] = (re * re + im * im).sqrt();
                    } else if nsym == 2 {
                        let s_re = state.cs_re[GRAY_MAP[i2] as usize * NN + ks - 1]
                            + state.cs_re[GRAY_MAP[i3] as usize * NN + ks];
                        let s_im = state.cs_im[GRAY_MAP[i2] as usize * NN + ks - 1]
                            + state.cs_im[GRAY_MAP[i3] as usize * NN + ks];
                        state.s2[i] = (s_re * s_re + s_im * s_im).sqrt();
                    } else {
                        let s_re = state.cs_re[GRAY_MAP[i1] as usize * NN + ks - 1]
                            + state.cs_re[GRAY_MAP[i2] as usize * NN + ks]
                            + state.cs_re[GRAY_MAP[i3] as usize * NN + ks + 1];
                        let s_im = state.cs_im[GRAY_MAP[i1] as usize * NN + ks - 1]
                            + state.cs_im[GRAY_MAP[i2] as usize * NN + ks]
                            + state.cs_im[GRAY_MAP[i3] as usize * NN + ks + 1];
                        state.s2[i] = (s_re * s_re + s_im * s_im).sqrt();
                    }
                }

                let i32 = 1 + (k - 1) * 3 + (ihalf - 1) * 87;
                for ib in 0..=ibmax {
                    let mut max1 = -1e30;
                    let mut max0 = -1e30;
                    for i in 0..nt {
                        let bit_set = (i & (1 << (ibmax - ib))) != 0;
                        if bit_set {
                            if state.s2[i] > max1 { max1 = state.s2[i]; }
                        } else {
                            if state.s2[i] > max0 { max0 = state.s2[i]; }
                        }
                    }

                    let idx = (i32 as isize + ib as isize - 1) as usize;
                    if idx >= N_LDPC { continue; }

                    let bm = max1 - max0;
                    if nsym == 1 {
                        state.bmeta[idx] = bm;
                        let den = max1.max(max0);
                        state.bmetd[idx] = if den > 0.0 { bm / den } else { 0.0 };
                    } else if nsym == 2 {
                        state.bmetb[idx] = bm;
                    } else {
                        state.bmetc[idx] = bm;
                    }
                }
            }
        }
    }

    normalize_bmet(&mut state.bmeta);
    normalize_bmet(&mut state.bmetb);
    normalize_bmet(&mut state.bmetc);
    normalize_bmet(&mut state.bmetd);
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

fn sync8d(cd0_re: &[f64], cd0_im: &[f64], i0: usize, sync_re: &[f64], sync_im: &[f64]) -> f64 {
    let mut sync = 0.0;
    let stride = 36 * COSTAS_SYMBOL_LEN;

    for i in 0..COSTAS_BLOCKS {
        let base = i * COSTAS_SYMBOL_LEN;
        let mut i_start = i0 as isize + (i as isize) * (COSTAS_SYMBOL_LEN as isize);

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

fn try_decode_passes(state: &mut Ft8bLocalState, depth: usize) -> Option<DecodeResult> {
    let maxosd_base = if depth >= 3 { 2 } else if depth >= 2 { 0 } else { -1 };
    let scalefac = 2.83;
    let bmetrics = [
        &state.bmeta,
        &state.bmetb,
        &state.bmetc,
        &state.bmetd,
    ];

    state.apmask.fill(0);

    for ipass in 0..4 {
        let metric = bmetrics[ipass];
        for i in 0..N_LDPC {
            state.llr[i] = scalefac * metric[i];
        }

        if let Some(result) = decode174_91(&state.llr, &state.apmask, maxosd_base) {
            if result.nharderrors <= 36 {
                return Some(result);
            }
        }
    }

    // AP passes (depth >= 2)
    if depth >= 2 {
        let apmag = bmetrics[0].iter()
            .map(|&x| (scalefac * x).abs())
            .fold(0.0f64, f64::max) * 1.01;

        if apmag > 0.1 {
            for i in 0..N_LDPC {
                state.llr[i] = scalefac * bmetrics[0][i];
            }

            // AP Pass 5: CQ call mask
            state.apmask.fill(0);
            for i in 0..29 {
                state.apmask[i] = 1;
                state.llr[i] = if i == 26 { apmag } else { -apmag };
            }
            state.apmask[74] = 1; state.llr[74] = -apmag;
            state.apmask[75] = 1; state.llr[75] = -apmag;
            state.apmask[76] = 1; state.llr[76] = apmag;

            if let Some(result) = decode174_91(&state.llr, &state.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }

            // AP Pass 6: CQ + alternate i3
            state.apmask[74] = 1; state.llr[74] = -apmag;
            state.apmask[75] = 1; state.llr[75] = apmag;
            state.apmask[76] = 1; state.llr[76] = -apmag;

            if let Some(result) = decode174_91(&state.llr, &state.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }

            // AP Pass 7: Message type only
            state.apmask.fill(0);
            for i in 0..N_LDPC {
                state.llr[i] = scalefac * bmetrics[0][i];
            }
            state.apmask[71] = 1; state.llr[71] = -apmag;
            state.apmask[72] = 1; state.llr[72] = -apmag;
            state.apmask[73] = 1; state.llr[73] = -apmag;
            state.apmask[74] = 1; state.llr[74] = -apmag;
            state.apmask[75] = 1; state.llr[75] = -apmag;
            state.apmask[76] = 1; state.llr[76] = apmag;

            if let Some(result) = decode174_91(&state.llr, &state.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }

            // AP Pass 8: n3=0, i3=2
            state.apmask[75] = 1; state.llr[75] = apmag;
            state.apmask[76] = 1; state.llr[76] = -apmag;

            if let Some(result) = decode174_91(&state.llr, &state.apmask, maxosd_base) {
                if result.nharderrors <= 36 {
                    return Some(result);
                }
            }
        }
    }

    None
}
