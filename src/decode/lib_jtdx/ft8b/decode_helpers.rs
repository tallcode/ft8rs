use super::super::chkfalse8::{accept_decoded_message, FilterContext};
use super::super::delbraces::delbraces;
use super::super::ft8_mod1::{
    GRAYMAP, ICOS7, NAPPASSES, NAPTYPES, NDXNSAPTYPES, NHAPTYPES, NMYCNSAPTYPES,
};
use super::super::ft8v2::bpdecode174_91::{BpDecodeResult, N};
use super::super::ft8v2::packjt77::{unpack77_with_context, HashCallBook, UnpackContext};
use super::super::genft8::get_tones_from_77bits;
use super::super::msgparser::msgparser;
use super::state::{
    BitMetrics, CsMatrix, DecodeSource, Ft8bCandidateContext, Ft8bDecodeResult, MetricSource,
    SignalClassifier, SymbolMetrics,
};
use crate::stream::session::StreamDecodeConfig;

pub(super) fn build_bit_metrics(metrics: &SymbolMetrics, source: MetricSource) -> BitMetrics {
    build_bit_metrics_inner(metrics, source, None)
}

pub(super) fn build_bit_metrics_with_csold(
    metrics: &SymbolMetrics,
    source: MetricSource,
    csold: &CsMatrix,
) -> BitMetrics {
    build_bit_metrics_inner(metrics, source, Some(csold))
}

