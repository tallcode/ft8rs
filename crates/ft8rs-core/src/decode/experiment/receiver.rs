//! Experimental receive foundation retained from the single-slot campaigns.
//! Protocol, waveform synthesis and FEC call unchanged upstream routines.
//! This private module has no standalone profile, CallDB or persistent AP state.
use super::ExperimentInput;
use crate::stream::session::StreamDecodedMessage;
#[path = "drift.rs"]
mod drift;
#[path = "impulse.rs"]
mod impulse;
pub(crate) use drift::recover_drift;
#[path = "polar.rs"]
mod polar;
#[path = "waveform.rs"]
mod waveform;
pub(crate) use polar::{recover as recover_polar, ApContext as PolarApContext};
use rustfft::{num_complex::Complex64 as C, FftPlanner};
use std::cell::OnceCell;
use std::f64::consts::TAU;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use waveform::{cancel_smooth, fit_message, WaveformFit};
const FS: f64 = 12000.0;
const GRAY: [usize; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
const COSTAS: [usize; 7] = [3, 1, 4, 0, 6, 5, 2];
// Development-only calibration: the earlier incorrect codeword scored 103.186.
// A declared 50% margin rounded upward gives 160. This is not a p-value.
const MIN_SCORE: f64 = 160.0;
const MAX_FEC: usize = 4096;
const MAX_CANDIDATES: usize = 96;
const PULSE_CANDIDATES: usize = 32;
// Four hypotheses with four metrics each, plus eight coherent hypotheses.
const MAX_FEC_PER_CANDIDATE: usize = 4 * 4 + 8;
#[derive(Clone, Debug, Default)]
pub struct ExperimentDiagnostics {
    pub fec_calls: usize,
    pub tentative_rows: usize,
    pub rejected_rows: usize,
    pub accepted_rows: usize,
    pub residual_searches: usize,
    /// Samples blanked or tapered in the terminal pulse recovery search.
    pub impulse_blanked_samples: usize,
    pub impulse_accepted_rows: usize,
    /// Terminal, isolated drift recovery; shares the slot's FEC budget.
    pub drift_accepted_rows: usize,
    /// Compensated views with at least one persistent-line removal (at most two).
    pub drift_removed_lines: usize,
    pub drift_seed_models: usize,
    pub drift_elapsed: Duration,
    /// Terminal Gaussian-channel demapping and cross-fitted data support.
    pub polar_elapsed: Duration,
    pub polar_fec_calls: usize,
    pub polar_ap_calls: usize,
    pub polar_accepted_rows: usize,
    pub polar_ap_accepted_rows: usize,
    pub polar_rejected_rows: usize,
    pub native_seed_candidates: usize,
    pub native_seed_models: usize,
    pub native_seed_rejections: usize,
    pub data_support_rejections: usize,
    pub conditioning: Duration,
    pub initial_search: Duration,
    pub residual_search: Duration,
    pub budget_exhausted: bool,
    pub elapsed: Duration,
    pub accepted_scores: Vec<f64>,
    pub accepted_data_scores: Vec<f64>,
    pub accepted_methods: Vec<String>,
    pub error: Option<String>,
}
struct Budget {
    deadline: Instant,
    low: f64,
    high: f64,
    diagnostics: ExperimentDiagnostics,
}
impl Budget {
    fn exhausted(&mut self) -> bool {
        let done = Instant::now() >= self.deadline || self.diagnostics.fec_calls >= MAX_FEC;
        self.diagnostics.budget_exhausted |= done;
        done
    }
    fn decode(&mut self, llr: &[f64]) -> Option<(String, [i32; 79])> {
        if self.exhausted() {
            return None;
        }
        self.diagnostics.fec_calls += 1;
        decode_with_tones(llr)
    }
}
struct Band {
    samples: Vec<C>,
    center: f64,
    observed_end: usize,
}
struct Spectrum {
    values: [[C; 8]; 79],
    observed: [usize; 79],
}
impl std::ops::Deref for Spectrum {
    type Target = [[C; 8]; 79];
    fn deref(&self) -> &Self::Target {
        &self.values
    }
}
fn basis() -> &'static [[C; 32]; 8] {
    static B: OnceLock<[[C; 32]; 8]> = OnceLock::new();
    B.get_or_init(|| {
        std::array::from_fn(|q| {
            std::array::from_fn(|n| C::from_polar(1.0, -TAU * q as f64 * n as f64 / 32.0))
        })
    })
}
impl Band {
    fn new(audio: &[f64], center: f64) -> Self {
        let mut planner = FftPlanner::<f64>::new();
        let mut fft: Vec<C> = audio.iter().map(|&x| C::new(x, 0.0)).collect();
        fft.resize(180000, C::new(0.0, 0.0));
        planner.plan_fft_forward(180000).process(&mut fft);
        Self::from_fft(&fft, center, audio.len())
    }
    fn from_fft(fft: &[C], center: f64, observed_samples: usize) -> Self {
        let mut planner = FftPlanner::<f64>::new();
        let bin = (center * 15.0).round() as isize;
        let mut z = vec![C::new(0.0, 0.0); 3000];
        for (i, v) in z.iter_mut().enumerate() {
            let off = if i < 1500 {
                i as isize
            } else {
                i as isize - 3000
            };
            let src = bin + off;
            if src > 0 && src < 90000 {
                *v = fft[src as usize] / 180000.0;
            }
        }
        planner.plan_fft_inverse(3000).process(&mut z);
        Self {
            samples: z,
            center: bin as f64 / 15.0,
            observed_end: observed_samples / 60,
        }
    }
    fn spectra(&self, f: f64, dt: f64, drift: f64) -> Spectrum {
        let mut out = [[C::new(0.0, 0.0); 8]; 79];
        let mut observed = [0usize; 79];
        let start = ((dt + 0.5) * 200.0).round() as isize;
        for (k, row) in out.iter_mut().enumerate() {
            for n in 0..32 {
                let j = start + (32 * k + n) as isize;
                if j < 0 || j >= self.observed_end as isize {
                    continue;
                }
                observed[k] += 1;
                let t = (j - start) as f64 / 200.0;
                let z = self.samples[j as usize]
                    * C::from_polar(1.0, -TAU * ((f - self.center) * t + 0.5 * drift * t * t));
                for (q, v) in row.iter_mut().enumerate() {
                    *v += z * basis()[q][n];
                }
            }
        }
        Spectrum {
            values: out,
            observed,
        }
    }
    fn sync_stats(&self, dt: f64, oscillators: &[[C; 32]; 21]) -> (f64, f64) {
        let start = ((dt + 0.5) * 200.0).round() as isize;
        let mut power = 0.0;
        let mut total = 0.0;
        let mut count = 0;
        let mut coherent = C::new(0.0, 0.0);
        let basis = basis();
        for (block, base) in [0, 36, 72].into_iter().enumerate() {
            for (k, &tone) in COSTAS.iter().enumerate() {
                let j0 = start + 32 * (base + k) as isize;
                if j0 < 0 || j0 + 31 >= self.observed_end as isize {
                    continue;
                }
                let mut z = C::new(0.0, 0.0);
                let mut en = 0.0;
                let samples = &self.samples[j0 as usize..][..32];
                for ((&sample, &reference), &osc) in samples
                    .iter()
                    .zip(&basis[tone])
                    .zip(&oscillators[block * 7 + k])
                {
                    z += sample * reference * osc;
                    en += sample.norm_sqr();
                }
                power += z.norm_sqr();
                coherent += z;
                total += en;
                count += 1;
            }
        }
        if count < 14 {
            return (0.0, 0.0);
        }
        (
            power / total.max(1e-15),
            coherent.norm_sqr() / total.max(1e-15),
        )
    }
    /// The zero-drift reference for each Costas symbol depends on frequency
    /// and symbol index. Reuse it for all timing hypotheses at that frequency.
    fn sync_oscillators(&self, f: f64) -> [[C; 32]; 21] {
        let df = f - self.center;
        let step = C::from_polar(1.0, -TAU * (df / 200.0));
        std::array::from_fn(|i| {
            let symbol = [0, 36, 72][i / 7] + i % 7;
            let t0 = (32 * symbol) as f64 / 200.0;
            let mut osc = C::from_polar(1.0, -TAU * (df * t0));
            std::array::from_fn(|_| {
                let current = osc;
                osc *= step;
                current
            })
        })
    }
}
fn median(mut x: Vec<f64>) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    let middle = x.len() / 2;
    *x.select_nth_unstable_by(middle, f64::total_cmp).1
}
fn variance(s: &Spectrum) -> f64 {
    let mut v = Vec::new();
    for base in [0, 36, 72] {
        for k in 0..7 {
            if s.observed[base + k] != 32 {
                continue;
            }
            for q in 0..8 {
                if q != COSTAS[k] {
                    v.push(s[base + k][q].norm_sqr());
                }
            }
        }
    }
    (median(v) / std::f64::consts::LN_2).max(1e-12)
}
fn log_i0(x: f64) -> f64 {
    let x = x.abs();
    if x < 3.75 {
        let y = (x / 3.75).powi(2);
        (1.0 + y
            * (3.5156229
                + y * (3.0899424
                    + y * (1.2067492 + y * (0.2659732 + y * (0.0360768 + y * 0.0045813))))))
            .ln()
    } else {
        let y = 3.75 / x;
        x - 0.5 * x.ln()
            + (0.39894228
                + y * (0.01328592
                    + y * (0.00225319
                        + y * (-0.00157565
                            + y * (0.00916281
                                + y * (-0.02057706
                                    + y * (0.02635537 + y * (-0.01647633 + y * 0.00392377))))))))
                .ln()
    }
}
fn lse(x: &[f64]) -> f64 {
    let m = x.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    m + x.iter().map(|v| (v - m).exp()).sum::<f64>().ln()
}
#[derive(Clone, Copy, Default)]
struct BesselGroup {
    maximum: f64,
    partitions: u32,
    ones: usize,
    zeroes: usize,
}

