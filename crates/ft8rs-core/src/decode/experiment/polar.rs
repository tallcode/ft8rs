//! Bounded terminal Gaussian-channel demapping and out-of-sample data support.
use super::*;

#[cfg(test)]
thread_local! {static DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };}

#[cfg(test)]
pub(super) fn without_polar<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            DISABLED.set(self.0);
        }
    }
    let reset = Reset(DISABLED.replace(true));
    let result = f();
    drop(reset);
    result
}

#[derive(Clone, Copy)]
pub(super) struct Model {
    pub width: f64,
    pub differential: f64,
}
pub(super) const MODELS: [Model; 4] = [
    Model {
        width: 2.0,
        differential: 0.0,
    },
    Model {
        width: 5.0,
        differential: 0.0,
    },
    Model {
        width: 10.0,
        differential: 0.0,
    },
    Model {
        width: 2.0,
        differential: 6.0,
    },
];

/// Explicit immutable receive context only. Experimental results never populate it.
#[derive(Clone, Copy, Default)]
pub(crate) struct ApContext<'a> {
    pub enabled: bool,
    pub mycall: Option<&'a str>,
    pub hiscall: Option<&'a str>,
    pub tails: bool,
    pub frequency: f64,
    pub width: f64,
}

pub(super) struct Hint {
    pub bits: Vec<u8>,
    pub mask: [i8; 174],
    pub name: &'static str,
}

fn hint(template: &str, ranges: &[(usize, usize)], name: &'static str) -> Option<Hint> {
    let bits = crate::decode::packjt77::pack77_strict(template)?;
    if bits.len() != 77 {
        return None;
    }
    let mut mask = [0; 174];
    for &(start, end) in ranges {
        mask[start..end].fill(1);
    }
    Some(Hint { bits, mask, name })
}

pub(super) fn hints(context: ApContext<'_>, frequency: f64) -> Vec<Hint> {
    if !context.enabled {
        return Vec::new();
    }
    // CQ and standard-message type are a public hypothesis, not a target oracle.
    let mut out = vec![hint("CQ K1ABC AA00", &[(0, 29), (74, 77)], "cq").unwrap()];
    if !frequency.is_finite()
        || !context.frequency.is_finite()
        || !context.width.is_finite()
        || (frequency - context.frequency).abs() > context.width.clamp(5.0, 100.0)
    {
        return out;
    }
    fn standard(s: Option<&str>) -> Option<&str> {
        s.map(str::trim)
            .filter(|s| crate::decode::packjt77::is_stdcall(s))
    }
    if let Some(his) = standard(context.hiscall) {
        if let Some(h) = hint(&format!("K1JT {his} RRR"), &[(29, 58), (74, 77)], "target") {
            out.push(h);
        }
        if let Some(h) = hint(&format!("CQ {his} AA00"), &[(0, 58), (74, 77)], "cq-target") {
            out.push(h);
        }
        if let Some(my) = standard(context.mycall).filter(|my| *my != his) {
            if let Some(h) = hint(&format!("{my} {his} RRR"), &[(0, 58), (74, 77)], "calls") {
                out.push(h);
            }
            if context.tails {
                for tail in ["RRR", "RR73", "73"] {
                    if let Some(h) = hint(&format!("{my} {his} {tail}"), &[(0, 77)], "tail") {
                        out.push(h);
                    }
                }
            }
        }
    }
    out
}

