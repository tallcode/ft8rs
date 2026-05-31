//! Mirrors JTDX `lib/ft8b.f90`.

use super::chkfalse8::{accept_decoded_message, FilterContext};
use super::ft8_downsample::{ft8_downsample, DownsampleOutput, DownsampleWorkspace};
use super::ft8_mod1::{
    GRAYMAP, ICOS7, NAPPASSES, NAPTYPES, NDXNSAPTYPES, NHAPTYPES, NMYCNSAPTYPES,
};
use super::ft8_params::{DT2, FS2};
use super::ft8apset::build_ap_mask;
use super::ft8v2::bpdecode174_91::{bpdecode174_91, BpDecodeResult, N};
use super::ft8v2::osd174_91::osd174_91;
use super::ft8v2::packjt77::{unpack77_with_context, HashCallBook, UnpackContext};
use super::ft8v2::subtractft8::subtractft8;
use super::gen_ft8wave::gen_ft8wave;
use super::sync8::SyncCandidate;
use super::sync8d::{build_ctwk, sync8d, Sync8dContext};
use crate::stream::session::StreamDecodeConfig;
use crate::util::four2a_c2c;

#[derive(Clone, Copy, Debug)]
pub struct Ft8bCandidateContext {
    pub ipass: usize,
    pub npass: usize,
    pub lsubtract: bool,
    pub lhighsens: bool,
    pub lcqcand: bool,
    pub levenint: bool,
    pub loddint: bool,
    pub lqsomsgdcd: bool,
    pub stophint: bool,
    pub nlasttx: usize,
    pub call_dt_xdt: Option<f32>,
    pub last_rx_xdt: Option<f32>,
    pub last_rx_is_rrr: bool,
}

#[derive(Clone, Debug)]
pub struct Ft8bDecodeResult {
    pub msg37: String,
    pub msg37_2: String,
    pub snr: f32,
    pub freq: f32,
    pub dt: f32,
    pub iaptype: i32,
    pub i3: i32,
    pub n3: i32,
    pub itone: [i32; 79],
}

#[derive(Clone, Debug)]
struct SymbolMetrics {
    #[allow(dead_code)]
    s8: [[f32; 79]; 8],
    cs_re: [[f32; 79]; 8],
    cs_im: [[f32; 79]; 8],
    syncav: f32,
    nsync: usize,
    nsync2: usize,
}

#[derive(Debug)]
pub struct Ft8bWorkspace {
    downsample: DownsampleWorkspace,
    downsample_out: DownsampleOutput,
    freqsub: Vec<f32>,
    npos: usize,
    lsubtracted: bool,
}

impl Default for Ft8bWorkspace {
    fn default() -> Self {
        Self {
            downsample: DownsampleWorkspace::new(),
            downsample_out: DownsampleOutput::default(),
            freqsub: vec![0.0; 200],
            npos: 0,
            lsubtracted: false,
        }
    }
}

impl Ft8bWorkspace {
    pub fn new_pass(&mut self) {
        self.npos = 0;
    }
}

pub fn ft8b(
    workspace: &mut Ft8bWorkspace,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    dd8: &mut [f32],
    newdat1: bool,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
) -> Option<Ft8bDecodeResult> {
    let qso_plan = jtdx_qso_plan(config, candidate, context);
    ft8_downsample(
        &mut workspace.downsample,
        dd8,
        newdat1,
        candidate.freq,
        qso_plan.nqso,
        context.lhighsens,
        &mut workspace.lsubtracted,
        &mut workspace.npos,
        &workspace.freqsub,
        &mut workspace.downsample_out,
    );

    let attempts = qso_attempts(qso_plan);
    for iqso in attempts {
        let cd0 = match iqso {
            2 => &workspace.downsample_out.c2,
            3 => &workspace.downsample_out.c3,
            _ => &workspace.downsample_out.c0,
        };
        if let Some((result, ibest)) =
            try_ft8b_decode_for_iqso(cd0, config, book, candidate, context, iqso, qso_plan.xdt0)
        {
            if context.lsubtract {
                let xdt3 = refined_subtract_dt(cd0, &result.itone, ibest);
                subtractft8(dd8, &result.itone, result.freq, xdt3 as f32, config.swl);
                workspace.lsubtracted = true;
                if workspace.npos < workspace.freqsub.len() {
                    workspace.freqsub[workspace.npos] = result.freq;
                    workspace.npos += 1;
                }
            }
            return Some(result);
        }
    }

    None
}