fn score_partition(values: &[f64], run: usize, offset: usize) -> impl Iterator<Item = f64> + '_ {
    values
        .chunks_exact(2 * run)
        .flat_map(move |chunk| chunk[offset..offset + run].iter().copied())
}

fn bessel_maxima(scores: &[f64], maxima: &mut [f64], scratch: &mut Vec<f64>) -> f64 {
    // Keep the original ordered reductions for NaNs, infinities and signed
    // zero ties. Ordinary finite maxima have a unique bit representation.
    if scores
        .iter()
        .any(|score| !score.is_finite() || score.to_bits() == (-0.0f64).to_bits())
    {
        for (i, maximum) in maxima.iter_mut().enumerate() {
            let run = scores.len() >> (1 + i / 2);
            *maximum =
                score_partition(scores, run, (i % 2) * run).fold(f64::NEG_INFINITY, f64::max);
        }
        return scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    }

    // Each level records the maxima of the alternating runs for one bit.
    // The merged adjacent runs supply the next more significant bit.
    let mut count = scores.len() / 2;
    scratch.resize(count, 0.0);
    let mut levels = maxima.as_chunks_mut::<2>().0.iter_mut().rev();
    let least_significant = levels.next().expect("at least one Bessel bit");
    least_significant.fill(f64::NEG_INFINITY);
    for (&[zero, one], merged) in scores.as_chunks::<2>().0.iter().zip(scratch.iter_mut()) {
        least_significant[0] = least_significant[0].max(zero);
        least_significant[1] = least_significant[1].max(one);
        *merged = zero.max(one);
    }
    for level in levels {
        count /= 2;
        level.fill(f64::NEG_INFINITY);
        for i in 0..count {
            let zero = scratch[2 * i];
            let one = scratch[2 * i + 1];
            level[0] = level[0].max(zero);
            level[1] = level[1].max(one);
            scratch[i] = zero.max(one);
        }
    }
    scratch[0]
}

