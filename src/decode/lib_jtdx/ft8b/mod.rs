//! Mirrors JTDX `lib/ft8b.f90`.

use super::ft8_downsample::ft8_downsample;
use super::ft8_mod1::ICOS7;
use super::ft8_params::{DT2, FS2, TWO_PI};
use super::ft8apset::Ft8ApSet;
use super::ft8v2::bpdecode174_91::N;
use super::ft8v2::packjt77::HashCallBook;
use super::ft8v2::subtractft8::subtractft8;
use super::gen_ft8wave::gen_ft8wave;
use super::sync8::SyncCandidate;
use super::sync8d::{build_ctwk, sync8d, Sync8dContext};
use super::syncdist::sync_rank_distribution;
use super::tone8::Tone8Tables;
use super::tonesd::tonesd;
use super::twkfreq1::twkfreq1;
use crate::decode::lib_jtdx::four2a::four2a_c2c;
use crate::stream::session::StreamDecodeConfig;
mod classify;
mod decode_helpers;
mod qso;
mod regular;
mod state;

use decode_helpers::*;
use qso::{jtdx_qso_plan, qso_attempts};
use regular::{regular_decode, try_ft8s_virtual, try_ft8sd_iqso4};
pub use state::{
    DecodeSource, Ft8bCandidateContext, Ft8bDecodeResult, Ft8bWorkspace, LastRxMsgText,
};
use state::{MetricSource, SignalMemory, SymbolMetrics, SyncGate};

#[derive(Clone, Debug)]
struct QsoRefinementState {
    cd0: super::ft8_downsample::ComplexC,
    ibest: isize,
    refined_freq: f64,
    refined_dt: f64,
}

