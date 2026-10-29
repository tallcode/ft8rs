//! Hybrid-only receive DSP, developed independently in experiments/single-slot.
//! Protocol, waveform synthesis and FEC call unchanged upstream routines.
//! This private module has no standalone profile, CallDB or persistent AP state.
use crate::stream::session::StreamDecodedMessage;
use rustfft::{num_complex::Complex64 as C, FftPlanner};
use std::f64::consts::TAU;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
const FS: f64 = 12000.0;
const GRAY: [usize; 8] = [0, 1, 3, 2, 5, 6, 4, 7];
const COSTAS: [usize; 7] = [3, 1, 4, 0, 6, 5, 2];
// Development-only calibration: the earlier incorrect codeword scored 103.186.
// A declared 50% margin rounded upward gives 160. This is not a p-value.
const MIN_SCORE: f64 = 160.0;
const MAX_FEC: usize = 4096;
#[derive(Clone, Debug, Default)]
pub struct SpecialistDiagnostics {
    pub fec_calls: usize,
    pub tentative_rows: usize,
    pub rejected_rows: usize,
    pub accepted_rows: usize,
    pub residual_searches: usize,
    pub budget_exhausted: bool,
    pub elapsed: Duration,
    pub accepted_scores: Vec<f64>,
    pub accepted_methods: Vec<String>,
    pub error: Option<String>,
}
struct Budget {
    deadline: Instant,
    low: f64,
    high: f64,
    diagnostics: SpecialistDiagnostics,
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
    fn sync_stats(&self, f: f64, dt: f64, drift: f64) -> (f64, f64) {
        let start = ((dt + 0.5) * 200.0).round() as isize;
        let mut power = 0.0;
        let mut total = 0.0;
        let mut count = 0;
        let mut coherent = C::new(0.0, 0.0);
        for base in [0, 36, 72] {
            for (k, &tone) in COSTAS.iter().enumerate() {
                let j0 = start + 32 * (base + k) as isize;
                if j0 < 0 || j0 + 31 >= self.observed_end as isize {
                    continue;
                }
                let mut z = C::new(0.0, 0.0);
                let mut en = 0.0;
                let t0 = (j0 - start) as f64 / 200.0;
                let df = f - self.center;
                let mut osc = C::from_polar(1.0, -TAU * (df * t0 + 0.5 * drift * t0 * t0));
                let mut step = C::from_polar(
                    1.0,
                    -TAU * ((df + drift * t0) / 200.0 + 0.5 * drift / 40000.0),
                );
                let accel = C::from_polar(1.0, -TAU * drift / 40000.0);
                for n in 0..32 {
                    let j = j0 + n as isize;
                    let sample = self.samples[j as usize];
                    z += sample * basis()[tone][n] * osc;
                    osc *= step;
                    step *= accel;
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
    fn sync_score(&self, f: f64, dt: f64, drift: f64) -> f64 {
        self.sync_stats(f, dt, drift).0
    }
}
fn median(mut x: Vec<f64>) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.sort_by(f64::total_cmp);
    x[x.len() / 2]
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
fn metrics(s: &Spectrum, block: usize, mode: &str) -> Vec<f64> {
    let var = variance(s);
    let varq: [f64; 8] = std::array::from_fn(|q| {
        (median(
            s.iter()
                .enumerate()
                .filter(|(k, _)| s.observed[*k] == 32)
                .map(|(_, r)| r[q].norm_sqr())
                .collect(),
        ) / std::f64::consts::LN_2)
            .max(var * 0.6)
    });
    let pilot_powers: Vec<f64> = [0, 36, 72]
        .iter()
        .flat_map(|&base| {
            (0..7)
                .filter(move |&k| s.observed[base + k] == 32)
                .map(move |k| s[base + k][COSTAS[k]].norm_sqr())
        })
        .collect();
    let sync_power = pilot_powers.iter().sum::<f64>() / (pilot_powers.len().max(1) as f64);
    let robust_power = median(pilot_powers);
    let amp = (if mode == "robust" {
        robust_power - var
    } else {
        sync_power - var
    })
    .max(0.1 * var)
    .sqrt();
    let mut llr = vec![0.0; 174];
    for (half, base) in [7, 43].iter().enumerate() {
        let mut k = 0;
        while k < 29 {
            let b = block.min(29 - k);
            let count = 8usize.pow(b as u32);
            let mut scores = vec![0.0; count];
            for (pattern, v) in scores.iter_mut().enumerate() {
                let mut z = C::new(0.0, 0.0);
                let mut invsum = 0.0;
                for n in 0..b {
                    let bits = (pattern >> (3 * (b - 1 - n))) & 7;
                    let q = GRAY[bits];
                    let weight = if mode == "robust" { 1.0 / varq[q] } else { 1.0 };
                    z += s[base + k + n][q] * weight;
                    invsum += weight * s.observed[base + k + n] as f64 / 32.0;
                }
                *v = match mode {
                    "amp" => z.norm(),
                    "power" => z.norm_sqr(),
                    "lse" => {
                        z.norm_sqr()
                            / (((0..b)
                                .map(|n| s.observed[base + k + n] as f64 / 32.0)
                                .sum::<f64>())
                                * var)
                                .max(1e-12)
                    }
                    "bessel" => log_i0(2.0 * amp * z.norm() / var),
                    "robust" => log_i0(2.0 * amp * z.norm()) - amp * amp * invsum,
                    _ => panic!("mode"),
                };
            }
            for bit in 0..3 * b {
                let mut a = Vec::new();
                let mut c = Vec::new();
                for (p, &v) in scores.iter().enumerate() {
                    if (p >> (3 * b - 1 - bit)) & 1 == 1 {
                        a.push(v)
                    } else {
                        c.push(v)
                    }
                }
                let value = if mode == "amp" || mode == "power" {
                    a.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                        - c.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                } else {
                    lse(&a) - lse(&c)
                };
                llr[half * 87 + k * 3 + bit] = value;
            }
            k += b;
        }
    }
    if mode == "amp" || mode == "power" {
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
    let result = crate::decode::decode174_91::decode174_91(llr, &[0; 174], 2)?;
    if result.cw.iter().all(|&x| x == 0) {
        return None;
    }
    let message = crate::decode::packjt77::unpack77(&result.message91[..77], None)?;
    Some((
        message,
        crate::decode::genft8::get_ft8_tones_from_codeword(&result.cw),
    ))
}
fn coherent_metrics(s: &Spectrum, scheme: usize) -> Vec<f64> {
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
    let h: [C; 3] = std::array::from_fn(|g| {
        if groups[g].1 > 0 {
            groups[g].0 / groups[g].1 as f64
        } else {
            global
        }
    });
    let mut llr = Vec::with_capacity(174);
    for base in [7, 43] {
        for k in base..base + 29 {
            let channel = match scheme {
                0 => global,
                1 => {
                    h[if k < 21 {
                        0
                    } else if k < 57 {
                        1
                    } else {
                        2
                    }]
                }
                _ => {
                    let x = ((k as f64 - 3.0) / 36.0).clamp(0.0, 2.0);
                    let lo = (x.floor() as usize).min(1);
                    h[lo] * (1.0 - (x - lo as f64)) + h[lo + 1] * (x - lo as f64)
                }
            };
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
        if chosen.len() == 96 {
            break;
        }
    }
    (bands, chosen)
}
fn refine(band: &Band, c: &Candidate) -> Vec<Candidate> {
    let mut grid = Vec::new();
    for df in -3..=3 {
        for dt in -4..=4 {
            let f = c.f + df as f64 * 0.5;
            let t = c.dt + dt as f64 * 0.01;
            grid.push(Candidate {
                f,
                dt: t,
                score: band.sync_score(f, t, 0.0),
                band: c.band,
            });
        }
    }
    grid.sort_by(|a, b| b.score.total_cmp(&a.score));
    let best = grid[0].clone();
    for df in -1..=1 {
        for dt in -1..=1 {
            let f = best.f + df as f64 * 0.125;
            let t = best.dt + dt as f64 * 0.005;
            grid.push(Candidate {
                f,
                dt: t,
                score: band.sync_score(f, t, 0.0),
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
        for dt in -8..=8 {
            let f = c.f + df as f64 * 0.025;
            let t = c.dt + dt as f64 * 0.005;
            grid.push(Candidate {
                f,
                dt: t,
                score: band.sync_stats(f, t, 0.0).1,
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
fn search(audio: &[f64], low: f64, high: f64, budget: &mut Budget) -> Vec<Row> {
    if budget.exhausted() {
        return Vec::new();
    }
    let (bands, candidates) = discover(audio, low, high);
    let mut rows: Vec<Row> = Vec::new();
    for (ci, c) in candidates.into_iter().enumerate() {
        if budget.exhausted() {
            break;
        }
        let band = &bands[c.band];
        let hypotheses = refine(band, &c);
        for (hi, h) in hypotheses.iter().enumerate() {
            if budget.exhausted() {
                break;
            }
            let s = band.spectra(h.f, h.dt, 0.0);
            for (block, mode) in [(1, "amp"), (3, "amp"), (4, "bessel"), (3, "robust")] {
                if let Some((msg, tones)) = budget.decode(&metrics(&s, block, mode)) {
                    let row = Row {
                        msg,
                        f: h.f,
                        dt: h.dt,
                        method: format!("h{hi}-{block}-{mode}"),
                        tones: Some(tones),
                    };
                    if !rows.iter().any(|old| row_identity(old, &row)) {
                        rows.push(row);
                    }
                    break;
                }
            }
            if rows
                .iter()
                .any(|r| (r.f - h.f).abs() < 3.0 && (r.dt - h.dt).abs() < 0.2)
            {
                break;
            }
        }
        if ci < 32
            && !rows
                .iter()
                .any(|r| (r.f - c.f).abs() < 3.0 && (r.dt - c.dt).abs() < 0.2)
        {
            if budget.exhausted() {
                break;
            }
            for (hi, h) in refine_coherent(band, &hypotheses[0]).iter().enumerate() {
                if budget.exhausted() {
                    break;
                }
                let s = band.spectra(h.f, h.dt, 0.0);
                if let Some((msg, tones)) = budget.decode(&coherent_metrics(&s, 0)) {
                    let row = Row {
                        msg,
                        f: h.f,
                        dt: h.dt,
                        method: format!("phase-{hi}"),
                        tones: Some(tones),
                    };
                    if !rows.iter().any(|old| row_identity(old, &row)) {
                        rows.push(row);
                    }
                    break;
                }
            }
        }
    }
    budget.diagnostics.tentative_rows += rows.len();
    rows
}
fn fit_message(audio: &[f64], row: &Row) -> Option<(f64, f64, f64)> {
    let tones = row.tones?;
    if [0, 36, 72]
        .iter()
        .any(|&base| (0..7).any(|k| tones[base + k] != COSTAS[k] as i32))
    {
        return None;
    }
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, 0.0);
    let band = Band::new(audio, row.f + 22.0);
    let evaluate = |f: f64, dt: f64| -> f64 {
        let start = (dt + 0.5) * 200.0;
        let rotation = C::from_polar(1.0, TAU * (f - band.center) / 200.0);
        let first = start.ceil().max(0.0) as usize;
        let last = (start + re.len() as f64 / 60.0)
            .floor()
            .min(band.observed_end as f64 - 1.0) as usize;
        let mut phase = C::from_polar(
            1.0,
            TAU * (f - band.center) * (first as f64 - start) / 200.0,
        );
        let mut z = C::new(0.0, 0.0);
        let mut en = 0.0;
        // Interpolate the known 12-kHz template, never the 200-Hz observation.
        // Interpolating the observation biases timing toward integer sample positions.
        for j in first..=last {
            let source = (j as f64 - start) * 60.0;
            let i = source.floor() as usize;
            let x = source - i as f64;
            if i + 1 >= re.len() {
                break;
            }
            let v = C::new(
                re[i] * (1.0 - x) + re[i + 1] * x,
                im[i] * (1.0 - x) + im[i + 1] * x,
            ) * phase;
            z += band.samples[j] * v.conj();
            en += v.norm_sqr();
            phase *= rotation;
        }
        z.norm_sqr() / en.max(1e-15)
    };
    let mut best = (0.0, row.f, row.dt);
    for df in -40..=40 {
        for dt in -4..=4 {
            let f = row.f + df as f64 * 0.025;
            let t = row.dt + dt as f64 * 0.005;
            let score = evaluate(f, t);
            if score > best.0 {
                best = (score, f, t);
            }
        }
    }
    let anchor = best;
    for df in -3..=3 {
        for dt in -2..=2 {
            let f = anchor.1 + df as f64 * 0.005;
            let t = anchor.2 + dt as f64 * 0.001;
            let score = evaluate(f, t);
            if score > best.0 {
                best = (score, f, t);
            }
        }
    }
    let anchor = best;
    for df in -5..=5 {
        for dt in -2..=2 {
            let f = anchor.1 + df as f64 * 0.0005;
            let t = anchor.2 + dt as f64 * 0.00025;
            let score = evaluate(f, t);
            if score > best.0 {
                best = (score, f, t);
            }
        }
    }
    let n0 = variance(&band.spectra(best.1, best.2, 0.0)) / 32.0;
    Some((best.1, best.2, best.0 / n0))
}
fn cancel_smooth(audio: &[f64], row: &Row, f: f64, dt: f64) -> Vec<f64> {
    let tones = row.tones.expect("validated original tones");
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, f);
    let start = ((dt + 0.5) * FS).round() as isize;
    let hop = 1920usize;
    let radius = 1920isize;
    let nodes = re.len().div_ceil(hop) + 1;
    let mut coeff = Vec::new();
    for node in 0..nodes {
        let center = (node * hop) as isize;
        let mut aa = 0.0;
        let mut bb = 0.0;
        let mut ab = 0.0;
        let mut ya = 0.0;
        let mut yb = 0.0;
        for offset in -radius..=radius {
            let i = center + offset;
            let j = start + i;
            if i < 0 || i >= re.len() as isize || j < 0 || j >= audio.len() as isize {
                continue;
            }
            let w = 0.5 + 0.5 * (std::f64::consts::PI * offset as f64 / radius as f64).cos();
            let a = re[i as usize];
            let b = im[i as usize];
            let y = audio[j as usize];
            aa += w * a * a;
            bb += w * b * b;
            ab += w * a * b;
            ya += w * y * a;
            yb += w * y * b;
        }
        let det = (aa * bb - ab * ab).max(1e-15);
        coeff.push(((ya * bb - yb * ab) / det, (yb * aa - ya * ab) / det));
    }
    let mut out = audio.to_vec();
    for i in 0..re.len() {
        let j = start + i as isize;
        if j < 0 || j >= out.len() as isize {
            continue;
        }
        let node = i / hop;
        let x = (i % hop) as f64 / hop as f64;
        let a = coeff[node].0 * (1.0 - x) + coeff[node + 1].0 * x;
        let b = coeff[node].1 * (1.0 - x) + coeff[node + 1].1 * x;
        out[j as usize] -= a * re[i] + b * im[i];
    }
    out
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
        if outputs.iter().any(|old| row_identity(old, &row)) {
            continue;
        }
        if let Some((f, dt, score)) = fit_message(ranking_audio, &row) {
            row.f = f;
            row.dt = dt;
            if score > MIN_SCORE {
                ranked.push((score, row));
            } else {
                budget.diagnostics.rejected_rows += 1;
            }
        }
    }
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.f.total_cmp(&b.1.f)));
    for (_, mut row) in ranked {
        if budget.exhausted() {
            return;
        }
        let residual = conditioned_audio(audio, models, &row);
        let Some((f, dt, score)) = fit_message(&residual, &row) else {
            continue;
        };
        if score <= MIN_SCORE || !score.is_finite() || !(budget.low..=budget.high).contains(&f) {
            budget.diagnostics.rejected_rows += 1;
            continue;
        }
        row.f = f;
        row.dt = dt;
        let cleaned = cancel_smooth(&residual, &row, f, dt);
        let wave = residual.iter().zip(cleaned).map(|(&x, y)| x - y).collect();
        let decode = StreamDecodedMessage {
            freq: f,
            dt,
            snr: 10.0 * (score / (12.64 * 2500.0)).log10(),
            msg: row.msg.clone(),
            sync: score,
            itone: row.tones.expect("FEC tones"),
        };
        emit(&decode);
        budget.diagnostics.accepted_rows += 1;
        budget.diagnostics.accepted_scores.push(score);
        budget.diagnostics.accepted_methods.push(row.method.clone());
        models.push(ExplainedSignal {
            row: row.clone(),
            wave,
            score,
        });
        outputs.push(row);
    }
}
/// Called only by Hybrid, after all original workers have emitted their rows.
/// A finite search can finish an in-flight FEC/fit slightly beyond the deadline.
pub(super) fn run(
    samples: &[f32],
    low: f64,
    high: f64,
    deadline: Instant,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) -> SpecialistDiagnostics {
    let start = Instant::now();
    let mut budget = Budget {
        deadline,
        low: low.max(200.0),
        high: high.min(3000.0),
        diagnostics: SpecialistDiagnostics::default(),
    };
    if samples.len() < 120000
        || samples.len() > 180000
        || samples.iter().any(|x| !x.is_finite())
        || !low.is_finite()
        || !high.is_finite()
        || low >= high
        || budget.exhausted()
    {
        return budget.diagnostics;
    }
    let original: Vec<f64> = samples.iter().map(|&x| f64::from(x)).collect();
    if original.iter().all(|x| *x == 0.0) {
        return budget.diagnostics;
    }
    let audio = clean_lines(&original);
    let candidates = search(&audio, low, high, &mut budget);
    let mut models = Vec::new();
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
    if !models.is_empty() && !budget.exhausted() {
        let mut order: Vec<usize> = (0..models.len()).collect();
        order.sort_by(|&a, &b| models[b].score.total_cmp(&models[a].score));
        order.truncate(3);
        let mut residual = audio.clone();
        for i in order {
            for (x, &y) in residual.iter_mut().zip(&models[i].wave) {
                *x -= y;
            }
        }
        budget.diagnostics.residual_searches = 1;
        let candidates = search(&residual, low, high, &mut budget);
        admit(
            &audio,
            &residual,
            candidates,
            &mut models,
            &mut outputs,
            &mut budget,
            emit,
        );
    }
    budget.diagnostics.elapsed = start.elapsed();
    budget.diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_input_is_not_observed_zero_padding() {
        let band = Band::new(&vec![0.0; 144000], 1000.0);
        let spectra = band.spectra(1000.0, 0.0, 0.0);
        assert_eq!(spectra.observed[0], 32);
        assert_eq!(spectra.observed[78], 0);
        let full = Band::new(&vec![0.0; 180000], 1000.0);
        assert_eq!(full.spectra(1000.0, 0.0, 0.0).observed[78], 32);
    }
    #[test]
    fn expired_budget_silence_and_invalid_input_never_emit() {
        for samples in [vec![0.0; 180000], vec![f32::NAN; 180000], vec![0.0; 100]] {
            let mut rows = 0;
            let result = run(
                &samples,
                200.0,
                3000.0,
                Instant::now() + Duration::from_secs(1),
                &mut |_| rows += 1,
            );
            assert_eq!(rows, 0);
            assert_eq!(result.fec_calls, 0);
        }
        let result = run(
            &vec![1.0; 180000],
            200.0,
            3000.0,
            Instant::now(),
            &mut |_| panic!("expired budget"),
        );
        assert!(result.budget_exhausted);
    }
    #[test]
    fn hybrid_cw_specialist_adds_truth_without_replacing_baseline() {
        use crate::decode::hybrid::HybridStreamDecodeSession;
        use crate::{SlotTimestamp, StreamDecodeConfig};
        let bits = crate::decode::packjt77::pack77("CQ K1ABC FN42");
        let tones = crate::decode::genft8::get_ft8_tones_from_77bits(&bits);
        let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, 1000.37);
        let mut state = 2026106991u64;
        let mut uniform = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            ((state.wrapping_mul(2685821657736338717) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let mut audio = Vec::with_capacity(180000);
        for _ in 0..90000 {
            let r = (-2.0 * uniform().ln()).sqrt();
            let p = TAU * uniform();
            audio.push(r * p.cos());
            audio.push(r * p.sin());
        }
        let amplitude = (2.0 * 2500.0 / 6000.0 * 10f64.powf(-2.0)).sqrt();
        let start = ((0.143 + 0.5) * FS).round() as usize;
        for i in 0..re.len().min(audio.len() - start) {
            audio[start + i] += amplitude * (re[i] * 0.27f64.cos() - im[i] * 0.27f64.sin());
        }
        for (i, y) in audio.iter_mut().enumerate() {
            *y += 0.45 * (TAU * 1012.9 * i as f64 / FS + 0.4).cos();
        }
        let samples: Vec<f32> = audio.iter().map(|&x| x as f32).collect();
        let timestamp = SlotTimestamp::parse("261029_120000").unwrap();
        let mut results = Vec::new();
        for enabled in [false, true] {
            let mut session = HybridStreamDecodeSession::new(StreamDecodeConfig {
                hybrid_specialist: enabled,
                ..Default::default()
            });
            let mut callbacks = Vec::new();
            let rows = session
                .decode_slot_streaming_at(&timestamp, &samples, |row| {
                    callbacks.push(row.clone());
                    Ok(())
                })
                .unwrap();
            assert_eq!(callbacks.len(), rows.len());
            results.push(rows);
        }
        for row in &results[0] {
            assert!(results[1]
                .iter()
                .any(|other| crate::decode::cooperative::same_signal(row, other)));
        }
        assert!(!results[0].iter().any(|r| r.msg == "CQ K1ABC FN42"));
        assert!(results[1].iter().any(|r| r.msg == "CQ K1ABC FN42"));
        assert!(results[1].iter().all(|r| r.msg == "CQ K1ABC FN42"));
    }
}