pub(super) fn build_bit_metrics_inner(
    metrics: &SymbolMetrics,
    source: MetricSource,
    csold: Option<&CsMatrix>,
) -> BitMetrics {
    let mut out = BitMetrics {
        bmeta: [0.0f32; N],
        bmetb: [0.0f32; N],
        bmetc: [0.0f32; N],
        bmetd: [0.0f32; N],
    };
    let srr = sync_snr_ratio(metrics);
    for nsym in 1..=3 {
        let nt = (1usize << (3 * nsym)) - 1;
        for ihalf in 0..2 {
            for k in (1..=29).step_by(nsym) {
                // F90 k/ks are 1-based. Keep source-shaped k, but map ks to
                // Rust's 0-based symbol matrix index here.
                let ks = if ihalf == 0 { k + 6 } else { k + 42 };
                let ks1 = ks + 1;
                let ks2 = ks + 2;
                let ibmax = match nsym {
                    1 => 2,
                    2 => 5,
                    _ => 8,
                };
                let mut s2 = [0.0f32; 512];
                for i in 0..=nt {
                    let i1 = i / 64;
                    let i2 = (i & 63) / 8;
                    let i33 = i & 7;

                    s2[i] = match source {
                        MetricSource::Cs => match nsym {
                            1 => cabs1(metrics, MetricSource::Cs, GRAYMAP[i33] as usize, ks),
                            2 => cabs2(
                                metrics,
                                MetricSource::Cs,
                                GRAYMAP[i2] as usize,
                                ks,
                                GRAYMAP[i33] as usize,
                                ks1,
                            ),
                            _ => cabs3(
                                metrics,
                                MetricSource::Cs,
                                GRAYMAP[i1] as usize,
                                ks,
                                GRAYMAP[i2] as usize,
                                ks1,
                                GRAYMAP[i33] as usize,
                                ks2,
                            ),
                        },
                        MetricSource::Csr => match nsym {
                            1 => cabs1(metrics, MetricSource::Csr, GRAYMAP[i33] as usize, ks),
                            2 => cabs2(
                                metrics,
                                MetricSource::Csr,
                                GRAYMAP[i2] as usize,
                                ks,
                                GRAYMAP[i33] as usize,
                                ks1,
                            ),
                            _ => cabs3(
                                metrics,
                                MetricSource::Csr,
                                GRAYMAP[i1] as usize,
                                ks,
                                GRAYMAP[i2] as usize,
                                ks1,
                                GRAYMAP[i33] as usize,
                                ks2,
                            ),
                        },
                        MetricSource::CscsCsrPower => match nsym {
                            1 => {
                                let a = cabs1(
                                    metrics,
                                    MetricSource::CscsCsrPower,
                                    GRAYMAP[i33] as usize,
                                    ks,
                                );
                                let b =
                                    cabs1(metrics, MetricSource::Csr, GRAYMAP[i33] as usize, ks);
                                a * a + b * b
                            }
                            2 => {
                                let a = cabs2(
                                    metrics,
                                    MetricSource::CscsCsrPower,
                                    GRAYMAP[i2] as usize,
                                    ks,
                                    GRAYMAP[i33] as usize,
                                    ks1,
                                );
                                let b = cabs2(
                                    metrics,
                                    MetricSource::Csr,
                                    GRAYMAP[i2] as usize,
                                    ks,
                                    GRAYMAP[i33] as usize,
                                    ks1,
                                );
                                a * a + b * b
                            }
                            _ => {
                                let a = cabs3(
                                    metrics,
                                    MetricSource::CscsCsrPower,
                                    GRAYMAP[i1] as usize,
                                    ks,
                                    GRAYMAP[i2] as usize,
                                    ks1,
                                    GRAYMAP[i33] as usize,
                                    ks2,
                                );
                                let b = cabs3(
                                    metrics,
                                    MetricSource::Csr,
                                    GRAYMAP[i1] as usize,
                                    ks,
                                    GRAYMAP[i2] as usize,
                                    ks1,
                                    GRAYMAP[i33] as usize,
                                    ks2,
                                );
                                a * a + b * b
                            }
                        },
                        MetricSource::CsCsoldPower => {
                            let old = csold.expect("csold metric source requires csold");
                            match nsym {
                                1 => {
                                    let a =
                                        cabs1(metrics, MetricSource::Cs, GRAYMAP[i33] as usize, ks);
                                    let b = cabs1_csold(old, GRAYMAP[i33] as usize, ks);
                                    a * a + b * b
                                }
                                2 => {
                                    let a = cabs2(
                                        metrics,
                                        MetricSource::Cs,
                                        GRAYMAP[i2] as usize,
                                        ks,
                                        GRAYMAP[i33] as usize,
                                        ks1,
                                    );
                                    let b = cabs2_csold(
                                        old,
                                        GRAYMAP[i2] as usize,
                                        ks,
                                        GRAYMAP[i33] as usize,
                                        ks1,
                                    );
                                    a * a + b * b
                                }
                                _ => {
                                    let a = cabs3(
                                        metrics,
                                        MetricSource::Cs,
                                        GRAYMAP[i1] as usize,
                                        ks,
                                        GRAYMAP[i2] as usize,
                                        ks1,
                                        GRAYMAP[i33] as usize,
                                        ks2,
                                    );
                                    let b = cabs3_csold(
                                        old,
                                        GRAYMAP[i1] as usize,
                                        ks,
                                        GRAYMAP[i2] as usize,
                                        ks1,
                                        GRAYMAP[i33] as usize,
                                        ks2,
                                    );
                                    a * a + b * b
                                }
                            }
                        }
                        MetricSource::CsCsoldSum => {
                            let old = csold.expect("csold metric source requires csold");
                            match nsym {
                                1 => {
                                    cabs1(metrics, MetricSource::Cs, GRAYMAP[i33] as usize, ks)
                                        + cabs1_csold(old, GRAYMAP[i33] as usize, ks)
                                }
                                2 => {
                                    cabs2(
                                        metrics,
                                        MetricSource::Cs,
                                        GRAYMAP[i2] as usize,
                                        ks,
                                        GRAYMAP[i33] as usize,
                                        ks1,
                                    ) + cabs2_csold(
                                        old,
                                        GRAYMAP[i2] as usize,
                                        ks,
                                        GRAYMAP[i33] as usize,
                                        ks1,
                                    )
                                }
                                _ => {
                                    cabs3(
                                        metrics,
                                        MetricSource::Cs,
                                        GRAYMAP[i1] as usize,
                                        ks,
                                        GRAYMAP[i2] as usize,
                                        ks1,
                                        GRAYMAP[i33] as usize,
                                        ks2,
                                    ) + cabs3_csold(
                                        old,
                                        GRAYMAP[i1] as usize,
                                        ks,
                                        GRAYMAP[i2] as usize,
                                        ks1,
                                        GRAYMAP[i33] as usize,
                                        ks2,
                                    )
                                }
                            }
                        }
                    };

                    if source == MetricSource::Cs && srr < 2.5 {
                        s2[i] = shape_primary_metric(s2[i], srr);
                    }
                    if source != MetricSource::Cs && srr < 2.5 {
                        let ss1 = 0.5 * s2[i];
                        s2[i] = ss1 * ss1 * ss1;
                    }
                }

                let i32 = 1 + (k - 1) * 3 + ihalf * 87;
                for ib in 0..=ibmax {
                    let bit = ibmax - ib;
                    let (s2_one, s2_zero) = max_by_bit_fortran(&s2, nt, bit);
                    let bm = s2_one - s2_zero;
                    let idx = i32 + ib - 1;
                    if idx >= N {
                        continue;
                    }
                    match nsym {
                        1 => {
                            out.bmeta[idx] = bm;
                            let den = s2_one.max(s2_zero);
                            out.bmetd[idx] = if den > 0.0 { bm / den } else { 0.0 };
                        }
                        2 => out.bmetb[idx] = bm,
                        _ => out.bmetc[idx] = bm,
                    }
                }
            }
        }
    }

    normalizebmet(&mut out.bmeta);
    normalizebmet(&mut out.bmetb);
    normalizebmet(&mut out.bmetc);
    normalizebmet(&mut out.bmetd);
    out
}