fn try_ft8b_decode_for_iqso(
    cd0: &super::ft8_downsample::ComplexC,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
    iqso: usize,
    xdt0: f32,
) -> Option<(Ft8bDecodeResult, isize)> {
    let sync_context = Sync8dContext {
        ipass: context.ipass,
        lastsync: false,
        iqso,
        lcq: false,
        lcallsstd: true,
        lcqcand: context.lcqcand,
    };
    let i0 = nint((xdt0 as f64 + 0.5) * FS2);
    let mut smax = 0.0;
    let mut ibest = i0;
    for idt in (i0 - 8)..=(i0 + 8) {
        let sync = sync8d(cd0, idt, None, None, sync_context);
        if sync > smax {
            smax = sync;
            ibest = idt;
        }
    }

    let xdt2 = ibest as f64 * DT2;
    let i0 = nint(xdt2 * FS2);
    let mut delfbest = 0.0;
    smax = 0.0;
    let freq_step = if iqso == 1 { 0.5 } else { 0.25 };
    for ifr in -5..=5 {
        let delf = ifr as f64 * freq_step;
        let (ctwk_re, ctwk_im) = build_ctwk(delf);
        let sync = sync8d(
            cd0,
            i0,
            Some(&ctwk_re),
            Some(&ctwk_im),
            Sync8dContext {
                lastsync: false,
                ..sync_context
            },
        );
        if sync > smax {
            smax = sync;
            delfbest = delf;
        }
    }

    let _refined_freq = candidate.freq as f64 + delfbest;
    let _refined_dt = xdt2;
    let metrics = extract_symbol_metrics(cd0, ibest);
    if !passes_jtdx_regular_sync_gate(&metrics, context) {
        return None;
    }

    regular_decode(
        &metrics,
        candidate,
        _refined_freq,
        _refined_dt,
        config,
        book,
        context,
    )
    .map(|result| (result, ibest))
}

#[derive(Clone, Copy, Debug)]
struct QsoPlan {
    nqso: usize,
    xdt0: f32,
    lvirtual2: bool,
    lvirtual3: bool,
}

fn qso_attempts(plan: QsoPlan) -> Vec<usize> {
    if plan.lvirtual2 {
        return vec![2];
    }
    if plan.lvirtual3 {
        return vec![2, 3];
    }
    match plan.nqso {
        2 => vec![1, 2],
        3 => vec![1, 3],
        _ => vec![1],
    }
}

fn jtdx_qso_plan(
    config: &StreamDecodeConfig,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
) -> QsoPlan {
    let mut plan = QsoPlan {
        nqso: 1,
        xdt0: candidate.dt,
        lvirtual2: false,
        lvirtual3: false,
    };
    let lqsothread = config.nfqso >= config.nfa && config.nfqso <= config.nfb;
    if !lqsothread || config.hiscall.as_deref().unwrap_or("").trim().len() < 3 {
        return plan;
    }

    let fdelta = (candidate.freq as f64 - config.nfqso).abs();
    if !context.lqsomsgdcd
        && !context.stophint
        && (1..=4).contains(&context.nlasttx)
        && fdelta < 2.51
    {
        if let Some(last_xdt) = context.last_rx_xdt {
            if (last_xdt - candidate.dt).abs() < 0.18 {
                plan.nqso = 2;
            }
        } else {
            plan.nqso = 2;
        }
    }

    if context.lqsomsgdcd || context.stophint || fdelta >= 0.1 {
        return plan;
    }

    let mut maxlasttx = 4;
    if candidate.dt.abs() > 4.9 && context.last_rx_is_rrr {
        maxlasttx = 5;
    }
    if !(1..=maxlasttx).contains(&context.nlasttx) {
        return plan;
    }

    if candidate.dt > 4.9 {
        if let Some(last_xdt) = context.last_rx_xdt {
            plan.xdt0 = last_xdt;
            plan.nqso = 2;
            plan.lvirtual2 = true;
        } else if let Some(call_dt) = context.call_dt_xdt {
            plan.xdt0 = call_dt;
            plan.nqso = 3;
            plan.lvirtual2 = true;
        }
    } else if candidate.dt < -4.9 {
        if let Some(last_xdt) = context.last_rx_xdt {
            plan.xdt0 = last_xdt;
            plan.nqso = 3;
            plan.lvirtual3 = true;
        } else if let Some(call_dt) = context.call_dt_xdt {
            plan.xdt0 = call_dt;
            plan.nqso = 3;
            plan.lvirtual3 = true;
        }
    }

    if !context.levenint && !context.loddint {
        plan.lvirtual2 = false;
        plan.lvirtual3 = false;
    }

    plan
}

