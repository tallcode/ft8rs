//! Mirrors JTDX `lib/ft8b.f90`.

use super::ft8_downsample::ft8_downsample;
use super::ft8_mod1::ICOS7;
use super::ft8_params::{DT2, FS2, TWO_PI};
use super::ft8apset::build_ap_mask;
use super::ft8mf1::ft8mf1;
use super::ft8mfcq::ft8mfcq;
use super::ft8s::ft8s;
use super::ft8sd::ft8sd;
use super::ft8sd1::ft8sd1;
use super::ft8v2::bpdecode174_91::{bpdecode174_91, N};
use super::ft8v2::osd174_91::osd174_91;
use super::ft8v2::packjt77::HashCallBook;
use super::ft8v2::subtractft8::subtractft8;
use super::gen_ft8wave::gen_ft8wave;
use super::sync8::SyncCandidate;
use super::sync8d::{build_ctwk, sync8d, Sync8dContext};
use super::tone8::build_csynce;
use super::tonesd::tonesd;
use super::twkfreq1::twkfreq1;
use crate::stream::session::StreamDecodeConfig;
use crate::util::four2a_c2c;
mod decode_helpers;
mod state;

use decode_helpers::*;
use state::{
    CsMatrix, MetricSource, SignalClassifier, SignalKind, SignalMemory, SymbolMetrics, SyncGate,
    ToneHints,
};
pub use state::{
    DecodeSource, Ft8bCandidateContext, Ft8bDecodeResult, Ft8bWorkspace, LastRxMsgText,
};

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
        let cd0 = match (qso_plan.lvirtual3, iqso) {
            (true, 2 | 3) => workspace.downsample_out.c3.clone(),
            (_, 2 | 3) => workspace.downsample_out.c2.clone(),
            _ => workspace.downsample_out.c0.clone(),
        };
        if let Some((result, ibest)) = try_ft8b_decode_for_iqso(
            &cd0,
            config,
            book,
            candidate,
            context,
            iqso,
            qso_plan.xdt0,
            qso_plan.lvirtual2 || qso_plan.lvirtual3,
            &mut workspace.signal_memory,
        ) {
            if context.lsubtract {
                let xdt3 = refined_subtract_dt(&cd0, &result.itone, ibest);
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
    lvirtual: bool,
    signal_memory: &mut SignalMemory,
) -> Option<(Ft8bDecodeResult, isize)> {
    let tonesd_templates = if iqso == 4 {
        context
            .sd_msg
            .as_ref()
            .and_then(|msg| tonesd(msg.as_str(), context.sd_lcq))
    } else {
        None
    };
    let csynce_templates = if iqso == 2 || iqso == 3 {
        normalized_config_call(config.mycall.as_deref()).and_then(|mycall| {
            normalized_config_call(config.hiscall.as_deref())
                .and_then(|hiscall| build_csynce(&mycall, &hiscall))
        })
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

    let cd0 = twkfreq1(cd0, 3199, FS2, -delfbest);
    if iqso == 3 {
        ibest += 1;
    }
    let refined_freq = candidate.freq as f64 + delfbest;
    let refined_dt = xdt2;
    let metrics = extract_symbol_metrics(&cd0, ibest, config, context);

    if iqso > 1 && iqso < 4 {
        return try_ft8s_virtual(
            &metrics,
            refined_freq,
            refined_dt,
            config,
            book,
            context,
            lvirtual,
        )
        .map(|result| (result, ibest));
    }

    if iqso == 4 {
        return try_ft8sd(
            &metrics,
            refined_freq,
            refined_dt,
            config,
            book,
            context,
            middle_sync_ratio(&metrics.s8),
        )
        .map(|result| (result, ibest));
    }

    let sync_gate = jtdx_sync_gate(&metrics, config, context, refined_freq, refined_dt)?;

    regular_decode(
        &metrics,
        candidate,
        refined_freq,
        refined_dt,
        config,
        book,
        context,
        sync_gate,
        signal_memory,
    )
    .map(|result| (result, ibest))
}

fn jtdx_config_calls_standard(config: &StreamDecodeConfig) -> bool {
    let lmycallstd = normalized_config_call(config.mycall.as_deref())
        .is_some_and(|call| !is_nonstandard_call(&call));
    let lhiscallstd = normalized_config_call(config.hiscall.as_deref())
        .is_some_and(|call| !is_nonstandard_call(&call));
    lmycallstd && lhiscallstd
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
        3 => vec![1, 2, 3],
        4 => vec![1, 4],
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
    let qso_thread_active = lqsothread
        && !context.lft8sdec
        && config.hiscall.as_deref().unwrap_or("").trim().len() >= 3;

    let fdelta = (candidate.freq as f64 - config.nfqso).abs();
    if qso_thread_active {
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
            if context.sd_msg.is_some() && plan.nqso == 1 {
                plan.nqso = 4;
            }
            return plan;
        }

        let mut maxlasttx = 4;
        if candidate.dt.abs() > 4.9 && context.last_rx_is_rrr {
            maxlasttx = 5;
        }
        if !(1..=maxlasttx).contains(&context.nlasttx) {
            if context.sd_msg.is_some() && plan.nqso == 1 {
                plan.nqso = 4;
            }
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
    }

    if !context.levenint && !context.loddint {
        plan.lvirtual2 = false;
        plan.lvirtual3 = false;
    }
    if context.sd_msg.is_some() && plan.nqso == 1 {
        plan.nqso = 4;
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
            cs_re[tone][k] = (re[tone] / 1000.0) as f32;
            cs_im[tone][k] = (im[tone] / 1000.0) as f32;
            s8[tone][k] = (re[tone] * re[tone] + im[tone] * im[tone]).sqrt() as f32;
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
    let syncavemax = snrsync[0..7]
        .iter()
        .sum::<f32>()
        .max(snrsync[7..14].iter().sum::<f32>())
        .max(snrsync[14..21].iter().sum::<f32>())
        / 7.0;
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
        sp[tone] = s8[tone][0..7].iter().sum::<f32>() + s8[tone][17..79].iter().sum::<f32>();
    }
    let Some((ka, &spmin)) = sp
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
    else {
        return;
    };
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

fn sync_rank_distribution(s8: &[[f32; 79]; 8]) -> [usize; 8] {
    let mut nsmax = [0usize; 8];
    for k in 0..7 {
        for sym in [k, k + 36, k + 72] {
            let target = ICOS7[k] as usize;
            let mut used = [false; 8];
            for rank in 0..8 {
                let mut best = 0usize;
                let mut best_value = f32::NEG_INFINITY;
                for tone in 0..8 {
                    if !used[tone] && s8[tone][sym] > best_value {
                        best_value = s8[tone][sym];
                        best = tone;
                    }
                }
                used[best] = true;
                if best == target {
                    nsmax[rank] += 1;
                    break;
                }
            }
        }
    }
    nsmax
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
            sumkw[tone] = (s8[tone].iter().sum::<f32>() - syncw[tone]) / 25.333;
        }
    } else if rrxdt < -0.5 {
        for k in 0..7 {
            syncw[ICOS7[k] as usize] =
                s8[ICOS7[k] as usize][k + 36] + s8[ICOS7[k] as usize][k + 72];
        }
        for tone in 0..7 {
            sumkw[tone] = (s8[tone][25..79].iter().sum::<f32>() - syncw[tone]) / 26.0;
        }
    } else {
        for k in 0..7 {
            syncw[ICOS7[k] as usize] = s8[ICOS7[k] as usize][k] + s8[ICOS7[k] as usize][k + 36];
        }
        for tone in 0..7 {
            sumkw[tone] = (s8[tone][0..54].iter().sum::<f32>() - syncw[tone]) / 26.0;
        }
    }

    let mut nsyncscorew = 0usize;
    let mut scoreratiow = [0.0f32; 7];
    for tone in 0..7 {
        if sumkw[tone] > 0.0 {
            scoreratiow[tone] = syncw[tone] / sumkw[tone];
        }
        if syncw[tone] > sumkw[tone] {
            nsyncscorew += 1;
        }
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
    if nsyncscore2 > 0 {
        scoreratio2 /= nsyncscore2 as f32;
    } else {
        scoreratio2 = 0.0;
    }
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
            return scoreratio2 > 15.0;
        }
        if nsyncscorew == 4 {
            return nsyncscore2 == 7 || nsyncscore3 == 7 || scoreratio2 > 10.0;
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
            return nsyncscore1 == 7 || nsyncscore2 == 7 || scoreratio1 > 10.0;
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

fn maxloc_1based(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(idx, _)| idx + 1)
        .unwrap_or(0)
}

fn regular_decode(
    metrics: &SymbolMetrics,
    _candidate: SyncCandidate,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    sync_gate: SyncGate,
    signal_memory: &mut SignalMemory,
) -> Option<Ft8bDecodeResult> {
    let tone_hints = ToneHints::from_config(config);
    let classifier = classify_signal(metrics, config, refined_freq, context, &tone_hints);
    remember_candidate_signal(signal_memory, metrics, classifier, refined_freq, refined_dt);
    let csold = select_csold(signal_memory, classifier, context, refined_freq, refined_dt);
    let nsubpasses = nsubpasses_with_csold(classifier, csold.is_some());
    let primary_metrics = build_bit_metrics(metrics, MetricSource::Cs);

    let apmask = [0i8; N];
    for isubp1 in 1..=nsubpasses {
        if classifier.nweak == 1 && isubp1 == 2 {
            continue;
        }
        if isubp1 > 2 && isubp1 < 6 && classifier.lmycsignal {
            continue;
        }
        let bit_metrics = if isubp1 == 1 {
            primary_metrics.clone()
        } else if isubp1 == 2 {
            build_bit_metrics(metrics, MetricSource::Csr)
        } else if matches!(isubp1, 3 | 6 | 9) {
            build_bit_metrics(metrics, MetricSource::CscsCsrPower)
        } else if matches!(isubp1, 4 | 7 | 10) {
            let Some(csold) = csold.as_ref() else {
                continue;
            };
            build_bit_metrics_with_csold(metrics, MetricSource::CsCsoldPower, csold)
        } else if matches!(isubp1, 5 | 8 | 11) {
            let Some(csold) = csold.as_ref() else {
                continue;
            };
            build_bit_metrics_with_csold(metrics, MetricSource::CsCsoldSum, csold)
        } else {
            continue;
        };

        for isubp2 in 1..=4 {
            if sync_gate.lapcqonly || sync_gate.lskipnotap {
                continue;
            }
            if metrics.syncavemax < 1.8 {
                continue;
            }
            if !config.swl && isubp2 == 4 {
                continue;
            }
            if isubp1 > 2 {
                continue;
            }
            let llr_source = regular_llr_source(config, context, isubp1, isubp2, &bit_metrics);
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

        if config.lft8apon {
            let apmag = bit_metrics
                .bmeta
                .iter()
                .map(|value| (2.83 * value).abs())
                .fold(0.0f32, f32::max)
                * 1.01;
            for (isubp2, iaptype) in plan_ap_subpasses(config) {
                if !jtdx_ap_subpass_allowed(
                    config,
                    context,
                    classifier,
                    refined_freq,
                    isubp1,
                    sync_gate,
                    isubp2,
                    iaptype,
                ) {
                    continue;
                }
                let Some(ap) = build_ap_mask(config, iaptype) else {
                    continue;
                };
                let llr_source = ap_llr_source(
                    isubp2,
                    &bit_metrics.bmeta,
                    &bit_metrics.bmetb,
                    &bit_metrics.bmetc,
                );
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
                    .or_else(|| {
                        osd174_91(
                            &llrz,
                            &ap.apmask,
                            ap_ndeep(config, context, classifier, refined_freq, iaptype),
                        )
                    })
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
        }
    }

    if let Some(result) = try_ft8s(
        metrics,
        refined_freq,
        refined_dt,
        config,
        book,
        context,
        classifier,
    ) {
        return Some(result);
    }

    if let Some(result) = try_ft8sd(
        metrics,
        refined_freq,
        refined_dt,
        config,
        book,
        context,
        middle_sync_ratio(&metrics.s8),
    ) {
        return Some(result);
    }

    None
}

fn try_ft8s(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    classifier: SignalClassifier,
) -> Option<Ft8bDecodeResult> {
    if context.lqsomsgdcd || context.stophint || context.lft8sdec {
        return None;
    }
    if (refined_freq - config.nfqso).abs() >= 2.0 {
        return None;
    }
    let mycall = normalized_config_call(config.mycall.as_deref())?;
    let hiscall = normalized_config_call(config.hiscall.as_deref())?;
    let srr = if classifier.lqsosig || classifier.lmycsignal {
        0.0
    } else {
        middle_sync_ratio(&metrics.s8)
    };
    try_ft8s_with_s8(
        &metrics.s8,
        metrics,
        refined_freq,
        refined_dt,
        config,
        book,
        context,
        &mycall,
        &hiscall,
        srr,
    )
}

fn try_ft8s_virtual(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    lvirtual: bool,
) -> Option<Ft8bDecodeResult> {
    if context.lqsomsgdcd || context.lft8sdec {
        return None;
    }
    if jtdx_both_config_calls_nonstandard(config) {
        return None;
    }
    if (refined_freq - config.nfqso).abs() >= 2.0 {
        return None;
    }
    let mycall = normalized_config_call(config.mycall.as_deref())?;
    let hiscall = normalized_config_call(config.hiscall.as_deref())?;
    let s8 = sqrt_s8(&metrics.s8);
    let srr = if lvirtual {
        0.0
    } else {
        middle_sync_ratio(&metrics.s8)
    };
    try_ft8s_with_s8(
        &s8,
        metrics,
        refined_freq,
        refined_dt,
        config,
        book,
        context,
        &mycall,
        &hiscall,
        srr,
    )
}

#[allow(clippy::too_many_arguments)]
fn try_ft8s_with_s8(
    s8: &[[f32; 79]; 8],
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    mycall: &str,
    hiscall: &str,
    srr: f32,
) -> Option<Ft8bDecodeResult> {
    let result = ft8s(
        s8,
        srr,
        3,
        context.stophint,
        &mycall,
        &hiscall,
        context.nlasttx,
        context.last_rx_msg.as_ref().map(LastRxMsgText::as_str),
    )?;
    decoded_bits_to_result(
        metrics,
        refined_freq,
        refined_dt,
        result.msg37,
        result.msgbits,
        result.itone,
        config,
        book,
        DecodeSource::Ft8s,
    )
}

fn sqrt_s8(s8: &[[f32; 79]; 8]) -> [[f32; 79]; 8] {
    let mut out = [[0.0f32; 79]; 8];
    for tone in 0..8 {
        for sym in 0..79 {
            out[tone][sym] = s8[tone][sym].sqrt();
        }
    }
    out
}

fn jtdx_both_config_calls_nonstandard(config: &StreamDecodeConfig) -> bool {
    let lmycallstd = normalized_config_call(config.mycall.as_deref())
        .is_some_and(|call| !is_nonstandard_call(&call));
    let lhiscallstd = normalized_config_call(config.hiscall.as_deref())
        .is_some_and(|call| !is_nonstandard_call(&call));
    !lmycallstd && !lhiscallstd
}

fn try_ft8sd(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    srr: f32,
) -> Option<Ft8bDecodeResult> {
    let msgd = context.sd_msg.as_ref()?.as_str();
    let mycall = normalized_config_call(config.mycall.as_deref()).unwrap_or_default();
    let result = ft8sd1(&metrics.s8, msgd, context.sd_lcq, &mycall)
        .map(|result| (result.msg37, result.msgbits, result.itone))
        .or_else(|| {
            ft8sd(&metrics.s8, srr, msgd, context.sd_lcq, &mycall)
                .map(|result| (result.msg37, result.msgbits, result.itone))
        })
        .or_else(|| {
            if context.sd_lcq {
                ft8mfcq(&metrics.s8, msgd)
                    .map(|result| (result.msg37, result.msgbits, result.itone))
            } else {
                ft8mf1(&metrics.s8, msgd).map(|result| (result.msg37, result.msgbits, result.itone))
            }
        })?;
    decoded_bits_to_result(
        metrics,
        refined_freq,
        refined_dt,
        result.0,
        result.1,
        result.2,
        config,
        book,
        DecodeSource::Ft8sd,
    )
}

fn middle_sync_ratio(s8: &[[f32; 79]; 8]) -> f32 {
    let mut synclev = 0.0;
    for k in 0..7 {
        synclev += s8[ICOS7[k] as usize][k + 36];
    }
    let mut snoiselev = 0.0;
    for k in 36..43 {
        snoiselev += sum_tones(s8, k);
    }
    snoiselev = (snoiselev - synclev) / 7.0;
    if snoiselev < 0.1 {
        snoiselev = 1.0;
    }
    synclev / snoiselev
}

fn nsubpasses_with_csold(classifier: SignalClassifier, has_csold: bool) -> usize {
    if !has_csold {
        return classifier.nsubpasses;
    }
    if classifier.lqsocandave {
        11
    } else if classifier.lmycsignal {
        8
    } else if classifier.lcqsignal {
        5
    } else {
        classifier.nsubpasses
    }
}

fn jtdx_ap_subpass_allowed(
    config: &StreamDecodeConfig,
    context: Ft8bCandidateContext,
    classifier: SignalClassifier,
    refined_freq: f64,
    isubp1: usize,
    sync_gate: SyncGate,
    isubp2: usize,
    iaptype: i32,
) -> bool {
    let lapmyc = normalized_config_call(config.mycall.as_deref()).is_some();
    let lnomycall = !lapmyc;
    let lnohiscall = normalized_config_call(config.hiscall.as_deref()).is_none();
    let lmycallstd = lapmyc && !is_nonstandard_call(config.mycall.as_deref().unwrap_or(""));
    let lhiscallstd = !lnohiscall && !is_nonstandard_call(config.hiscall.as_deref().unwrap_or(""));
    let loutapwid = (refined_freq - config.nfqso).abs() > config.napwid
        && (refined_freq - config.nftx).abs() > config.napwid;
    let lapcqonly = config.lapcqonly || sync_gate.lapcqonly;

    if !jtdx_ap_signal_pruning_allowed(config, classifier, isubp2, iaptype) {
        return false;
    }

    if classifier.lqsocandave {
        if isubp1 > 2 && isubp1 < 9 {
            return false;
        }
        if context.lqsomsgdcd {
            return false;
        }
        if isubp1 > 8 && !is_qso_candidate_ap_type(iaptype) {
            return false;
        }
    } else if classifier.lmycsignal && lmycallstd {
        if isubp1 > 2 && isubp1 < 6 {
            return false;
        }
        if isubp1 > 5 && isubp1 < 9 && iaptype != 2 {
            return false;
        }
    }

    if config.lhound {
        if lnomycall && iaptype > 1 && iaptype < 31 {
            return false;
        }
        if lhiscallstd && iaptype == 31 && !classifier.lcqsignal {
            return false;
        }
        if context.lqsomsgdcd && iaptype > 0 && iaptype < 25 {
            return false;
        }
        if !context.stophint && (iaptype == 31 || iaptype == 36) {
            return false;
        }
        if config.nQSOProgress == 1 {
            if classifier.lfoxspecrpt {
                if iaptype == 21 {
                    return false;
                }
                if matches!(iaptype, 31 | 36) && classifier.nfoxspecrpt > 3 {
                    return false;
                }
            } else {
                if iaptype == 22 {
                    return false;
                }
                if matches!(iaptype, 31 | 36) && classifier.nmic > 3 {
                    return false;
                }
            }
        }
        if config.nQSOProgress == 3 {
            if classifier.lfoxspecrpt {
                if iaptype == 21 {
                    return false;
                }
            } else if iaptype == 22 {
                return false;
            }
            if classifier.lfoxstdr73 {
                if iaptype == 24 {
                    return false;
                }
            } else if iaptype == 23 {
                return false;
            }
        }
        if !lapmyc && matches!(iaptype, 23 | 24) {
            return false;
        }
        let fdelta = (refined_freq - config.nfqso).abs();
        let fdeltam = fdelta.rem_euclid(60.0);
        if config.nQSOProgress > 0 && iaptype < 31 && (fdelta > 245.0 || fdeltam > 3.0) {
            return false;
        }
        if matches!(iaptype, 31 | 36) && (fdelta > 245.0 || fdeltam > 3.0) {
            return false;
        }
        if lapcqonly && matches!(iaptype, 31 | 36 | 111) {
            return false;
        }
        return true;
    }

    if lmycallstd && (lhiscallstd || lnohiscall) {
        if context.lqsomsgdcd && iaptype > 2 && iaptype < 31 {
            return false;
        }
        if context.lft8sdec && iaptype > 2 {
            return false;
        }
        if iaptype == 2 {
            if !lapmyc || lapcqonly {
                return false;
            }
            if config.nQSOProgress != 0 && classifier.nmic < 2 {
                return false;
            }
        }
        if !context.stophint && iaptype > 30 {
            return false;
        }
        if context.stophint && iaptype > 2 && iaptype < 31 {
            return false;
        }
        if iaptype > 2 && lnohiscall {
            return false;
        }
        if iaptype > 2 && iaptype < 31 && loutapwid {
            return false;
        }
        if iaptype == 3 && !classifier.lqsosigtype3 {
            return false;
        }
        if iaptype == 4 && !classifier.lqsorrr {
            return false;
        }
        if iaptype == 5 && !classifier.lqso73 {
            return false;
        }
        if iaptype == 6 && !classifier.lqsorr73 {
            return false;
        }
        if iaptype == 31 && !classifier.lcqdxcsig {
            return false;
        }
        if iaptype == 31 && !lhiscallstd && lapcqonly {
            return false;
        }
        if iaptype > 31 && lapcqonly {
            return false;
        }
        if iaptype == 35 && !classifier.lqso73 {
            return false;
        }
        if iaptype == 36 && !classifier.lqsorr73 {
            return false;
        }
        return true;
    }

    if lmycallstd && !lhiscallstd && !lnohiscall {
        if iaptype == 2 && lapcqonly {
            return false;
        }
        if !context.stophint && iaptype > 30 {
            return false;
        }
        if (context.lqsomsgdcd || !lapmyc) && iaptype > 1 && iaptype < 15 {
            return false;
        }
        if iaptype == 12 && !classifier.lqsorrr {
            return false;
        }
        if iaptype == 13 && !classifier.lqso73 {
            return false;
        }
        if iaptype == 14 && !classifier.lqsorr73 {
            return false;
        }
        if iaptype > 30 && lapcqonly {
            return false;
        }
        if iaptype > 2 && iaptype < 15 && loutapwid {
            return false;
        }
        return true;
    }

    if !lmycallstd && !lhiscallstd && !lnohiscall {
        if iaptype > 1 && iaptype < 31 {
            return false;
        }
        if !context.stophint && iaptype > 1 {
            return false;
        }
        if iaptype > 30 && lapcqonly {
            return false;
        }
        if iaptype == 31 && !classifier.lcqdxcnssig {
            return false;
        }
        if iaptype > 34 && !classifier.ldxcsig {
            return false;
        }
        return true;
    }

    if !lmycallstd && (lhiscallstd || lnohiscall) {
        if isubp1 == 2 && classifier.nweak == 1 {
            return false;
        }
        if isubp1 > 5 {
            return false;
        }
        if iaptype == 40 && lapcqonly {
            return false;
        }
        if iaptype > 40 && iaptype < 45 && context.lqsomsgdcd {
            return false;
        }
        if iaptype == 42 && !classifier.lqsorrr {
            return false;
        }
        if iaptype == 43 && !classifier.lqso73 {
            return false;
        }
        if iaptype == 44 && !classifier.lqsorr73 {
            return false;
        }
        if iaptype > 39 && !lapmyc {
            return false;
        }
        if lnomycall && iaptype > 39 && iaptype < 45 {
            return false;
        }
        if lnohiscall && iaptype != 1 && iaptype != 40 {
            return false;
        }
        if iaptype > 30 && iaptype < 40 && !context.stophint {
            return false;
        }
        if iaptype == 31 && !classifier.lcqdxcsig {
            return false;
        }
        if iaptype > 34 && iaptype < 37 && (!classifier.ldxcsig || lapcqonly) {
            return false;
        }
        if iaptype > 30 && iaptype < 40 && loutapwid {
            return false;
        }
        return true;
    }

    false
}

fn is_qso_candidate_ap_type(iaptype: i32) -> bool {
    matches!(iaptype, 3..=6 | 11..=14 | 21 | 23 | 24 | 41..=44)
}

fn jtdx_ap_signal_pruning_allowed(
    config: &StreamDecodeConfig,
    classifier: SignalClassifier,
    isubp2: usize,
    iaptype: i32,
) -> bool {
    if config.swl {
        return true;
    }
    match iaptype {
        1 => {
            if isubp2 == 20 && classifier.scqnr < 1.0 && !classifier.lcqsignal {
                return false;
            }
            if isubp2 == 21 {
                if config.lft8lowth {
                    return classifier.scqnr >= 1.2 || classifier.lcqsignal;
                }
                return classifier.scqnr >= 1.3 || classifier.lcqsignal;
            }
            true
        }
        2 => {
            if isubp2 == 17 && classifier.smycnr < 1.0 && !classifier.lmycsignal {
                return false;
            }
            if isubp2 == 18 && config.lft8lowth {
                return classifier.smycnr >= 1.2 || classifier.lmycsignal;
            }
            true
        }
        3 => {
            if isubp2 == 5 {
                return classifier.smycnr >= 1.0;
            }
            if isubp2 == 6 {
                return classifier.smycnr >= 1.2;
            }
            true
        }
        _ => true,
    }
}

impl ToneHints {
    fn from_config(config: &StreamDecodeConfig) -> Self {
        let mut hints = Self::default();
        hints.idtone25_2 = tones58_from_sd_message("CQ 2E0DLA IO92");
        let Some(mycall) = normalized_config_call(config.mycall.as_deref()) else {
            return hints;
        };

        hints.idtonemyc = tones58_from_message(&format!("{mycall} AA1AAA FN25"));
        if let Some(hiscall) = normalized_config_call(config.hiscall.as_deref()) {
            if config.lhound {
                if let (Some(mybcall), Some(hisbcall)) =
                    (base_config_call(&mycall), base_config_call(&hiscall))
                {
                    hints.idtonefox73 = tones58_from_message(&format!("{mybcall} {hisbcall} RR73"));
                    hints.idtonespec =
                        tones58_from_message(&format!("{mybcall} RR73; {mybcall} <{hiscall}> -12"));
                }
            }
            hints.idtone56 = build_idtone56(&mycall, &hiscall);
            if is_nonstandard_call(config.hiscall.as_deref().unwrap_or("")) {
                let hiscall_raw = config
                    .hiscall
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_uppercase();
                hints.idtonecqdxcns = tones58_from_message(&format!("CQ {hiscall_raw}"));
                hints.idtonedxcns73 = tones58_from_message(&format!("<AA1AAA> {hiscall_raw} 73"));
            }
        }
        hints
    }
}

fn remember_candidate_signal(
    signal_memory: &mut SignalMemory,
    metrics: &SymbolMetrics,
    classifier: SignalClassifier,
    refined_freq: f64,
    refined_dt: f64,
) {
    let cs = CsMatrix {
        re: metrics.cs_re,
        im: metrics.cs_im,
    };
    if classifier.lcqsignal {
        signal_memory.remember_tmp(SignalKind::Cq, refined_freq, refined_dt, cs.clone());
    }
    if classifier.lmycsignal {
        signal_memory.remember_tmp(SignalKind::MyCall, refined_freq, refined_dt, cs.clone());
    }
    if classifier.lqsocandave {
        signal_memory.remember_tmp(SignalKind::Qso, refined_freq, refined_dt, cs);
    }
}

fn select_csold(
    signal_memory: &SignalMemory,
    classifier: SignalClassifier,
    context: Ft8bCandidateContext,
    refined_freq: f64,
    refined_dt: f64,
) -> Option<CsMatrix> {
    if classifier.lqsocandave {
        return signal_memory.find_old(SignalKind::Qso, context, refined_freq, refined_dt);
    }
    if classifier.lmycsignal {
        return signal_memory.find_old(SignalKind::MyCall, context, refined_freq, refined_dt);
    }
    if classifier.lcqsignal {
        return signal_memory.find_old(SignalKind::Cq, context, refined_freq, refined_dt);
    }
    None
}

fn classify_signal(
    metrics: &SymbolMetrics,
    config: &StreamDecodeConfig,
    refined_freq: f64,
    context: Ft8bCandidateContext,
    hints: &ToneHints,
) -> SignalClassifier {
    let lapmyc = normalized_config_call(config.mycall.as_deref()).is_some();
    let mut nmic = 0usize;
    if let Some(idtonemyc) = &hints.idtonemyc {
        for k11 in 8..=16 {
            let sym = k11 - 1;
            if max_tone(&metrics.s8, sym, None) as i32 == idtonemyc[k11 - 8] {
                nmic += 1;
            }
        }
    }
    let mut rscq = 0.0f32;
    for k11 in 8..=16 {
        let sym = k11 - 1;
        let best = max_tone(&metrics.s8, sym, None);
        if k11 < 16 {
            if best == 0 {
                rscq += 1.0;
            }
        } else if best == 1 {
            rscq += 1.0;
        }
    }
    for (sym, tones) in [(16usize, [0usize, 1usize]), (26, [0, 1]), (32, [2, 3])] {
        let best = max_tone(&metrics.s8, sym, None);
        if tones.contains(&best) {
            rscq += 0.5;
        }
    }

    let s256_peak = maxloc_1based(&metrics.s256[..=8]);
    let mut lcqsignal = s256_peak == 5 || rscq > 3.1;
    if (!lcqsignal && s256_peak == 4) || s256_peak == 6 {
        let s2563_peak = maxloc_1based(&metrics.s256);
        if s2563_peak == 4 || s2563_peak == 6 {
            lcqsignal = true;
        }
    }
    let lmycsignal = lapmyc && nmic > 2;
    let dfqso = (config.nfqso - refined_freq).abs();
    let mut lqsosig = false;
    let mut lqsosigtype3 = false;
    let mut lqso73 = false;
    let mut lqsorr73 = false;
    let mut lqsorrr = false;
    let mut ndxt = 0usize;
    if !context.lqsomsgdcd
        && (dfqso < config.napwid || (config.nftx - refined_freq).abs() < config.napwid)
        && lapmyc
        && normalized_config_call(config.hiscall.as_deref()).is_some()
        && !hints.idtone56.is_empty()
    {
        let qso_tones = &hints.idtone56[0];
        let mut nqsot = 0usize;
        for i in 1..=19 {
            if max_tone(&metrics.s8, i + 6, None) as i32 == qso_tones[i - 1] {
                nqsot += 1;
            }
        }
        lqsosig = nqsot > 6;
        for i in 20..=22 {
            if max_tone(&metrics.s8, i + 6, None) as i32 == qso_tones[i - 1] {
                nqsot += 1;
            }
        }
        lqsosigtype3 = nqsot > 3;

        for k11 in 17..=26 {
            if max_tone(&metrics.s8, k11 - 1, None) as i32 == qso_tones[k11 - 8] {
                ndxt += 1;
            }
        }

        if dfqso < config.napwid
            && matches!(config.nQSOProgress, 3 | 4)
            && hints.idtone56.len() >= 56
        {
            let mut nqsoend = [0usize; 3];
            for i in 24..=58 {
                let sym = if i < 30 { i + 6 } else { i + 13 };
                let best = max_tone(&metrics.s8, sym, None) as i32;
                if best == hints.idtone56[55][i - 1] {
                    nqsoend[0] += 1;
                }
                if best == hints.idtone56[54][i - 1] {
                    nqsoend[1] += 1;
                }
                if best == hints.idtone56[53][i - 1] {
                    nqsoend[2] += 1;
                }
            }
            if let Some((idx, count)) = nqsoend
                .iter()
                .copied()
                .enumerate()
                .max_by_key(|&(_, count)| count)
            {
                if count > 6 {
                    match idx {
                        0 => lqso73 = true,
                        1 => lqsorr73 = true,
                        _ => lqsorrr = true,
                    }
                }
            }
        }
    }

    let hiscall_is_nonstandard = is_nonstandard_call(config.hiscall.as_deref().unwrap_or(""));
    let mut ldxcsig = !hiscall_is_nonstandard && ndxt > 3;
    let lcqdxcsig = lcqsignal && ldxcsig;
    let mut lcqdxcnssig = false;
    if hiscall_is_nonstandard && normalized_config_call(config.hiscall.as_deref()).is_some() {
        let mut ncqdxcnst = 0usize;
        if let Some(idtonecqdxcns) = &hints.idtonecqdxcns {
            for i in 1..=4 {
                if max_tone(&metrics.s8, i + 6, None) as i32 == idtonecqdxcns[i - 1] {
                    ncqdxcnst += 1;
                }
            }
            let mut ndxt_ns = 0usize;
            for i in 5..=23 {
                let best = max_tone(&metrics.s8, i + 6, None) as i32;
                if let Some(idtonedxcns73) = &hints.idtonedxcns73 {
                    if best == idtonedxcns73[i - 1] {
                        ndxt_ns += 1;
                    }
                }
                if best == idtonecqdxcns[i - 1] {
                    ncqdxcnst += 1;
                }
            }
            ldxcsig = if dfqso < config.napwid {
                ndxt_ns > 4
            } else {
                ndxt_ns > 5
            };
            lcqdxcnssig = if dfqso < config.napwid {
                ncqdxcnst > 5
            } else {
                ncqdxcnst > 6
            };
        }
    }

    let lsubptxfreq = lapmyc
        && (refined_freq - config.nftx).abs() < 2.0
        && !config.lhound
        && !context.lqsomsgdcd
        && (context.nlasttx == 1 || context.nlasttx == 2);
    let nweak = if config.swl || dfqso < 2.0 || lsubptxfreq {
        2
    } else {
        1
    };
    let mut nsubpasses = nweak;
    if lcqsignal {
        nsubpasses = 3;
    }
    if lmycsignal && !is_nonstandard_call(config.mycall.as_deref().unwrap_or("")) {
        nsubpasses = 6;
    }
    let lqsocandave = lapmyc
        && ndxt > 2
        && nmic > 2
        && !context.lqsomsgdcd
        && !is_nonstandard_call(config.mycall.as_deref().unwrap_or(""))
        && !is_nonstandard_call(config.hiscall.as_deref().unwrap_or(""))
        && dfqso < config.napwid / 2.0;
    if lqsocandave {
        nsubpasses = 9;
    }
    let scqnr = hints
        .idtone25_2
        .as_ref()
        .map(|tones| first_nine_tone_snr(&metrics.s8, tones))
        .unwrap_or(2.0);
    let smycnr = hints
        .idtonemyc
        .as_ref()
        .map(|tones| first_nine_tone_snr(&metrics.s8, tones))
        .unwrap_or(2.0);
    let hound = classify_hound_signal(metrics, config, refined_freq, hints);

    SignalClassifier {
        lcqsignal,
        lmycsignal,
        lqsosig,
        lqsosigtype3,
        lqsocandave,
        lqso73,
        lqsorr73,
        lqsorrr,
        ldxcsig,
        lcqdxcsig,
        lcqdxcnssig,
        nmic,
        nweak,
        nsubpasses,
        scqnr,
        smycnr,
        lfoxspecrpt: hound.lfoxspecrpt,
        lfoxstdr73: hound.lfoxstdr73,
        nfoxspecrpt: hound.nfoxspecrpt,
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct HoundSignalClassifier {
    lfoxspecrpt: bool,
    lfoxstdr73: bool,
    nfoxspecrpt: usize,
}

fn classify_hound_signal(
    metrics: &SymbolMetrics,
    config: &StreamDecodeConfig,
    refined_freq: f64,
    hints: &ToneHints,
) -> HoundSignalClassifier {
    if !config.lhound || !matches!(config.nQSOProgress, 1 | 3) {
        return HoundSignalClassifier::default();
    }
    let Some(idtonefox73) = &hints.idtonefox73 else {
        return HoundSignalClassifier::default();
    };
    let Some(idtonespec) = &hints.idtonespec else {
        return HoundSignalClassifier::default();
    };

    let fdelta = (refined_freq - config.nfqso).abs();
    let fdeltam = fdelta.rem_euclid(60.0);
    if fdelta >= 245.0 || fdeltam >= 3.0 {
        return HoundSignalClassifier::default();
    }

    let mut nfoxstdbase = 0usize;
    let mut nfoxspecrpt = 0usize;
    let mut nfoxspecr73 = 0usize;
    for i in 1..=18 {
        let best = max_tone(&metrics.s8, i + 6, None) as i32;
        if best == idtonefox73[i - 1] {
            nfoxstdbase += 1;
        }
        if i > 10 && best == idtonespec[i - 1] {
            nfoxspecrpt += 1;
        }
    }
    for i in 20..=22 {
        if max_tone(&metrics.s8, i + 6, None) as i32 == idtonespec[i - 1] {
            nfoxspecrpt += 1;
            nfoxspecr73 += 1;
        }
    }
    if max_tone(&metrics.s8, 31, None) as i32 == idtonespec[24] {
        nfoxspecrpt += 1;
        nfoxspecr73 += 1;
    }

    let rspecstdrpt = if nfoxstdbase == 0 {
        nfoxspecrpt as f32 * 18.0 / 1.2
    } else {
        nfoxspecrpt as f32 * 18.0 / (12.0 * nfoxstdbase as f32)
    };
    let lfoxspecrpt = rspecstdrpt > 1.0;

    let mut lfoxstdr73 = false;
    if config.nQSOProgress == 3 {
        let mut nfoxr73 = 0usize;
        for i in 24..=58 {
            let sym = if i < 30 { i + 6 } else { i + 13 };
            if max_tone(&metrics.s8, sym, None) as i32 == idtonefox73[i - 1] {
                nfoxr73 += 1;
            }
        }
        let rstdr73 = if nfoxspecr73 == 0 {
            nfoxr73 as f32 * 4.0 / 3.5
        } else {
            nfoxr73 as f32 * 4.0 / (35.0 * nfoxspecr73 as f32)
        };
        lfoxstdr73 = rstdr73 > 1.0;
    }

    HoundSignalClassifier {
        lfoxspecrpt,
        lfoxstdr73,
        nfoxspecrpt,
    }
}

fn first_nine_tone_snr(s8: &[[f32; 79]; 8], tones: &[i32; 58]) -> f32 {
    let mut signal = 0.0f32;
    for i in 0..9 {
        let tone = tones[i].clamp(0, 7) as usize;
        signal += s8[tone][i + 7];
    }
    let mut total = 0.0f32;
    for tone_values in s8.iter() {
        total += tone_values[7..16].iter().sum::<f32>();
    }
    let noise = (total - signal) / 7.0;
    if noise > 0.0 {
        signal / noise
    } else {
        2.0
    }
}
