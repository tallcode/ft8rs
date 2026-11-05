//! Hybrid BP/OSD decoder entry for FT8 LDPC(174,91).
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/decode174_91.f90`

use crate::decode::lib_wsjtx_plus::bpdecode174_91::bp_decode174_91_with_posteriors;
use crate::decode::lib_wsjtx_plus::osd174_91::osd_decode174_91;

pub(crate) use crate::decode::decode174_91::{DecodeResult, N_LDPC};

/// Hybrid BP + OSD decoder.
pub fn decode174_91(llr: &[f64], apmask: &[i8], maxosd: isize) -> Option<DecodeResult> {
    if super::fft::mode() == super::fft::Mode::Reference {
        return crate::decode::decode174_91::decode174_91(llr, apmask, maxosd);
    }
    let max_iterations: usize = 30;
    let maxosd = maxosd.min(3);
    let (nosd, bp_save_limit, channel_llr_osd) = if maxosd < 0 {
        (0, 0, false)
    } else if maxosd == 0 {
        (1, 0, true)
    } else {
        (maxosd as usize, maxosd as usize, false)
    };

    let bp = bp_decode174_91_with_posteriors(
        llr,
        apmask,
        max_iterations,
        nosd,
        bp_save_limit,
        channel_llr_osd,
    );

    if let Some(result) = bp.decoded {
        return Some(result);
    }

    // Try OSD with accumulated BP posteriors (WSJT-X approach)
    if nosd >= 1 {
        for i in 0..nosd {
            if let Some(mut result) = osd_decode174_91(&bp.zsave[i], apmask, 2) {
                result.nharderrors = channel_hard_errors(llr, &result.cw);
                if result.nharderrors > 0 {
                    return Some(result);
                }
            }
        }
    }

    None
}

fn channel_hard_errors(llr: &[f64], cw: &[u8]) -> usize {
    llr.iter()
        .zip(cw.iter())
        .filter(|&(&soft, &bit)| {
            let hard = if soft >= 0.0 { 1 } else { 0 };
            hard != bit
        })
        .count()
}
