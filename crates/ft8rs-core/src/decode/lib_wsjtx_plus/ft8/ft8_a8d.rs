//! WSJT-X FT8 a8 list decoder.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/ft8_a8d.f90`

use super::{ft8_downsample_from_cx, nint_wsjtx_real, twkfreq1, NFFT1_LONG, NFFT2, NN, NSPS};
use crate::decode::genft8::get_ft8_tones_from_77bits;
use crate::decode::lib_wsjtx_plus::compatibility::legacy_rr73;
use crate::decode::lib_wsjtx_plus::fft::{four2a_c2c, four2a_r2c};
use crate::decode::packjt77::{is_stdcall_suffix, pack77};
use std::sync::OnceLock;

const NWAVE: usize = NN * NSPS / 60;
const NZZ: usize = 3200;
const NFFT: usize = NZZ;
const NH: isize = (NFFT / 2) as isize;
const NMSGS: usize = 206;
const FSAMPLE: f32 = 200.0;
const NSPS_A8: usize = 32;
const NTAB: usize = 65536;

#[derive(Clone, Debug)]
pub(crate) struct Ft8A8dResult {
    pub(crate) msg: String,
    pub(crate) freq: f64,
    pub(crate) dt: f64,
    pub(crate) snr: f64,
    pub(crate) itone: [i32; 79],
}