/// Share exponents only when the pattern and the partition maximum agree.
/// Each partition still reduces its terms in ascending pattern order.
fn partition_llrs(scores: &[f64], bits: usize, exps: &mut Vec<f64>, output: &mut [f64]) {
    const MAX_PARTITIONS: usize = 2 * 4 * 3;
    debug_assert_eq!(scores.len(), 1 << bits);
    debug_assert_eq!(output.len(), bits);
    let mut maxima = [0.0; MAX_PARTITIONS];
    let peak = bessel_maxima(scores, &mut maxima[..2 * bits], exps);
    exps.clear();
    exps.extend(scores.iter().map(|v| (v - peak).exp()));
    let mut values = [0.0; MAX_PARTITIONS];
    let mut groups = [BesselGroup::default(); MAX_PARTITIONS];
    let mut count = 0;
    for (i, (value, maximum)) in values.iter_mut().zip(maxima).take(2 * bits).enumerate() {
        let run = scores.len() >> (1 + i / 2);
        let offset = (i % 2) * run;
        if maximum.to_bits() == peak.to_bits() {
            *value = peak + score_partition(exps, run, offset).sum::<f64>().ln();
            continue;
        }
        let index = groups[..count]
            .iter()
            .position(|group| group.maximum.to_bits() == maximum.to_bits())
            .unwrap_or_else(|| {
                count += 1;
                count - 1
            });
        let group = &mut groups[index];
        group.maximum = maximum;
        group.partitions |= 1 << i;
        group.ones |= offset;
        group.zeroes |= run ^ offset;
    }
    for group in &groups[..count] {
        let mut partitions = group.partitions;
        let maximum = group.maximum;
        let shared = !partitions.is_power_of_two();
        if shared {
            // A pattern is in the union if any of its selected bits differs
            // from the one combination excluded by every member partition.
            let mask = group.ones | group.zeroes;
            let whole_block = group.ones & group.zeroes != 0;
            let stride = 1 << mask.trailing_zeros();
            for (index, (scores, exps)) in scores
                .chunks_exact(stride)
                .zip(exps.chunks_exact_mut(stride))
                .enumerate()
            {
                if whole_block || (index * stride) & mask != group.zeroes {
                    for (&score, exp) in scores.iter().zip(exps) {
                        *exp = (score - maximum).exp();
                    }
                }
            }
        }
        while partitions != 0 {
            let i = partitions.trailing_zeros() as usize;
            partitions &= partitions - 1;
            let run = scores.len() >> (1 + i / 2);
            let offset = (i % 2) * run;
            let sum = if shared {
                score_partition(exps, run, offset).sum::<f64>()
            } else {
                score_partition(scores, run, offset)
                    .map(|score| (score - maximum).exp())
                    .sum::<f64>()
            };
            values[i] = maximum + sum.ln();
        }
    }
    for (bit, value) in output.iter_mut().enumerate() {
        *value = values[2 * bit + 1] - values[2 * bit];
    }
}
struct MetricStats {
    var: f64,
    sync_power: f64,
    robust_power: f64,
    varq: OnceCell<[f64; 8]>,
}

impl MetricStats {
    fn new(s: &Spectrum) -> Self {
        let var = variance(s);
        let pilot_powers: Vec<f64> = [0, 36, 72]
            .iter()
            .flat_map(|&base| {
                (0..7)
                    .filter(move |&k| s.observed[base + k] == 32)
                    .map(move |k| s[base + k][COSTAS[k]].norm_sqr())
            })
            .collect();
        let sync_power = pilot_powers.iter().sum::<f64>() / (pilot_powers.len().max(1) as f64);
        Self {
            var,
            sync_power,
            robust_power: median(pilot_powers),
            varq: OnceCell::new(),
        }
    }
}

fn tone_variance(s: &Spectrum, var: f64) -> [f64; 8] {
    std::array::from_fn(|q| {
        (median(
            s.iter()
                .enumerate()
                .filter(|(k, _)| s.observed[*k] == 32)
                .map(|(_, r)| r[q].norm_sqr())
                .collect(),
        ) / std::f64::consts::LN_2)
            .max(var * 0.6)
    })
}

fn bessel_scores(
    symbols: &[[C; 8]],
    amp: f64,
    var: f64,
    prefixes: &mut [C; 512],
    scores: &mut [f64],
) {
    let (last, first) = symbols.split_last().expect("non-empty symbol block");
    prefixes[0] = C::new(0.0, 0.0);
    let mut parents = 1;
    for row in first {
        // Expand in reverse so the children cannot overwrite unread parents.
        // Each path retains the original complex-addition order.
        for parent in (0..parents).rev() {
            let sum = prefixes[parent];
            for (digit, &tone) in GRAY.iter().enumerate() {
                prefixes[parent * 8 + digit] = sum + row[tone] * 1.0;
            }
        }
        parents *= 8;
    }
    let (groups, _) = scores.as_chunks_mut::<8>();
    for (parent, group) in groups.iter_mut().enumerate() {
        for (score, &tone) in group.iter_mut().zip(&GRAY) {
            let sum = prefixes[parent] + last[tone] * 1.0;
            *score = log_i0(2.0 * amp * sum.norm() / var);
        }
    }
}