/// Runs after all previous experimental publications; accepted rows never seed AP.
pub(crate) fn recover(
    input: ExperimentInput<'_>,
    context: ApContext<'_>,
    previous: &ExperimentDiagnostics,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) -> Option<ExperimentDiagnostics> {
    #[cfg(test)]
    if DISABLED.get() {
        return None;
    }
    let clock = Instant::now();
    let deadline = input.deadline.min(clock + Duration::from_millis(500));
    let samples = &input.samples[..input.samples.len().min(180000)];
    if samples.len() != 180000
        || samples.iter().any(|x| !x.is_finite())
        || samples.iter().all(|x| *x == 0.0)
        || !input.low.is_finite()
        || !input.high.is_finite()
        || input.low.max(200.0) >= input.high.min(3000.0)
        || clock >= deadline
        || previous.fec_calls >= MAX_FEC
    {
        return None;
    }
    let audio: Vec<f64> = samples.iter().map(|&x| f64::from(x)).collect();
    let (bands, candidates) = discover(&audio, input.low, input.high);
    let mut report = previous.clone();
    let initial_calls = report.fec_calls;
    let mut emitted: Vec<Row> = Vec::new();
    'candidates: for candidate in candidates.iter().take(16) {
        let band = &bands[candidate.band];
        for refined in refine(band, candidate) {
            if !(input.low.max(200.0)..=input.high.min(3000.0)).contains(&refined.f) {
                continue;
            }
            let priors = hints(context, refined.f);
            for (mi, model) in MODELS.into_iter().enumerate() {
                if Instant::now() >= deadline {
                    break 'candidates;
                }
                let llr = likelihoods(band, refined.f, refined.dt, model);
                for prior in std::iter::once(None).chain(priors.iter().map(Some)) {
                    if Instant::now() >= deadline
                        || report.fec_calls >= MAX_FEC
                        || report.fec_calls - initial_calls >= 320
                    {
                        break 'candidates;
                    }
                    report.fec_calls += 1;
                    report.polar_fec_calls += 1;
                    report.polar_ap_calls += usize::from(prior.is_some());
                    let Some((message, tones)) = decode_hint(&llr, prior) else {
                        continue;
                    };
                    let row = Row {
                        msg: message,
                        f: refined.f,
                        dt: refined.dt,
                        method: format!("polar-{mi}-{}", prior.map_or("none", |p| p.name)),
                        tones: Some(tones),
                    };
                    if emitted.iter().any(|old| row_identity(old, &row)) {
                        continue;
                    }
                    report.tentative_rows += 1;
                    let Some(fit) = support(&audio, &row, model, &llr) else {
                        continue;
                    };
                    if !fit.accepted() {
                        report.rejected_rows += 1;
                        report.polar_rejected_rows += 1;
                        continue;
                    }
                    if Instant::now() >= deadline {
                        break 'candidates;
                    }
                    emit(&StreamDecodedMessage {
                        freq: row.f,
                        dt: row.dt,
                        snr: 10.0 * (fit.rho * 200.0 / 2500.0).log10(),
                        msg: row.msg.clone(),
                        sync: fit.data_score,
                        itone: tones,
                    });
                    report.accepted_rows += 1;
                    report.polar_accepted_rows += 1;
                    report.polar_ap_accepted_rows += usize::from(prior.is_some());
                    report.accepted_scores.push(fit.data_score);
                    report.accepted_data_scores.push(fit.soft_score);
                    report.accepted_methods.push(row.method.clone());
                    emitted.push(row);
                }
            }
        }
    }
    let elapsed = clock.elapsed();
    report.polar_elapsed += elapsed;
    report.elapsed += elapsed;
    Some(report)
}

pub(super) fn decode_hint(llr: &[f64], prior: Option<&Hint>) -> Option<(String, [i32; 79])> {
    let mut conditioned = llr.to_vec();
    let empty = [0; 174];
    let mask = if let Some(h) = prior {
        let magnitude = llr.iter().map(|x| x.abs()).fold(0.0, f64::max).max(1.0) * 1.1;
        for (i, value) in conditioned.iter_mut().enumerate().take(77) {
            if h.mask[i] == 1 {
                *value = magnitude * (2.0 * f64::from(h.bits[i]) - 1.0);
            }
        }
        &h.mask
    } else {
        &empty
    };
    let result = crate::decode::lib_wsjtx_plus::decode174_91::decode174_91(&conditioned, mask, 2)?;
    if result.cw.iter().all(|&x| x == 0) {
        return None;
    }
    if let Some(h) = prior {
        if (0..77).any(|i| h.mask[i] == 1 && result.cw[i] != h.bits[i]) {
            return None;
        }
    }
    let message = crate::decode::packjt77::unpack77(&result.message91[..77], None)?;
    Some((
        message,
        crate::decode::genft8::get_ft8_tones_from_codeword(&result.cw),
    ))
}

