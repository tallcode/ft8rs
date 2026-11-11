//! Terminal drift recovery with shared chirp removal and native waveform cancellation.
use super::*;
fn analytic_audio(audio: &[f64]) -> Vec<C> {
    let mut z: Vec<_> = audio.iter().map(|&x| C::new(x, 0.0)).collect();
    let mut planner = FftPlanner::new();
    planner.plan_fft_forward(180000).process(&mut z);
    z[1..90000].iter_mut().for_each(|x| *x *= 2.0);
    z[90001..].fill(C::new(0.0, 0.0));
    planner.plan_fft_inverse(180000).process(&mut z);
    z.iter_mut().for_each(|x| *x /= 180000.0);
    z
}

fn chirp_phase(i: usize, slope: f64) -> C {
    let t = i as f64 / FS - 7.5;
    C::from_polar(1.0, TAU * 0.5 * slope * t * t)
}

fn dechirp(z: &[C], slope: f64) -> Vec<f64> {
    z.iter()
        .enumerate()
        .map(|(i, &x)| (x * chirp_phase(i, -slope)).re)
        .collect()
}

fn shared_line_cleanup(original: &[f64]) -> (Vec<f64>, usize) {
    let analytic = analytic_audio(original);
    let mut clean = original.to_vec();
    let mut removed = 0;
    for slope in [0.60, -0.60] {
        let rotated = dechirp(&analytic, slope);
        let without_line = clean_lines(&rotated);
        if without_line == rotated {
            continue;
        }
        removed += 1;
        let model: Vec<_> = rotated
            .iter()
            .zip(without_line)
            .map(|(&a, b)| a - b)
            .collect();
        for (i, (x, interference)) in clean.iter_mut().zip(analytic_audio(&model)).enumerate() {
            *x -= (interference * chirp_phase(i, slope)).re;
        }
    }
    (clean, removed)
}

