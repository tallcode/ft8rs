//! Standalone Plus recovery for a recent, complete codeword on the same parity.

use super::{a7_xbase, normal, SlotTimestamp, StreamDecodedMessage};
use crate::decode::lib_wsjtx_plus::fft::SymbolFft;
use crate::decode::lib_wsjtx_plus::ft8_a7::{ap_downsample, template_snr, ApDownsampleCache};
use crate::decode::lib_wsjtx_plus::ft8_decode::{extract_symbol_spectrum, DT2, FS2};
use crate::decode::packjt77::is_stdcall;
use std::collections::VecDeque;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

const COSTAS: [i32; 7] = [3, 1, 4, 0, 6, 5, 2];
const MAX_CANDIDATES: usize = 64;
pub(super) const BUDGET: Duration = Duration::from_millis(125);

#[derive(Default)]
pub(super) struct RepeatHistory {
    slots: VecDeque<(SlotTimestamp, Vec<StreamDecodedMessage>)>,
}

impl RepeatHistory {
    pub(super) fn recover(
        &mut self,
        timestamp: &SlotTimestamp,
        residual: &[f64],
        base: &[StreamDecodedMessage],
        sbase: &[f64],
        band: RangeInclusive<f64>,
        deadline: Instant,
    ) -> Vec<StreamDecodedMessage> {
        let mut seeds: Vec<&StreamDecodedMessage> = Vec::new();
        for lag in [30, 60, 90, 120] {
            let Some((_, rows)) = self
                .slots
                .iter()
                .find(|(time, _)| time.add_seconds(lag) == *timestamp)
            else {
                continue;
            };
            for row in rows {
                if base.iter().any(|other| {
                    (other.freq - row.freq).abs() <= 5.0
                        && ((other.dt - row.dt).abs() <= 0.10
                            || normal(&other.msg) == normal(&row.msg))
                }) || seeds.iter().any(|other| {
                    (other.freq - row.freq).abs() <= 5.0 && normal(&other.msg) == normal(&row.msg)
                }) {
                    continue;
                }
                seeds.push(row);
            }
        }

        let mut recovered = Vec::new();
        if !seeds.is_empty() && Instant::now() < deadline {
            let cache = ApDownsampleCache::new(residual);
            for row in seeds.into_iter().take(MAX_CANDIDATES) {
                if Instant::now() >= deadline {
                    break;
                }
                if let Some(found) = search(&cache, row, &band) {
                    if found.hits[0] >= 11
                        && found.hits[1] >= 11
                        && found.hits[2] >= 7
                        && base.iter().all(|other| {
                            (other.freq - found.freq).abs() > 3.0
                                || (other.dt - found.dt).abs() > 0.08
                        })
                    {
                        recovered.push(StreamDecodedMessage {
                            freq: found.freq,
                            dt: found.dt,
                            snr: template_snr(found.power, a7_xbase(found.freq, sbase)),
                            sync: 0.0,
                            ..row.clone()
                        });
                    }
                }
            }
        }

        // Capture the existing decoder's rows before any of our results are published.
        self.slots.retain(|(time, _)| time != timestamp);
        self.slots.push_front((
            timestamp.clone(),
            base.iter().filter(|row| eligible(row)).cloned().collect(),
        ));
        self.slots.truncate(9);
        recovered
    }
}

fn eligible(row: &StreamDecodedMessage) -> bool {
    if !row.freq.is_finite() || !row.dt.is_finite() || row.msg.contains(['<', '/', '>']) {
        return false;
    }
    let mut words = row.msg.split_whitespace();
    let (Some(first), Some(second), Some(_), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return false;
    };
    (first == "CQ" || is_stdcall(first))
        && is_stdcall(second)
        && [0, 36, 72].iter().all(|&i| row.itone[i..i + 7] == COSTAS)
        && row.itone.iter().all(|&tone| (0..=7).contains(&tone))
}

struct Match {
    freq: f64,
    dt: f64,
    hits: [usize; 3],
    power: f64,
}

fn search(
    cache: &ApDownsampleCache,
    row: &StreamDecodedMessage,
    band: &RangeInclusive<f64>,
) -> Option<Match> {
    let mut fft = SymbolFft::new();
    let center = ((row.dt + 0.5) * FS2) as f32;
    let center = center.round() as isize + 1;
    // Each alignment advances by four samples. A symbol's start is therefore
    // shared with symbol neighbors eight alignment steps away.
    const STEP: usize = 4;
    const RADIUS: isize = 64;
    const OFFSETS: usize = (2 * RADIUS) as usize / STEP + 1;
    const SYMBOL_STRIDE: usize = 32 / STEP;
    let mut bins = [None; (79 - 1) * SYMBOL_STRIDE + OFFSETS];
    let mut best = None;
    let mut best_score = f64::NEG_INFINITY;
    for df in -6..=6 {
        let freq = row.freq + f64::from(df) * 0.5;
        if !band.contains(&freq) {
            continue;
        }
        let (re, im) = ap_downsample(cache, freq);
        bins.fill(None);
        for (alignment, offset) in (-RADIUS..=RADIUS).step_by(STEP).enumerate() {
            let time = center + offset;
            let mut hits = [0; 3];
            let mut energy = [0.0; 3];
            let mut power = 0.0;
            for (i, &tone) in row.itone.iter().enumerate() {
                let group = match i {
                    7..=35 => 0,
                    43..=71 => 1,
                    _ => 2,
                };
                let start = time + i as isize * 32;
                let (powers, total) =
                    bins[alignment + i * SYMBOL_STRIDE].get_or_insert_with(|| {
                        let powers = extract_symbol_spectrum(&mut fft, &re, &im, start)
                            .map(|bin| bin.norm_sqr());
                        let total = powers.iter().sum::<f64>();
                        (powers, total)
                    });
                let total = *total;
                if total > 1.0e-30 {
                    let expected = powers[tone as usize];
                    power += expected;
                    energy[group] += 8.0 * expected / total;
                    hits[group] += usize::from(powers.iter().all(|&value| expected >= value));
                }
            }
            // The second half's data is held out of frequency and time alignment.
            let score = energy[0] / 29.0 + 0.5 * energy[2] / 21.0;
            if score > best_score {
                best_score = score;
                best = Some(Match {
                    freq,
                    dt: (time as f64 - 1.0) * DT2 - 0.5,
                    hits,
                    power,
                });
            }
        }
    }
    best
}