pub(super) fn likelihoods(band: &Band, f: f64, dt: f64, model: Model) -> Vec<f64> {
    let spectrum = band.spectra(f, dt, 0.0);
    let noise = (variance(&spectrum) / 32.0).max(1e-15);
    let rho = pilot_rho(band, f, dt, noise);
    let w = gp_weights(model.width, model.differential, rho);
    let scores = std::array::from_fn(|k| {
        let samples = samples_for(band, f, dt, k);
        std::array::from_fn(|q| {
            let z: [C; 32] = std::array::from_fn(|n| samples[n] * basis()[q][n]);
            let mut score = 0.0;
            for i in 0..32 {
                for j in 0..32 {
                    score += (z[i].conj() * z[j]).re * w[i][j];
                }
            }
            score / noise
        })
    });
    scores_to_llr(&scores)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Support {
    pub data_score: f64,
    pub groups: [f64; 3],
    pub soft_score: f64,
    pub rho: f64,
}
impl Support {
    pub fn accepted(self) -> bool {
        self.data_score > 80.0 && self.groups.iter().all(|x| *x > 10.0) && self.soft_score > 10.0
    }
}

// Each prediction uses only the opposite parity of the 200-Hz observations.
// Fixed Gaussian prior and ridge noise stop a free local channel fitting noise.
// Both folds reuse this selected recording: the empirical score is not a p-value.
pub(super) fn support(audio: &[f64], row: &Row, model: Model, llr: &[f64]) -> Option<Support> {
    let tones = row.tones?;
    if !valid_tones(&tones) || !row.f.is_finite() || !(-0.5..=1.86).contains(&row.dt) {
        return None;
    }
    let band = Band::new(audio, row.f + 22.0);
    let noise = (variance(&band.spectra(row.f, row.dt, 0.0)) / 32.0).max(1e-15);
    let rho = pilot_rho(&band, row.f, row.dt, noise);
    let offsets = [-7, -5, -3, -1, 1, 3, 5, 7];
    let correlation = |lag: f64| {
        (-0.5 * std::f64::consts::PI.powi(2) * model.width.powi(2) * lag.powi(2)).exp()
            * (std::f64::consts::PI * model.differential * lag).cos()
    };
    let matrix: Vec<Vec<f64>> = offsets
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            offsets
                .iter()
                .enumerate()
                .map(|(j, &b)| correlation(f64::from(a - b) / 200.0) + f64::from(i == j) / rho)
                .collect()
        })
        .collect();
    let inverse = inverse_positive(&matrix);
    let covariance: Vec<f64> = offsets
        .iter()
        .map(|&a| correlation(f64::from(a) / 200.0))
        .collect();
    let weights: Vec<f64> = (0..8)
        .map(|i| (0..8).map(|j| inverse[i][j] * covariance[j]).sum())
        .collect();
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, 0.0);
    let start = (row.dt + 0.5) * 200.0;
    let mut matched = vec![C::new(0.0, 0.0); 3000];
    let mut data = vec![None; 3000];
    for j in 0..band.observed_end {
        let source = (j as f64 - start) * 60.0;
        if source < 0.0 || source >= re.len() as f64 - 1.0 {
            continue;
        }
        let i = source.floor() as usize;
        let x = source - i as f64;
        let reference = C::new(
            re[i] * (1.0 - x) + re[i + 1] * x,
            im[i] * (1.0 - x) + im[i + 1] * x,
        ) * C::from_polar(
            1.0,
            TAU * (row.f - band.center) * (j as f64 - start) / 200.0,
        );
        matched[j] = band.samples[j] * reference.conj();
        data[j] = match i / 1920 {
            7..=35 => Some((i / 1920 - 7) / 20),
            43..=71 => Some(((i / 1920 - 43 + 29) / 20).min(2)),
            _ => None,
        };
    }
    let mut groups = [0.0; 3];
    let mut counts = [0; 3];
    for j in 7..band.observed_end.saturating_sub(7) {
        let Some(group) = data[j] else {
            continue;
        };
        let prediction: C = offsets
            .iter()
            .zip(&weights)
            .map(|(&off, &w)| matched[(j as i32 + off) as usize] * w)
            .sum();
        groups[group] +=
            (2.0 * (matched[j] * prediction.conj()).re - prediction.norm_sqr()) / noise;
        counts[group] += 1;
    }
    if counts.iter().any(|&n| n < 128) {
        return None;
    }
    let mut soft_score = 0.0;
    let mut bit = 0;
    for base in [7, 43] {
        for &tone in &tones[base..base + 29] {
            let word = GRAY.iter().position(|&t| t == tone as usize)?;
            for b in (0..3).rev() {
                let signed = llr[bit] * (if (word >> b) & 1 == 1 { 1.0 } else { -1.0 });
                soft_score +=
                    std::f64::consts::LN_2 - ((-signed).max(0.0) + (-signed.abs()).exp().ln_1p());
                bit += 1;
            }
        }
    }
    Some(Support {
        data_score: groups.iter().sum(),
        groups,
        soft_score,
        rho,
    })
}