#[derive(Clone, Debug)]
struct QsoAttemptOutcome {
    decoded: Option<Ft8bDecodeResult>,
    state: QsoRefinementState,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DxSymbolSeed {
    pub freq: f32,
    pub xdt0: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct DxSymbolField {
    pub s8: [[f32; 79]; 8],
    pub llr: [f32; N],
    // Keep the refined symbol index in the D1 wrapper contract for audits and
    // diagnostics, even though the current dx stack consumes refined freq/dt.
    #[allow(dead_code)]
    pub ibest: isize,
    pub refined_freq: f64,
    pub refined_dt: f64,
    pub syncavemax: f32,
    pub nsync: usize,
}

pub(crate) fn ft8b(
    workspace: &mut Ft8bWorkspace,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    tone8_tables: &Tone8Tables,
    ft8apset: &Ft8ApSet,
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

    let attempts = qso_attempts(qso_plan, config.nft8rxfsens);
    let mut previous_qso_state: Option<QsoRefinementState> = None;
    for iqso in attempts {
        if qso_plan.xdt0 < -4.9 || qso_plan.xdt0 > 4.9 {
            continue;
        }
        let cd0 = match (qso_plan.lvirtual3, iqso) {
            (true, 2 | 3) => workspace.downsample_out.c3.clone(),
            (_, 2 | 3) => workspace.downsample_out.c2.clone(),
            _ => workspace.downsample_out.c0.clone(),
        };
        let Some(outcome) = try_ft8b_decode_for_iqso(
            &cd0,
            config,
            book,
            tone8_tables,
            ft8apset,
            candidate,
            context,
            iqso,
            qso_plan.xdt0,
            qso_plan.lvirtual2 || qso_plan.lvirtual3,
            previous_qso_state.as_ref(),
            &mut workspace.signal_memory,
        ) else {
            continue;
        };
        previous_qso_state = Some(outcome.state.clone());
        if let Some(result) = outcome.decoded {
            if context.lsubtract {
                let xdt3 = refined_subtract_dt(&cd0, &result.itone, outcome.state.ibest);
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

pub(crate) fn dx_symbol_field(
    dd8: &mut [f32],
    workspace: &mut Ft8bWorkspace,
    config: &StreamDecodeConfig,
    seed: DxSymbolSeed,
) -> Option<DxSymbolField> {
    let context = Ft8bCandidateContext {
        ipass: 1,
        npass: 1,
        lsubtract: false,
        lhighsens: config.swl || config.nagain || config.lft8lowth || config.lft8subpass,
        lcqcand: false,
        levenint: false,
        loddint: false,
        lqsomsgdcd: false,
        lft8sdec: false,
        stophint: config.stophint,
        nlasttx: config.nQSOProgress,
        call_dt_xdt: None,
        sd_msg: None,
        sd_lcq: false,
        sd_index: None,
        last_rx_msg: None,
        last_rx_xdt: None,
        last_rx_is_rrr: false,
    };
    let candidate = SyncCandidate {
        freq: seed.freq,
        dt: seed.xdt0,
        sync: 0.0,
        lcq: false,
        sort_metric: 0.0,
    };

    ft8_downsample(
        &mut workspace.downsample,
        dd8,
        true,
        seed.freq,
        1,
        context.lhighsens,
        &mut workspace.lsubtracted,
        &mut workspace.npos,
        &workspace.freqsub,
        &mut workspace.downsample_out,
    );

    let sync_context = Sync8dContext {
        ipass: context.ipass,
        lastsync: false,
        iqso: 1,
        lcq: false,
        lcallsstd: jtdx_config_calls_standard(config),
        lcqcand: false,
        tonesd: None,
        csynce: None,
    };
    let state = refine_qso_sync(
        &workspace.downsample_out.c0,
        candidate,
        1,
        seed.xdt0,
        sync_context,
    );
    let metrics = extract_symbol_metrics(&state.cd0, state.ibest, config, context);
    let bmet = build_bit_metrics(&metrics, MetricSource::Cs);
    let mut llr = [0.0f32; N];
    for (dst, src) in llr.iter_mut().zip(bmet.bmeta.iter().copied()) {
        *dst = 2.83 * src;
    }

    Some(DxSymbolField {
        s8: metrics.s8,
        llr,
        ibest: state.ibest,
        refined_freq: state.refined_freq,
        refined_dt: state.refined_dt,
        syncavemax: metrics.syncavemax,
        nsync: metrics.nsync,
    })
}

pub(crate) fn dx_symbol_estimate_snr(
    field: &DxSymbolField,
    itone: &[i32; 79],
    iaptype: i32,
    lft8s_or_sd: bool,
) -> f32 {
    estimate_snr_from_s8(&field.s8, itone, iaptype, lft8s_or_sd)
}

fn try_ft8b_decode_for_iqso(
    cd0: &super::ft8_downsample::ComplexC,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    tone8_tables: &Tone8Tables,
    ft8apset: &Ft8ApSet,
    candidate: SyncCandidate,
    context: Ft8bCandidateContext,
    iqso: usize,
    xdt0: f32,
    lvirtual: bool,
    previous_qso_state: Option<&QsoRefinementState>,
    signal_memory: &mut SignalMemory,
) -> Option<QsoAttemptOutcome> {
    let ldeepsync = config.lft8lowth || config.lft8subpass || config.swl;
    let tonesd_templates = if iqso == 4 && ldeepsync {
        context
            .sd_msg
            .as_ref()
            .and_then(|msg| tonesd(msg.as_str(), context.sd_lcq))
    } else {
        None
    };
    let csynce_templates = if iqso == 2 || iqso == 3 {
        tone8_tables.csynce.clone()
    } else {
        None
    };
    let sync_context = Sync8dContext {
        ipass: context.ipass,
        lastsync: false,
        iqso,
        lcq: iqso == 4 && context.sd_lcq,
        lcallsstd: jtdx_config_calls_standard(config),
        lcqcand: context.lcqcand,
        tonesd: tonesd_templates.as_ref(),
        csynce: csynce_templates.as_ref(),
    };
    let state = if iqso == 4 && !ldeepsync {
        if let Some(previous) = previous_qso_state {
            previous.clone()
        } else {
            return None;
        }
    } else if iqso == 3 {
        if let Some(previous) = previous_qso_state {
            QsoRefinementState {
                cd0: previous.cd0.clone(),
                ibest: previous.ibest + 1,
                refined_freq: previous.refined_freq,
                refined_dt: previous.refined_dt,
            }
        } else {
            refine_qso_sync(cd0, candidate, iqso, xdt0, sync_context)
        }
    } else {
        refine_qso_sync(cd0, candidate, iqso, xdt0, sync_context)
    };
    let metrics = extract_symbol_metrics(&state.cd0, state.ibest, config, context);

    let decoded = if iqso > 1 && iqso < 4 {
        try_ft8s_virtual(
            &metrics,
            state.refined_freq,
            state.refined_dt,
            config,
            book,
            context,
            lvirtual,
            tone8_tables,
        )
    } else if iqso == 4 {
        try_ft8sd_iqso4(
            &metrics,
            state.refined_freq,
            state.refined_dt,
            config,
            book,
            context,
            ldeepsync,
        )
    } else if let Some(sync_gate) = jtdx_sync_gate(
        &metrics,
        config,
        context,
        state.refined_freq,
        state.refined_dt,
    ) {
        regular_decode(
            &metrics,
            candidate,
            state.refined_freq,
            state.refined_dt,
            config,
            book,
            tone8_tables,
            ft8apset,
            context,
            sync_gate,
            signal_memory,
        )
    } else {
        None
    };

    Some(QsoAttemptOutcome { decoded, state })
}

fn refine_qso_sync(
    cd0: &super::ft8_downsample::ComplexC,
    candidate: SyncCandidate,
    iqso: usize,
    xdt0: f32,
    sync_context: Sync8dContext,
) -> QsoRefinementState {
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
    let freq_step = if iqso == 1 || iqso == 4 { 0.5 } else { 0.25 };
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

    let mut a = [0.0f64; 5];
    a[0] = -delfbest;
    QsoRefinementState {
        cd0: twkfreq1(cd0, -800, 3199, 4000, FS2, &a),
        ibest,
        refined_freq: candidate.freq as f64 + delfbest,
        refined_dt: xdt2,
    }
}

fn jtdx_config_calls_standard(config: &StreamDecodeConfig) -> bool {
    let lmycallstd = normalized_config_call(config.mycall.as_deref())
        .is_some_and(|call| !is_nonstandard_call(&call));
    let lhiscallstd = normalized_config_call(config.hiscall.as_deref())
        .is_some_and(|call| !is_nonstandard_call(&call));
    lmycallstd && lhiscallstd
}

fn nint(x: f64) -> isize {
    (x as f32).round() as isize
}

fn extract_symbol_metrics(
    cd0: &super::ft8_downsample::ComplexC,
    ibest: isize,
    config: &StreamDecodeConfig,
    context: Ft8bCandidateContext,
) -> SymbolMetrics {
    let mut s8 = [[0.0f32; 79]; 8];
    let mut cs_re = [[0.0f32; 79]; 8];
    let mut cs_im = [[0.0f32; 79]; 8];
    let mut csr_re = [[0.0f32; 79]; 8];
    let mut csr_im = [[0.0f32; 79]; 8];
    let mut cscs_re = [[0.0f32; 79]; 8];
    let mut cscs_im = [[0.0f32; 79]; 8];
    let s256 = compute_s256(cd0, ibest);
    let mut snrsync = [0.0f32; 21];
    let mut re = [0.0f64; 32];
    let mut im = [0.0f64; 32];
    let mut rr_re = [0.0f64; 32];
    let mut rr_im = [0.0f64; 32];

    for k in 0..79 {
        let Some(costas_idx) = costas_index(k) else {
            continue;
        };
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
        let mut s81 = [0.0f32; 8];
        for tone in 0..8 {
            s81[tone] = (re[tone] * re[tone] + im[tone] * im[tone]).sqrt() as f32;
        }
        let tone = ICOS7[costas_idx] as usize;
        let synclev = s81[tone];
        let mut sum_s81 = 0.0f32;
        for value in s81 {
            sum_s81 += value;
        }
        let snoiselev = (sum_s81 - synclev) / 7.0;
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

    let mut sum_snrsync = 0.0f32;
    for value in snrsync {
        sum_snrsync += value;
    }
    let syncav = sum_snrsync / 21.0;
    let mut syncavpart = [3.0f32; 3];
    syncavpart[0] = sum_slice_f32(&snrsync, 0, 7) / 7.0;
    syncavpart[1] = sum_slice_f32(&snrsync, 7, 14) / 7.0;
    syncavpart[2] = sum_slice_f32(&snrsync, 14, 21) / 7.0;
    let mut syncavemax = syncavpart[0];
    for value in syncavpart.iter().skip(1) {
        if *value > syncavemax {
            syncavemax = *value;
        }
    }
    let lreverse = if !config.swl {
        if config.nft8cycles < 2 {
            context.ipass == 2
        } else {
            context.ipass == 5 || context.ipass == 7
        }
    } else if config.nft8swlcycles < 2 {
        context.ipass == 2
    } else {
        context.ipass == 5 || context.ipass == 7
    };

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

        if syncav < 2.5 {
            scale_weak_symbol_edges(&mut re, &mut im);
        }

        for i in 0..32 {
            rr_re[i] = re[31 - i];
            rr_im[i] = -im[31 - i];
        }

        if lreverse {
            four2a_c2c(&mut re, &mut im, -1);
            for tone in 0..8 {
                cscs_re[tone][k] = (re[tone] / 1000.0) as f32;
                cscs_im[tone][k] = (im[tone] / 1000.0) as f32;
            }
            four2a_c2c(&mut rr_re, &mut rr_im, -1);
            for tone in 0..8 {
                cs_re[tone][k] = (rr_re[tone] / 1000.0) as f32;
                cs_im[tone][k] = (rr_im[tone] / 1000.0) as f32;
                csr_re[tone][k] = cs_re[tone][k];
                csr_im[tone][k] = cs_im[tone][k];
                s8[tone][k] = (rr_re[tone] * rr_re[tone] + rr_im[tone] * rr_im[tone]).sqrt() as f32;
            }
        } else {
            four2a_c2c(&mut re, &mut im, -1);
            for tone in 0..8 {
                cs_re[tone][k] = (re[tone] / 1000.0) as f32;
                cs_im[tone][k] = (im[tone] / 1000.0) as f32;
                s8[tone][k] = (re[tone] * re[tone] + im[tone] * im[tone]).sqrt() as f32;
            }
            four2a_c2c(&mut rr_re, &mut rr_im, -1);
            for tone in 0..8 {
                csr_re[tone][k] = (rr_re[tone] / 1000.0) as f32;
                csr_im[tone][k] = (rr_im[tone] / 1000.0) as f32;
            }
        }
    }

    normalize_tone_spectra(
        &mut s8,
        &mut cs_re,
        &mut cs_im,
        &mut csr_re,
        &mut csr_im,
        &mut cscs_re,
        &mut cscs_im,
    );
    let (nsync, nsync2) = sync_quality(&s8);

    SymbolMetrics {
        s8,
        cs_re,
        cs_im,
        csr_re,
        csr_im,
        cscs_re,
        cscs_im,
        s256,
        syncavemax,
        nsync,
        nsync2,
    }
}

fn costas_index(k: usize) -> Option<usize> {
    if k <= 6 {
        Some(k)
    } else if (36..=42).contains(&k) {
        Some(k - 36)
    } else if (72..=78).contains(&k) {
        Some(k - 72)
    } else {
        None
    }
}

fn sync_quality(s8: &[[f32; 79]; 8]) -> (usize, usize) {
    let mut nsync = 0usize;
    let mut nsync2 = 0usize;
    for k in 0..7 {
        for base in [0usize, 36, 72] {
            let sym = base + k;
            let best = max_tone(s8, sym, None);
            if best == ICOS7[k] as usize {
                nsync += 1;
            } else {
                let second = max_tone(s8, sym, Some(best));
                if second == ICOS7[k] as usize {
                    nsync2 += 1;
                }
            }
        }
    }
    (nsync, nsync2)
}

fn compute_s256(cd0: &super::ft8_downsample::ComplexC, ibest: isize) -> [f32; 27] {
    let mut re = [0.0f64; 256];
    let mut im = [0.0f64; 256];
    let dphi = TWO_PI * 3.125 * DT2;
    let mut phi = 0.0f64;
    for i in 0..256 {
        let src = ibest + 224 + i as isize;
        let twk_re = phi.cos();
        let twk_im = phi.sin();
        if (super::ft8_downsample::C_LOW..=super::ft8_downsample::C_HIGH).contains(&src) {
            let idx = super::ft8_downsample::ComplexC::idx(src);
            re[i] = cd0.re[idx] * twk_re - cd0.im[idx] * twk_im;
            im[i] = cd0.re[idx] * twk_im + cd0.im[idx] * twk_re;
        }
        phi = (phi + dphi) % TWO_PI;
    }
    four2a_c2c(&mut re, &mut im, -1);

    let mut s256 = [0.0f32; 27];
    for i in 0..=8 {
        s256[i] = (re[i] * re[i] + im[i] * im[i]).sqrt() as f32;
    }
    for i in 9..=26 {
        let src = i - 1;
        s256[i] = (re[src] * re[src] + im[src] * im[src]).sqrt() as f32;
    }
    s256
}

fn normalize_tone_spectra(
    s8: &mut [[f32; 79]; 8],
    cs_re: &mut [[f32; 79]; 8],
    cs_im: &mut [[f32; 79]; 8],
    csr_re: &mut [[f32; 79]; 8],
    csr_im: &mut [[f32; 79]; 8],
    cscs_re: &mut [[f32; 79]; 8],
    cscs_im: &mut [[f32; 79]; 8],
) {
    let mut sp = [0.0f32; 8];
    for tone in 0..8 {
        let mut sum1 = 0.0f32;
        for k in 0..7 {
            sum1 += s8[tone][k];
        }
        let mut sum2 = 0.0f32;
        for k in 17..79 {
            sum2 += s8[tone][k];
        }
        sp[tone] = sum1 + sum2;
    }
    let mut ka = 0usize;
    let mut spmin = sp[0];
    for (tone, value) in sp.iter().enumerate().skip(1) {
        if *value < spmin {
            spmin = *value;
            ka = tone;
        }
    }
    if spmin <= 0.0 {
        return;
    }
    for tone in 0..8 {
        if tone == ka {
            continue;
        }
        let spr = sp[tone] / spmin;
        if spr > 1.5 {
            let sprsqr = spr.sqrt();
            for k in 0..79 {
                s8[tone][k] /= spr;
                cs_re[tone][k] /= sprsqr;
                cs_im[tone][k] /= sprsqr;
                csr_re[tone][k] /= sprsqr;
                csr_im[tone][k] /= sprsqr;
                cscs_re[tone][k] /= sprsqr;
                cscs_im[tone][k] /= sprsqr;
            }
        }
    }
}

fn scale_weak_symbol_edges(re: &mut [f64; 32], im: &mut [f64; 32]) {
    re[0] *= 1.9;
    im[0] *= 1.9;
    re[31] *= 1.9;
    im[31] *= 1.9;
    let a0 = (re[0] * re[0] + im[0] * im[0]).sqrt();
    let a31 = (re[31] * re[31] + im[31] * im[31]).sqrt();
    if a31 <= 0.0 {
        return;
    }
    let scr = a0.sqrt() / a31.sqrt();
    if scr > 1.0 {
        re[31] *= scr;
        im[31] *= scr;
    } else if scr > 1.0e-16 {
        re[0] /= scr;
        im[0] /= scr;
    }
}

fn jtdx_sync_gate(
    metrics: &SymbolMetrics,
    config: &StreamDecodeConfig,
    context: Ft8bCandidateContext,
    refined_freq: f64,
    refined_dt: f64,
) -> Option<SyncGate> {
    let mut lapcqonly = false;
    if context.lcqcand && metrics.nsync == 4 {
        if metrics.nsync + metrics.nsync2 < 12 && cq_shape_score(&metrics.s8) < 6.6 {
            return None;
        }
        lapcqonly = true;
    } else if context.lcqcand && metrics.nsync == 5 {
        if metrics.nsync + metrics.nsync2 < 12 && cq_shape_score(&metrics.s8) < 6.1 {
            return None;
        }
        lapcqonly = true;
    } else if context.lcqcand && metrics.nsync == 6 {
        if metrics.nsync + metrics.nsync2 < 11 && cq_shape_score(&metrics.s8) < 5.6 {
            return None;
        }
        lapcqonly = true;
    } else if metrics.nsync < 7 {
        return None;
    }

    let mut lskipnotap = false;
    if !lapcqonly && metrics.nsync < 11 {
        let nsmax = sync_rank_distribution(&metrics.s8);
        if nsmax[6] + nsmax[7] > nsmax[1] + nsmax[2] || nsmax[4] + nsmax[5] > nsmax[1] + nsmax[2] {
            lskipnotap = true;
        }
    }

    let dfqso = (config.nfqso - refined_freq).abs();
    if (dfqso >= 2.0 || (dfqso < 2.0 && context.stophint))
        && !jtdx_soft_sync_gate(&metrics.s8, refined_dt)
    {
        return None;
    }

    Some(SyncGate {
        lapcqonly,
        lskipnotap,
    })
}

fn cq_shape_score(s8: &[[f32; 79]; 8]) -> f32 {
    let mut rscq = 0.0f32;
    for k11 in 8..=16 {
        let sym = k11 - 1;
        let best = max_tone(s8, sym, None);
        if k11 < 16 {
            if best == 0 {
                rscq += 1.0;
            }
        } else if best == 1 {
            rscq += 1.0;
        }
    }
    for (sym, tones) in [(16usize, [0usize, 1usize]), (26, [0, 1]), (32, [2, 3])] {
        if tones.contains(&max_tone(s8, sym, None)) {
            rscq += 0.5;
        }
    }
    rscq
}

fn jtdx_soft_sync_gate(s8: &[[f32; 79]; 8], refined_dt: f64) -> bool {
    let rrxdt = refined_dt as f32 - 0.5;
    let mut syncw = [0.0f32; 7];
    let mut sumkw = [1.0f32; 7];
    if (-0.5..=2.13).contains(&rrxdt) {
        for k in 0..7 {
            syncw[ICOS7[k] as usize] = s8[ICOS7[k] as usize][k]
                + s8[ICOS7[k] as usize][k + 36]
                + s8[ICOS7[k] as usize][k + 72];
        }
        for tone in 0..7 {
            sumkw[tone] = (sum_symbol_range(s8, tone, 0, 79) - syncw[tone]) / 25.333;
        }
    } else if rrxdt < -0.5 {
        for k in 0..7 {
            syncw[ICOS7[k] as usize] =
                s8[ICOS7[k] as usize][k + 36] + s8[ICOS7[k] as usize][k + 72];
        }
        for tone in 0..7 {
            sumkw[tone] = (sum_symbol_range(s8, tone, 25, 79) - syncw[tone]) / 26.0;
        }
    } else {
        for k in 0..7 {
            syncw[ICOS7[k] as usize] = s8[ICOS7[k] as usize][k] + s8[ICOS7[k] as usize][k + 36];
        }
        for tone in 0..7 {
            sumkw[tone] = (sum_symbol_range(s8, tone, 0, 54) - syncw[tone]) / 26.0;
        }
    }

    let mut nsyncscorew = 0usize;
    let mut scoreratiow = [0.0f32; 7];
    for tone in 0..7 {
        if syncw[tone] > sumkw[tone] {
            nsyncscorew += 1;
        }
        scoreratiow[tone] = syncw[tone] / sumkw[tone];
    }

    let mut nsyncscore1 = 0usize;
    let mut nsyncscore2 = 0usize;
    let mut nsyncscore3 = 0usize;
    let mut scoreratio1 = 0.0f32;
    let mut scoreratio2 = 0.0f32;
    let mut scoreratio3 = 0.0f32;
    for k in 0..7 {
        if rrxdt >= -0.5 {
            let (hit, ratio) = sync_score_at(s8, ICOS7[k] as usize, k);
            if hit {
                nsyncscore1 += 1;
                scoreratio1 += ratio;
            }
        }
        let (hit, ratio) = sync_score_at(s8, ICOS7[k] as usize, k + 36);
        if hit {
            nsyncscore2 += 1;
            scoreratio2 += ratio;
        }
        if rrxdt <= 2.13 {
            let (hit, ratio) = sync_score_at(s8, ICOS7[k] as usize, k + 72);
            if hit {
                nsyncscore3 += 1;
                scoreratio3 += ratio;
            }
        }
    }

    let nsyncscore = nsyncscore1 + nsyncscore2 + nsyncscore3;
    let mut scoreratio = scoreratio1 + scoreratio2 + scoreratio3;
    if nsyncscore > 0 {
        scoreratio /= nsyncscore as f32;
    } else {
        scoreratio = 0.0;
    }
    if nsyncscore1 > 0 {
        scoreratio1 /= nsyncscore1 as f32;
    } else {
        scoreratio1 = 0.0;
    }
    // JTDX ft8b.f90 normalizes scoreratio, scoreratio1, and scoreratio3 here,
    // but leaves scoreratio2 as the accumulated middle-sync ratio.
    if nsyncscore3 > 0 {
        scoreratio3 /= nsyncscore3 as f32;
    } else {
        scoreratio3 = 0.0;
    }

    if (-0.5..=2.13).contains(&rrxdt) {
        if nsyncscore < 8
            || (nsyncscore < 10 && scoreratio < 5.5)
            || (nsyncscore < 11 && scoreratio < 3.63)
        {
            return false;
        }
        if nsyncscore == 11 && scoreratio < 5.37 {
            return !(nsyncscore1 < 5 && nsyncscore3 < 5 && scoreratio1 < 4.2 && scoreratio3 < 4.2);
        }
        if nsyncscore == 12 && scoreratio < 4.6 {
            return !(nsyncscore1 < 5 && nsyncscore3 < 5 && scoreratio1 < 4.0 && scoreratio3 < 4.0);
        }
        if nsyncscore == 13 && scoreratio < 4.4 {
            return !(nsyncscore1 < 5
                && nsyncscore2 < 6
                && nsyncscore3 < 5
                && scoreratio1 < 4.4
                && scoreratio3 < 4.4);
        }
        if nsyncscorew < 3 {
            return (nsyncscore1 > 5 && scoreratio1 > 13.8)
                || (nsyncscore2 > 5 && scoreratio2 > 13.8)
                || (nsyncscore3 > 5 && scoreratio3 > 13.8);
        }
        if nsyncscorew == 3 {
            return scoreratio1 > 15.0 || scoreratio2 > 15.0 || scoreratio3 > 15.0;
        }
        if nsyncscorew == 4 {
            return nsyncscore1 == 7
                || nsyncscore2 == 7
                || nsyncscore3 == 7
                || scoreratio1 > 10.0
                || scoreratio2 > 10.0
                || scoreratio3 > 10.0;
        }
        if nsyncscorew == 5 {
            return nsyncscore > 17
                || nsyncscore1 == 7
                || nsyncscore2 == 7
                || nsyncscore3 == 7
                || scoreratio1 > 10.0
                || scoreratio2 > 10.0
                || scoreratio3 > 10.0;
        }
    } else if rrxdt < -0.5 {
        if nsyncscore < 6
            || (nsyncscore > 5
                && nsyncscore < 8
                && nsyncscorew < 6
                && scoreratio2 < 5.5
                && scoreratio3 < 5.5)
        {
            return false;
        }
        if nsyncscore == 8 {
            return !(nsyncscore2 < 6 && nsyncscore3 < 6 && scoreratio2 < 6.6 && scoreratio3 < 6.6);
        }
        if nsyncscore == 9 && scoreratio < 6.0 {
            return !(nsyncscore2 < 6 && nsyncscore3 < 6 && scoreratio2 < 6.6 && scoreratio3 < 6.5);
        }
        if nsyncscorew < 3 {
            return (nsyncscore2 > 5 && scoreratio2 > 13.8)
                || (nsyncscore3 > 5 && scoreratio3 > 13.8);
        }
        if nsyncscorew == 3 {
            // JTDX compares nsyncscore3 here, not scoreratio3.
            return scoreratio2 > 15.0 || nsyncscore3 > 15;
        }
        if nsyncscorew == 4 {
            // JTDX compares nsyncscore3 here, not scoreratio3.
            return nsyncscore2 == 7 || nsyncscore3 == 7 || scoreratio2 > 10.0 || nsyncscore3 > 10;
        }
        if nsyncscorew == 5 {
            return nsyncscore > 11
                || nsyncscore2 == 7
                || nsyncscore3 == 7
                || scoreratio2 > 10.0
                || scoreratio3 > 10.0;
        }
    } else {
        if nsyncscore < 6
            || (nsyncscore > 5
                && nsyncscore < 8
                && nsyncscorew < 6
                && scoreratio1 < 5.5
                && scoreratio2 < 5.5)
        {
            return false;
        }
        if nsyncscore == 8 {
            return !(nsyncscore1 < 6 && nsyncscore2 < 6 && scoreratio1 < 6.6 && scoreratio2 < 6.6);
        }
        if nsyncscore == 9 && scoreratio < 6.0 {
            return !(nsyncscore1 < 6 && nsyncscore2 < 6 && scoreratio2 < 6.6 && scoreratio1 < 6.5);
        }
        if nsyncscorew < 3 {
            return (nsyncscore1 > 5 && scoreratio1 > 13.8)
                || (nsyncscore2 > 5 && scoreratio2 > 13.8);
        }
        if nsyncscorew == 3 {
            return scoreratio1 > 15.0 || scoreratio2 > 15.0;
        }
        if nsyncscorew == 4 {
            // JTDX compares nsyncscore2 here, not scoreratio2.
            return nsyncscore1 == 7 || nsyncscore2 == 7 || scoreratio1 > 10.0 || nsyncscore2 > 10;
        }
        if nsyncscorew == 5 {
            return nsyncscore > 11
                || nsyncscore1 == 7
                || nsyncscore2 == 7
                || scoreratio1 > 10.0
                || scoreratio2 > 10.0;
        }
    }
    true
}

fn sync_score_at(s8: &[[f32; 79]; 8], tone: usize, sym: usize) -> (bool, f32) {
    let synck = s8[tone][sym];
    let sumk = (sum_tones(s8, sym) - synck) / 7.0;
    if sumk > 0.0 && synck > sumk {
        (true, synck / sumk)
    } else {
        (false, 0.0)
    }
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

pub(super) fn sum_tones(s8: &[[f32; 79]; 8], k: usize) -> f32 {
    let mut sum = 0.0f32;
    for tone in s8.iter().take(8) {
        sum += tone[k];
    }
    sum
}

fn sum_symbol_range(s8: &[[f32; 79]; 8], tone: usize, start: usize, end: usize) -> f32 {
    let mut sum = 0.0f32;
    for k in start..end {
        sum += s8[tone][k];
    }
    sum
}

fn sum_slice_f32(values: &[f32], start: usize, end: usize) -> f32 {
    let mut sum = 0.0f32;
    for value in values.iter().take(end).skip(start) {
        sum += *value;
    }
    sum
}

pub(super) fn max_tone(s8: &[[f32; 79]; 8], k: usize, skip: Option<usize>) -> usize {
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

pub(super) fn maxloc_1based(values: &[f32]) -> usize {
    let mut best_idx = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (idx, &value) in values.iter().enumerate() {
        if value > best_value {
            best_value = value;
            best_idx = idx + 1;
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::super::ft8_params::NFFT1_LONG;
    use super::*;

    #[test]
    fn dx_symbol_field_returns_ap_free_channel_llr_shape() {
        let mut dd8 = vec![0.0f32; NFFT1_LONG];
        let mut workspace = Ft8bWorkspace::default();
        let config = StreamDecodeConfig::default().clone_for_profile_jtdx_high_sensitivity();

        let field = dx_symbol_field(
            &mut dd8,
            &mut workspace,
            &config,
            DxSymbolSeed {
                freq: 1000.0,
                xdt0: 0.0,
            },
        )
        .expect("D1 wrapper should return a shaped field for a valid coarse seed");

        assert_eq!(field.s8.len(), 8);
        assert_eq!(field.s8[0].len(), 79);
        assert_eq!(field.llr.len(), N);
        assert!(field.llr.iter().all(|value| value.is_finite()));
        assert!(field.refined_freq.is_finite());
        assert!(field.refined_dt.is_finite());
    }
}