fn metrics(s: &Spectrum, stats: &MetricStats, block: usize, mode: &str) -> Vec<f64> {
    let var = stats.var;
    let varq = (mode == "robust").then(|| stats.varq.get_or_init(|| tone_variance(s, var)));
    let amp = (if mode == "robust" {
        stats.robust_power - var
    } else {
        stats.sync_power - var
    })
    .max(0.1 * var)
    .sqrt();
    let mut llr = vec![0.0; 174];
    let mut partition_exps = Vec::new();
    let mut bessel_prefixes = (mode == "bessel").then(|| [C::new(0.0, 0.0); 512]);
    for (half, base) in [7, 43].iter().enumerate() {
        let mut k = 0;
        while k < 29 {
            let b = block.min(29 - k);
            let count = 8usize.pow(b as u32);
            let mut scores = vec![0.0; count];
            if let Some(prefixes) = bessel_prefixes.as_mut() {
                bessel_scores(&s[base + k..base + k + b], amp, var, prefixes, &mut scores);
                let start = half * 87 + k * 3;
                partition_llrs(
                    &scores,
                    3 * b,
                    &mut partition_exps,
                    &mut llr[start..start + 3 * b],
                );
                k += b;
                continue;
            }
            for (pattern, v) in scores.iter_mut().enumerate() {
                let mut z = C::new(0.0, 0.0);
                let mut invsum = 0.0;
                for n in 0..b {
                    let bits = (pattern >> (3 * (b - 1 - n))) & 7;
                    let q = GRAY[bits];
                    let weight = varq.map_or(1.0, |v| 1.0 / v[q]);
                    z += s[base + k + n][q] * weight;
                    invsum += weight * s.observed[base + k + n] as f64 / 32.0;
                }
                *v = match mode {
                    "amp" => z.norm(),
                    "robust" => log_i0(2.0 * amp * z.norm()) - amp * amp * invsum,
                    _ => panic!("mode"),
                };
            }
            if mode == "robust" {
                let start = half * 87 + k * 3;
                partition_llrs(
                    &scores,
                    3 * b,
                    &mut partition_exps,
                    &mut llr[start..start + 3 * b],
                );
                k += b;
                continue;
            }
            for bit in 0..3 * b {
                let run = count >> (bit + 1);
                let zero = score_partition(&scores, run, 0).fold(f64::NEG_INFINITY, f64::max);
                let one = score_partition(&scores, run, run).fold(f64::NEG_INFINITY, f64::max);
                llr[half * 87 + k * 3 + bit] = one - zero;
            }
            k += b;
        }
    }
    if mode == "amp" {
        let mean = llr.iter().sum::<f64>() / 174.0;
        let sd = (llr.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / 174.0)
            .sqrt()
            .max(1e-12);
        for v in &mut llr {
            *v = *v / sd * 2.83;
        }
    }
    llr
}
fn decode_with_tones(llr: &[f64]) -> Option<(String, [i32; 79])> {
    let result = crate::decode::lib_wsjtx_plus::decode174_91::decode174_91(llr, &[0; 174], 2)?;
    if result.cw.iter().all(|&x| x == 0) {
        return None;
    }
    let message = crate::decode::packjt77::unpack77(&result.message91[..77], None)?;
    Some((
        message,
        crate::decode::genft8::get_ft8_tones_from_codeword(&result.cw),
    ))
}
fn coherent_metrics(s: &Spectrum) -> Vec<f64> {
    let var = variance(s);
    let groups: [(C, usize); 3] = std::array::from_fn(|g| {
        let mut z = C::new(0.0, 0.0);
        let mut n = 0;
        for k in 0..7 {
            let j = [0, 36, 72][g] + k;
            if s.observed[j] == 32 {
                z += s[j][COSTAS[k]];
                n += 1;
            }
        }
        (z, n)
    });
    let global = groups.iter().map(|(z, _)| *z).sum::<C>()
        / (groups.iter().map(|(_, n)| *n).sum::<usize>().max(1) as f64);
    let mut llr = Vec::with_capacity(174);
    for base in [7, 43] {
        for k in base..base + 29 {
            let channel = global;
            let scores: [f64; 8] =
                std::array::from_fn(|q| 2.0 * (s[k][GRAY[q]] * channel.conj()).re / var);
            for bit in (0..3).rev() {
                let one: Vec<f64> = scores
                    .iter()
                    .enumerate()
                    .filter(|(q, _)| (q >> bit) & 1 == 1)
                    .map(|(_, v)| *v)
                    .collect();
                let zero: Vec<f64> = scores
                    .iter()
                    .enumerate()
                    .filter(|(q, _)| (q >> bit) & 1 == 0)
                    .map(|(_, v)| *v)
                    .collect();
                llr.push(lse(&one) - lse(&zero));
            }
        }
    }
    llr
}
#[derive(Clone)]
struct Candidate {
    f: f64,
    dt: f64,
    score: f64,
    band: usize,
}
#[derive(Clone)]
struct Row {
    msg: String,
    f: f64,
    dt: f64,
    method: String,
    tones: Option<[i32; 79]>,
}
fn row_identity(a: &Row, b: &Row) -> bool {
    if (a.f - b.f).abs() >= 5.0 || (a.dt - b.dt).abs() >= 0.3 {
        return false;
    }
    let unknown = |s: &str| s.contains("<...>") || s.contains("<>");
    if a.msg == b.msg && !unknown(&a.msg) {
        return true;
    }
    let valid = |t: &[i32; 79]| {
        t.iter().all(|&v| (0..8).contains(&v))
            && [0, 36, 72]
                .iter()
                .all(|&base| (0..7).all(|k| t[base + k] == COSTAS[k] as i32))
    };
    let same_payload = match (a.tones, b.tones) {
        (Some(x), Some(y)) => x == y && valid(&x) && valid(&y),
        _ => false,
    };
    if !same_payload {
        return false;
    }
    let x: Vec<_> = a.msg.split_whitespace().collect();
    let y: Vec<_> = b.msg.split_whitespace().collect();
    if x.len() != y.len() {
        return false;
    }
    x.iter().zip(y).all(|(x, y)| {
        if *x == y {
            return true;
        }
        let plain = |s: &str| s.trim_start_matches('<').trim_end_matches('>').to_string();
        let px = plain(x);
        let py = plain(y);
        px == py
            || ((px == "..." || px.is_empty())
                && y.starts_with('<')
                && !py.is_empty()
                && py != "...")
            || ((py == "..." || py.is_empty())
                && x.starts_with('<')
                && !px.is_empty()
                && px != "...")
    })
}
fn discover(audio: &[f64], low: f64, high: f64) -> (Vec<Band>, Vec<Candidate>) {
    let mut bands = Vec::new();
    let mut candidates = Vec::new();
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(64);
    let mut input: Vec<C> = audio.iter().map(|&x| C::new(x, 0.0)).collect();
    input.resize(180000, C::new(0.0, 0.0));
    planner.plan_fft_forward(180000).process(&mut input);
    for bi in 0..28 {
        let band = Band::from_fft(&input, 250.0 + 100.0 * bi as f64, audio.len());
        let mut powers = vec![[0.0; 64]; 372];
        for (step, row) in powers.iter_mut().enumerate() {
            let mut z = vec![C::new(0.0, 0.0); 64];
            for (n, value) in z.iter_mut().enumerate().take(32) {
                let j = step * 8 + n;
                if j < band.observed_end {
                    *value = band.samples[j];
                }
            }
            fft.process(&mut z);
            for q in 0..64 {
                row[q] = z[q].norm_sqr();
            }
        }
        {
            let levels: [f64; 64] =
                std::array::from_fn(|q| median(powers.iter().map(|r| r[q]).collect()));
            let floor = median(levels.to_vec()) * 0.6;
            for row in &mut powers {
                for q in 0..64 {
                    row[q] /= levels[q].max(floor).max(1e-15);
                }
            }
        }
        // Each candidate gets its best coarse time; no target frequency/message is supplied.
        for fq in -16isize..=16 {
            let f = band.center + fq as f64 * 3.125;
            if !(low.max(200.0)..=high.min(3000.0)).contains(&f) {
                continue;
            }
            let mut best = (0.0, 0.0);
            for lag in -50isize..=62 {
                let mut good = 0.0;
                let mut off = 0.0;
                let mut count = 0;
                for base in [0isize, 36, 72] {
                    for k in 0..7 {
                        let step = lag + 4 * (base + k as isize);
                        if step < 0 || step >= powers.len() as isize {
                            continue;
                        }
                        let p = &powers[step as usize];
                        good += p[(fq + 2 * COSTAS[k] as isize).rem_euclid(64) as usize];
                        for q in 0..8 {
                            if q != COSTAS[k] {
                                off += p[(fq + 2 * q as isize).rem_euclid(64) as usize];
                            }
                        }
                        count += 1;
                    }
                }
                if count < 14 {
                    continue;
                }
                let score = 7.0 * good / off.max(1e-15);
                if score > best.0 {
                    best = (score, lag as f64 * 0.04 - 0.5);
                }
            }
            if best.0 > 1.5 {
                candidates.push(Candidate {
                    f,
                    dt: best.1,
                    score: best.0,
                    band: bi,
                });
            }
        }
        bands.push(band);
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut chosen: Vec<Candidate> = Vec::new();
    for c in candidates {
        if chosen
            .iter()
            .any(|x| (x.f - c.f).abs() < 4.0 && (x.dt - c.dt).abs() < 0.08)
        {
            continue;
        }
        chosen.push(c);
        if chosen.len() == MAX_CANDIDATES {
            break;
        }
    }
    (bands, chosen)
}
fn refine(band: &Band, c: &Candidate) -> Vec<Candidate> {
    let mut grid = Vec::new();
    for df in -3..=3 {
        let f = c.f + df as f64 * 0.5;
        let oscillators = band.sync_oscillators(f);
        for dt in -4..=4 {
            let t = c.dt + dt as f64 * 0.01;
            grid.push(Candidate {
                f,
                dt: t,
                score: band.sync_stats(t, &oscillators).0,
                band: c.band,
            });
        }
    }
    grid.sort_by(|a, b| b.score.total_cmp(&a.score));
    let best = grid[0].clone();
    for df in -1..=1 {
        let f = best.f + df as f64 * 0.125;
        let oscillators = band.sync_oscillators(f);
        for dt in -1..=1 {
            let t = best.dt + dt as f64 * 0.005;
            grid.push(Candidate {
                f,
                dt: t,
                score: band.sync_stats(t, &oscillators).0,
                band: c.band,
            });
        }
    }
    grid.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out: Vec<Candidate> = Vec::new();
    for c in grid {
        if !out
            .iter()
            .any(|p| (p.f - c.f).abs() < 0.2 && (p.dt - c.dt).abs() < 0.009)
        {
            out.push(c);
        }
        if out.len() == 4 {
            break;
        }
    }
    out
}
fn refine_coherent(band: &Band, c: &Candidate) -> Vec<Candidate> {
    let mut grid = Vec::new();
    for df in -40..=40 {
        let f = c.f + df as f64 * 0.025;
        let oscillators = band.sync_oscillators(f);
        for dt in -8..=8 {
            let t = c.dt + dt as f64 * 0.005;
            grid.push(Candidate {
                f,
                dt: t,
                score: band.sync_stats(t, &oscillators).1,
                band: c.band,
            });
        }
    }
    grid.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out: Vec<Candidate> = Vec::new();
    for c in grid {
        if !out
            .iter()
            .any(|p| (p.f - c.f).abs() < 0.05 && (p.dt - c.dt).abs() < 0.009)
        {
            out.push(c);
        }
        if out.len() == 8 {
            break;
        }
    }
    out
}
fn worker_count() -> usize {
    static WORKERS: OnceLock<usize> = OnceLock::new();
    *WORKERS
        .get_or_init(|| std::thread::available_parallelism().map_or(1, |count| count.get().min(6)))
}

fn search(
    audio: &[f64],
    low: f64,
    high: f64,
    candidate_limit: usize,
    budget: &mut Budget,
) -> Vec<Row> {
    search_with_workers(audio, low, high, candidate_limit, budget, worker_count())
}

fn search_one(
    band: &Band,
    c: &Candidate,
    ci: usize,
    prior: &[Row],
    budget: &mut Budget,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let hypotheses = refine(band, c);
    for (hi, h) in hypotheses.iter().enumerate() {
        if budget.exhausted() {
            break;
        }
        let s = band.spectra(h.f, h.dt, 0.0);
        let stats = MetricStats::new(&s);
        for (block, mode) in [(1, "amp"), (3, "amp"), (4, "bessel"), (3, "robust")] {
            if let Some((msg, tones)) = budget.decode(&metrics(&s, &stats, block, mode)) {
                let row = Row {
                    msg,
                    f: h.f,
                    dt: h.dt,
                    method: format!("h{hi}-{block}-{mode}"),
                    tones: Some(tones),
                };
                if !prior.iter().chain(&rows).any(|old| row_identity(old, &row)) {
                    rows.push(row);
                }
                break;
            }
        }
        if prior
            .iter()
            .chain(&rows)
            .any(|r| (r.f - h.f).abs() < 3.0 && (r.dt - h.dt).abs() < 0.2)
        {
            break;
        }
    }
    if ci < 32
        && !prior
            .iter()
            .chain(&rows)
            .any(|r| (r.f - c.f).abs() < 3.0 && (r.dt - c.dt).abs() < 0.2)
    {
        if budget.exhausted() {
            return rows;
        }
        for (hi, h) in refine_coherent(band, &hypotheses[0]).iter().enumerate() {
            if budget.exhausted() {
                break;
            }
            let s = band.spectra(h.f, h.dt, 0.0);
            if let Some((msg, tones)) = budget.decode(&coherent_metrics(&s)) {
                let row = Row {
                    msg,
                    f: h.f,
                    dt: h.dt,
                    method: format!("phase-{hi}"),
                    tones: Some(tones),
                };
                if !prior.iter().chain(&rows).any(|old| row_identity(old, &row)) {
                    rows.push(row);
                }
                break;
            }
        }
    }
    rows
}

fn search_with_workers(
    audio: &[f64],
    low: f64,
    high: f64,
    candidate_limit: usize,
    budget: &mut Budget,
    width: usize,
) -> Vec<Row> {
    if budget.exhausted() {
        return Vec::new();
    }
    let (bands, candidates) = discover(audio, low, high);
    let candidates = &candidates[..candidates.len().min(candidate_limit)];
    let width = width.max(1);
    let worst = candidates.len() * 16 + candidates.len().min(32) * 8;
    let rows = if width == 1 || worst >= MAX_FEC - budget.diagnostics.fec_calls {
        let mut rows = Vec::new();
        for (ci, c) in candidates.iter().enumerate() {
            if budget.exhausted() {
                break;
            }
            let result = search_one(&bands[c.band], c, ci, &rows, budget);
            rows.extend(result);
        }
        rows
    } else {
        let mut result = vec![None::<Vec<Row>>; candidates.len()];
        let mut todo = candidates.len();
        while todo > 0 && !budget.exhausted() {
            // Each hypothesis stays within 2.625 Hz of its coarse candidate;
            // a prior row affects this candidate only within 5 Hz. Unfinished
            // candidates more than 11 Hz apart can therefore run independently.
            let ready: Vec<_> = candidates
                .iter()
                .enumerate()
                .filter(|(i, c)| {
                    result[*i].is_none()
                        && candidates[..*i].iter().enumerate().all(|(j, previous)| {
                            result[j].is_some() || (c.f - previous.f).abs() > 11.0
                        })
                })
                .map(|(i, _)| i)
                .take(width)
                .collect();
            assert!(!ready.is_empty());
            let finished = &result;
            let deadline = budget.deadline;
            let solve = |ci: usize| {
                let c = &candidates[ci];
                let prior: Vec<_> = finished[..ci]
                    .iter()
                    .flatten()
                    .flatten()
                    .filter(|r| (r.f - c.f).abs() <= 12.0)
                    .cloned()
                    .collect();
                let mut local = Budget {
                    deadline,
                    low,
                    high,
                    diagnostics: ExperimentDiagnostics::default(),
                };
                let rows = search_one(&bands[c.band], c, ci, &prior, &mut local);
                (
                    ci,
                    rows,
                    local.diagnostics.fec_calls,
                    local.diagnostics.budget_exhausted,
                )
            };
            let work = std::thread::scope(|scope| {
                let children: Vec<_> = ready[1..]
                    .iter()
                    .map(|&ci| {
                        let solve = &solve;
                        (
                            ci,
                            std::thread::Builder::new().spawn_scoped(scope, move || solve(ci)),
                        )
                    })
                    .collect();
                let head = solve(ready[0]);
                std::iter::once(head)
                    .chain(children.into_iter().map(|(ci, task)| match task {
                        Ok(task) => task.join().expect("experimental candidate worker panicked"),
                        Err(_) => solve(ci),
                    }))
                    .collect::<Vec<_>>()
            });
            todo -= work.len();
            for (ci, rows, calls, cut) in work {
                let max_calls = if ci < 32 { MAX_FEC_PER_CANDIDATE } else { 16 };
                assert!(calls <= max_calls);
                budget.diagnostics.fec_calls += calls;
                budget.diagnostics.budget_exhausted |= cut;
                result[ci] = Some(rows);
            }
        }
        result.into_iter().flatten().flatten().collect()
    };
    budget.diagnostics.tentative_rows += rows.len();
    rows
}
fn clean_lines(audio: &[f64]) -> Vec<f64> {
    let mut out = audio.to_vec();
    for _ in 0..3 {
        let mut fft: Vec<C> = out.iter().map(|&x| C::new(x, 0.0)).collect();
        fft.resize(180000, C::new(0.0, 0.0));
        FftPlanner::<f64>::new()
            .plan_fft_forward(180000)
            .process(&mut fft);
        let mut peaks: Vec<usize> = (3000..45000)
            .filter(|&i| {
                fft[i].norm_sqr() > fft[i - 1].norm_sqr()
                    && fft[i].norm_sqr() > fft[i + 1].norm_sqr()
            })
            .collect();
        peaks.sort_by(|&a, &b| fft[b].norm_sqr().total_cmp(&fft[a].norm_sqr()));
        peaks.truncate(16);
        let mut accepted = None;
        for bin in peaks {
            let local = median(
                (bin - 150..=bin + 150)
                    .filter(|&i| i.abs_diff(bin) > 2)
                    .map(|i| fft[i].norm_sqr())
                    .collect(),
            );
            if fft[bin].norm_sqr() < 200.0 * local {
                continue;
            }
            let coarse = bin as f64 / 15.0;
            let band = Band::new(&out, coarse);
            let evaluate = |f: f64| -> (f64, Vec<C>) {
                let mut phase = C::new(1.0, 0.0);
                let step = C::from_polar(1.0, -TAU * (f - band.center) / 200.0);
                let mut chunks = vec![C::new(0.0, 0.0); 15];
                for (n, &z) in band.samples.iter().enumerate() {
                    chunks[n / 200] += z * phase;
                    phase *= step;
                }
                (chunks.iter().sum::<C>().norm_sqr(), chunks)
            };
            let mut best = (0.0, coarse);
            for i in -10..=10 {
                let f = coarse + i as f64 / 300.0;
                let score = evaluate(f).0;
                if score > best.0 {
                    best = (score, f);
                }
            }
            let anchor = best.1;
            for i in -10..=10 {
                let f = anchor + i as f64 / 3000.0;
                let score = evaluate(f).0;
                if score > best.0 {
                    best = (score, f);
                }
            }
            let h = 1.0 / 3000.0;
            let left = evaluate(best.1 - h).0;
            let right = evaluate(best.1 + h).0;
            let curvature = left - 2.0 * best.0 + right;
            if curvature < 0.0 {
                best.1 += ((left - right) / (2.0 * curvature)).clamp(-1.0, 1.0) * h;
            }
            let (score, chunks) = evaluate(best.1);
            let coherence = score / (15.0 * chunks.iter().map(C::norm_sqr).sum::<f64>()).max(1e-15);
            if coherence > 0.85 {
                accepted = Some((best.1, coherence));
                break;
            }
        }
        let Some((f, _coherence)) = accepted else {
            break;
        };
        let mut aa = 0.0;
        let mut bb = 0.0;
        let mut ab = 0.0;
        let mut ya = 0.0;
        let mut yb = 0.0;
        let mut z = C::new(1.0, 0.0);
        let step = C::from_polar(1.0, TAU * f / FS);
        for &y in &out {
            aa += z.re * z.re;
            bb += z.im * z.im;
            ab += z.re * z.im;
            ya += y * z.re;
            yb += y * z.im;
            z *= step;
        }
        let det = (aa * bb - ab * ab).max(1e-15);
        let a = (ya * bb - yb * ab) / det;
        let b = (yb * aa - ya * ab) / det;
        z = C::new(1.0, 0.0);
        for y in &mut out {
            *y -= a * z.re + b * z.im;
            z *= step;
        }
    }
    out
}
struct ExplainedSignal {
    row: Row,
    wave: Vec<f64>,
    score: f64,
}
fn conditioned_audio(audio: &[f64], models: &[ExplainedSignal], candidate: &Row) -> Vec<f64> {
    let mut residual = audio.to_vec();
    for model in models {
        if (model.row.f - candidate.f).abs() <= 80.0 {
            for (x, &y) in residual.iter_mut().zip(&model.wave) {
                *x -= y;
            }
        }
    }
    residual
}
fn emit_fit(
    row: &Row,
    fit: WaveformFit,
    diagnostics: &mut ExperimentDiagnostics,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) {
    emit(&StreamDecodedMessage {
        freq: fit.f,
        dt: fit.dt,
        snr: 10.0 * (fit.score / (12.64 * 2500.0)).log10(),
        msg: row.msg.clone(),
        sync: fit.score,
        itone: row.tones.expect("FEC tones"),
    });
    diagnostics.accepted_rows += 1;
    diagnostics.accepted_scores.push(fit.score);
    diagnostics.accepted_data_scores.push(fit.data_score);
    diagnostics.accepted_methods.push(row.method.clone());
}
fn admit(
    audio: &[f64],
    ranking_audio: &[f64],
    candidates: Vec<Row>,
    models: &mut Vec<ExplainedSignal>,
    outputs: &mut Vec<Row>,
    budget: &mut Budget,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) {
    let mut ranked = Vec::new();
    for mut row in candidates {
        if budget.exhausted() {
            return;
        }
        if outputs.iter().any(|old| row_identity(old, &row))
            || models.iter().any(|model| row_identity(&model.row, &row))
        {
            continue;
        }
        if let Some(fit) = fit_message(ranking_audio, &row, budget.deadline) {
            row.f = fit.f;
            row.dt = fit.dt;
            if fit.supported() {
                ranked.push((fit.score, row));
            } else {
                budget.diagnostics.rejected_rows += 1;
                budget.diagnostics.data_support_rejections += usize::from(!fit.data_supported());
            }
        }
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.f.total_cmp(&b.1.f)));
    for (_, mut row) in ranked {
        if budget.exhausted() {
            return;
        }
        let residual = conditioned_audio(audio, models, &row);
        let Some(fit) = fit_message(&residual, &row, budget.deadline) else {
            continue;
        };
        if !fit.supported() || !(budget.low..=budget.high).contains(&fit.f) {
            budget.diagnostics.rejected_rows += 1;
            budget.diagnostics.data_support_rejections += usize::from(!fit.data_supported());
            continue;
        }
        if budget.exhausted() {
            return;
        }
        let WaveformFit { f, dt, score, .. } = fit;
        row.f = f;
        row.dt = dt;
        let cleaned = cancel_smooth(&residual, &row, f, dt);
        let wave = residual.iter().zip(cleaned).map(|(&x, y)| x - y).collect();
        emit_fit(&row, fit, &mut budget.diagnostics, emit);
        models.push(ExplainedSignal {
            row: row.clone(),
            wave,
            score,
        });
        outputs.push(row);
    }
}
/// Bounded, independently checked nuisance models. Neither the native rows nor
/// their provenance, audio or state are changed by this fit. Exp rows cannot
/// become native seeds, and these models are never published or fed back to AP.
fn native_models(
    audio: &[f64],
    baseline: &[StreamDecodedMessage],
    budget: &mut Budget,
) -> Vec<ExplainedSignal> {
    let mut seeds: Vec<_> = baseline
        .iter()
        .filter(|row| {
            row.freq.is_finite()
                && row.dt.is_finite()
                && row.snr.is_finite()
                && row.dt.abs() <= 2.5
                && (budget.low - 80.0..=budget.high + 80.0).contains(&row.freq)
                && valid_tones(&row.itone)
        })
        .collect();
    seeds.sort_by(|a, b| {
        b.snr
            .total_cmp(&a.snr)
            .then_with(|| a.freq.total_cmp(&b.freq))
    });
    seeds.truncate(8);
    budget.diagnostics.native_seed_candidates = seeds.len();
    let mut models = Vec::new();
    for seed in seeds {
        if budget.exhausted() {
            break;
        }
        let mut row = Row {
            msg: seed.msg.clone(),
            f: seed.freq,
            dt: seed.dt,
            method: "native-model".into(),
            tones: Some(seed.itone),
        };
        if models
            .iter()
            .any(|model: &ExplainedSignal| row_identity(&model.row, &row))
        {
            continue;
        }
        let residual = conditioned_audio(audio, &models, &row);
        let Some(fit) = fit_message(&residual, &row, budget.deadline) else {
            continue;
        };
        if !fit.supported() {
            budget.diagnostics.native_seed_rejections += 1;
            continue;
        }
        if budget.exhausted() {
            break;
        }
        row.f = fit.f;
        row.dt = fit.dt;
        let cleaned = cancel_smooth(&residual, &row, fit.f, fit.dt);
        models.push(ExplainedSignal {
            row,
            score: fit.score,
            wave: residual.iter().zip(cleaned).map(|(&x, y)| x - y).collect(),
        });
        budget.diagnostics.native_seed_models += 1;
    }
    models
}

