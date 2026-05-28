//! ft8_a7d — WSJT-X AP decode: brute-force over 206 message variants
//!
//! Complete port of wsjtx/lib/ft8/ft8_a7.f90:ft8_a7d
//!
//! Algorithm:
//!   1. Downsample dd0 to baseband at f1
//!   2. Time alignment ±10 → ibest
//!   3. Frequency alignment ±2.5Hz → delfbest
//!   4. twkfreq1 frequency correction
//!   5. Second downsample with refined f1
//!   6. Time refinement ±4
//!   7. Extract soft symbols (79×8)
//!   8. Build 4 bit-metric sets → normalize → LLRs
//!   9. Brute-force 206 message variants → Hamming distance → best match
//!  10. Validate: dmin<100 AND dmin2/dmin>1.3

use super::decode::{
    build_costas_sync_templates, normalize_bmet, COSTAS_BLOCKS, COSTAS_SYMBOL_LEN, DOWNSAMPLE_BAUD,
    DOWNSAMPLE_DF, DOWNSAMPLE_FAC, DT2, FS2, NFFT1_LONG, NFFT2, NN, NP2, TAPER_SIZE, TWO_PI,
};
use crate::ft8::pack_jt77::{is_stdcall, pack77};
use crate::ft8::protocol::G_HEX;

const ICOS7: [usize; 7] = [3, 1, 4, 0, 6, 5, 2];

/// Result of AP decode
#[derive(Clone, Debug)]
pub struct ApDecodeResult {
    pub msg: String,
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub nharderrors: i32,
}

/// WSJT-X `ft8_downsample` state for one unchanged `dd0` buffer.
///
/// The Fortran routine keeps the long `cx` FFT in `save` storage and refreshes
/// it only when `newdat=.true.`. AP decode sets `newdat=.true.` once before the
/// previous-slot loop, so all `ft8_a7d` calls for the slot share this spectrum.
pub(crate) struct ApDownsampleCache {
    cx_re: Vec<f64>,
    cx_im: Vec<f64>,
}

impl ApDownsampleCache {
    pub(crate) fn new(dd0: &[f64]) -> Self {
        let mut cx_re = dd0.to_vec();
        cx_re.resize(NFFT1_LONG, 0.0);
        let mut cx_im = vec![0.0f64; NFFT1_LONG];
        crate::util::four2a_r2c(&mut cx_re, &mut cx_im);
        Self { cx_re, cx_im }
    }
}

/// WSJT-X ft8_a7d: AP decode at known (call_1, call_2, grid4, xdt, f1) position.
///
/// dd0: 15s audio at 12kHz
/// call_1, call_2: callsigns from previous slot decode
/// grid4: grid square, report, or empty
/// xdt: time offset relative to 0.5s center
/// f1: frequency in Hz
/// xbase: noise baseline estimate at f1
pub fn ft8_a7d(
    dd0: &[f64],
    call_1: &str,
    call_2: &str,
    grid4: &str,
    xdt: f64,
    f1: f64,
    xbase: f64,
) -> Option<ApDecodeResult> {
    let downsample_cache = ApDownsampleCache::new(dd0);
    ft8_a7d_with_downsample_cache(&downsample_cache, call_1, call_2, grid4, xdt, f1, xbase)
}