/// Runs after native, SIC, retry and pulse callbacks on a private full slot.
/// Its waveform models and recovered rows never seed another search or AP state.
pub(crate) fn recover_drift(
    input: ExperimentInput<'_>,
    previous: &ExperimentDiagnostics,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) -> Option<ExperimentDiagnostics> {
    let ExperimentInput {
        samples,
        baseline: native,
        low,
        high,
        deadline,
    } = input;
    let start = Instant::now();
    let samples = &samples[..samples.len().min(180000)];
    let (low, high) = (low.max(200.0), high.min(3000.0));
    // Reserve both searches below the hard FEC limit so admission can still run.
    let limit = (MAX_FEC.saturating_sub(previous.fec_calls.saturating_add(1))
        / (2 * MAX_FEC_PER_CANDIDATE))
        .min(32);
    if limit == 0
        || start >= deadline
        || samples.len() != 180000
        || !input.low.is_finite()
        || !input.high.is_finite()
        || low >= high
        || samples.iter().any(|x| !x.is_finite())
        || samples.iter().all(|x| *x == 0.0)
    {
        return None;
    }
    let audio: Vec<f64> = samples.iter().map(|&x| f64::from(x)).collect();
    let (mut residual, lines) = shared_line_cleanup(&audio);
    let mut budget = Budget {
        deadline,
        low,
        high,
        diagnostics: ExperimentDiagnostics {
            fec_calls: previous.fec_calls,
            ..Default::default()
        },
    };
    let models = native_models(&residual, native, &mut budget);
    for model in &models {
        for (x, &y) in residual.iter_mut().zip(&model.wave) {
            *x -= y;
        }
    }
    let analytic = analytic_audio(&residual);
    let shift = |slope| clean_lines(&dechirp(&analytic, slope));
    let supported = |fit: WaveformFit| {
        // Every symbol must be present for this specialist's extra results.
        let start = fit.dt + 0.5;
        fit.supported() && (0.0..=(15.0 - 12.64)).contains(&start) && (low..=high).contains(&fit.f)
    };
    let mut rejected = Vec::new();
    for slope in [0.60, -0.60] {
        if budget.exhausted() {
            break;
        }
        let corrected = shift(slope);
        for row in search(&corrected, low, high, limit, &mut budget) {
            // The complete fitting grid can move timing by at most 22.5 ms.
            if !(-0.523..=1.883).contains(&row.dt) {
                budget.diagnostics.rejected_rows += 1;
                continue;
            }
            let Some(fit) = fit_message(&corrected, &row, deadline) else {
                continue;
            };
            if supported(fit) {
                if budget.exhausted() {
                    break;
                }
                emit_drift(&row, fit, slope, &mut budget.diagnostics, emit);
            } else {
                rejected.push((slope, row));
            }
        }
    }
    let cache: [OnceLock<Vec<f64>>; 18] = std::array::from_fn(|_| OnceLock::new());
    for (coarse, row) in rejected {
        if budget.exhausted() {
            break;
        }
        let deadline = budget.deadline;
        let solve = |offset: i32| {
            if Instant::now() >= deadline {
                return None;
            }
            let (coarse_rate, cache_base) = if coarse > 0.0 { (30, 0) } else { (-30, 9) };
            let slope = f64::from(coarse_rate + offset) / 50.0;
            let corrected = cache[(cache_base + 4 + offset) as usize].get_or_init(|| shift(slope));
            let fit = fit_message(corrected, &row, deadline)?;
            supported(fit).then_some((slope, fit))
        };
        let mut best: Option<(f64, WaveformFit)> = None;
        for offsets in [-4, -3, -2, -1, 1, 2, 3, 4].chunks(worker_count().min(4)) {
            if budget.exhausted() {
                break;
            }
            let result = std::thread::scope(|scope| {
                let solve = &solve;
                let children: Vec<_> = offsets[1..]
                    .iter()
                    .map(|&offset| {
                        (
                            offset,
                            std::thread::Builder::new().spawn_scoped(scope, move || solve(offset)),
                        )
                    })
                    .collect();
                let head = solve(offsets[0]);
                std::iter::once(head)
                    .chain(children.into_iter().map(|(offset, task)| match task {
                        Ok(task) => task.join().expect("experimental drift fit worker panicked"),
                        Err(_) => solve(offset),
                    }))
                    .collect::<Vec<_>>()
            });
            for (slope, fit) in result.into_iter().flatten() {
                if best.is_none_or(|(_, previous)| fit.score > previous.score) {
                    best = Some((slope, fit));
                }
            }
        }
        if let Some((slope, fit)) = best {
            if budget.exhausted() {
                break;
            }
            emit_drift(&row, fit, slope, &mut budget.diagnostics, emit);
        } else {
            budget.diagnostics.rejected_rows += 1;
        }
    }
    let current = budget.diagnostics;
    let mut report = previous.clone();
    report.fec_calls = current.fec_calls;
    report.tentative_rows += current.tentative_rows;
    report.rejected_rows += current.rejected_rows;
    report.accepted_rows += current.accepted_rows;
    report.drift_accepted_rows += current.accepted_rows;
    report.drift_removed_lines += lines;
    report.drift_seed_models += models.len();
    report.budget_exhausted |= current.budget_exhausted;
    report.accepted_scores.extend(current.accepted_scores);
    report
        .accepted_data_scores
        .extend(current.accepted_data_scores);
    report.accepted_methods.extend(current.accepted_methods);
    let elapsed = start.elapsed();
    report.drift_elapsed += elapsed;
    report.elapsed += elapsed;
    Some(report)
}

fn emit_drift(
    row: &Row,
    fit: WaveformFit,
    slope: f64,
    diagnostics: &mut ExperimentDiagnostics,
    emit: &mut dyn FnMut(&StreamDecodedMessage),
) {
    let mut row = row.clone();
    row.method = format!("drift-{slope:+.2}-{}", row.method);
    emit_fit(&row, fit, diagnostics, emit);
}