pub(super) fn shape_primary_metric(value: f32, srr: f32) -> f32 {
    if srr > 2.3 {
        value * value
    } else if value < 5.77 {
        let ss2 = value * value;
        1.0 + 8.0 * ss2 - 0.12 * ss2 * ss2
    } else {
        let ss1 = value + 5.82;
        ss1 * ss1
    }
}

pub(super) fn regular_llr_source<'a>(
    config: &StreamDecodeConfig,
    context: Ft8bCandidateContext,
    isubp1: usize,
    isubp2: usize,
    metrics: &'a BitMetrics,
) -> &'a [f32; N] {
    match isubp2 {
        1 => {
            if (!config.swl && context.ipass == 1) || (isubp1 > 1 && context.ipass > 1) {
                &metrics.bmetd
            } else {
                &metrics.bmeta
            }
        }
        2 => {
            if isubp1 > 1 {
                &metrics.bmeta
            } else {
                &metrics.bmetb
            }
        }
        3 => &metrics.bmetc,
        4 => &metrics.bmetd,
        _ => unreachable!(),
    }
}

pub(super) fn ap_llr_source<'a>(
    isubp2: usize,
    iaptype: i32,
    bmeta: &'a [f32; N],
    bmetb: &'a [f32; N],
    bmetc: &'a [f32; N],
) -> &'a [f32; N] {
    if matches!(iaptype, 4..=6) && matches!(isubp2, 10 | 13 | 16) {
        return bmetb;
    }
    match (isubp2 - 5) % 3 {
        0 => bmetc,
        1 => bmetb,
        _ => bmeta,
    }
}

pub(super) fn ap_ndeep(
    config: &StreamDecodeConfig,
    context: Ft8bCandidateContext,
    classifier: SignalClassifier,
    refined_freq: f64,
    iaptype: i32,
) -> usize {
    let mut ndeep = 3;
    let dfqso = (config.nfqso - refined_freq).abs();
    let lapmyc = normalized_config_call(config.mycall.as_deref()).is_some();
    if classifier.lqsosig || classifier.lmycsignal {
        if (dfqso < config.napwid || ((config.nftx - refined_freq).abs() < config.napwid && lapmyc))
            && !config.nagain
        {
            ndeep = 4;
        }
        if (lapmyc && context.lqsomsgdcd) || iaptype == 0 {
            ndeep = 3;
        }
        if !context.stophint && normalized_config_call(config.hiscall.as_deref()).is_some() {
            ndeep = 3;
        }
    }
    if classifier.ldxcsig && context.stophint && dfqso < config.napwid {
        ndeep = 4;
    }
    if config.lhound {
        ndeep = 3;
    }
    if config.nagain {
        5
    } else {
        ndeep
    }
}