#[allow(clippy::needless_range_loop)] // Indexed triangular solves preserve the algebra's order.
fn inverse_positive(matrix: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = matrix.len();
    let mut lower = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let x = matrix[i][j] - (0..j).map(|k| lower[i][k] * lower[j][k]).sum::<f64>();
            lower[i][j] = if i == j { x.sqrt() } else { x / lower[j][j] };
            assert!(lower[i][j].is_finite());
        }
    }
    let mut inverse = vec![vec![0.0; n]; n];
    for column in 0..n {
        let mut y = vec![0.0; n];
        for i in 0..n {
            y[i] = (f64::from(i == column) - (0..i).map(|j| lower[i][j] * y[j]).sum::<f64>())
                / lower[i][i];
        }
        for i in (0..n).rev() {
            inverse[i][column] = (y[i]
                - (i + 1..n)
                    .map(|j| lower[j][i] * inverse[j][column])
                    .sum::<f64>())
                / lower[i][i];
        }
    }
    inverse
}

// Marginalize a zero-mean Gaussian channel rather than fitting arbitrary phase
// to each competing tone. C_q=D_q(I+rho R)D_q*. Equal determinants cancel.

fn gp_weights(width: f64, differential: f64, rho: f64) -> Vec<Vec<f64>> {
    let matrix: Vec<Vec<f64>> = (0..32)
        .map(|i| {
            (0..32)
                .map(|j| {
                    let lag = (i as f64 - j as f64) / 200.0;
                    let r = (-0.5 * std::f64::consts::PI.powi(2) * width.powi(2) * lag.powi(2))
                        .exp()
                        * (std::f64::consts::PI * differential * lag).cos();
                    f64::from(i == j) + rho * r
                })
                .collect()
        })
        .collect();
    let inverse = inverse_positive(&matrix);
    (0..32)
        .map(|i| (0..32).map(|j| f64::from(i == j) - inverse[i][j]).collect())
        .collect()
}

fn samples_for(band: &Band, f: f64, dt: f64, k: usize) -> [C; 32] {
    let start = ((dt + 0.5) * 200.0).round() as isize;
    std::array::from_fn(|n| {
        let j = start + (32 * k + n) as isize;
        if j < 0 || j >= band.observed_end as isize {
            return C::new(0.0, 0.0);
        }
        let t = (j - start) as f64 / 200.0;
        band.samples[j as usize] * C::from_polar(1.0, -TAU * (f - band.center) * t)
    })
}

fn pilot_rho(band: &Band, f: f64, dt: f64, noise: f64) -> f64 {
    let mut energy = 0.0;
    for base in [0, 36, 72] {
        for k in 0..7 {
            energy += samples_for(band, f, dt, base + k)
                .iter()
                .map(C::norm_sqr)
                .sum::<f64>();
        }
    }
    (energy / (21.0 * 32.0) / noise - 1.0).clamp(0.03, 10.0)
}

fn scores_to_llr(scores: &[[f64; 8]; 79]) -> Vec<f64> {
    let mut llr = Vec::with_capacity(174);
    for base in [7, 43] {
        for row in &scores[base..base + 29] {
            for bit in (0..3).rev() {
                let one: Vec<_> = (0..8)
                    .filter(|q| (q >> bit) & 1 == 1)
                    .map(|q| row[GRAY[q]])
                    .collect();
                let zero: Vec<_> = (0..8)
                    .filter(|q| (q >> bit) & 1 == 0)
                    .map(|q| row[GRAY[q]])
                    .collect();
                llr.push((lse(&one) - lse(&zero)).clamp(-20.0, 20.0));
            }
        }
    }
    llr
}