pub(crate) fn ft8_a7d_with_downsample_cache(
    downsample_cache: &ApDownsampleCache,
    call_1: &str,
    call_2: &str,
    grid4: &str,
    xdt: f64,
    f1: f64,
    xbase: f64,
) -> Option<ApDecodeResult> {
    let one = build_one_table();
    let costas = build_costas_sync_templates();
    let taper_data = build_taper();

    let std_1 = is_stdcall(call_1) || is_cq_call_1(call_1);
    let std_2 = is_stdcall(call_2);

    let mut delfbest: f64 = 0.0;
    let mut ibest: isize = 0;

    // ── First downsample at f1 ──
    let (cd0_re, cd0_im) = ap_downsample(downsample_cache, f1, &taper_data);

    // ── Time alignment ±10 ──
    let i0 = nint_wsjtx_f32((xdt + 0.5) * FS2);
    let mut smax = 0.0f64;
    for idt in (i0 - 10)..=(i0 + 10) {
        let sync = ap_sync8d(&cd0_re, &cd0_im, idt, &costas.re, &costas.im);
        if sync > smax {
            smax = sync;
            ibest = idt;
        }
    }

    // ── Frequency alignment ±2.5Hz ──
    smax = 0.0;
    for ifr in -5..=5 {
        let delf = ifr as f64 * 0.5;
        let dphi = TWO_PI * delf * DT2;
        let (ctwk_re, ctwk_im) = build_ctwk(dphi);
        let sync = ap_sync8d_twk(
            &cd0_re, &cd0_im, ibest, &costas.re, &costas.im, &ctwk_re, &ctwk_im,
        );
        if sync > smax {
            smax = sync;
            delfbest = delf;
        }
    }

    // ── twkfreq1: frequency correction ──
    let mut a = [0.0f64; 5];
    a[0] = -delfbest;
    let _ = twkfreq1(&cd0_re, &cd0_im, NP2, FS2, &a);
    let f1_refined = f1 + delfbest;

    // ── Second downsample with refined f1 ──
    let (cd0_re, cd0_im) = ap_downsample(downsample_cache, f1 + delfbest, &taper_data);

    // ── Time refinement ±4 ──
    let mut ss = [0.0f64; 9];
    for idt in -4..=4 {
        let sync = ap_sync8d(&cd0_re, &cd0_im, ibest + idt, &costas.re, &costas.im);
        ss[(idt + 4) as usize] = sync;
    }
    let mut idx = 0usize;
    let mut smax = ss[0];
    for (i, sync) in ss.iter().enumerate().skip(1) {
        if *sync > smax {
            smax = *sync;
            idx = i;
        }
    }
    ibest = idx as isize - 4 + ibest;
    let xdt_refined = (ibest as f64 - 1.0) * DT2 - 0.5;

    // ── Extract soft symbols: 79 symbols × 8 tones ──
    // cs[tone][symbol], s8[tone][symbol]  (1-indexed for symbol, 0-indexed for tone)
    let mut cs_re = [[0.0f64; 80]; 8];
    let mut cs_im = [[0.0f64; 80]; 8];
    let mut s8 = [[0.0f64; 80]; 8];

    let mut symb_re = [0.0f64; 32];
    let mut symb_im = [0.0f64; 32];
    for k in 1..=NN {
        let i1 = ibest + (k as isize - 1) * 32;
        if i1 >= 0 && i1 + 31 <= NP2 as isize - 1 {
            let start = i1 as usize;
            for j in 0..32 {
                symb_re[j] = cd0_re[start + j];
                symb_im[j] = cd0_im[start + j];
            }
        } else {
            symb_re.fill(0.0);
            symb_im.fill(0.0);
        }
        // 32-point FFT
        fft32(&mut symb_re, &mut symb_im);
        for tone in 0..8 {
            let sym_re = symb_re[tone] as f32;
            let sym_im = symb_im[tone] as f32;
            cs_re[tone][k] = (sym_re / 1000.0) as f64;
            cs_im[tone][k] = (sym_im / 1000.0) as f64;
            s8[tone][k] = ap_wsjtx_cabs(sym_re, sym_im) as f64;
        }
    }

    // ── Build bit metrics ──
    let mut bmeta = [0.0f64; 174];
    let mut bmetb = [0.0f64; 174];
    let mut bmetc = [0.0f64; 174];
    let mut bmetd = [0.0f64; 174];

    let mut s2 = [0.0f64; 512];

    for nsym in 1..=3usize {
        let nt = 1 << (3 * nsym); // 2^(3*nsym)
        for ihalf in 1..=2 {
            let ks_base = if ihalf == 1 { 7 } else { 43 };
            let mut k = 1usize;
            while k <= 29 {
                let ks = ks_base + k;

                // Compute s2[0..nt-1]
                for i in 0..nt {
                    let i1_val = i / 64;
                    let i2_val = (i & 63) / 8;
                    let i3_val = i & 7;
                    s2[i] = if nsym == 1 {
                        let t = crate::ft8::constants::GRAY_MAP[i3_val] as usize;
                        ap_wsjtx_cabs(cs_re[t][ks] as f32, cs_im[t][ks] as f32) as f64
                    } else if nsym == 2 {
                        let t2 = crate::ft8::constants::GRAY_MAP[i2_val] as usize;
                        let t3 = crate::ft8::constants::GRAY_MAP[i3_val] as usize;
                        let re = cs_re[t2][ks] as f32 + cs_re[t3][ks + 1] as f32;
                        let im = cs_im[t2][ks] as f32 + cs_im[t3][ks + 1] as f32;
                        ap_wsjtx_cabs(re, im) as f64
                    } else {
                        let t1 = crate::ft8::constants::GRAY_MAP[i1_val] as usize;
                        let t2 = crate::ft8::constants::GRAY_MAP[i2_val] as usize;
                        let t3 = crate::ft8::constants::GRAY_MAP[i3_val] as usize;
                        let re = cs_re[t1][ks] as f32
                            + cs_re[t2][ks + 1] as f32
                            + cs_re[t3][ks + 2] as f32;
                        let im = cs_im[t1][ks] as f32
                            + cs_im[t2][ks + 1] as f32
                            + cs_im[t3][ks + 2] as f32;
                        ap_wsjtx_cabs(re, im) as f64
                    };
                }

                let i32 = 1 + (k - 1) * 3 + (ihalf - 1) * 87;
                let ibmax = if nsym == 1 {
                    2
                } else if nsym == 2 {
                    5
                } else {
                    8
                };

                for ib in 0..=ibmax {
                    let idx = i32 + ib;
                    if idx > 174 {
                        continue;
                    }

                    let bit_idx = ibmax - ib;
                    let mut max_ones = f64::NEG_INFINITY;
                    let mut max_zeros = f64::NEG_INFINITY;
                    for i in 0..nt {
                        if one[i][bit_idx] {
                            if s2[i] > max_ones {
                                max_ones = s2[i];
                            }
                        } else {
                            if s2[i] > max_zeros {
                                max_zeros = s2[i];
                            }
                        }
                    }
                    let bm = ((max_ones as f32) - (max_zeros as f32)) as f64;

                    if nsym == 1 {
                        bmeta[idx - 1] = bm;
                        let den = (max_ones as f32).max(max_zeros as f32);
                        bmetd[idx - 1] = if den > 0.0 {
                            ((bm as f32) / den) as f64
                        } else {
                            0.0
                        };
                    } else if nsym == 2 {
                        bmetb[idx - 1] = bm;
                    } else {
                        bmetc[idx - 1] = bm;
                    }
                }

                k += nsym;
            }
        }
    }

    // Normalize bit metrics
    normalize_bmet(&mut bmeta);
    normalize_bmet(&mut bmetb);
    normalize_bmet(&mut bmetc);
    normalize_bmet(&mut bmetd);

    let scalefac = 2.83f32;
    let llra: [f64; 174] = core::array::from_fn(|i| (scalefac * bmeta[i] as f32) as f64);
    let llrb: [f64; 174] = core::array::from_fn(|i| (scalefac * bmetb[i] as f32) as f64);
    let llrc: [f64; 174] = core::array::from_fn(|i| (scalefac * bmetc[i] as f32) as f64);
    let llrd: [f64; 174] = core::array::from_fn(|i| (scalefac * bmetd[i] as f32) as f64);

    // ── Brute-force 206 message variants ──
    let mut dmin = 1e30f64;
    let mut nharderrors: i32 = -1;
    let mut msgbest = String::new();
    let mut pbest = 0.0f64;
    let mut dmm = [1e30f64; 207]; // 1-indexed
    let mut best_imsg = 1; // track which imsg gave dmin

    for imsg in 1..=206 {
        let msg = build_ap_message(call_1, call_2, grid4, std_1, std_2, imsg);

        let msg77 = pack77(&msg);
        if msg77.len() != 77 {
            continue;
        }

        // WSJT-X genft8 does: pack77 → unpack77 → msgsent (normalized form)
        // We must use msgsent as msgbest, not raw msg
        let msgsent = crate::ft8::unpack_jt77::unpack77(&msg77, None);
        if msgsent.is_none() {
            continue;
        }

        let cw = codeword_174_91(&msg77);
        if cw.len() != 174 {
            continue;
        }

        let itone = tones_from_codeword(&cw);

        // Signal power
        let mut pow = 0.0f64;
        for i in 0..NN {
            let t = itone[i] as usize;
            pow += s8[t][i + 1] * s8[t][i + 1];
        }

        // Hamming distance
        let da = hamming_dist(&cw, &llra);
        let db = hamming_dist(&cw, &llrb);
        let dc = hamming_dist(&cw, &llrc);
        let dd_val = hamming_dist(&cw, &llrd);

        let dm = da.min(db).min(dc).min(dd_val);
        dmm[imsg] = dm;

        if dm < dmin {
            dmin = dm;
            // Use the unpacked message (WSJT-X genft8 returns msgsent, not the raw msg)
            msgbest = msgsent.clone().unwrap_or(msg);
            pbest = pow;
            best_imsg = imsg;

            let best_llr = if dm == da {
                &llra
            } else if dm == db {
                &llrb
            } else if dm == dc {
                &llrc
            } else {
                &llrd
            };
            nharderrors = count_hard_errors(&cw, best_llr) as i32;
        }
    }

    // Second minimum (exclude the best match) — matching WSJT-X exactly
    let mut dmin2 = 1e30f64;
    for imsg in 1..=206 {
        if imsg != best_imsg && dmm[imsg] < dmin2 {
            dmin2 = dmm[imsg];
        }
    }

    // SNR — WSJT-X ft8_a7.f90: xsnr = max(-25, db(pbest/xbase/3e6 - 1) - 27).
    // AP decode s8 has WSJT-X scale (no /1000), so 3e6 divisor is correct.
    let xsnr = {
        let arg = pbest / xbase / 3e6 - 1.0;
        if arg > 0.0 {
            (-25.0f64).max(10.0 * arg.log10() - 27.0)
        } else {
            -25.0
        }
    };

    // Validation
    if dmin > 100.0 || dmin2 / dmin < 1.3 {
        return None;
    }
    if msgbest.starts_with("CQ ") && std_2 && grid4.trim().is_empty() {
        return None;
    }
    if msgbest.starts_with("QU1RK ") {
        return None;
    }

    Some(ApDecodeResult {
        msg: msgbest,
        freq: f1_refined,
        dt: xdt_refined,
        snr: xsnr,
        nharderrors,
    })
}

