//! WSJT-X-style FT8 regular decoder facade and outer pass control.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8_decode.f90` outer pass/subtract flow
//! - `wsjtx/lib/ft8/sync8.f90` candidate search
//! - `wsjtx/lib/ft8/ft8b.f90` candidate decode

use crate::ft8::hashcall::HashCallBook;
use crate::ft8::protocol::SAMPLE_RATE;
use crate::util::four2a_r2c;
use std::time::Instant;

mod baseline;
mod ft8_downsample;
mod ft8_params;
mod ft8b;
mod symbols;
mod sync8;
mod sync8d;
mod sync_templates;
mod workspace;

use self::baseline::get_spectrum_baseline;
pub(crate) use self::ft8_downsample::ft8_downsample_from_cx;
pub(crate) use self::ft8_params::{
    COSTAS_BLOCKS, COSTAS_SYMBOL_LEN, DOWNSAMPLE_BAUD, DOWNSAMPLE_DF, DOWNSAMPLE_FAC, DT2, FS2,
    NFFT1, NFFT1_LONG, NFFT2, NHSYM, NMAX, NN, NP2, NSPS, NSTEP, PI_F32, TAPER_SIZE, TWO_PI,
    TWO_PI_F32,
};
pub(crate) use self::ft8b::normalize_bmet;
use self::ft8b::{duration_ms, ft8_ap_set, ft8b, trace_timer, trace_timers_enabled};
pub(crate) use self::symbols::extract_symbol_spectrum;
use self::sync8::sync8;
pub(crate) use self::sync8d::{sync8d, sync8d_twk};
pub(crate) use self::sync_templates::build_costas_sync_templates;
use self::sync_templates::{build_frequency_shift_sync_templates, build_taper};
use self::workspace::*;

/// sync8 spectral mode - different representations favour different SNR regimes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncMode {
    /// Power spectrum: s = Re2 + Im2 (best for strong signals)
    Power = 0,
    /// Amplitude spectrum: s = sqrt(Re2 + Im2) (better for weak signals, compresses dynamic range)
    Amplitude = 1,
    /// Absolute sum: s = |Re| + |Im| (most robust against impulsive noise)
    AbsSum = 2,
}

#[derive(Clone)]
pub struct DecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
    /// FT8 itone pattern for signal subtraction (79 tones)
    pub itone: Vec<i32>,
}

#[allow(non_snake_case)]
#[derive(Default)]
pub struct DecodeOptions {
    pub sample_rate: Option<usize>,
    pub nfa: Option<f64>,
    pub nfb: Option<f64>,
    pub syncmin: Option<f64>,
    pub ndepth: Option<usize>,
    pub ncand: Option<usize>,
    pub hashcallbook: Option<HashCallBook>,
    pub mycall: Option<String>,
    pub hiscall: Option<String>,
    pub nfqso: Option<f64>,
    pub nftx: Option<f64>,
    pub nQSOProgress: Option<usize>,
    pub ncontest: Option<usize>,
    pub napwid: Option<f64>,
    pub lft8apon: Option<bool>,
    pub lapcqonly: Option<bool>,
    pub nagain: Option<bool>,
    pub nzhsym: Option<usize>,
    /// Messages already decoded in an earlier progressive stage for the same
    /// slot. WSJT-X carries these in `allmessages`/`ndecodes` when nzhsym=50.
    pub initial_messages: Vec<String>,
    /// Sync spectral mode: Power (default), Amplitude (better for weak signals),
    /// AbsSum (robust against impulsive noise).
    pub sync_mode: Option<SyncMode>,
}

pub fn decode(samples: &[f32], options: DecodeOptions) -> Vec<DecodedMessage> {
    let t_start = std::time::Instant::now();
    let sample_rate = options.sample_rate.unwrap_or(SAMPLE_RATE);

    let dd = if sample_rate == SAMPLE_RATE {
        copy_samples_to_decode_window(samples)
    } else {
        resample(samples, sample_rate, SAMPLE_RATE, NMAX)
    };
    let t_dd = t_start.elapsed();
    let (msgs, _, _) = decode_from_f64(dd, options, t_dd, t_start);
    msgs
}

