//! Conservative blanking of short bursts spanning the FT8 audio passband.
use super::{median, FftPlanner, C};
use std::f64::consts::TAU;

const WINDOW: usize = 64;
const HOP: usize = WINDOW / 2;
const BANDS: usize = 6;
const BINS_PER_BAND: usize = 3;
const MAX_BURST_HOPS: usize = 60; // 160 ms at 12 kHz

pub(super) fn blank(audio: &[f32]) -> Option<(Vec<f64>, usize)> {
    let window: [f64; WINDOW] =
        std::array::from_fn(|i| 0.5 - 0.5 * (TAU * i as f64 / WINDOW as f64).cos());
    let fft = FftPlanner::<f64>::new().plan_fft_forward(WINDOW);
    let powers: Vec<[f64; BANDS]> = audio
        .windows(WINDOW)
        .step_by(HOP)
        .map(|frame| {
            let mut bins: [C; WINDOW] =
                std::array::from_fn(|i| C::new(f64::from(frame[i]) * window[i], 0.0));
            fft.process(&mut bins);
            // Skip DC. These groups span FFT bins 1 through 18 at 12 kHz.
            std::array::from_fn(|band| {
                let first = 1 + band * BINS_PER_BAND;
                bins[first..first + BINS_PER_BAND]
                    .iter()
                    .map(C::norm_sqr)
                    .sum()
            })
        })
        .collect();
    let levels: [f64; BANDS] =
        std::array::from_fn(|band| median(powers.iter().map(|p| p[band]).collect()));
    let total_level = levels.iter().sum::<f64>();
    if total_level <= 0.0 {
        return None;
    }
    // A quiet band still needs appreciable absolute energy. Narrow carriers and
    // their window sidelobes should not count as simultaneous broadband energy.
    let thresholds = levels.map(|level| 4.0 * level.max(0.025 * total_level));
    let mut mask = vec![false; audio.len().div_ceil(HOP)];
    for (hop, power) in powers.iter().enumerate() {
        if power
            .iter()
            .zip(thresholds)
            .filter(|&(&p, threshold)| p > threshold)
            .count()
            >= 4
        {
            // Cover the whole FFT window and one hop on either side.
            let end = (hop + 3).min(mask.len());
            mask[hop.saturating_sub(1)..end].fill(true);
        }
    }
    if !mask.iter().any(|&marked| marked)
        || mask
            .split(|&marked| !marked)
            .any(|run| run.len() > MAX_BURST_HOPS)
    {
        return None;
    }
    let mut weights = vec![(false, false, false); mask.len()];
    let mut touched = 0;
    for (hop, block) in audio.chunks(HOP).enumerate() {
        let fade_in = hop > 0 && mask[hop - 1];
        let fade_out = mask.get(hop + 1) == Some(&true);
        weights[hop] = (mask[hop], fade_in, fade_out);
        if mask[hop] || fade_in || fade_out {
            touched += block.len();
        }
    }
    // Never run the supplemental search after discarding a large part of a slot.
    if touched * 100 > audio.len() * 15 {
        return None;
    }
    let mut blanked: Vec<_> = audio.iter().copied().map(f64::from).collect();
    for (block, (marked, fade_in, fade_out)) in blanked.chunks_mut(HOP).zip(weights) {
        if marked {
            block.fill(0.0);
        } else if fade_in || fade_out {
            for (i, sample) in block.iter_mut().enumerate() {
                if fade_in {
                    *sample *= window[i];
                }
                if fade_out {
                    *sample *= window[HOP - 1 - i];
                }
            }
        }
    }
    Some((blanked, touched))
}