fn ap_wsjtx_cabs(re: f32, im: f32) -> f32 {
    (re * re + im * im).sqrt()
}

/// Build imsg-th message variant. Matches the ft8_a7.f90 imsg loop.
fn build_ap_message(
    call_1: &str,
    call_2: &str,
    grid4: &str,
    std_1: bool,
    std_2: bool,
    imsg: usize,
) -> String {
    let i = imsg;

    // imsg=1: call_1 call_2 (base)
    // imsg=2: call_1 call_2 RRR
    // imsg=3: call_1 call_2 RR73
    // imsg=4: call_1 call_2 73
    // imsg=5: CQ call_2 [grid4]  (or call_1 call_2 if call_1 has _)
    // imsg=6: call_1 call_2 grid4
    // imsg>=7: SNR reports

    let base = format!("{} {}", call_1.trim(), call_2.trim());

    if is_cq_call_1(call_1) && i != 5 {
        return format!("QU1RK {}", call_2.trim());
    }

    let msg = if !std_1 {
        if i == 1 || i >= 6 {
            format!("<{}> {}", call_1.trim(), call_2.trim())
        } else {
            format!("{} <{}>", call_1.trim(), call_2.trim())
        }
    } else if !std_2 {
        if i <= 4 || i == 6 {
            format!("<{}> {}", call_1.trim(), call_2.trim())
        } else {
            format!("{} <{}>", call_1.trim(), call_2.trim())
        }
    } else {
        base.clone()
    };

    match i {
        1 => msg,
        2 => format!("{} RRR", msg.trim_end()),
        3 => format!("{} RR73", msg.trim_end()),
        4 => format!("{} 73", msg.trim_end()),
        5 => {
            if std_2 {
                let mut m = format!("CQ {}", call_2.trim());
                if call_1.chars().nth(2) == Some('_') {
                    m = format!("{} {}", call_1.trim(), call_2.trim());
                }
                if grid4.trim() != "RR73" && !grid4.trim().is_empty() {
                    m = format!("{} {}", m, grid4.trim());
                }
                m
            } else {
                format!("CQ {}", call_2.trim())
            }
        }
        6 => {
            if std_2 {
                format!("{} {}", msg.trim_end(), grid4.trim())
            } else {
                msg
            }
        }
        _ if i >= 7 => {
            let isnr = -50isize + ((i as isize) - 7) / 2;
            // WSJT-X format: {+,-}NN (always 2-digit with sign, 3 chars total)
            // For imsg odd: "+NN" or "-NN"
            // For imsg even: "R+NN" or "R-NN"
            let abs_val = isnr.abs() as usize;
            let report = if i % 2 == 1 {
                // SNR report: "+NN" or "-NN"
                if isnr >= 0 {
                    format!("+{:02}", abs_val)
                } else {
                    format!("-{:02}", abs_val)
                }
            } else {
                // R report: "R+NN" or "R-NN"
                if isnr >= 0 {
                    format!("R+{:02}", abs_val)
                } else {
                    format!("R-{:02}", abs_val)
                }
            };
            format!("{} {}", msg.trim_end(), report)
        }
        _ => msg,
    }
}