/// Decode directly from f64 samples (used for cleaned residual after subtraction).
pub fn decode_f64(samples: &[f64], options: DecodeOptions) -> Vec<DecodedMessage> {
    let t_start = std::time::Instant::now();
    let len = samples.len().min(NMAX);
    let dd = samples[..len].to_vec();
    let t_dd = t_start.elapsed();
    let (msgs, _, _) = decode_from_f64(dd, options, t_dd, t_start);
    msgs
}

/// Decode directly from f64 samples and return the sbase noise baseline.
pub fn decode_f64_with_sbase(
    samples: &[f64],
    options: DecodeOptions,
) -> (Vec<DecodedMessage>, Vec<f64>) {
    let t_start = std::time::Instant::now();
    let len = samples.len().min(NMAX);
    let dd = samples[..len].to_vec();
    let t_dd = t_start.elapsed();
    let (msgs, sbase, _) = decode_from_f64(dd, options, t_dd, t_start);
    (msgs, sbase)
}

pub fn decode_f64_with_sbase_and_residual(
    samples: &[f64],
    options: DecodeOptions,
) -> (Vec<DecodedMessage>, Vec<f64>, Vec<f64>) {
    let t_start = std::time::Instant::now();
    let len = samples.len().min(NMAX);
    let dd = samples[..len].to_vec();
    let t_dd = t_start.elapsed();
    decode_from_f64(dd, options, t_dd, t_start)
}

/// Decode returning both messages and the sbase noise baseline.
pub fn decode_with_sbase(
    samples: &[f32],
    options: DecodeOptions,
) -> (Vec<DecodedMessage>, Vec<f64>) {
    let t_start = std::time::Instant::now();
    let sample_rate = options.sample_rate.unwrap_or(SAMPLE_RATE);
    let dd = if sample_rate == SAMPLE_RATE {
        copy_samples_to_decode_window(samples)
    } else {
        resample(samples, sample_rate, SAMPLE_RATE, NMAX)
    };
    let t_dd = t_start.elapsed();
    let (msgs, sbase, _) = decode_from_f64(dd, options, t_dd, t_start);
    (msgs, sbase)
}