#[allow(dead_code)]
fn jtdx_nqso(
    config: &StreamDecodeConfig,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
) -> usize {
    jtdx_qso_plan(config, candidate, context).nqso
}

fn nint(x: f64) -> isize {
    (x as f32).round() as isize
}

fn extract_symbol_metrics(cd0: &super::ft8_downsample::ComplexC, ibest: isize) -> SymbolMetrics {
    let mut s8 = [[0.0f32; 79]; 8];
    let mut cs_re = [[0.0f32; 79]; 8];
    let mut cs_im = [[0.0f32; 79]; 8];
    let mut snrsync = [0.0f32; 21];
    let mut re = [0.0f64; 32];
    let mut im = [0.0f64; 32];

    for k in 0..79 {
        let i1 = ibest + k as isize * 32;
        for i in 0..32 {
            let src = i1 + i as isize;
            if (super::ft8_downsample::C_LOW..=super::ft8_downsample::C_HIGH).contains(&src) {
                let idx = super::ft8_downsample::ComplexC::idx(src);
                re[i] = cd0.re[idx];
                im[i] = cd0.im[idx];
            } else {
                re[i] = 0.0;
                im[i] = 0.0;
            }
        }
        four2a_c2c(&mut re, &mut im, -1);
        for tone in 0..8 {
            cs_re[tone][k] = (re[tone + 1] / 1000.0) as f32;
            cs_im[tone][k] = (im[tone + 1] / 1000.0) as f32;
            s8[tone][k] = (re[tone + 1] * re[tone + 1] + im[tone + 1] * im[tone + 1]).sqrt() as f32;
        }

        if (0..=6).contains(&k) || (36..=42).contains(&k) || (72..=78).contains(&k) {
            let costas_idx = if k <= 6 {
                k
            } else if k <= 42 {
                k - 36
            } else {
                k - 72
            };
            let tone = ICOS7[costas_idx] as usize;
            let synclev = s8[tone][k];
            let snoiselev = (sum_tones(&s8, k) - synclev) / 7.0;
            if snoiselev > 1e-16 {
                let out_idx = if k <= 6 {
                    k
                } else if k <= 42 {
                    k - 29
                } else {
                    k - 58
                };
                snrsync[out_idx] = synclev / snoiselev;
            }
        }
    }

    let syncav = snrsync.iter().sum::<f32>() / 21.0;
    let mut nsync = 0usize;
    let mut nsync2 = 0usize;
    for k in 0..7 {
        for base in [0usize, 36, 72] {
            let sym = base + k;
            let best = max_tone(&s8, sym, None);
            if best == ICOS7[k] as usize {
                nsync += 1;
            } else {
                let second = max_tone(&s8, sym, Some(best));
                if second == ICOS7[k] as usize {
                    nsync2 += 1;
                }
            }
        }
    }

    SymbolMetrics {
        s8,
        cs_re,
        cs_im,
        syncav,
        nsync,
        nsync2,
    }
}

fn passes_jtdx_regular_sync_gate(metrics: &SymbolMetrics, context: Ft8bCandidateContext) -> bool {
    let _ = metrics.syncav;
    if context.lcqcand {
        return metrics.nsync >= 4 && metrics.nsync + metrics.nsync2 >= 7;
    }
    metrics.nsync >= 7
}