fn valid_tones(tones: &[i32; 79]) -> bool {
    tones.iter().all(|&tone| (0..8).contains(&tone))
        && [0, 36, 72]
            .iter()
            .all(|&base| (0..7).all(|k| tones[base + k] == COSTAS[k] as i32))
}

/// Called only by Hybrid, after all original workers have emitted their rows.
/// A finite search can finish an in-flight FEC/fit slightly beyond the deadline.
pub(super) fn run(
    input: ExperimentInput<'_>,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) -> ExperimentDiagnostics {
    let ExperimentInput {
        samples,
        baseline,
        low,
        high,
        deadline,
    } = input;
    let start = Instant::now();
    let mut budget = Budget {
        deadline,
        low: low.max(200.0),
        high: high.min(3000.0),
        diagnostics: ExperimentDiagnostics::default(),
    };
    if samples.len() < 120000
        || samples.len() > 180000
        || samples.iter().any(|x| !x.is_finite())
        || !low.is_finite()
        || !high.is_finite()
        || low.max(200.0) >= high.min(3000.0)
        || budget.exhausted()
    {
        return budget.diagnostics;
    }
    let original: Vec<f64> = samples.iter().map(|&x| f64::from(x)).collect();
    if original.iter().all(|x| *x == 0.0) {
        return budget.diagnostics;
    }
    let conditioning = Instant::now();
    let audio = clean_lines(&original);
    let mut models = native_models(&audio, baseline, &mut budget);
    budget.diagnostics.conditioning = conditioning.elapsed();
    let initial_search = Instant::now();
    let candidates = search(&audio, budget.low, budget.high, MAX_CANDIDATES, &mut budget);
    let mut outputs = Vec::new();
    admit(
        &audio,
        &audio,
        candidates,
        &mut models,
        &mut outputs,
        &mut budget,
        emit,
    );
    budget.diagnostics.initial_search = initial_search.elapsed();
    if !models.is_empty() && !budget.exhausted() {
        let residual_search = Instant::now();
        let mut order: Vec<usize> = (0..models.len()).collect();
        order.sort_by(|&a, &b| models[b].score.total_cmp(&models[a].score));
        order.truncate(8);
        let mut residual = audio.clone();
        for i in order {
            for (x, &y) in residual.iter_mut().zip(&models[i].wave) {
                *x -= y;
            }
        }
        budget.diagnostics.residual_searches = 1;
        let candidates = search(
            &residual,
            budget.low,
            budget.high,
            MAX_CANDIDATES,
            &mut budget,
        );
        admit(
            &audio,
            &residual,
            candidates,
            &mut models,
            &mut outputs,
            &mut budget,
            emit,
        );
        budget.diagnostics.residual_search = residual_search.elapsed();
    }
    budget.diagnostics.elapsed = start.elapsed();
    budget.diagnostics
}