pub(super) fn plan_ap_subpasses(config: &StreamDecodeConfig) -> Vec<(usize, i32)> {
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

pub(super) fn ap_type_table(config: &StreamDecodeConfig) -> &'static [[i32; 27]; 6] {
    let lapmyc = normalized_config_call(config.mycall.as_deref()).is_some();
    let lnohiscall = normalized_config_call(config.hiscall.as_deref()).is_none();
    let lmycallstd = lapmyc && !is_nonstandard_call(config.mycall.as_deref().unwrap_or(""));
    let lhiscallstd = !lnohiscall && !is_nonstandard_call(config.hiscall.as_deref().unwrap_or(""));

    if config.lhound {
        &NHAPTYPES
    } else if lmycallstd && (lhiscallstd || lnohiscall) {
        &NAPTYPES
    } else if lmycallstd && !lhiscallstd && !lnohiscall {
        &NDXNSAPTYPES
    } else if !lmycallstd && !lhiscallstd && !lnohiscall {
        &NDXNSAPTYPES
    } else {
        &NMYCNSAPTYPES
    }
}

pub(super) fn normalized_config_call(call: Option<&str>) -> Option<String> {
    let call = call?.trim().trim_start_matches('<').trim_end_matches('>');
    if call.len() < 3 {
        return None;
    }
    Some(call.to_ascii_uppercase())
}

pub(super) fn is_nonstandard_call(call: &str) -> bool {
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

pub(super) fn decoded_to_result(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    decoded: BpDecodeResult,
    config: &StreamDecodeConfig,
    book: &HashCallBook,
    iaptype: i32,
    isubp2: usize,
) -> Option<Ft8bDecodeResult> {
    if decoded_all_zero(&decoded) {
        return None;
    }
    if decoded_quality_rejected(&decoded, isubp2) {
        return None;
    }
    let (i3, n3) = i3_n3(&decoded.message77);
    if i3 > 4 || (i3 == 0 && n3 > 5) {
        return None;
    }
    let unpack_context = UnpackContext::with_calls(
        Some(book),
        config.mycall.as_deref(),
        config.hiscall.as_deref(),
    );
    let Some(mut msg) = unpack77_with_context(&decoded.message77, unpack_context) else {
        return None;
    };
    let l_free_text = i3 == 0 && n3 == 0;
    let mut msg37_2 = String::new();
    let mut l_special = false;
    if i3 == 0 && n3 == 1 {
        if let Some((parsed_msg, parsed_msg2)) = msgparser(&msg) {
            msg = parsed_msg;
            msg37_2 = parsed_msg2;
            l_special = true;
        }
    }
    let quality = 1.0 - (decoded.nharderror as f32 + decoded.dmin) / 60.0;
    let itone = get_tones_from_77bits(&decoded.message77);
    let xsnr = estimate_snr(metrics, &itone, iaptype, false);
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
    let lcall2hash = msg.find('<').is_some_and(|idx| idx > 3);
    let mut l_hashmsg = false;
    if !l_free_text && !l_special && msg.contains('<') {
        l_hashmsg = true;
        msg = delbraces(&msg);
    }
    if !accept_decoded_message(
        &msg,
        &msg37_2,
        i3,
        n3,
        iaptype,
        lcall1hash,
        lcall2hash,
        &filter_context,
    ) {
        return None;
    }
    if config.hide_hash && msg.find("<...>").is_some_and(|idx| idx >= 6) {
        return None;
    }
    if i3 == 3 && msg.starts_with("TU;") {
        let (parsed_msg, parsed_msg2, parsed_special) = split_tu_message(msg);
        msg = parsed_msg;
        msg37_2 = parsed_msg2;
        l_special = parsed_special;
    }
    Some(Ft8bDecodeResult {
        msg37: msg,
        msg37_2,
        l_free_text,
        l_special,
        l_hashmsg,
        snr: xsnr,
        freq: refined_freq as f32,
        dt: (refined_dt - 0.5) as f32,
        iaptype,
        i3: i3 as i32,
        n3: n3 as i32,
        itone,
        source: DecodeSource::Regular,
    })
}

pub(super) fn decoded_all_zero(decoded: &BpDecodeResult) -> bool {
    decoded.cw.iter().all(|&bit| bit == 0)
}

pub(super) fn decoded_quality_rejected(decoded: &BpDecodeResult, isubp2: usize) -> bool {
    decoded.nharderror < 0
        || decoded.nharderror as f32 + decoded.dmin >= 60.0
        || (isubp2 > 2 && decoded.nharderror > 39)
}

pub(super) fn decoded_bits_to_result(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    mut msg: String,
    message77: [u8; 77],
    itone: [i32; 79],
    config: &StreamDecodeConfig,
    _book: &HashCallBook,
    source: DecodeSource,
) -> Option<Ft8bDecodeResult> {
    let (i3, n3) = i3_n3(&message77);
    if i3 > 4 || (i3 == 0 && n3 > 5) {
        return None;
    }
    let l_free_text = i3 == 0 && n3 == 0;
    let l_special = i3 == 0 && n3 == 1;
    let mut l_hashmsg = false;
    if (!l_free_text && !l_special || source != DecodeSource::Regular) && msg.contains('<') {
        l_hashmsg = true;
        msg = delbraces(&msg);
    }
    let xsnr = estimate_snr(metrics, &itone, 0, source != DecodeSource::Regular);
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
        quality: 1.0,
        xsnr,
        rxdt: refined_dt as f32 - 0.5,
    };
    if source == DecodeSource::Regular {
        let lcall1hash = msg.starts_with('<');
        let lcall2hash = msg.find('<').is_some_and(|idx| idx > 3);
        if !accept_decoded_message(&msg, "", i3, n3, 0, lcall1hash, lcall2hash, &filter_context) {
            return None;
        }
    }
    if config.hide_hash && msg.find("<...>").is_some_and(|idx| idx >= 6) {
        return None;
    }
    Some(Ft8bDecodeResult {
        msg37: msg,
        msg37_2: String::new(),
        l_free_text,
        l_special,
        l_hashmsg,
        snr: xsnr,
        freq: refined_freq as f32,
        dt: (refined_dt - 0.5) as f32,
        iaptype: 0,
        i3: i3 as i32,
        n3: n3 as i32,
        itone,
        source,
    })
}

