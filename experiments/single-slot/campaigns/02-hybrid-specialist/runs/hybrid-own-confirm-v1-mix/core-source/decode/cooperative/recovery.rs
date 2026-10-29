//! A single, bounded cancellation round on an independent audio copy.
//! Signal support only controls seed admission; it never accepts a decode or
//! changes an upstream decoder's thresholds.
use std::sync::OnceLock;

use super::ParsedMessage;
use crate::decode::{
    gen_ft8wave::gen_ft8wave, genft8::get_ft8_tones_from_77bits, packjt77::pack77,
    subtractft8::subtract_ft8,
};
use crate::stream::session::StreamDecodedMessage;

pub(crate) const MAX_SEEDS: usize = 3;
const NSPS: usize = 1920;

#[derive(Clone, Debug, Default)]
pub struct CancellationStats {
    pub proposed: usize,
    pub accepted: usize,
    pub rejected_support: usize,
}

pub(crate) fn eligible_shape(row: &StreamDecodedMessage, low: f64, high: f64) -> bool {
    let parsed = ParsedMessage::parse(&row.msg);
    if parsed.sender.is_none()
        || parsed.unresolved
        || row.msg.contains('<')
        || !row.freq.is_finite()
        || row.freq < low
        || row.freq > high
        || !row.dt.is_finite()
        || row.dt.abs() > 3.5
        || !row.snr.is_finite()
    {
        return false;
    }
    // Re-encode only to verify the *existing* tone sequence, never to fill in
    // missing tones or guess the payload of a display-resolved hash message.
    let bits = pack77(&row.msg);
    bits.len() == 77 && get_ft8_tones_from_77bits(&bits) == row.itone
}

pub(crate) fn prepare(
    samples: &[f32],
    seeds: &[StreamDecodedMessage],
    low: f64,
    high: f64,
) -> (Vec<f32>, CancellationStats) {
    let mut residual: Vec<f64> = samples.iter().map(|v| f64::from(*v)).collect();
    let mut stats = CancellationStats::default();
    let mut ordered = seeds.iter().take(MAX_SEEDS).collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        b.snr
            .total_cmp(&a.snr)
            .then_with(|| a.freq.total_cmp(&b.freq))
    });
    for seed in ordered {
        stats.proposed += 1;
        if !eligible_shape(seed, low, high) || waveform_support(&residual, seed) < 0.70 {
            stats.rejected_support += 1;
            continue;
        }
        let before = residual.clone();
        let energy = before.iter().map(|x| x * x).sum::<f64>();
        subtract_ft8(&mut residual, &seed.itone, seed.freq, seed.dt + 0.5);
        let after = residual.iter().map(|x| x * x).sum::<f64>();
        if !after.is_finite() || after > energy * 1.01 {
            residual = before;
            stats.rejected_support += 1;
        } else {
            stats.accepted += 1;
        }
    }
    (residual.into_iter().map(|x| x as f32).collect(), stats)
}

/// Data-symbol energy agreement against the supplied waveform and its seven
/// alternate tone offsets. This does not search messages, infer bits or decode.
fn waveform_support(samples: &[f64], row: &StreamDecodedMessage) -> f64 {
    static SHIFTS: OnceLock<Vec<Vec<(f64, f64)>>> = OnceLock::new();
    let shifts = SHIFTS.get_or_init(|| {
        (-7..=7)
            .map(|offset| {
                (0..NSPS)
                    .map(|i| {
                        let phase =
                            std::f64::consts::TAU * offset as f64 * 6.25 * i as f64 / 12000.0;
                        (phase.cos(), phase.sin())
                    })
                    .collect()
            })
            .collect()
    });
    let (re, im) = gen_ft8wave(&row.itone, row.freq);
    let start = ((row.dt + 0.5) * 12000.0).round() as isize;
    let mut correct = 0.0;
    let mut strongest = 0.0;
    for symbol in (7..36).chain(43..72) {
        let mut best: f64 = 0.0;
        let mut actual = 0.0;
        for tone in 0..8 {
            let offset = tone - row.itone[symbol] + 7;
            let shift = &shifts[offset as usize];
            let (mut zr, mut zi) = (0.0, 0.0);
            for (i, &(cr, ci)) in shift.iter().enumerate() {
                let index = symbol * NSPS + i;
                let source = start + index as isize;
                if source < 0 || source as usize >= samples.len() {
                    continue;
                }
                let value = samples[source as usize];
                zr += value * (re[index] * cr - im[index] * ci);
                zi += value * (im[index] * cr + re[index] * ci);
            }
            let power = zr * zr + zi * zi;
            best = best.max(power);
            if tone == row.itone[symbol] {
                actual = power;
            }
        }
        correct += actual;
        strongest += best;
    }
    if strongest > 0.0 {
        correct / strongest
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed(msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq: 1000.0,
            dt: 0.0,
            snr: -10.0,
            msg: msg.into(),
            sync: 2.0,
            itone: get_ft8_tones_from_77bits(&pack77(msg)),
        }
    }
    #[test]
    fn missing_or_mismatched_tones_cannot_become_seeds() {
        let mut row = seed("K1JT BG5ATV PM00");
        row.itone = [0; 79];
        assert!(!eligible_shape(&row, 200.0, 3000.0));
        row.itone = seed("CQ K1ABC FN42").itone;
        assert!(!eligible_shape(&row, 200.0, 3000.0));
    }
    #[test]
    fn waveform_evidence_rejects_a_different_message_and_silence() {
        let actual = seed("K1JT BG5ATV PM00");
        let (wave, _) = gen_ft8wave(&actual.itone, actual.freq);
        let mut audio = vec![0.0; 180000];
        audio[6000..6000 + wave.len()].copy_from_slice(&wave);
        assert!(waveform_support(&audio, &actual) > 0.95);
        assert!(waveform_support(&audio, &seed("CQ K1ABC FN42")) < 0.70);
        assert_eq!(waveform_support(&vec![0.0; 180000], &actual), 0.0);
    }
}
