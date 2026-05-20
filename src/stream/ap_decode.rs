/// AP (A Priori) decode for streaming FT8.
/// Matches WSJT-X ft8_a7d subroutine.
/// Uses known callsigns from previous slot to force decode at known freq/dt.

use crate::stream::cross_slot::SavedDecode;
use crate::stream::ft8b_stream::{ft8b_stream, Ft8bResult};

/// AP decode using previous slot decodes.
/// For each previous decode, attempt to re-decode at the known freq/dt
/// with the full 15s data (may pick up weaker signals at same location).
pub fn ap_decode(
    dd0: &[f64],
    cx_re: &[f64],
    cx_im: &[f64],
    sbase: &[f64],
    previous_decodes: &[SavedDecode],
    depth: usize,
) -> Vec<Ft8bResult> {
    let mut results = Vec::new();

    for prev in previous_decodes {
        // Skip if message contains hashed callsigns (temporary, matching WSJT-X)
        if prev.msg.contains('<') {
            continue;
        }

        // Try to re-decode at known freq/dt
        if let Some(result) = ft8b_stream(
            dd0,
            cx_re,
            cx_im,
            prev.freq,
            prev.dt + 0.5, // WSJT-X uses xdt+0.5 convention
            sbase,
            depth,
            prev.sync,
        ) {
            // Only add if we got a different/better message
            if !results.iter().any(|r: &Ft8bResult| r.msg == result.msg) {
                results.push(result);
            }
        }
    }

    results
}