/// Terminal recovery on private audio. Its budget comes from the original
/// worker and its candidates cannot become cancellation or AP seeds.
pub(crate) fn recover_impulses(
    samples: &[f32],
    low: f64,
    high: f64,
    deadline: Instant,
    previous: &ExperimentDiagnostics,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) -> Option<ExperimentDiagnostics> {
    let start = Instant::now();
    if !low.is_finite() || !high.is_finite() {
        return None;
    }
    let samples = &samples[..samples.len().min(180_000)];
    let (low, high) = (low.max(200.0), high.min(3000.0));
    // Stop below MAX_FEC so a candidate already found can still be fitted.
    let limit = (MAX_FEC.saturating_sub(previous.fec_calls + 1) / MAX_FEC_PER_CANDIDATE)
        .min(PULSE_CANDIDATES);
    if limit == 0
        || start >= deadline
        || low >= high
        || samples.len() < 120_000
        || samples.iter().any(|x| !x.is_finite())
    {
        return None;
    }
    let (blanked, touched) = impulse::blank(samples)?;
    let mut budget = Budget {
        deadline,
        low,
        high,
        diagnostics: previous.clone(),
    };
    budget.diagnostics.impulse_blanked_samples = touched;
    for mut row in search(&blanked, low, high, limit, &mut budget) {
        if budget.exhausted() {
            break;
        }
        let Some(fit) = fit_message(&blanked, &row, deadline) else {
            continue;
        };
        if !fit.supported() || !(low..=high).contains(&fit.f) {
            budget.diagnostics.rejected_rows += 1;
            budget.diagnostics.data_support_rejections += usize::from(!fit.data_supported());
            continue;
        }
        if budget.exhausted() {
            break;
        }
        row.method.insert_str(0, "impulse-");
        emit_fit(&row, fit, &mut budget.diagnostics, emit);
        budget.diagnostics.impulse_accepted_rows += 1;
    }
    budget.diagnostics.elapsed += start.elapsed();
    Some(budget.diagnostics)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "drift_tests.rs"]
mod drift_tests;

#[cfg(test)]
#[path = "polar_tests.rs"]
mod polar_tests;
