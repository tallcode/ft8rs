use super::super::chkfalse8::{accept_decoded_message, FilterContext};
use super::super::ft8_mod1::{
    GRAYMAP, ICOS7, NAPPASSES, NAPTYPES, NDXNSAPTYPES, NHAPTYPES, NMYCNSAPTYPES,
};
use super::super::ft8v2::bpdecode174_91::{BpDecodeResult, N};
use super::super::ft8v2::encode174_91::encode174_91;
use super::super::ft8v2::packjt77::{unpack77_with_context, HashCallBook, UnpackContext};
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
            let mut k = 0usize;
            while k < 29 {
                let ks = if ihalf == 0 { k + 7 } else { k + 43 };
                let i32 = 1 + k * 3 + ihalf * 87;
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
                    let tones = match nsym {
                        1 => [(GRAYMAP[i33] as usize, ks), (0, 0), (0, 0)],
                        2 => [
                            (GRAYMAP[i2] as usize, ks),
                            (GRAYMAP[i33] as usize, ks + 1),
                            (0, 0),
                        ],
                        _ => [
                            (GRAYMAP[i1] as usize, ks),
                            (GRAYMAP[i2] as usize, ks + 1),
                            (GRAYMAP[i33] as usize, ks + 2),
                        ],
                    };
                    let value = metric_source_value(metrics, source, csold, &tones[..nsym]);
                    *slot = if source == MetricSource::Cs && srr < 2.5 {
                        shape_primary_metric(value, srr)
                    } else if source != MetricSource::Cs && srr < 2.5 {
                        (0.5 * value).powi(3)
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
                            out.bmeta[idx] = bm;
                            let den = max_by_bit(&s2, bit, true).max(max_by_bit(&s2, bit, false));
                            out.bmetd[idx] = if den > 0.0 { bm / den } else { 0.0 };
                        }
                        2 => out.bmetb[idx] = bm,
                        _ => out.bmetc[idx] = bm,
                    }
                }
                k += nsym;
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
        1.0 + 8.0 * value.powi(2) - 0.12 * value.powi(4)
    } else {
        (value + 5.82).powi(2)
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
) -> Option<Ft8bDecodeResult> {
    if decoded.cw.iter().all(|&bit| bit == 0) {
        return None;
    }
    let unpack_context = UnpackContext::with_calls(
        Some(book),
        config.mycall.as_deref(),
        config.hiscall.as_deref(),
    );
    let mut msg = unpack77_with_context(&decoded.message77, unpack_context)?;
    let (i3, n3) = i3_n3(&decoded.message77);
    if i3 > 4 || (i3 == 0 && n3 > 5) {
        return None;
    }
    let l_free_text = i3 == 0 && n3 == 0;
    let mut msg37_2 = String::new();
    let l_special = i3 == 0 && n3 == 1;
    if l_special {
        if let Some((parsed_msg, parsed_msg2)) = msgparser(&msg) {
            msg = parsed_msg;
            msg37_2 = parsed_msg2;
        }
    }
    let quality = 1.0 - (decoded.nharderror as f32 + decoded.dmin) / 60.0;
    let codeword = encode174_91(&decoded.message77);
    let itone = tones_from_codeword(&codeword);
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
    if !accept_decoded_message(&msg, &msg37_2, i3, n3, iaptype, lcall1hash, &filter_context) {
        return None;
    }
    if config.hide_hash && msg.find("<...>").is_some_and(|idx| idx >= 6) {
        return None;
    }
    Some(Ft8bDecodeResult {
        msg37: msg,
        msg37_2,
        l_free_text,
        l_special,
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

pub(super) fn decoded_bits_to_result(
    metrics: &SymbolMetrics,
    refined_freq: f64,
    refined_dt: f64,
    msg: String,
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
    let xsnr = estimate_snr(metrics, &itone, 0);
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
        if !accept_decoded_message(&msg, "", i3, n3, 0, lcall1hash, &filter_context) {
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

pub(super) fn tones_from_codeword(codeword: &[u8; N]) -> [i32; 79] {
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

pub(super) fn metric_source_value(
    metrics: &SymbolMetrics,
    source: MetricSource,
    csold: Option<&CsMatrix>,
    pairs: &[(usize, usize)],
) -> f32 {
    match source {
        MetricSource::Cs => complex_abs_sum(&metrics.cs_re, &metrics.cs_im, pairs),
        MetricSource::Csr => complex_abs_sum(&metrics.csr_re, &metrics.csr_im, pairs),
        MetricSource::CscsCsrPower => {
            let a = complex_abs_sum(&metrics.cscs_re, &metrics.cscs_im, pairs);
            let b = complex_abs_sum(&metrics.csr_re, &metrics.csr_im, pairs);
            a * a + b * b
        }
        MetricSource::CsCsoldPower => {
            let old = csold.expect("csold metric source requires csold");
            let a = complex_abs_sum(&metrics.cs_re, &metrics.cs_im, pairs);
            let b = complex_abs_sum(&old.re, &old.im, pairs);
            a * a + b * b
        }
        MetricSource::CsCsoldSum => {
            let old = csold.expect("csold metric source requires csold");
            complex_abs_sum(&metrics.cs_re, &metrics.cs_im, pairs)
                + complex_abs_sum(&old.re, &old.im, pairs)
        }
    }
}

pub(super) fn complex_abs_sum(
    re_table: &[[f32; 79]; 8],
    im_table: &[[f32; 79]; 8],
    pairs: &[(usize, usize)],
) -> f32 {
    let mut re = 0.0f32;
    let mut im = 0.0f32;
    for &(tone, k) in pairs {
        re += re_table[tone][k];
        im += im_table[tone][k];
    }
    (re * re + im * im).sqrt()
}

pub(super) fn max_by_bit(values: &[f32], bit: usize, wanted: bool) -> f32 {
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

pub(super) fn estimate_snr(metrics: &SymbolMetrics, itone: &[i32; 79], iaptype: i32) -> f32 {
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
    let xsnrs = xsnr;
    if xsnr < -17.0 {
        if xsnr < -22.5 && xsnr > -23.5 {
            xsnr = -22.5;
        }
        xsnr = xsnr - (1.0 + 1.4 / (23.0 + xsnr)).powi(2) + 1.2;
    }
    xsnr = if iaptype == 0 {
        xsnr.max(-23.0)
    } else {
        xsnr.max(-24.0)
    };
    if iaptype > 4 {
        if xsnr < -22.0 {
            xsnr = xsnrs - 1.0;
        }
        if xsnr < -26.0 {
            xsnr = -26.0;
        }
    }
    xsnr
}