pub(crate) fn ft8_a8d_result(
    dd: &[f64],
    mycall: &str,
    dxcall: &str,
    dxgrid: &str,
    f1a: f64,
    compatibility: bool,
) -> Option<Ft8A8dResult> {
    let mut xdt = 0.0f64;
    let mut fbest = 0.0f64;
    let mut xsnr = 0.0f64;
    let mut plog = 0.0f64;
    let mut msgbest = String::new();
    let mut itone_best = [0i32; 79];
    ft8_a8d(
        dd,
        mycall,
        dxcall,
        dxgrid,
        f1a,
        compatibility,
        &mut xdt,
        &mut fbest,
        &mut xsnr,
        &mut plog,
        &mut msgbest,
        &mut itone_best,
    );
    if msgbest.trim().is_empty() {
        None
    } else {
        Some(Ft8A8dResult {
            msg: msgbest.trim().to_string(),
            freq: fbest,
            dt: xdt,
            snr: xsnr,
            itone: itone_best,
        })
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ft8_a8d(
    dd: &[f64],
    mycall: &str,
    dxcall: &str,
    dxgrid: &str,
    f1a: f64,
    compatibility: bool,
    xdt: &mut f64,
    fbest_out: &mut f64,
    xsnr_out: &mut f64,
    plog_out: &mut f64,
    msgbest_out: &mut String,
    itone_best_out: &mut [i32; 79],
) {
    let _span = crate::decode::lib_wsjtx_plus::metrics::Span::new(
        crate::decode::lib_wsjtx_plus::metrics::Kind::A8,
    );
    *xdt = 0.0;
    *fbest_out = 0.0;
    *xsnr_out = 0.0;
    *plog_out = 0.0;
    msgbest_out.clear();
    itone_best_out.fill(0);

    let f1 = f1a as f32;
    let mut cd_re = vec![0.0f64; NZZ];
    let mut cd_im = vec![0.0f64; NZZ];
    downsample_dd(dd, f1 as f64, &mut cd_re, &mut cd_im);

    let fac = 1.0e-6f32;
    let fsd = 200.0f32;
    let dt = 1.0f32 / fsd;
    let df = 200.0f32 / NFFT as f32;
    let mut s = vec![0.0f32; NFFT + 1];
    let mut s0 = vec![0.0f32; NFFT + 1];
    let mut s1 = vec![0.0f32; NFFT + 1];

    let mut sbest = 0.0f32;
    let mut tbest = 0.0f32;
    let mut fbest = 0.0f32;
    let mut msgbest = String::new();
    let mut itone_best = [0i32; 79];
    let mut best_is_legacy = false;

    // WSJT-X 015b4894: retain both the suffix-aware standard hypotheses
    // and the former nonstandard-call hypotheses, in that order.
    let suffix_dx = is_stdcall_suffix(dxcall)
        && (dxcall.trim().ends_with("/R") || dxcall.trim().ends_with("/P"));
    let nrun = if suffix_dx {
        if is_stdcall_suffix(mycall) {
            411
        } else {
            207
        }
    } else {
        NMSGS
    };
    // Upstream 25a626af prepares templates before the lag search and keeps
    // worker scratch private. The classic caller uses one worker.
    let hypotheses: Vec<_> = (1..=nrun)
        .filter_map(|imsg| {
            let msg = getmsg(imsg, mycall, dxcall, dxgrid);
            if msg.trim().is_empty() {
                return None;
            }
            let msgbits = pack77(&msg);
            if msgbits.len() != 77 {
                return None;
            }
            let make = |msg, bits: &[u8], legacy| {
                let itone = get_ft8_tones_from_77bits(bits);
                let (cwave_re, cwave_im) = gen_ft8wave_a8(&itone);
                (msg, itone, cwave_re, cwave_im, legacy)
            };
            let legacy = if compatibility && msg.ends_with(" RR73") {
                legacy_rr73(&msgbits).map(|bits| make(msg.clone(), &bits, true))
            } else {
                None
            };
            Some((make(msg, &msgbits, false), legacy))
        })
        .flat_map(|(current, legacy)| std::iter::once(current).chain(legacy))
        .collect();
    if hypotheses.is_empty() {
        return;
    }
    let mut cd0_re = vec![0.0f64; NFFT];
    let mut cd0_im = vec![0.0f64; NFFT];
    let mut raw_spectrum = vec![0.0f32; NFFT + 1];

    for (msg, itone, cwave_re, cwave_im, is_legacy) in hypotheses {
        let mut spk = 0.0f32;
        let mut fpk = 0.0f32;
        let mut tpk = 0.0f32;
        let mut lagpk = 0isize;
        let mut lag1 = -200isize;
        let mut lag2 = 200isize;
        let mut lagstep = 4isize;

        for iter in 1..=2 {
            if iter == 2 {
                lag1 = lagpk - 8;
                lag2 = lagpk + 8;
                lagstep = 1;
            }
            let mut lag = lag1;
            while lag <= lag2 {
                cd0_re.fill(0.0);
                cd0_im.fill(0.0);
                let offset = lag + 100;
                let first = (-offset).max(0) as usize;
                let end = (NWAVE as isize - offset).min(NWAVE as isize).max(0) as usize;
                for i in first..end {
                    let j = (i as isize + offset) as usize;
                    let d_re = cd_re[j] as f32;
                    let d_im = cd_im[j] as f32;
                    let w_re = cwave_re[i];
                    let w_im = cwave_im[i];
                    cd0_re[i] = (d_re * w_re + d_im * w_im) as f64;
                    cd0_im[i] = (d_im * w_re - d_re * w_im) as f64;
                }

                four2a_c2c(&mut cd0_re, &mut cd0_im, -1);

                for i in 0..NFFT {
                    let mut j = i as isize;
                    if i as isize > NH {
                        j = i as isize - NFFT as isize;
                    }
                    let re = cd0_re[i] as f32;
                    let im = cd0_im[i] as f32;
                    raw_spectrum[s_index(j)] = fac * (re * re + im * im);
                }

                s[0] = raw_spectrum[0];
                s[NFFT] = raw_spectrum[NFFT];
                // Upstream uses an integer OR reduction so smoothing lanes
                // can vectorize without an early-exit dependency.
                let mut peak_mask = u32::from(s[0] > spk || s[NFFT] > spk);
                for i in 1..NFFT {
                    s[i] =
                        0.5 * raw_spectrum[i] + 0.25 * (raw_spectrum[i - 1] + raw_spectrum[i + 1]);
                    peak_mask |= u32::from(s[i] > spk);
                }
                // Preserve ascending-bin and earliest-hypothesis ties. Copy
                // a winning spectrum once per lag, not once per new peak bin.
                if peak_mask != 0 {
                    for j in -NH..=NH {
                        let sj = s[s_index(j)];
                        if sj > spk {
                            spk = sj;
                            fpk = j as f32 * df + f1;
                            lagpk = lag;
                            tpk = lag as f32 * dt;
                        }
                    }
                    s0.copy_from_slice(&s);
                }

                lag += lagstep;
            }
        }

        if spk > sbest {
            sbest = spk;
            fbest = fpk;
            tbest = tpk;
            msgbest = msg;
            itone_best = itone;
            best_is_legacy = is_legacy;
            s1.copy_from_slice(&s0);
        }
    }

    if compatibility && !best_is_legacy {
        return;
    }
    let mut a = [0.0f64; 5];
    a[0] = (f1 - fbest) as f64;
    if a[0].abs() > 5.0 {
        msgbest.clear();
        return;
    }
    let (cd_re, cd_im) = twkfreq1(&cd_re, &cd_im, NZZ, FSAMPLE as f64, &a);

    *xdt = tbest as f64;
    let ave = (sum_range(&s1, -200, -100) + sum_range(&s1, 100, 200)) / 202.0f32;
    for value in &mut s1 {
        *value = *value / ave - 1.0;
    }
    let mut s1pk = f32::NEG_INFINITY;
    for i in -32..=32 {
        s1pk = s1pk.max(s1[s_index(i)]);
    }
    let mut sig = 0.0f32;
    let mut nsig = 0usize;
    for i in -32..=32 {
        let value = s1[s_index(i)];
        if value < 0.5 * s1pk {
            continue;
        }
        sig += value;
        nsig += 1;
    }
    if nsig == 0 {
        msgbest.clear();
        return;
    }
    sig /= nsig as f32;
    let mut xsnr = db(sig) - 35.0;
    if xsnr < -30.0 {
        xsnr = -30.0;
    }
    *xsnr_out = xsnr as f64;

    if !msgbest.trim().is_empty() {
        let mut plog = 0.0f32;
        let mut nhard = 0usize;
        let mut nsum = 0usize;
        let mut sum_sync = 0.0f32;
        let mut sum_sig = 0.0f32;
        let mut sum_big = 0.0f32;
        let mut csymb_re = [0.0f64; 32];
        let mut csymb_im = [0.0f64; 32];
        let mut s8 = [0.0f32; 8];

        for k in 1..=NN {
            let i0 = 32 * (k as isize - 1) + nint_wsjtx_real((tbest + 0.5) / 0.005);
            csymb_re.fill(0.0);
            csymb_im.fill(0.0);
            for i in 0..32 {
                let idx = i0 + i as isize;
                if idx >= 0 && idx <= (NZZ - 1) as isize {
                    csymb_re[i] = cd_re[idx as usize];
                    csymb_im[i] = cd_im[idx as usize];
                }
            }
            four2a_c2c(&mut csymb_re, &mut csymb_im, -1);
            for i in 0..8 {
                let re = csymb_re[i] as f32;
                let im = csymb_im[i] as f32;
                s8[i] = re * re + im * im;
            }
            let s8sum: f32 = s8.iter().sum();
            let tone = itone_best[k - 1] as usize;
            if s8sum > 0.0 {
                let p = s8[tone] / s8sum;
                plog += p.ln();
                nsum += 1;
            }
            let ipk = maxloc0(&s8);
            if ipk != tone {
                nhard += 1;
            }
            if k <= 7 || (37..=43).contains(&k) || k >= 73 {
                sum_sync += s8[tone];
            } else {
                sum_sig += s8[tone];
            }
            sum_big += s8[ipk];
        }

        if nsum < NN {
            plog += (NN - nsum) as f32 * 0.125f32.ln();
        }
        let sigobig = (sum_sync + sum_sig) / sum_big;
        if nhard > 54 || plog < -159.0 || sigobig < 0.71 {
            msgbest.clear();
        }
        if !msgbest.trim().is_empty() {
            *fbest_out = fbest as f64;
            *plog_out = plog as f64;
            *msgbest_out = msgbest;
            *itone_best_out = itone_best;
        }
    }
}

fn downsample_dd(dd: &[f64], f1: f64, cd_re: &mut [f64], cd_im: &mut [f64]) {
    let mut cx_re = dd.to_vec();
    cx_re.resize(NFFT1_LONG, 0.0);
    let mut cx_im = vec![0.0f64; NFFT1_LONG];
    four2a_r2c(&mut cx_re, &mut cx_im);
    let mut shift_re = vec![0.0f64; NFFT2];
    let mut shift_im = vec![0.0f64; NFFT2];
    ft8_downsample_from_cx(
        &cx_re,
        &cx_im,
        f1,
        cd_re,
        cd_im,
        &mut shift_re,
        &mut shift_im,
    );
}

fn getmsg(i: usize, mycall: &str, hiscall: &str, hisgrid: &str) -> String {
    let mycall = mycall.trim();
    let hiscall = hiscall.trim();
    let hisgrid = hisgrid.trim();
    let my_std = is_stdcall_suffix(mycall);
    let his_std = is_stdcall_suffix(hiscall);
    if i >= 207 {
        if !his_std || !(hiscall.ends_with("/R") || hiscall.ends_with("/P")) {
            return String::new();
        }
        return match i {
            207 => format!("CQ {hiscall}"),
            208..=211 if my_std => {
                let tail = ["", " RRR", " RR73", " 73"][i - 208];
                format!("<{mycall}> {hiscall}{tail}")
            }
            212..=411 if my_std => {
                let j = i - 205;
                let snr = -50 + (j as isize - 7) / 2;
                if snr.abs() > 30 {
                    String::new()
                } else {
                    let roger = if j & 1 == 0 { "R" } else { "" };
                    format!("{mycall} <{hiscall}> {roger}{snr:+03}")
                }
            }
            _ => String::new(),
        };
    }

    let mut isnr = 0isize;
    let mut msg = format!("{mycall} {hiscall}");
    if !my_std {
        if i == 1 || i >= 6 {
            msg = format!("<{mycall}> {hiscall}");
        }
        if (2..=4).contains(&i) {
            msg = format!("{mycall} <{hiscall}>");
        }
    } else if !his_std {
        if i <= 4 || i == 6 {
            msg = format!("<{mycall}> {hiscall}");
        }
        if i >= 7 {
            msg = format!("{mycall} <{hiscall}>");
        }
    }

    if i == 2 {
        msg = format!("{} RRR", msg.trim_end());
    }
    if i == 3 {
        msg = format!("{} RR73", msg.trim_end());
    }
    if i == 4 {
        msg = format!("{} 73", msg.trim_end());
    }
    if i == 5 {
        if his_std {
            msg = format!(
                "CQ {hiscall} {}",
                hisgrid.chars().take(4).collect::<String>()
            );
        }
        if !his_std {
            msg = format!("CQ {hiscall}");
        }
    }
    if i == 6 && his_std {
        msg = format!(
            "{} {}",
            msg.trim_end(),
            hisgrid.chars().take(4).collect::<String>()
        );
    }
    if (7..=206).contains(&i) {
        isnr = -50 + (i as isize - 7) / 2;
        let abs = isnr.unsigned_abs();
        let rpt = if i & 1 == 1 {
            if isnr >= 0 {
                format!("+{abs:02}")
            } else {
                format!("-{abs:02}")
            }
        } else if isnr >= 0 {
            format!("R+{abs:02}")
        } else {
            format!("R-{abs:02}")
        };
        msg = format!("{} {rpt}", msg.trim_end());
    }

    if isnr.abs() > 30 {
        msg.clear();
    }
    msg
}

fn gen_ft8wave_a8(itone: &[i32; 79]) -> (Vec<f32>, Vec<f32>) {
    let twopi = 8.0f32 * 1.0f32.atan();
    let dt = 1.0f32 / FSAMPLE;
    let f0 = 0.0f32;
    let hmod = 1.0f32;
    let dphi_peak = twopi * hmod / NSPS_A8 as f32;
    let pulse = pulse_a8();
    let ctab = ctab_a8();
    let mut dphi = vec![0.0f32; (NN + 2) * NSPS_A8];

    for j in 1..=NN {
        let ib = (j - 1) * NSPS_A8;
        for i in 0..3 * NSPS_A8 {
            dphi[ib + i] += dphi_peak * pulse[i] * itone[j - 1] as f32;
        }
    }
    for i in 0..2 * NSPS_A8 {
        dphi[i] += dphi_peak * itone[0] as f32 * pulse[NSPS_A8 + i];
    }
    for i in 0..2 * NSPS_A8 {
        dphi[NN * NSPS_A8 + i] += dphi_peak * itone[NN - 1] as f32 * pulse[i];
    }

    let mut cwave_re = vec![0.0f32; NWAVE];
    let mut cwave_im = vec![0.0f32; NWAVE];
    let mut phi = 0.0f32;
    for value in &mut dphi {
        *value += twopi * f0 * dt;
    }
    for (k, &phase_step) in dphi[NSPS_A8..NSPS_A8 + NWAVE].iter().enumerate() {
        let idx = (phi * NTAB as f32 / twopi) as usize;
        cwave_re[k] = ctab[idx].0;
        cwave_im[k] = ctab[idx].1;
        // Upstream emit_ft8_waveform: these finite nonnegative increments
        // are below half a turn, so at most one wrap is needed.
        let phase_sum = phi + phase_step;
        phi = if phase_sum >= twopi {
            phase_sum - twopi
        } else {
            phase_sum
        };
    }

    let nramp = nint_wsjtx_real(NSPS_A8 as f32 / 8.0) as usize;
    for i in 0..nramp {
        let env = (1.0 - (twopi * i as f32 / (2.0 * nramp as f32)).cos()) / 2.0;
        cwave_re[i] *= env;
        cwave_im[i] *= env;
    }
    let k1 = NN * NSPS_A8 - nramp;
    for i in 0..nramp {
        let env = (1.0 + (twopi * i as f32 / (2.0 * nramp as f32)).cos()) / 2.0;
        cwave_re[k1 + i] *= env;
        cwave_im[k1 + i] *= env;
    }

    (cwave_re, cwave_im)
}

fn pulse_a8() -> &'static [f32] {
    static PULSE: OnceLock<Vec<f32>> = OnceLock::new();
    PULSE.get_or_init(|| {
        let bt = 2.0f32;
        let mut pulse = vec![0.0f32; 3 * NSPS_A8];
        for i in 1..=3 * NSPS_A8 {
            let tt = (i as f32 - 1.5 * NSPS_A8 as f32) / NSPS_A8 as f32;
            pulse[i - 1] = gfsk_pulse(bt, tt);
        }
        pulse
    })
}

fn ctab_a8() -> &'static [(f32, f32)] {
    static CTAB: OnceLock<Vec<(f32, f32)>> = OnceLock::new();
    CTAB.get_or_init(|| {
        let twopi = 8.0f32 * 1.0f32.atan();
        let mut ctab = vec![(0.0f32, 0.0f32); NTAB];
        for (i, slot) in ctab.iter_mut().enumerate() {
            let phi = i as f32 * twopi / NTAB as f32;
            *slot = (phi.cos(), phi.sin());
        }
        ctab
    })
}

