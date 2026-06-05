use super::super::ft8_mod1::ICOS7;
use super::super::ft8apset::Ft8ApSet;
use super::super::ft8mf1::ft8mf1;
use super::super::ft8mfcq::ft8mfcq;
use super::super::ft8s::ft8s;
use super::super::ft8sd::ft8sd;
use super::super::ft8sd1::ft8sd1;
use super::super::ft8v2::bpdecode174_91::{bpdecode174_91, N};
use super::super::ft8v2::osd174_91::osd174_91;
use super::super::ft8v2::packjt77::HashCallBook;
use super::super::sync8::SyncCandidate;
use super::super::tone8::Tone8Tables;
use super::classify::{classify_signal, remember_failed_candidate_signal, select_csold};
use super::decode_helpers::*;
use super::state::{
    DecodeSource, Ft8bCandidateContext, Ft8bDecodeResult, LastRxMsgText, MetricSource,
    SignalClassifier, SignalMemory, SymbolMetrics, SyncGate, ToneHints,
};
use super::sum_tones;
use crate::stream::session::StreamDecodeConfig;

pub(super) fn regular_decode(
    metrics: &SymbolMetrics,
    _candidate: SyncCandidate,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    tone8_tables: &Tone8Tables,
    ft8apset: &Ft8ApSet,
    context: Ft8bCandidateContext,
    sync_gate: SyncGate,
    signal_memory: &mut SignalMemory,
) -> Option<Ft8bDecodeResult> {
    let tone_hints = ToneHints::from_tables(tone8_tables);
    let classifier = classify_signal(metrics, config, refined_freq, context, &tone_hints);
    let csold = select_csold(signal_memory, classifier, context, refined_freq, refined_dt);
    let nsubpasses = nsubpasses_with_csold(classifier, csold.is_some());
    let apmask = [0i8; N];
    for isubp1 in 1..=nsubpasses {
        // JTDX ft8b.f90 computes syncavemax earlier, then resets it to 3.0
        // inside the regular/AP subpass loop before the isubp2 gates.
        let syncavemax = 3.0f32;
        if classifier.nweak == 1 && isubp1 == 2 {
            continue;
        }
        if isubp1 > 2 && isubp1 < 6 && classifier.lmycsignal {
            continue;
        }
        if classifier.lqsocandave {
            if isubp1 > 2 && isubp1 < 9 {
                continue;
            }
            if context.lqsomsgdcd {
                continue;
            }
        }
        let bit_metrics = if isubp1 == 1 {
            build_bit_metrics(metrics, MetricSource::Cs)
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
            if syncavemax < 1.8 {
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
            let decoded =
                bpdecode174_91(&llrz, &apmask, 30).or_else(|| osd174_91(&llrz, &apmask, 3));
            if let Some(decoded) = decoded {
                if let Some(result) = decoded_to_result(
                    metrics,
                    refined_freq,
                    refined_dt,
                    decoded,
                    config,
                    book,
                    0,
                    isubp2,
                ) {
                    return Some(result);
                }
            }
            if let Some(result) = try_ft8sd_regular_failure(
                metrics,
                refined_freq,
                refined_dt,
                config,
                book,
                context,
                isubp2,
                middle_sync_ratio(&metrics.s8),
            ) {
                return Some(result);
            }
        }

        if config.lft8apon {
            let mut apmag = 0.0f32;
            for value in &bit_metrics.bmeta {
                let llra_abs = (2.83 * *value).abs();
                if llra_abs > apmag {
                    apmag = llra_abs;
                }
            }
            apmag *= 1.01;
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
                let Some(ap) = ft8apset.get(iaptype) else {
                    continue;
                };
                let llr_source = ap_llr_source(
                    isubp2,
                    iaptype,
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
                        llrz[i] = apmag * ap.apsym[i] as f32;
                    }
                }
                let decoded = bpdecode174_91(&llrz, &ap.apmask, 30).or_else(|| {
                    osd174_91(
                        &llrz,
                        &ap.apmask,
                        ap_ndeep(config, context, classifier, refined_freq, iaptype),
                    )
                });
                let Some(decoded) = decoded else {
                    if let Some(result) = try_ft8sd_regular_failure(
                        metrics,
                        refined_freq,
                        refined_dt,
                        config,
                        book,
                        context,
                        isubp2,
                        middle_sync_ratio(&metrics.s8),
                    ) {
                        return Some(result);
                    }
                    remember_failed_candidate_signal(
                        signal_memory,
                        metrics,
                        classifier,
                        refined_freq,
                        refined_dt,
                        context,
                        isubp1,
                        isubp2,
                        iaptype,
                    );
                    continue;
                };
                if decoded_all_zero(&decoded) {
                    if let Some(result) = try_ft8sd_regular_failure(
                        metrics,
                        refined_freq,
                        refined_dt,
                        config,
                        book,
                        context,
                        isubp2,
                        middle_sync_ratio(&metrics.s8),
                    ) {
                        return Some(result);
                    }
                    continue;
                }
                if decoded_quality_rejected(&decoded, isubp2) {
                    if let Some(result) = try_ft8sd_regular_failure(
                        metrics,
                        refined_freq,
                        refined_dt,
                        config,
                        book,
                        context,
                        isubp2,
                        middle_sync_ratio(&metrics.s8),
                    ) {
                        return Some(result);
                    }
                    remember_failed_candidate_signal(
                        signal_memory,
                        metrics,
                        classifier,
                        refined_freq,
                        refined_dt,
                        context,
                        isubp1,
                        isubp2,
                        iaptype,
                    );
                    continue;
                }
                if let Some(result) = decoded_to_result(
                    metrics,
                    refined_freq,
                    refined_dt,
                    decoded,
                    config,
                    book,
                    iaptype,
                    isubp2,
                ) {
                    return Some(result);
                }
                if let Some(result) = try_ft8sd_regular_failure(
                    metrics,
                    refined_freq,
                    refined_dt,
                    config,
                    book,
                    context,
                    isubp2,
                    middle_sync_ratio(&metrics.s8),
                ) {
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
        tone8_tables,
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
    tone8_tables: &Tone8Tables,
) -> Option<Ft8bDecodeResult> {
    if context.lqsomsgdcd || context.stophint || context.lft8sdec {
        return None;
    }
    if (refined_freq - config.nfqso).abs() >= 2.0 {
        return None;
    }
    let mycall = normalized_config_call(config.mycall.as_deref())?;
    let hiscall = normalized_config_call(config.hiscall.as_deref())?;
    let srr = middle_sync_ratio(&metrics.s8);
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
        tone8_tables,
    )
}

pub(super) fn try_ft8s_virtual(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    lvirtual: bool,
    tone8_tables: &Tone8Tables,
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
        tone8_tables,
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
    tone8_tables: &Tone8Tables,
) -> Option<Ft8bDecodeResult> {
    let result = ft8s(
        s8,
        srr,
        config.nft8rxfsens,
        context.stophint,
        &mycall,
        &hiscall,
        context.nlasttx,
        context.last_rx_msg.as_ref().map(LastRxMsgText::as_str),
        Some(tone8_tables),
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

pub(super) fn try_ft8sd_iqso4(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    ldeepsync: bool,
) -> Option<Ft8bDecodeResult> {
    let msgd = context.sd_msg.as_ref()?.as_str();
    let mycall = normalized_config_call(config.mycall.as_deref()).unwrap_or_default();
    let result = if ldeepsync {
        ft8sd1(&metrics.s8, msgd, context.sd_lcq, &mycall)
            .map(|result| (result.msg37, result.msgbits, result.itone))
    } else {
        None
    }
    .or_else(|| {
        if context.sd_lcq {
            ft8mfcq(&metrics.s8, msgd).map(|result| (result.msg37, result.msgbits, result.itone))
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

fn try_ft8sd_regular_failure(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    context: Ft8bCandidateContext,
    isubp2: usize,
    srr: f32,
) -> Option<Ft8bDecodeResult> {
    if isubp2 != 3 {
        return None;
    }
    if srr >= 7.0 {
        return None;
    }
    let msgd = context.sd_msg.as_ref()?.as_str();
    let mycall = normalized_config_call(config.mycall.as_deref()).unwrap_or_default();
    let result = ft8sd(&metrics.s8, srr, msgd, context.sd_lcq, &mycall)
        .map(|result| (result.msg37, result.msgbits, result.itone))?;
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

pub(super) fn middle_sync_ratio(s8: &[[f32; 79]; 8]) -> f32 {
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
    } else if classifier.lmycsignal && classifier.nsubpasses >= 6 {
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
    } else if classifier.lmycsignal && lmycallstd {
        if isubp1 > 2 && isubp1 < 6 {
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
        if matches!(iaptype, 31 | 36) && !config.lwidedxcsearch && (fdelta > 245.0 || fdeltam > 3.0)
        {
            return false;
        }
        if iaptype == 31 && !lhiscallstd && lapcqonly {
            return false;
        }
        if iaptype == 36 && config.lwidedxcsearch && lapcqonly {
            return false;
        }
        if iaptype == 111 && lapcqonly {
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
        if iaptype > 30 && !config.lenabledxcsearch {
            return false;
        }
        if iaptype > 30 && !config.lwidedxcsearch && loutapwid {
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
        if !classifier.lqsocandave
            && classifier.lmycsignal
            && isubp1 > 5
            && isubp1 < 9
            && iaptype != 2
        {
            return false;
        }
        if classifier.lqsocandave && isubp1 > 8 && !(3..=6).contains(&iaptype) {
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
        if iaptype > 30 && !config.lenabledxcsearch {
            return false;
        }
        if iaptype > 30 && !config.lwidedxcsearch && loutapwid {
            return false;
        }
        if iaptype > 30 && lapcqonly {
            return false;
        }
        if iaptype == 31 && !classifier.lcqdxcnssig {
            return false;
        }
        if iaptype == 35 && !classifier.lqso73 {
            return false;
        }
        if iaptype == 36 && !classifier.lqsorr73 {
            return false;
        }
        if iaptype > 2 && iaptype < 15 && loutapwid {
            return false;
        }
        if classifier.lqsocandave && isubp1 > 8 && !(11..=14).contains(&iaptype) {
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
        if iaptype == 1 && !classifier.lcqsignal {
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
        if iaptype > 30 && iaptype < 40 && !config.lwidedxcsearch && loutapwid {
            return false;
        }
        return true;
    }

    false
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
                if config.lft8lowth || config.lft8subpass {
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
            if isubp2 == 18 {
                if config.lft8lowth || config.lft8subpass {
                    return classifier.smycnr >= 1.2 || classifier.lmycsignal;
                }
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