fn split_tu_message(msg: String) -> (String, String, bool) {
    let words: Vec<&str> = msg.split_whitespace().collect();
    if words.len() >= 3 {
        let msg37_2 = msg.get(4..).unwrap_or("").trim().to_string();
        return (format!("DE {} TU", words[2]), msg37_2, true);
    }
    (msg, String::new(), false)
}

pub(super) fn sync_snr_ratio(metrics: &SymbolMetrics) -> f32 {
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

fn metric_tables<'a>(
    metrics: &'a SymbolMetrics,
    source: MetricSource,
) -> (&'a [[f32; 79]; 8], &'a [[f32; 79]; 8]) {
    match source {
        MetricSource::Cs | MetricSource::CsCsoldPower | MetricSource::CsCsoldSum => {
            (&metrics.cs_re, &metrics.cs_im)
        }
        MetricSource::Csr => (&metrics.csr_re, &metrics.csr_im),
        MetricSource::CscsCsrPower => (&metrics.cscs_re, &metrics.cscs_im),
    }
}

fn cabs1(metrics: &SymbolMetrics, source: MetricSource, tone1: usize, k1: usize) -> f32 {
    let (re_table, im_table) = metric_tables(metrics, source);
    let re = re_table[tone1][k1];
    let im = im_table[tone1][k1];
    (re * re + im * im).sqrt()
}

fn cabs2(
    metrics: &SymbolMetrics,
    source: MetricSource,
    tone1: usize,
    k1: usize,
    tone2: usize,
    k2: usize,
) -> f32 {
    let (re_table, im_table) = metric_tables(metrics, source);
    let re = re_table[tone1][k1] + re_table[tone2][k2];
    let im = im_table[tone1][k1] + im_table[tone2][k2];
    (re * re + im * im).sqrt()
}

fn cabs3(
    metrics: &SymbolMetrics,
    source: MetricSource,
    tone1: usize,
    k1: usize,
    tone2: usize,
    k2: usize,
    tone3: usize,
    k3: usize,
) -> f32 {
    let (re_table, im_table) = metric_tables(metrics, source);
    let re = re_table[tone1][k1] + re_table[tone2][k2] + re_table[tone3][k3];
    let im = im_table[tone1][k1] + im_table[tone2][k2] + im_table[tone3][k3];
    (re * re + im * im).sqrt()
}

fn cabs1_csold(csold: &CsMatrix, tone1: usize, k1: usize) -> f32 {
    let re = csold.re[tone1][k1];
    let im = csold.im[tone1][k1];
    (re * re + im * im).sqrt()
}