fn decode_from_f64(
    mut dd: Vec<f64>,
    options: DecodeOptions,
    _t_dd: std::time::Duration,
    _t_start: std::time::Instant,
) -> (Vec<DecodedMessage>, Vec<f64>, Vec<f64>) {
    let t_decode_total = Instant::now();
    // Truncate to NMAX (15s @ 12kHz = 180000 samples) matching WSJT-X NPTS
    if dd.len() > NMAX {
        dd.truncate(NMAX);
    }
    let nfa = options.nfa.unwrap_or(200.0);
    let nfb = options.nfb.unwrap_or(3000.0);
    let ndepth = options.ndepth.unwrap_or(3);
    let syncmin = options
        .syncmin
        .unwrap_or_else(|| default_outer_sync_min(ndepth));
    let ncand = options.ncand.unwrap_or(1000);
    let book = options.hashcallbook;
    let sync_mode = options.sync_mode.unwrap_or(SyncMode::Power);
    let nfqso = options.nfqso.unwrap_or(0.0);
    let nagain = options.nagain.unwrap_or(false);
    let ncontest = options.ncontest.unwrap_or(0);
    let mycall = options.mycall.clone();
    let hiscall = options.hiscall.clone();
    let nzhsym = options.nzhsym.unwrap_or(50);
    let ap_options = Ft8bApOptions {
        enabled: options.lft8apon.unwrap_or(true),
        cq_only: options.lapcqonly.unwrap_or(false),
        nqso_progress: options.nQSOProgress.unwrap_or(0).min(5),
        ncontest,
        nfqso,
        nftx: options.nftx.unwrap_or(0.0),
        napwid: options.napwid.unwrap_or(50.0),
        nzhsym,
        ap_set: ft8_ap_set(mycall.as_deref(), hiscall.as_deref(), ncontest),
        mycall,
        hiscall,
    };
    let mut residual = dd.clone();

    let mut cx_re = vec![0.0; NFFT1_LONG];
    let mut cx_im = vec![0.0; NFFT1_LONG];

    let mut decoded: Vec<DecodedMessage> = Vec::new();
    let mut seen_messages: std::collections::HashSet<String> = options
        .initial_messages
        .iter()
        .map(|msg| normalize_message_key(msg))
        .collect();
    let mut ndecodes_total = seen_messages.len();
    let max_passes = if ndepth == 1 { 2 } else { 3 };

    // WSJT-X sync8 refreshes sbase for each pass on the current residual.
    let mut sbase: Vec<f64> = Vec::new();
    for pass_idx in 0..max_passes {
        if pass_idx == 2 && ndecodes_total == 0 {
            trace_timer(
                "decode.pass.skip",
                t_decode_total,
                Some(format!("nzhsym={nzhsym} pass={}", pass_idx + 1)),
            );
            continue;
        }
        let t_pass = Instant::now();
        let pass_syncmin = syncmin;
        cx_re.fill(0.0);
        cx_im.fill(0.0);

        let t_fft = Instant::now();
        cx_re[..residual.len()].copy_from_slice(&residual);
        four2a_r2c(&mut cx_re, &mut cx_im);
        trace_timer(
            "decode.pass.fft",
            t_fft,
            Some(format!("nzhsym={nzhsym} pass={}", pass_idx + 1)),
        );

        let (ifa, ifb) = if nagain {
            (nfqso - 20.0, nfqso + 20.0)
        } else {
            (nfa, nfb)
        };

        let t_sync = Instant::now();
        let (candidates, pass_sbase) =
            sync8(&residual, ifa, ifb, pass_syncmin, nfqso, ncand, sync_mode);
        sbase = pass_sbase;
        trace_timer(
            "decode.pass.sync8",
            t_sync,
            Some(format!(
                "nzhsym={nzhsym} pass={} candidates={}",
                pass_idx + 1,
                candidates.len()
            )),
        );

        // WSJT-X ft8_decode.f90: pass 1 uses imetric=1, passes 2/3 use imetric=2.
        let pass_imetric = if pass_idx == 0 { 1 } else { 2 };

        let mut cand_ws = create_decode_workspace();
        let mut ft8b_stats = trace_timers_enabled().then(Ft8bStats::default);
        // Candidate decoding is sequential so each accepted signal updates the residual
        // before later candidates are evaluated.
        let t_candidates = Instant::now();
        let decoded_before = decoded.len();
        let mut accepted = 0usize;
        let mut duplicates = 0usize;
        for cand in &candidates {
            if let Some(r) = ft8b(
                &residual,
                &cx_re,
                &cx_im,
                cand.freq,
                cand.dt,
                &sbase,
                ndepth,
                pass_imetric,
                nagain,
                &ap_options,
                &book,
                None,
                &mut cand_ws,
                ft8b_stats.as_mut(),
            ) {
                let message_key = normalize_message_key(&r.msg);
                crate::ft8::subtract_ft8::subtract_ft8(&mut residual, &r.itone, r.freq, r.dt);
                if seen_messages.contains(&message_key) {
                    duplicates += 1;
                    continue;
                }
                seen_messages.insert(message_key);
                ndecodes_total += 1;
                accepted += 1;
                decoded.push(DecodedMessage {
                    freq: r.freq,
                    dt: r.dt - 0.5,
                    snr: r.snr,
                    msg: r.msg.clone(),
                    sync: cand.sync,
                    itone: r.itone.to_vec(),
                });
            }
        }
        trace_timer(
            "decode.pass.candidates",
            t_candidates,
            Some(format!(
                "nzhsym={nzhsym} pass={} accepted={accepted} duplicates={duplicates} total_decoded={}",
                pass_idx + 1,
                decoded.len()
            )),
        );
        if let Some(stats) = &ft8b_stats {
            trace_timer(
                "decode.pass.ft8b",
                t_candidates,
                Some(format!(
                    "nzhsym={nzhsym} pass={} calls={} sync_rejects={} decode_failures={} downsample={:.1}ms align={:.1}ms symbols={:.1}ms metrics={:.1}ms ldpc={:.1}ms post={:.1}ms",
                    pass_idx + 1,
                    stats.calls,
                    stats.sync_rejects,
                    stats.decode_failures,
                    duration_ms(stats.downsample),
                    duration_ms(stats.align),
                    duration_ms(stats.symbols),
                    duration_ms(stats.metrics),
                    duration_ms(stats.ldpc),
                    duration_ms(stats.post),
                )),
            );
        }
        trace_timer(
            "decode.pass.total",
            t_pass,
            Some(format!(
                "nzhsym={nzhsym} pass={} new_decodes={}",
                pass_idx + 1,
                decoded.len() - decoded_before
            )),
        );
    }

    trace_timer(
        "decode.total",
        t_decode_total,
        Some(format!("nzhsym={nzhsym} decoded={}", decoded.len())),
    );
    (decoded, sbase, residual)
}