fn is_cq_call_1(call_1: &str) -> bool {
    let c = call_1.trim_end();
    c == "CQ" || c.starts_with("CQ ")
}

fn codeword_174_91(msg77: &[u8]) -> Vec<u8> {
    let g = generate_ldpc_g_matrix();
    let poly = 0x2757u16;
    let mut crc: u16 = 0;

    for bit_idx in 0..96 {
        let next_bit = if bit_idx < 77 { msg77[bit_idx] } else { 0 };
        if (crc & 0x2000) != 0 {
            crc = ((crc << 1) | next_bit as u16) ^ poly;
        } else {
            crc = (crc << 1) | next_bit as u16;
        }
        crc &= 0x3fff;
    }

    let mut msg91 = msg77.to_vec();
    for i in 0..14 {
        msg91.push(((crc >> (13 - i)) & 1) as u8);
    }

    let mut codeword = msg91.clone();
    for row in g.iter().take(83) {
        let mut sum = 0;
        for j in 0..91 {
            sum += msg91[j] * row[j];
        }
        codeword.push(sum % 2);
    }
    codeword
}

fn generate_ldpc_g_matrix() -> Vec<Vec<u8>> {
    let k = 91;
    let m = 83;
    let mut gen = vec![vec![0u8; k]; m];

    for i in 0..m {
        let hex_str = G_HEX[i];
        for j in 0..23 {
            let byte = hex_str.as_bytes()[j];
            let val = u8::from_str_radix(&format!("{}", byte as char), 16).unwrap_or(0);
            let limit = if j == 22 { 3 } else { 4 };
            for jj in 1..=limit {
                let col = j * 4 + jj - 1;
                if (val & (1 << (4 - jj))) != 0 {
                    gen[i][col] = 1;
                }
            }
        }
    }
    gen
}

