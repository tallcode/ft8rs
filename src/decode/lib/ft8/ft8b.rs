//! Candidate decode for one FT8 sync candidate.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/ft8b.f90`

use super::ft8_downsample::ft8_downsample;
use super::{
    build_costas_sync_templates, build_frequency_shift_sync_templates, nint_wsjtx_f32, sync8d,
    DecodeWorkspace, FrequencySearchResult, Ft8bApOptions, Ft8bResult, TimeRefineResult,
    TimeSearchResult, COSTAS_BLOCKS, COSTAS_SYMBOL_LEN, DT2, FS2, M73, MCQ, MCQFD, MCQRU, MCQTEST,
    MCQWW, MRR73, MRRR, NFFT1, NN, NP2, SAMPLE_RATE,
};
use crate::decode::decode174_91::{decode174_91, DecodeResult, N_LDPC};
use crate::decode::genft8::get_ft8_tones_from_codeword;
use crate::decode::packjt77::{unpack77_with_context, UnpackContext};
use crate::util::four2a_c2c;
use crate::HashCallBook;

const COSTAS: [u8; 7] = [3, 1, 4, 0, 6, 5, 2];
const GRAY_MAP: [u8; 8] = [0, 1, 3, 2, 5, 6, 4, 7];

/// Shared WSJT-X-shaped 32-sample symbol FFT from `ft8b.f90`.
pub(crate) fn extract_symbol_spectrum(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i1: isize,
    symb_re: &mut [f64],
    symb_im: &mut [f64],
) {
    debug_assert!(symb_re.len() >= COSTAS_SYMBOL_LEN);
    debug_assert!(symb_im.len() >= COSTAS_SYMBOL_LEN);

    symb_re[..COSTAS_SYMBOL_LEN].fill(0.0);
    symb_im[..COSTAS_SYMBOL_LEN].fill(0.0);

    if i1 >= 0 && (i1 + COSTAS_SYMBOL_LEN as isize - 1) < NP2 as isize {
        let i1 = i1 as usize;
        symb_re[..COSTAS_SYMBOL_LEN].copy_from_slice(&cd0_re[i1..i1 + COSTAS_SYMBOL_LEN]);
        symb_im[..COSTAS_SYMBOL_LEN].copy_from_slice(&cd0_im[i1..i1 + COSTAS_SYMBOL_LEN]);
    }

    four2a_c2c(
        &mut symb_re[..COSTAS_SYMBOL_LEN],
        &mut symb_im[..COSTAS_SYMBOL_LEN],
        -1,
    );
}

pub(super) fn ft8b(
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
) -> Option<Ft8bResult> {
    ft8_downsample(cx_re, cx_im, f1, workspace);

    let time0 = find_best_time_offset(&workspace.cd0_re, &workspace.cd0_im, xdt);
    let freq0 = find_best_frequency_shift(&workspace.cd0_re, &workspace.cd0_im, time0.ibest);
    f1 += freq0.delfbest;
    ft8_downsample(cx_re, cx_im, f1, workspace);

    let time1 = refine_time_offset(
        &workspace.cd0_re,
        &workspace.cd0_im,
        time0.ibest,
        &mut workspace.ss,
    );
    let ibest = time1.ibest;
    let xdt = (ibest as f64 - 1.0) * DT2;

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
    if nsync < min_costas_hits {
        return None;
    }

    build_bit_metrics(workspace, imetric);

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

    let result = try_decode_passes(workspace, depth, f1, ap_options, _book);
    let Some(result) = result else {
        return None;
    };

    if result.cw.iter().all(|&b| b == 0) {
        return None;
    }

    let message77 = &result.message91[..77];
    let (_n3v, i3v) = message_type(message77);
    if !is_valid_message_type(message77) {
        return None;
    }

    let unpack_context = UnpackContext::with_calls(
        _book.as_ref(),
        ap_options.mycall.as_deref(),
        ap_options.hiscall.as_deref(),
    );
    let msg = unpack77_with_context(message77, unpack_context);
    let Some(msg) = msg else {
        return None;
    };
    if !is_acceptable_unpacked_message(&msg, i3v, ap_options.ncontest) {
        return None;
    }
    if msg.trim().is_empty() {
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
    Some(Ft8bResult {
        msg,
        freq: f1,
        dt: xdt,
        snr,
        itone,
    })
}

fn find_best_time_offset(cd0_re: &[f64], cd0_im: &[f64], xdt: f64) -> TimeSearchResult {
    let i0_raw = nint_wsjtx_f32((xdt + 0.5) * FS2);
    let mut smax = 0.0;
    let mut ibest = i0_raw;
    let cs = build_costas_sync_templates();
    for offset in -10..=10 {
        let idx = i0_raw + offset;
        let sync = sync8d(cd0_re, cd0_im, idx, &cs.re, &cs.im);
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
        let sync = sync8d(cd0_re, cd0_im, ibest, &tpl.re, &tpl.im);
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
        ss[(idt + 4) as usize] = sync8d(cd0_re, cd0_im, ibest + idt, &cs.re, &cs.im);
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
        extract_symbol_spectrum(
            cd0_re,
            cd0_im,
            i1,
            &mut workspace.symb_re,
            &mut workspace.symb_im,
        );
        for tone in 0..8 {
            let idx = tone * NN + k;
            let csymb_re = workspace.symb_re[tone] as f32;
            let csymb_im = workspace.symb_im[tone] as f32;

            // WSJT-X ft8b.f90:
            //   cs(0:7,k)=csymb(1:8)/1e3
            //   s8(0:7,k)=abs(csymb(1:8))
            workspace.cs_re[idx] = (csymb_re / 1000.0) as f64;
            workspace.cs_im[idx] = (csymb_im / 1000.0) as f64;
            workspace.s8[idx] = wsjtx_cabs(csymb_re, csymb_im) as f64;
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

fn get_tones(cw: &[u8]) -> Vec<u8> {
    get_ft8_tones_from_codeword(cw)
        .iter()
        .map(|&tone| tone as u8)
        .collect()
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
    if !is_acceptable_unpacked_message(&msg, i3v, ap_options.ncontest) {
        return false;
    }

    !msg.trim().is_empty()
}

pub(super) fn is_acceptable_unpacked_message(msg: &str, i3v: usize, ncontest: usize) -> bool {
    // WSJT-X ft8b.f90 only rejects these contest/portable quirks in the
    // default non-contest mode after unpack77 succeeds.
    if ncontest == 0 && (1..=3).contains(&i3v) && (msg.contains("/R") || msg.starts_with("TU; ")) {
        return false;
    }
    !msg.trim().is_empty()
}

pub(super) fn apply_wsjt_ap_mask(
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
