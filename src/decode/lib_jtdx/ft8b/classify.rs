use super::decode_helpers::{is_nonstandard_call, normalized_config_call};
use super::state::{
    CsMatrix, Ft8bCandidateContext, SignalClassifier, SignalKind, SignalMemory, SymbolMetrics,
    ToneHints,
};
use super::{max_tone, maxloc_1based};
use crate::decode::lib_jtdx::tone8::Tone8Tables;
use crate::stream::session::StreamDecodeConfig;

impl ToneHints {
    pub(super) fn from_tables(tables: &Tone8Tables) -> Self {
        Self {
            idtone25_2: tables.idtone25_2,
            idtonemyc: tables.idtonemyc,
            idtone56: tables.idtone56.clone(),
            idtonecqdxcns: tables.idtonecqdxcns,
            idtonedxcns73: tables.idtonedxcns73,
            idtonefox73: tables.idtonefox73,
            idtonespec: tables.idtonespec,
        }
    }
}

pub(super) fn remember_failed_candidate_signal(
    signal_memory: &mut SignalMemory,
    metrics: &SymbolMetrics,
    classifier: SignalClassifier,
    refined_freq: f64,
    refined_dt: f64,
    context: Ft8bCandidateContext,
    isubp1: usize,
    isubp2: usize,
    iaptype: i32,
) {
    if !((classifier.nweak == 1 && isubp1 == 1) || (classifier.nweak == 2 && isubp1 == 2)) {
        return;
    }

    let cs = CsMatrix {
        re: if isubp1 == 2 {
            metrics.csr_re
        } else {
            metrics.cs_re
        },
        im: if isubp1 == 2 {
            metrics.csr_im
        } else {
            metrics.cs_im
        },
    };
    if context.ipass == context.npass.saturating_sub(1)
        && classifier.lcqsignal
        && isubp2 == 22
        && !signal_memory.has_decoded_tmp(SignalKind::Cq, refined_freq, refined_dt)
    {
        signal_memory.remember_tmp(SignalKind::Cq, refined_freq, refined_dt, cs.clone());
    }
    if context.ipass == context.npass.saturating_sub(1)
        && classifier.lmycsignal
        && isubp2 == 19
        && !signal_memory.has_decoded_tmp(SignalKind::MyCall, refined_freq, refined_dt)
    {
        signal_memory.remember_tmp(SignalKind::MyCall, refined_freq, refined_dt, cs.clone());
    }
    if context.ipass == context.npass && classifier.lqsocandave && matches!(iaptype, 3 | 6) {
        signal_memory.remember_tmp(SignalKind::Qso, refined_freq, refined_dt, cs);
    }
}

pub(super) fn select_csold(
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

pub(super) fn classify_signal(
    metrics: &SymbolMetrics,
    config: &StreamDecodeConfig,
    refined_freq: f64,
    context: Ft8bCandidateContext,
    hints: &ToneHints,
) -> SignalClassifier {
    let lapmyc = normalized_config_call(config.mycall.as_deref()).is_some();
    let mycall_raw = config.mycall.as_deref().unwrap_or("");
    let hiscall_raw = config.hiscall.as_deref().unwrap_or("");
    let lmycallstd = normalized_config_call(config.mycall.as_deref()).is_some()
        && !is_nonstandard_call(mycall_raw);
    let lhiscallstd = normalized_config_call(config.hiscall.as_deref()).is_some()
        && !is_nonstandard_call(hiscall_raw);
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
            let mut best_idx = 0usize;
            let mut best_count = 0usize;
            for (idx, &count) in nqsoend.iter().enumerate() {
                if count > best_count {
                    best_count = count;
                    best_idx = idx;
                }
            }
            if best_count > 6 {
                match best_idx {
                    0 => lqso73 = true,
                    1 => lqsorr73 = true,
                    _ => lqsorrr = true,
                }
            }
        }
    }

    let mut ndxt = 0usize;
    let mut ldxcsig = false;
    if lhiscallstd && !hints.idtone56.is_empty() {
        let qso_tones = &hints.idtone56[0];
        for k11 in 17..=26 {
            if max_tone(&metrics.s8, k11 - 1, None) as i32 == qso_tones[k11 - 8] {
                ndxt += 1;
            }
        }
        ldxcsig = ndxt > 3;
    }
    let lcqdxcsig = lcqsignal && ldxcsig;
    let mut lcqdxcnssig = false;
    if !lhiscallstd && normalized_config_call(config.hiscall.as_deref()).is_some() {
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
        && !context.lft8sdec
        && !context.lqsomsgdcd
        && ((!config.lskiptx1 && context.nlasttx == 1)
            || (config.lskiptx1 && context.nlasttx == 2));
    let nweak = if config.lft8subpass || config.swl || dfqso < 2.0 || lsubptxfreq {
        2
    } else {
        1
    };
    let mut nsubpasses = nweak;
    if lcqsignal {
        nsubpasses = 3;
    }
    if lmycsignal && lmycallstd {
        nsubpasses = 6;
    }
    let lqsocandave = lapmyc
        && ndxt > 2
        && nmic > 2
        && !context.lqsomsgdcd
        && lmycallstd
        && lhiscallstd
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
    for tone_values in s8.iter().take(8) {
        for value in tone_values.iter().take(16).skip(7) {
            total += *value;
        }
    }
    let noise = (total - signal) / 7.0;
    signal / noise
}