#[cfg(test)]
mod tests {
    use super::build_ap_message;

    #[test]
    fn ap_message_imsg5_nonstandard_calls_matches_wsjtx_cq_override() {
        let msg = build_ap_message("EA5/DH0YAH", "RK4FF/P", "JN00", false, false, 5);
        assert_eq!(msg, "CQ RK4FF/P");
    }

    #[test]
    fn ap_message_imsg5_standard_call_preserves_grid_override() {
        let msg = build_ap_message("K1ABC", "W9XYZ", "FN42", true, true, 5);
        assert_eq!(msg, "CQ W9XYZ FN42");
    }

    #[test]
    fn ap_message_compact_cq_call_matches_fortran_padded_cq() {
        let msg = build_ap_message("CQ", "D1DX", "KN87", true, true, 1);
        assert_eq!(msg, "QU1RK D1DX");
        let msg = build_ap_message("CQ", "D1DX", "KN87", true, true, 5);
        assert_eq!(msg, "CQ D1DX KN87");
    }
}

/// Hamming-weighted distance: sum of |llr[i]| where decoded bit != codeword bit
fn hamming_dist(cw: &[u8], llr: &[f64]) -> f64 {
    let mut dist = 0.0f64;
    for i in 0..174 {
        let hdec_bit = if llr[i] >= 0.0 { 1u8 } else { 0u8 };
        if hdec_bit != cw[i] {
            dist += llr[i].abs();
        }
    }
    dist
}