fn cabs2_csold(csold: &CsMatrix, tone1: usize, k1: usize, tone2: usize, k2: usize) -> f32 {
    let re = csold.re[tone1][k1] + csold.re[tone2][k2];
    let im = csold.im[tone1][k1] + csold.im[tone2][k2];
    (re * re + im * im).sqrt()
}

fn cabs3_csold(
    csold: &CsMatrix,
    tone1: usize,
    k1: usize,
    tone2: usize,
    k2: usize,
    tone3: usize,
    k3: usize,
) -> f32 {
    let re = csold.re[tone1][k1] + csold.re[tone2][k2] + csold.re[tone3][k3];
    let im = csold.im[tone1][k1] + csold.im[tone2][k2] + csold.im[tone3][k3];
    (re * re + im * im).sqrt()
}

fn max_by_bit_fortran(values: &[f32; 512], nt: usize, bit: usize) -> (f32, f32) {
    let mut s2_one = f32::NEG_INFINITY;
    let mut s2_zero = f32::NEG_INFINITY;
    for (i, value) in values.iter().enumerate().take(nt + 1) {
        if ((i >> bit) & 1) == 1 {
            if *value > s2_one {
                s2_one = *value;
            }
        } else if *value > s2_zero {
            s2_zero = *value;
        }
    }
    (s2_one, s2_zero)
}

fn normalizebmet(bmet: &mut [f32; N]) {
    let mut bmet2av = 0.0f32;
    for value in bmet.iter() {
        bmet2av += *value * *value;
    }
    bmet2av /= N as f32;
    let sigma = bmet2av.sqrt();
    if sigma > 0.0 {
        for value in bmet {
            *value /= sigma;
        }
    }
}

pub(super) fn i3_n3(message77: &[u8; 77]) -> (usize, usize) {
    let n3 = bits_to_usize(&message77[71..74]);
    let i3 = bits_to_usize(&message77[74..77]);
    (i3, n3)
}

pub(super) fn bits_to_usize(bits: &[u8]) -> usize {
    let mut value = 0usize;
    for &bit in bits {
        value = (value << 1) | bit as usize;
    }
    value
}

pub(super) fn estimate_snr(
    metrics: &SymbolMetrics,
    itone: &[i32; 79],
    iaptype: i32,
    lft8s_or_sd: bool,
) -> f32 {
    estimate_snr_from_s8(&metrics.s8, itone, iaptype, lft8s_or_sd)
}

pub(super) fn estimate_snr_from_s8(
    s8: &[[f32; 79]; 8],
    itone: &[i32; 79],
    iaptype: i32,
    lft8s_or_sd: bool,
) -> f32 {
    let mut xsnrtmp = 0.001f32;
    for (i, &tone) in itone.iter().enumerate() {
        let tone = tone.clamp(0, 7) as usize;
        let xsig = s8[tone][i] * s8[tone][i];
        let mut total = 0.0f32;
        for itone in 0..8 {
            total += s8[itone][i] * s8[itone][i];
        }
        let mut xnoi = (total - xsig) / 7.0;
        if xnoi < 0.01 {
            xnoi = 0.01;
        }
        let xsnr = if xnoi < xsig { xsig / xnoi } else { 1.01 };
        xsnrtmp += xsnr;
    }

    let mut xsnr = xsnrtmp / 79.0 - 1.0;
    xsnr = 10.0 * xsnr.log10() - 26.5;
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
    let xsnrs = xsnr;
    if xsnr < -17.0 {
        if xsnr < -22.5 && xsnr > -23.5 {
            xsnr = -22.5;
        }
        let xsnr_adj = 1.0 + 1.4 / (23.0 + xsnr);
        xsnr = xsnr - xsnr_adj * xsnr_adj + 1.2;
    }
    if iaptype == 0 {
        if xsnr < -23.0 {
            xsnr = -23.0;
        }
    } else {
        if xsnr < -24.0 {
            xsnr = -24.0;
        }
    }
    if lft8s_or_sd {
        if xsnr < -22.0 {
            xsnr = xsnrs - 1.0;
        }
        if xsnr < -26.0 {
            xsnr = -26.0;
        }
    }
    xsnr
}