fn refined_subtract_dt(
    cd0: &super::ft8_downsample::ComplexC,
    itone: &[i32; 79],
    ibest: isize,
) -> f64 {
    let noff = 10isize;
    let (syncm, sync0, syncp) = (
        subtract_sync_at(cd0, itone, ibest - noff),
        subtract_sync_at(cd0, itone, ibest),
        subtract_sync_at(cd0, itone, ibest + noff),
    );
    let dx = peakup(syncm, sync0, syncp);
    let scorr = if dx.abs() > 1.0 {
        0.0
    } else {
        noff as f64 * dx
    };
    (ibest as f64 + scorr) * DT2
}

fn subtract_sync_at(cd0: &super::ft8_downsample::ComplexC, itone: &[i32; 79], i0: isize) -> f64 {
    let (csig_re, csig_im) = gen_ft8wave(itone, 0.0);
    let mut sync = 0.0f64;
    for i in 0..79 {
        let mut z_re = 0.0f64;
        let mut z_im = 0.0f64;
        for j in 0..32 {
            let src = i0 + i as isize * 32 + j as isize;
            if !(super::ft8_downsample::C_LOW..=super::ft8_downsample::C_HIGH).contains(&src) {
                continue;
            }
            let idx = super::ft8_downsample::ComplexC::idx(src);
            let wav = i * 1920 + j * 60;
            z_re += cd0.re[idx] * csig_re[wav] + cd0.im[idx] * csig_im[wav];
            z_im += cd0.im[idx] * csig_re[wav] - cd0.re[idx] * csig_im[wav];
        }
        sync += z_re * z_re + z_im * z_im;
    }
    sync
}

fn peakup(ym: f64, y0: f64, yp: f64) -> f64 {
    let denominator = yp + ym - 2.0 * y0;
    if denominator.abs() <= f64::EPSILON {
        0.0
    } else {
        -(yp - ym) / (2.0 * denominator)
    }
}

fn sum_tones(s8: &[[f32; 79]; 8], k: usize) -> f32 {
    s8.iter().map(|tone| tone[k]).sum()
}

fn max_tone(s8: &[[f32; 79]; 8], k: usize, skip: Option<usize>) -> usize {
    let mut best = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (tone, values) in s8.iter().enumerate() {
        if skip == Some(tone) {
            continue;
        }
        if values[k] > best_value {
            best_value = values[k];
            best = tone;
        }
    }
    best
}