/// Count hard errors: (2*cw[i]-1)*llr[i] < 0
fn count_hard_errors(cw: &[u8], llr: &[f64]) -> usize {
    let mut count = 0;
    for i in 0..174 {
        if (2.0 * (cw[i] as f64) - 1.0) * llr[i] < 0.0 {
            count += 1;
        }
    }
    count
}

/// Decode 174-bit codeword to 79 tones (genft8 entry get_ft8_tones_from_77bits).
fn tones_from_codeword(cw: &[u8]) -> [i32; 79] {
    let mut itone = [0i32; 79];
    for i in 0..7 {
        itone[i] = ICOS7[i] as i32;
        itone[36 + i] = ICOS7[i] as i32;
        itone[72 + i] = ICOS7[i] as i32;
    }
    let mut k = 7;
    for j in 1..=58 {
        let idx = 3 * (j - 1);
        if j == 30 {
            k += 7;
        }
        let bits = (cw[idx] as usize) * 4 + (cw[idx + 1] as usize) * 2 + (cw[idx + 2] as usize);
        itone[k] = crate::ft8::constants::GRAY_MAP[bits] as i32;
        k += 1;
    }
    itone
}

fn build_one_table() -> &'static [[bool; 9]; 512] {
    static ONE: std::sync::OnceLock<[[bool; 9]; 512]> = std::sync::OnceLock::new();
    ONE.get_or_init(|| {
        let mut table = [[false; 9]; 512];
        for i in 0..512 {
            for j in 0..9 {
                table[i][j] = (i & (1 << j)) != 0;
            }
        }
        table
    })
}

fn build_taper() -> &'static [f64; 101] {
    static T: std::sync::OnceLock<[f64; 101]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0.0f64; 101];
        for i in 0..101 {
            let x = (i as f32 * std::f32::consts::PI) / 100.0f32;
            t[i] = (0.5f32 * (1.0f32 + x.cos())) as f64;
        }
        t
    })
}