fn default_outer_sync_min(depth: usize) -> f64 {
    if depth <= 2 {
        2.1
    } else {
        1.3
    }
}

fn normalize_message_key(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn copy_samples_to_decode_window(samples: &[f32]) -> Vec<f64> {
    let len = samples.len().min(NMAX);
    samples[..len].iter().map(|&x| x as f64).collect()
}

fn resample(input: &[f32], from_rate: usize, to_rate: usize, out_len: usize) -> Vec<f64> {
    let ratio = from_rate as f64 / to_rate as f64;
    (0..out_len)
        .map(|i| {
            let src_idx = i as f64 * ratio;
            let lo = src_idx.floor() as usize;
            let frac = src_idx - lo as f64;
            let v0 = if lo < input.len() {
                input[lo] as f64
            } else {
                0.0
            };
            let v1 = if lo + 1 < input.len() {
                input[lo + 1] as f64
            } else {
                0.0
            };
            v0 * (1.0 - frac) + v1 * frac
        })
        .collect()
}

fn nint_wsjtx_f32(x: f64) -> isize {
    (x as f32).round() as isize
}

fn nint_wsjtx_real(x: f32) -> isize {
    x.round() as isize
}

#[cfg(test)]
mod tests {
    use super::ft8b::{apply_wsjt_ap_mask, is_acceptable_unpacked_message, M73, MCQ, MRR73, MRRR};
    use super::sync8::finalize_sync8_candidates;
    use super::*;

    #[test]
    fn default_outer_sync_min_matches_wsjtx_depth_gate() {
        assert_eq!(default_outer_sync_min(1), 2.1);
        assert_eq!(default_outer_sync_min(2), 2.1);
        assert_eq!(default_outer_sync_min(3), 1.3);
    }

    #[test]
    fn sync8_candidate_order_matches_wsjtx_priority_rules() {
        let mut candidate0 = vec![
            Candidate {
                freq: 100.0,
                dt: 0.00,
                sync: 4.5,
            },
            Candidate {
                freq: 102.5,
                dt: 0.02,
                sync: 5.0,
            },
            Candidate {
                freq: 210.0,
                dt: -0.01,
                sync: 6.0,
            },
            Candidate {
                freq: 300.0,
                dt: 0.10,
                sync: 8.0,
            },
            Candidate {
                freq: 400.0,
                dt: 0.20,
                sync: 3.0,
            },
        ];

        let ordered = finalize_sync8_candidates(&mut candidate0, 4.0, 212.0, 3);

        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].freq, 210.0);
        assert_eq!(ordered[1].freq, 300.0);
        assert_eq!(ordered[2].freq, 102.5);
    }

    #[test]
    fn non_contest_quirk_gate_matches_wsjtx() {
        assert!(is_acceptable_unpacked_message(
            "CQ 001 IZ7MMG 549 2025",
            3,
            0
        ));
        assert!(!is_acceptable_unpacked_message(
            "TU; K1ABC W9XYZ 549 CA",
            3,
            0
        ));
        assert!(!is_acceptable_unpacked_message("K1ABC/R W9XYZ 73", 1, 0));
    }

    fn test_ap_options() -> Ft8bApOptions {
        Ft8bApOptions {
            enabled: true,
            cq_only: false,
            nqso_progress: 0,
            ncontest: 0,
            nfqso: 1500.0,
            nftx: 1500.0,
            napwid: 100.0,
            nzhsym: 50,
            ap_set: Ft8ApSet {
                apsym: [1; 58],
                aph10: [1; 10],
            },
            mycall: Some("K1ABC".to_string()),
            hiscall: Some("W9XYZ".to_string()),
        }
    }

    fn assert_ap_sign(workspace: &DecodeWorkspace, idx_1based: usize, sign: i8, apmag: f64) {
        let idx = idx_1based - 1;
        assert_eq!(workspace.apmask[idx], 1, "AP mask bit {idx_1based}");
        assert_eq!(
            workspace.llr[idx],
            if sign > 0 { apmag } else { -apmag },
            "AP LLR bit {idx_1based}"
        );
    }

    #[test]
    fn cq_ap_mask_matches_wsjtx_bit_positions() {
        let ap = test_ap_options();
        let apmag = 7.0;
        let mut workspace = create_decode_workspace();

        assert!(apply_wsjt_ap_mask(&mut workspace, &ap, 1, apmag, 1500.0));

        for (offset, bit) in MCQ.iter().enumerate() {
            let sign = if *bit == 0 { -1 } else { 1 };
            assert_ap_sign(&workspace, offset + 1, sign, apmag);
        }
        assert_ap_sign(&workspace, 75, -1, apmag);
        assert_ap_sign(&workspace, 76, -1, apmag);
        assert_ap_sign(&workspace, 77, 1, apmag);

        let masked = workspace.apmask.iter().filter(|&&bit| bit == 1).count();
        assert_eq!(masked, 32);
    }

    #[test]
    fn standard_ap_masks_match_wsjtx_mycall_and_dxcall_positions() {
        let ap = test_ap_options();
        let apmag = 6.0;

        let mut mycall_workspace = create_decode_workspace();
        assert!(apply_wsjt_ap_mask(
            &mut mycall_workspace,
            &ap,
            2,
            apmag,
            1500.0
        ));
        for idx in 1..=29 {
            assert_ap_sign(&mycall_workspace, idx, 1, apmag);
        }
        assert_ap_sign(&mycall_workspace, 75, -1, apmag);
        assert_ap_sign(&mycall_workspace, 76, -1, apmag);
        assert_ap_sign(&mycall_workspace, 77, 1, apmag);
        let masked = mycall_workspace
            .apmask
            .iter()
            .filter(|&&bit| bit == 1)
            .count();
        assert_eq!(masked, 32);

        let mut dxcall_workspace = create_decode_workspace();
        assert!(apply_wsjt_ap_mask(
            &mut dxcall_workspace,
            &ap,
            3,
            apmag,
            1500.0
        ));
        for idx in 1..=58 {
            assert_ap_sign(&dxcall_workspace, idx, 1, apmag);
        }
        assert_ap_sign(&dxcall_workspace, 75, -1, apmag);
        assert_ap_sign(&dxcall_workspace, 76, -1, apmag);
        assert_ap_sign(&dxcall_workspace, 77, 1, apmag);
        let masked = dxcall_workspace
            .apmask
            .iter()
            .filter(|&&bit| bit == 1)
            .count();
        assert_eq!(masked, 61);
    }

    #[test]
    fn tail_ap_masks_match_wsjtx_rrr_73_rr73_bits() {
        for (iaptype, tail) in [(4, MRRR), (5, M73), (6, MRR73)] {
            let ap = test_ap_options();
            let apmag = 5.5;
            let mut workspace = create_decode_workspace();

            assert!(apply_wsjt_ap_mask(
                &mut workspace,
                &ap,
                iaptype,
                apmag,
                1500.0
            ));

            for idx in 1..=58 {
                assert_ap_sign(&workspace, idx, 1, apmag);
            }
            for (offset, bit) in tail.iter().enumerate() {
                let sign = if *bit == 0 { -1 } else { 1 };
                assert_ap_sign(&workspace, 59 + offset, sign, apmag);
            }
            let masked = workspace.apmask.iter().filter(|&&bit| bit == 1).count();
            assert_eq!(masked, 77);
        }
    }

    #[test]
    fn ap_mask_gates_match_wsjtx_contest_and_call_requirements() {
        let apmag = 4.0;

        let mut fox_contest = test_ap_options();
        fox_contest.ncontest = 6;
        let mut workspace = create_decode_workspace();
        assert!(!apply_wsjt_ap_mask(
            &mut workspace,
            &fox_contest,
            1,
            apmag,
            1500.0
        ));
        assert!(workspace.apmask.iter().all(|&bit| bit == 0));

        let mut field_day = test_ap_options();
        field_day.ncontest = 7;
        let mut workspace = create_decode_workspace();
        assert!(!apply_wsjt_ap_mask(
            &mut workspace,
            &field_day,
            1,
            apmag,
            951.0
        ));
        assert!(workspace.apmask.iter().all(|&bit| bit == 0));

        let mut missing_dx = test_ap_options();
        missing_dx.ap_set.apsym[29] = 99;
        let mut workspace = create_decode_workspace();
        assert!(!apply_wsjt_ap_mask(
            &mut workspace,
            &missing_dx,
            3,
            apmag,
            1500.0
        ));
        assert!(workspace.apmask.iter().all(|&bit| bit == 0));
    }

    #[test]
    fn ap_mask_matrix_covers_wsjtx_contest_iaptype_shape() {
        let cases = [
            (0, 1, Some(32)),
            (0, 2, Some(32)),
            (0, 3, Some(61)),
            (0, 4, Some(77)),
            (0, 5, Some(77)),
            (0, 6, Some(77)),
            (1, 1, Some(32)),
            (1, 2, Some(32)),
            (1, 3, Some(61)),
            (1, 4, Some(77)),
            (1, 5, Some(77)),
            (1, 6, Some(77)),
            (2, 1, Some(32)),
            (2, 2, Some(34)),
            (2, 3, Some(61)),
            (2, 4, Some(77)),
            (2, 5, Some(77)),
            (2, 6, Some(77)),
            (3, 1, Some(32)),
            (3, 2, Some(31)),
            (3, 3, Some(62)),
            (3, 4, Some(77)),
            (3, 5, Some(77)),
            (3, 6, Some(77)),
            (4, 1, Some(32)),
            (4, 2, Some(31)),
            (4, 3, Some(59)),
            (4, 4, Some(77)),
            (4, 5, Some(77)),
            (4, 6, Some(77)),
            (5, 1, Some(32)),
            (5, 2, Some(32)),
            (5, 3, Some(61)),
            (5, 4, Some(77)),
            (5, 5, Some(77)),
            (5, 6, Some(77)),
            (6, 1, None),
            (6, 2, None),
            (6, 3, None),
            (6, 4, None),
            (6, 5, None),
            (6, 6, None),
            (7, 1, Some(32)),
            (7, 2, Some(44)),
            (7, 3, Some(61)),
            (7, 4, Some(44)),
            (7, 5, None),
            (7, 6, Some(77)),
            (8, 1, Some(32)),
            (8, 2, Some(32)),
            (8, 3, Some(61)),
            (8, 4, Some(77)),
            (8, 5, Some(77)),
            (8, 6, Some(77)),
        ];

        for (ncontest, iaptype, expected_count) in cases {
            let mut ap = test_ap_options();
            ap.ncontest = ncontest;
            let mut workspace = create_decode_workspace();
            let f1 = if ncontest == 7 { 900.0 } else { 1500.0 };
            let accepted = apply_wsjt_ap_mask(&mut workspace, &ap, iaptype, 3.0, f1);
            assert_eq!(
                accepted,
                expected_count.is_some(),
                "ncontest={ncontest} iaptype={iaptype}"
            );
            let count = workspace.apmask.iter().filter(|&&bit| bit == 1).count();
            assert_eq!(
                count,
                expected_count.unwrap_or(0),
                "ncontest={ncontest} iaptype={iaptype}"
            );
        }
    }
}