fn regular_decode(
    metrics: &SymbolMetrics,
    _candidate: SyncCandidate,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
) -> Option<Ft8bDecodeResult> {
    let _ap_subpasses = plan_ap_subpasses(config);
    let mut bmeta = [0.0f32; N];
    let mut bmetb = [0.0f32; N];
    let mut bmetc = [0.0f32; N];
    let mut bmetd = [0.0f32; N];

    let srr = sync_snr_ratio(metrics);
    for nsym in 1..=3 {
        let nt = (1usize << (3 * nsym)) - 1;
        for ihalf in 1..=2 {
            let mut k = 1usize;
            while k <= 29 {
                let ks = if ihalf == 1 { k + 7 } else { k + 43 };
                let i32 = 1 + (k - 1) * 3 + (ihalf - 1) * 87;
                let ibmax = match nsym {
                    1 => 2,
                    2 => 5,
                    _ => 8,
                };
                let mut s2 = vec![0.0f32; nt + 1];
                for (i, slot) in s2.iter_mut().enumerate() {
                    let i1 = i / 64;
                    let i2 = (i & 63) / 8;
                    let i33 = i & 7;
                    let value = match nsym {
                        1 => complex_abs(metrics, GRAYMAP[i33] as usize, ks),
                        2 => complex_abs_sum(
                            metrics,
                            &[(GRAYMAP[i2] as usize, ks), (GRAYMAP[i33] as usize, ks + 1)],
                        ),
                        _ => complex_abs_sum(
                            metrics,
                            &[
                                (GRAYMAP[i1] as usize, ks),
                                (GRAYMAP[i2] as usize, ks + 1),
                                (GRAYMAP[i33] as usize, ks + 2),
                            ],
                        ),
                    };
                    *slot = if srr < 2.5 {
                        if srr > 2.3 {
                            value * value
                        } else if value < 5.77 {
                            1.0 + 8.0 * value.powi(2) - 0.12 * value.powi(4)
                        } else {
                            (value + 5.82).powi(2)
                        }
                    } else {
                        value
                    };
                }

                for ib in 0..=ibmax {
                    let bit = ibmax - ib;
                    let bm = max_by_bit(&s2, bit, true) - max_by_bit(&s2, bit, false);
                    let idx = i32 + ib - 1;
                    if idx >= N {
                        continue;
                    }
                    match nsym {
                        1 => {
                            bmeta[idx] = bm;
                            let den = max_by_bit(&s2, bit, true).max(max_by_bit(&s2, bit, false));
                            bmetd[idx] = if den > 0.0 { bm / den } else { 0.0 };
                        }
                        2 => bmetb[idx] = bm,
                        _ => bmetc[idx] = bm,
                    }
                }
                k += nsym;
            }
        }
    }

    normalizebmet(&mut bmeta);
    normalizebmet(&mut bmetb);
    normalizebmet(&mut bmetc);
    normalizebmet(&mut bmetd);

    let apmask = [0i8; N];
    for llr_source in regular_llr_sources(config, context, &bmeta, &bmetb, &bmetc, &bmetd) {
        let mut llrz = [0.0f32; N];
        for i in 0..N {
            llrz[i] = 2.83 * llr_source[i];
        }
        if let Some(decoded) =
            bpdecode174_91(&llrz, &apmask, 30).or_else(|| osd174_91(&llrz, &apmask, 3))
        {
            if let Some(result) =
                decoded_to_result(metrics, refined_freq, refined_dt, decoded, config, book, 0)
            {
                return Some(result);
            }
        }
    }

    let apmag = bmeta.iter().map(|value| value.abs()).fold(0.0f32, f32::max) * 2.83 * 1.01;
    for (isubp2, iaptype) in plan_ap_subpasses(config) {
        let Some(ap) = build_ap_mask(config, iaptype) else {
            continue;
        };
        let llr_source = ap_llr_source(isubp2, &bmeta, &bmetb, &bmetc);
        let mut llrz = [0.0f32; N];
        for i in 0..N {
            llrz[i] = 2.83 * llr_source[i];
        }
        for i in 0..77 {
            if ap.apmask[i] == 1 {
                llrz[i] = apmag * if ap.message77[i] == 1 { 1.0 } else { -1.0 };
            }
        }
        if let Some(result) = bpdecode174_91(&llrz, &ap.apmask, 30)
            .or_else(|| osd174_91(&llrz, &ap.apmask, ap_ndeep(config, iaptype)))
            .and_then(|decoded| {
                decoded_to_result(
                    metrics,
                    refined_freq,
                    refined_dt,
                    decoded,
                    config,
                    book,
                    iaptype,
                )
            })
        {
            return Some(result);
        }
    }

    None
}

fn regular_llr_sources<'a>(
    config: &StreamDecodeConfig,
    context: Ft8bCandidateContext,
    bmeta: &'a [f32; N],
    bmetb: &'a [f32; N],
    bmetc: &'a [f32; N],
    bmetd: &'a [f32; N],
) -> Vec<&'a [f32; N]> {
    let mut sources = Vec::with_capacity(if config.swl { 8 } else { 6 });
    for isubp1 in 1..=2 {
        for isubp2 in 1..=4 {
            if !config.swl && isubp2 == 4 {
                continue;
            }
            let source = match isubp2 {
                1 => {
                    if (!config.swl && context.ipass == 1) || (isubp1 > 1 && context.ipass > 1) {
                        bmetd
                    } else {
                        bmeta
                    }
                }
                2 => {
                    if isubp1 > 1 {
                        bmeta
                    } else {
                        bmetb
                    }
                }
                3 => bmetc,
                4 => bmetd,
                _ => unreachable!(),
            };
            sources.push(source);
        }
    }
    sources
}