fn ap_downsample(
    downsample_cache: &ApDownsampleCache,
    f0: f64,
    taper: &[f64; 101],
) -> (Vec<f64>, Vec<f64>) {
    let df = DOWNSAMPLE_DF;
    let baud = DOWNSAMPLE_BAUD;
    let f0 = f0 as f32;
    let i0 = nint_wsjtx_real(f0 / df).max(0) as usize;
    let ft = f0 + 8.5f32 * baud;
    let it_end = (nint_wsjtx_real(ft / df).max(0) as usize).min(NFFT1_LONG / 2);
    let fb = f0 - 1.5f32 * baud;
    let ib = 1.max(nint_wsjtx_real(fb / df).max(0) as usize);

    let mut cd0_re = vec![0.0f64; NFFT2];
    let mut cd0_im = vec![0.0f64; NFFT2];
    let mut k = 0;
    for i in ib..=it_end {
        if k >= NFFT2 {
            break;
        }
        cd0_re[k] = downsample_cache.cx_re[i];
        cd0_im[k] = downsample_cache.cx_im[i];
        k += 1;
    }

    for i in 0..TAPER_SIZE {
        if i >= NFFT2 {
            break;
        }
        let tap = taper[TAPER_SIZE - 1 - i];
        cd0_re[i] *= tap;
        cd0_im[i] *= tap;
    }
    let end_tap = k.saturating_sub(1);
    for i in 0..TAPER_SIZE {
        let idx = end_tap.saturating_sub(TAPER_SIZE - 1) + i;
        if idx < NFFT2 {
            let tap = taper[i];
            cd0_re[idx] *= tap;
            cd0_im[idx] *= tap;
        }
    }

    let shift = (i0 as isize) - (ib as isize);
    if shift != 0 {
        let mut tmp_re = vec![0.0f64; NFFT2];
        let mut tmp_im = vec![0.0f64; NFFT2];
        for i in 0..NFFT2 {
            let src = ((i as isize + shift).rem_euclid(NFFT2 as isize)) as usize;
            tmp_re[i] = cd0_re[src];
            tmp_im[i] = cd0_im[src];
        }
        cd0_re = tmp_re;
        cd0_im = tmp_im;
    }

    crate::util::four2a_c2c(&mut cd0_re, &mut cd0_im, 1);
    for i in 0..NFFT2 {
        cd0_re[i] *= DOWNSAMPLE_FAC;
        cd0_im[i] *= DOWNSAMPLE_FAC;
    }
    (cd0_re, cd0_im)
}

fn nint_wsjtx_f32(x: f64) -> isize {
    (x as f32).round() as isize
}

fn nint_wsjtx_real(x: f32) -> isize {
    x.round() as isize
}

fn ap_sync8d(cd0_re: &[f64], cd0_im: &[f64], i0: isize, sync_re: &[f64], sync_im: &[f64]) -> f64 {
    let mut sync = 0.0f32;
    let stride = 36 * COSTAS_SYMBOL_LEN;
    for i in 0..COSTAS_BLOCKS {
        let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize);
        for _block in 0..3 {
            if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                let s = i_start as usize;
                let mut zr = 0.0f32;
                let mut zi = 0.0f32;
                for j in 0..COSTAS_SYMBOL_LEN {
                    let base = i * COSTAS_SYMBOL_LEN + j;
                    let d_re = cd0_re[s + j] as f32;
                    let d_im = cd0_im[s + j] as f32;
                    let s_re = sync_re[base] as f32;
                    let s_im = sync_im[base] as f32;
                    zr += d_re * s_re + d_im * s_im;
                    zi += d_re * s_im - d_im * s_re;
                }
                sync += zr * zr + zi * zi;
            }
            i_start += stride as isize;
        }
    }
    sync as f64
}