fn gfsk_pulse(bt: f32, tt: f32) -> f32 {
    let c = std::f32::consts::PI * (2.0f32 / std::f32::consts::LN_2).sqrt();
    0.5 * (erf_approx(c * bt * (tt + 0.5)) - erf_approx(c * bt * (tt - 0.5)))
}

#[expect(
    clippy::excessive_precision,
    reason = "Preserve the reference error-function coefficients and f32 rounding"
)]
fn erf_approx(x: f32) -> f32 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t)
            * (-ax * ax).exp();
    sign * y
}

fn s_index(j: isize) -> usize {
    (j + NH) as usize
}

fn sum_range(s: &[f32], lo: isize, hi: isize) -> f32 {
    let mut sum = 0.0f32;
    for i in lo..=hi {
        sum += s[s_index(i)];
    }
    sum
}

fn db(x: f32) -> f32 {
    10.0 * x.log10()
}

fn maxloc0(values: &[f32; 8]) -> usize {
    let mut idx = 0usize;
    let mut best = values[0];
    for (i, &value) in values.iter().enumerate().skip(1) {
        if value > best {
            best = value;
            idx = i;
        }
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::getmsg;

    #[test]
    fn a8_hypotheses_match_all_current_native_suffix_and_legacy_candidates() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/decoder/data/ft8/upstream/wsjtx-3.3/a8.tsv"),
        )
        .unwrap();
        let mut count = 0;
        for line in source.lines().filter(|line| !line.starts_with('#')) {
            let row: Vec<_> = line.split('\t').collect();
            assert_eq!(row.len(), 5);
            let index = row[3].parse().unwrap();
            assert_eq!(
                getmsg(index, row[0], row[1], row[2]),
                row[4],
                "{} {} {index}",
                row[0],
                row[1]
            );
            count += 1;
        }
        assert_eq!(count, 2466);
    }

    #[test]
    fn a8_getmsg_matches_wsjtx_shapes() {
        assert_eq!(getmsg(1, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG");
        assert_eq!(getmsg(2, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG RRR");
        assert_eq!(getmsg(3, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG RR73");
        assert_eq!(getmsg(4, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG 73");
        assert_eq!(getmsg(5, "W3SZ", "DL3WDG", "JN68"), "CQ DL3WDG JN68");
        assert_eq!(getmsg(6, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG JN68");
        assert_eq!(getmsg(47, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG -30");
        assert_eq!(getmsg(48, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG R-30");
        assert_eq!(getmsg(49, "W3SZ", "DL3WDG", "JN68"), "W3SZ DL3WDG -29");
    }

    #[test]
    fn a8_getmsg_rejects_reports_outside_wsjtx_range() {
        assert_eq!(getmsg(7, "W3SZ", "DL3WDG", "JN68"), "");
        assert_eq!(getmsg(206, "W3SZ", "DL3WDG", "JN68"), "");
    }
}