fn ap_llr_source<'a>(
    isubp2: usize,
    bmeta: &'a [f32; N],
    bmetb: &'a [f32; N],
    bmetc: &'a [f32; N],
) -> &'a [f32; N] {
    match isubp2 {
        5 | 8 | 11 | 14 | 17 | 20 | 23 | 26 | 29 => bmetc,
        6 | 9 | 10 | 12 | 13 | 15 | 16 | 18 | 21 | 24 | 27 | 30 => bmetb,
        7 | 19 | 22 | 25 | 28 | 31 => bmeta,
        _ => bmeta,
    }
}

fn ap_ndeep(config: &StreamDecodeConfig, _iaptype: i32) -> usize {
    if config.nagain {
        5
    } else {
        3
    }
}

fn plan_ap_subpasses(config: &StreamDecodeConfig) -> Vec<(usize, i32)> {
    if !config.lft8apon {
        return Vec::new();
    }

    let iqso = config.nQSOProgress.min(NAPPASSES.len().saturating_sub(1));
    let ap_table = ap_type_table(config);
    let nappasses = NAPPASSES[iqso].min(ap_table[iqso].len());
    let mut subpasses = Vec::with_capacity(nappasses);

    for isubp2 in 5..(5 + nappasses) {
        let iaptype = ap_table[iqso][isubp2 - 5];
        if iaptype != 0 {
            subpasses.push((isubp2, iaptype));
        }
    }

    subpasses
}

fn ap_type_table(config: &StreamDecodeConfig) -> &'static [[i32; 27]; 6] {
    let mycall = config.mycall.as_deref().unwrap_or("");
    let hiscall = config.hiscall.as_deref().unwrap_or("");

    if config.lhound {
        &NHAPTYPES
    } else if is_nonstandard_call(mycall) {
        &NMYCNSAPTYPES
    } else if is_nonstandard_call(hiscall) {
        &NDXNSAPTYPES
    } else {
        &NAPTYPES
    }
}