fn ap_sync8d_twk(
    cd0_re: &[f64],
    cd0_im: &[f64],
    i0: isize,
    sync_re: &[f64],
    sync_im: &[f64],
    twk_re: &[f64; 32],
    twk_im: &[f64; 32],
) -> f64 {
    let mut sync = 0.0f32;
    let stride = 36 * COSTAS_SYMBOL_LEN;
    for i in 0..COSTAS_BLOCKS {
        let mut i_start = i0 + (i as isize) * (COSTAS_SYMBOL_LEN as isize);
        for _block in 0..3 {
            if i_start >= 0 && i_start + COSTAS_SYMBOL_LEN as isize <= NP2 as isize {
                let s = i_start as usize;
                let mut zr = 0.0f32;
                let mut zi = 0.0f32;
                for j in 0..COSTAS_SYMBOL_LEN {
                    let base = i * COSTAS_SYMBOL_LEN + j;
                    let twk_re = twk_re[j] as f32;
                    let twk_im = twk_im[j] as f32;
                    let sync_re = sync_re[base] as f32;
                    let sync_im = sync_im[base] as f32;
                    let tpl_re = twk_re * sync_re - twk_im * sync_im;
                    let tpl_im = twk_re * sync_im + twk_im * sync_re;
                    let d_re = cd0_re[s + j] as f32;
                    let d_im = cd0_im[s + j] as f32;
                    zr += d_re * tpl_re + d_im * tpl_im;
                    zi += d_re * tpl_im - d_im * tpl_re;
                }
                sync += zr * zr + zi * zi;
            }
            i_start += stride as isize;
        }
    }
    sync as f64
}

fn build_ctwk(dphi: f64) -> ([f64; 32], [f64; 32]) {
    let (mut re, mut im) = ([0.0f64; 32], [0.0f64; 32]);
    let dphi = dphi as f32;
    let twopi = TWO_PI as f32;
    let mut phi = 0.0f32;
    for j in 0..32 {
        re[j] = phi.cos() as f64;
        im[j] = phi.sin() as f64;
        phi = (phi + dphi) % twopi;
    }
    (re, im)
}

fn twkfreq1(
    ca_re: &[f64],
    ca_im: &[f64],
    npts: usize,
    fsample: f64,
    a: &[f64; 5],
) -> (Vec<f64>, Vec<f64>) {
    let twopi = 6.283185307;
    let x0 = 0.5 * (npts as f64 + 1.0);
    let s = 2.0 / npts as f64;
    let mut cb_re = Vec::with_capacity(npts);
    let mut cb_im = Vec::with_capacity(npts);
    let mut w_re = 1.0f64;
    let mut w_im = 0.0f64;
    for i in 1..=npts {
        let x = s * (i as f64 - x0);
        let p2 = 1.5 * x * x - 0.5;
        let p3 = 2.5 * x.powi(3) - 1.5 * x;
        let p4 = 4.375 * x.powi(4) - 3.75 * x * x + 0.375;
        let dphi = (a[0] + x * a[1] + p2 * a[2] + p3 * a[3] + p4 * a[4]) * (twopi / fsample);
        let ws_re = dphi.cos();
        let ws_im = dphi.sin();
        let nw_re = w_re * ws_re - w_im * ws_im;
        let nw_im = w_re * ws_im + w_im * ws_re;
        w_re = nw_re;
        w_im = nw_im;
        cb_re.push(w_re * ca_re[i - 1] - w_im * ca_im[i - 1]);
        cb_im.push(w_re * ca_im[i - 1] + w_im * ca_re[i - 1]);
    }
    (cb_re, cb_im)
}

fn fft32(re: &mut [f64; 32], im: &mut [f64; 32]) {
    let n = 32;
    let mut j = 0;
    for i in 0..n - 1 {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while j >= m && m > 0 {
            j -= m;
            m >>= 1;
        }
        j += m;
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let angle = -TWO_PI / len as f64;
        let wb_re = angle.cos();
        let wb_im = angle.sin();
        let mut i = 0;
        while i < n {
            let mut tw_re = 1.0f64;
            let mut tw_im = 0.0f64;
            for j in 0..half {
                let ar = re[i + j];
                let ai = im[i + j];
                let br = re[i + j + half];
                let bi = im[i + j + half];
                re[i + j] = ar + tw_re * br - tw_im * bi;
                im[i + j] = ai + tw_re * bi + tw_im * br;
                re[i + j + half] = ar - tw_re * br + tw_im * bi;
                im[i + j + half] = ai - tw_re * bi - tw_im * br;
                let nt = tw_re * wb_re - tw_im * wb_im;
                tw_im = tw_re * wb_im + tw_im * wb_re;
                tw_re = nt;
            }
            i += len;
        }
        len <<= 1;
    }
}