fn is_nonstandard_call(call: &str) -> bool {
    let call = call.trim();
    if call.is_empty() {
        return false;
    }
    call.starts_with('<')
        || call.ends_with('>')
        || call.contains('/')
        || call.len() > 6
        || !call
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn decoded_to_result(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    decoded: BpDecodeResult,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    iaptype: i32,
) -> Option<Ft8bDecodeResult> {
    if decoded.cw.iter().all(|&bit| bit == 0) {
        return None;
    }
    let unpack_context = UnpackContext::with_calls(
        Some(book),
        config.mycall.as_deref(),
        config.hiscall.as_deref(),
    );
    let msg = unpack77_with_context(&decoded.message77, unpack_context)?;
    let (i3, n3) = i3_n3(&decoded.message77);
    if i3 > 4 || (i3 == 0 && n3 > 5) {
        return None;
    }
    let quality = 1.0 - (decoded.nharderror as f32 + decoded.dmin) / 60.0;
    let itone = tones_from_codeword(&decoded.cw);
    let xsnr = estimate_snr(metrics, &itone, iaptype);
    let filter_context = FilterContext {
        mycall: config.mycall.clone().unwrap_or_default(),
        hiscall: config.hiscall.clone().unwrap_or_default(),
        hisgrid4: config
            .hisgrid
            .as_deref()
            .unwrap_or("")
            .chars()
            .take(4)
            .collect(),
        quality,
        xsnr,
        rxdt: refined_dt as f32 - 0.5,
    };
    let lcall1hash = msg.starts_with('<');
    if !accept_decoded_message(&msg, "", i3, n3, iaptype, lcall1hash, &filter_context) {
        return None;
    }
    if config.hide_hash && msg.find("<...>").is_some_and(|idx| idx >= 6) {
        return None;
    }
    let _niterations = decoded.iter;
    Some(Ft8bDecodeResult {
        msg37: msg,
        msg37_2: String::new(),
        snr: xsnr,
        freq: refined_freq as f32,
        dt: refined_dt as f32,
        iaptype,
        i3: i3 as i32,
        n3: n3 as i32,
        itone,
    })
}

fn tones_from_codeword(codeword: &[u8; N]) -> [i32; 79] {
    let mut itone = [0i32; 79];
    for i in 0..7 {
        itone[i] = ICOS7[i];
        itone[36 + i] = ICOS7[i];
        itone[72 + i] = ICOS7[i];
    }
    let mut k = 7usize;
    for j in 1..=58 {
        let i = (j - 1) * 3;
        if j == 30 {
            k += 7;
        }
        let indx =
            codeword[i] as usize * 4 + codeword[i + 1] as usize * 2 + codeword[i + 2] as usize;
        itone[k] = GRAYMAP[indx];
        k += 1;
    }
    itone
}

fn sync_snr_ratio(metrics: &SymbolMetrics) -> f32 {
    let mut synclev = 0.0f32;
    for k in 0..7 {
        synclev += metrics.s8[ICOS7[k] as usize][k + 36];
    }
    let mut total = 0.0f32;
    for tone in 0..8 {
        for k in 36..43 {
            total += metrics.s8[tone][k];
        }
    }
    let mut snoiselev = (total - synclev) / 7.0;
    if snoiselev < 0.1 {
        snoiselev = 1.0;
    }
    synclev / snoiselev
}

fn complex_abs(metrics: &SymbolMetrics, tone: usize, k: usize) -> f32 {
    (metrics.cs_re[tone][k].powi(2) + metrics.cs_im[tone][k].powi(2)).sqrt()
}

fn complex_abs_sum(metrics: &SymbolMetrics, pairs: &[(usize, usize)]) -> f32 {
    let mut re = 0.0f32;
    let mut im = 0.0f32;
    for &(tone, k) in pairs {
        re += metrics.cs_re[tone][k];
        im += metrics.cs_im[tone][k];
    }
    (re * re + im * im).sqrt()
}

fn max_by_bit(values: &[f32], bit: usize, wanted: bool) -> f32 {
    values
        .iter()
        .enumerate()
        .filter_map(|(i, &value)| {
            let is_one = ((i >> bit) & 1) == 1;
            (is_one == wanted).then_some(value)
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

fn normalizebmet(bmet: &mut [f32; N]) {
    let sigma = (bmet.iter().map(|value| value * value).sum::<f32>() / N as f32).sqrt();
    if sigma > 0.0 {
        for value in bmet {
            *value /= sigma;
        }
    }
}

fn i3_n3(message77: &[u8; 77]) -> (usize, usize) {
    let n3 = bits_to_usize(&message77[71..74]);
    let i3 = bits_to_usize(&message77[74..77]);
    (i3, n3)
}

fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}

fn estimate_snr(metrics: &SymbolMetrics, itone: &[i32; 79], iaptype: i32) -> f32 {
    let mut xsnrtmp = 0.001f32;
    for (i, &tone) in itone.iter().enumerate() {
        let tone = tone.clamp(0, 7) as usize;
        let xsig = metrics.s8[tone][i] * metrics.s8[tone][i];
        let mut total = 0.0f32;
        for itone in 0..8 {
            total += metrics.s8[itone][i] * metrics.s8[itone][i];
        }
        let mut xnoi = (total - xsig) / 7.0;
        if xnoi < 0.01 {
            xnoi = 0.01;
        }
        let xsnr = if xnoi < xsig { xsig / xnoi } else { 1.01 };
        xsnrtmp += xsnr;
    }

    let mut xsnr = xsnrtmp / 79.0 - 1.0;
    xsnr = 10.0 * xsnr.max(1.0e-12).log10() - 26.5;
    if xsnr > 7.0 {
        xsnr += (xsnr - 7.0) / 2.0;
    }
    if xsnr > 30.0 {
        xsnr -= 1.0;
        if xsnr > 40.0 {
            xsnr -= 1.0;
        }
        if xsnr > 49.0 {
            xsnr = 49.0;
        }
    }
    if xsnr < -17.0 {
        if xsnr < -22.5 && xsnr > -23.5 {
            xsnr = -22.5;
        }
        xsnr = xsnr - (1.0 + 1.4 / (23.0 + xsnr)).powi(2) + 1.2;
    }
    if iaptype == 0 {
        xsnr.max(-23.0)
    } else {
        xsnr.max(-24.0)
    }
}
