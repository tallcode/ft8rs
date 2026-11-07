//! Work allocation and confirmation for terminal Plus retries.
use super::*;
use crate::decode::lib_wsjtx_plus::ft8_decode::{create_decode_workspace, Candidate};

const START_POINTS: usize = 2;

#[derive(Default)]
struct Plan {
    order: Vec<Point>,
    untested: Vec<Point>,
    confirmations: Vec<ToneConfirmation>,
    hits: Vec<Hit>,
    tried: usize,
}

impl Plan {
    fn novel(hit: &Hit, base: &[crate::StreamDecodedMessage]) -> bool {
        !base.iter().any(|old| {
            old.msg == hit.row.msg
                || (old.itone == hit.row.itone
                    && (old.freq - hit.row.freq).abs() <= 5.0
                    && (old.dt - (hit.row.dt - 0.5)).abs() <= 0.3)
        })
    }

    fn next(&self, base: &[crate::StreamDecodedMessage]) -> Option<(f64, Point, bool)> {
        if self
            .hits
            .iter()
            .any(|hit| hit.spaced() && Self::novel(hit, base))
        {
            return None;
        }
        for hit in self.hits.iter().filter(|hit| Self::novel(hit, base)) {
            if let Some((_, order)) = self
                .confirmations
                .iter()
                .find(|(tones, _)| *tones == hit.row.itone)
            {
                if let Some(p) = order.first() {
                    return Some((4.0, *p, true));
                }
            }
            let anchor = hit.supports[0];
            // Seek a distinct alignment near the provisional codeword. The
            // same credit pool pays for these probes and for new attempts.
            if let Some(point) = self
                .untested
                .iter()
                .copied()
                .filter(|p| in_confirmation_neighborhood(*p, anchor))
                .max_by(|a, b| {
                    let distance = |p: &Point| {
                        (p.bin - anchor.bin).abs() / 8 + (p.time - anchor.time).abs() as i32
                    };
                    distance(b)
                        .cmp(&distance(a))
                        .then_with(|| a.quality.total_cmp(&b.quality))
                })
            {
                return Some((4.0, point, true));
            }
        }
        let point = *self.order.first()?;
        let opportunities = point.quality.max(point.partial_quality)
            + (point.partial_quality - point.quality).max(0.0);
        // Penalize repeated spending so that a single high-energy region
        // cannot keep winning the shared budget.
        let discount = (1.0 + self.tried as f64).sqrt();
        Some(((0.25 + opportunities) / discount, point, false))
    }

    fn take(&mut self, point: Point) {
        let not_used = |p: &Point| (p.bin, p.time) != (point.bin, point.time);
        self.untested.retain(not_used);
        self.order.retain(not_used);
        for (_, order) in &mut self.confirmations {
            order.retain(not_used);
        }
    }
}

fn in_confirmation_neighborhood(p: Point, anchor: Point) -> bool {
    (p.bin - anchor.bin).abs() <= 16
        && (p.time - anchor.time).abs() <= 4
        && (p.time != anchor.time || (p.bin - anchor.bin).abs() >= 8)
}

type ToneConfirmation = ([i32; 79], Vec<Point>);

fn tone_support(search: &mut CandidateSearch<'_>, p: Point, tones: &[i32; 79]) -> f64 {
    search.load(p.bin);
    extract_soft_symbols(p.time, search.workspace);
    let mut sums = [0.0; 3];
    let mut counts = [0; 3];
    for data in 0..58 {
        let symbol = if data < 29 { data + 7 } else { data + 14 };
        let tone = tones[symbol] as usize;
        let actual = search.workspace.s8[tone * NN + symbol];
        let other = (0..8)
            .filter(|&n| n != tone)
            .map(|n| search.workspace.s8[n * NN + symbol])
            .fold(0.0, f64::max);
        if actual + other > 0.0 {
            sums[data / 20] += (actual - other) / (actual + other);
            counts[data / 20] += 1;
        }
    }
    let mut groups = std::array::from_fn::<_, 3, _>(|i| sums[i] / f64::from(counts[i].max(1)));
    groups.sort_by(f64::total_cmp);
    // Prioritize repeatability even with a damaged data group. The codeword
    // is only used for ordering, never passed as a hint to the FEC decoder.
    (groups[1] + groups[2]) / 2.0
}

fn interleave_neighborhoods(
    anchors: Vec<Point>,
    points: &[Point],
    fine: (i32, isize),
    coarse: (i32, isize),
) -> Vec<Point> {
    let mut selected = std::collections::BTreeSet::new();
    let by_position: std::collections::BTreeMap<_, _> =
        points.iter().map(|p| ((p.bin, p.time), *p)).collect();
    let mut output = Vec::with_capacity(anchors.len());
    let mut push = |bin, time| {
        if let Some(p) = by_position.get(&(bin, time)) {
            if selected.insert((bin, time)) {
                output.push(*p);
            }
        }
    };
    for p in anchors.iter().take(2) {
        push(p.bin, p.time);
    }
    if (fine.1 - coarse.1).abs() > 2 || (fine.0 - coarse.0).abs() > 8 {
        // Keep the local timing stencil, then cover the gap between two
        // materially different synchronization estimates. Equal treatment
        // of both frequency directions avoids a one-sided sampling bias.
        for dt in [-1, 1] {
            push(fine.0, fine.1 + dt);
        }
        let midway = (fine.1 + coarse.1) / 2;
        let midpoint_bin = fine.0 + ((coarse.0 - fine.0) / 16) * 8;
        for bin in [fine.0, coarse.0, midpoint_bin] {
            for df in [0, -8, 8] {
                push(bin + df, midway);
            }
        }
    }
    for p in anchors {
        push(p.bin, p.time);
        // Cover both directions at a short distance before jumping to a new
        // lobe. A data-symbol boundary can move without changing Costas rank.
        for (df, dt) in [(0, -1), (0, 1), (-8, 0), (8, 0)] {
            push(p.bin + df, p.time + dt);
        }
    }
    output
}

const SLOT_POINT_BUDGET: usize = 256;
const MAX_WORKERS: usize = 4;

struct Inputs<'a> {
    spectrum: (&'a [f64], &'a [f64]),
    sbase: &'a [f64],
    depth: usize,
    ap: &'a Ft8bApOptions,
}

impl Inputs<'_> {
    fn search<'a>(
        &'a self,
        workspace: &'a mut DecodeWorkspace,
        book: &'a Option<HashCallBook>,
        hits: Vec<Hit>,
    ) -> CandidateSearch<'a> {
        CandidateSearch {
            cx_re: self.spectrum.0,
            cx_im: self.spectrum.1,
            sbase: self.sbase,
            depth: self.depth,
            ap_options: self.ap,
            book,
            workspace,
            loaded_bin: None,
            hits,
        }
    }
}

struct Probe {
    index: usize,
    point: Point,
    confirm: bool,
    hits: Vec<Hit>,
    neighbors: Vec<Point>,
    extra: Vec<Point>,
}

struct Outcome {
    index: usize,
    hits: Vec<Hit>,
    confirmations: Vec<ToneConfirmation>,
    tested: Vec<Point>,
    unused: usize,
}

#[allow(clippy::too_many_arguments)]
pub(in crate::decode::lib_wsjtx_plus::ft8_decode) fn run(
    spectrum: (&[f64], &[f64]),
    peaks: &[&Candidate],
    sbase: &[f64],
    depth: usize,
    ap: &Ft8bApOptions,
    book: &Option<HashCallBook>,
    base: &[crate::StreamDecodedMessage],
    deadline: Option<std::time::Instant>,
    mut emit: impl FnMut(Ft8bResult, f64) -> Result<(), String>,
) -> Result<(), String> {
    let within_budget = || deadline.is_none_or(|deadline| std::time::Instant::now() < deadline);
    if peaks.is_empty() || !within_budget() {
        return Ok(());
    }
    let inputs = Inputs {
        spectrum,
        sbase,
        depth,
        ap,
    };
    let workers = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(MAX_WORKERS)
        .min(peaks.len());

    let mut plans = std::thread::scope(|scope| {
        let mut tasks = Vec::new();
        for chunk in peaks.chunks(peaks.len().div_ceil(workers)) {
            let inputs = &inputs;
            tasks.push(scope.spawn(move || {
                let mut workspace = create_decode_workspace();
                chunk
                    .iter()
                    .map(|candidate| {
                        if !within_budget() {
                            return Plan::default();
                        }
                        let mut search = inputs.search(&mut workspace, &None, Vec::new());
                        let (mut points, fine, coarse) = search.screen(candidate);
                        let limit = SLOT_POINT_BUDGET.min(points.len());
                        let order = select_points(&mut points, limit, fine, coarse);
                        Plan {
                            order: interleave_neighborhoods(order, &points, fine, coarse),
                            untested: points,
                            confirmations: Vec::new(),
                            hits: Vec::new(),
                            tried: 0,
                        }
                    })
                    .collect::<Vec<_>>()
            }));
        }
        tasks
            .into_iter()
            .flat_map(|task| task.join().unwrap())
            .collect::<Vec<_>>()
    });

    std::thread::scope(|scope| {
        // Reuse FFT plans and workspaces across waves. Each worker owns its
        // call book and never writes to the original session.
        let mut queues = Vec::new();
        for _ in 0..workers {
            let (send, jobs) = std::sync::mpsc::channel::<Vec<Probe>>();
            let (complete, receive) = std::sync::mpsc::channel::<Vec<Outcome>>();
            queues.push((send, receive));
            let inputs = &inputs;
            let book = book.as_ref().map(HashCallBook::clone_book);
            scope.spawn(move || {
                let mut workspace = create_decode_workspace();
                let mut loaded_bin = None;
                while let Ok(batch) = jobs.recv() {
                    let results = batch
                        .into_iter()
                        .take_while(|_| within_budget())
                        .map(|probe| {
                            let mut search = inputs.search(&mut workspace, &book, probe.hits);
                            search.loaded_bin = loaded_bin;
                            search.decode(probe.point, probe.confirm);
                            let mut tested = Vec::new();
                            for point in probe.extra.iter().take_while(|_| within_budget()) {
                                if search
                                    .hits
                                    .iter()
                                    .any(|hit| hit.spaced() && Plan::novel(hit, base))
                                {
                                    break;
                                }
                                search.decode(*point, true);
                                tested.push(*point);
                            }
                            let confirmations = if probe.confirm || !within_budget() {
                                Vec::new()
                            } else {
                                let provisional: Vec<_> = search
                                    .hits
                                    .iter()
                                    .filter(|hit| {
                                        hit.supports.len() == 1
                                            && (hit.supports[0].bin, hit.supports[0].time)
                                                == (probe.point.bin, probe.point.time)
                                            && Plan::novel(hit, base)
                                    })
                                    .map(|hit| hit.row.itone)
                                    .collect();
                                provisional
                                    .into_iter()
                                    .map(|tones| {
                                        let mut order: Vec<_> = probe
                                            .neighbors
                                            .iter()
                                            .copied()
                                            .take_while(|_| within_budget())
                                            .map(|p| (tone_support(&mut search, p, &tones), p))
                                            .collect();
                                        order.sort_by(|a, b| b.0.total_cmp(&a.0));
                                        (tones, order.into_iter().map(|(_, p)| p).collect())
                                    })
                                    .collect()
                            };
                            loaded_bin = search.loaded_bin;
                            Outcome {
                                index: probe.index,
                                hits: search.hits,
                                confirmations,
                                unused: probe.extra.len() - tested.len(),
                                tested,
                            }
                        })
                        .collect();
                    if complete.send(results).is_err() {
                        break;
                    }
                }
            });
        }

        let mut credits = SLOT_POINT_BUDGET;
        while credits > 0 && within_budget() {
            let wave_limit = plans.len().div_ceil(2).min(credits);
            let mut eligible: Vec<_> = plans
                .iter()
                .enumerate()
                .filter_map(|(index, plan)| {
                    let (priority, point, confirm) = plan.next(base)?;
                    Some((index, plan.tried < START_POINTS, priority, point, confirm))
                })
                .collect();
            eligible.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.2.total_cmp(&a.2)));
            eligible.truncate(wave_limit);
            if eligible.is_empty() {
                break;
            }
            credits -= eligible.len();
            eligible.sort_by_key(|e| e.0);
            let chunk_size = eligible.len().div_ceil(workers);
            let mut batches = 0;
            for (chunk, (send, _)) in eligible.chunks(chunk_size).zip(&queues) {
                let batch = chunk
                    .iter()
                    .map(|&(index, _, _, point, confirm)| {
                        let plan = &mut plans[index];
                        // Reserve a bounded neighborhood for provisional codewords
                        // and return every point that was not needed.
                        let extra = if confirm {
                            plan.confirmations
                                .iter()
                                .find(|(_, order)| {
                                    order
                                        .first()
                                        .is_some_and(|p| (p.bin, p.time) == (point.bin, point.time))
                                })
                                .map(|(_, order)| {
                                    order
                                        .iter()
                                        .copied()
                                        .skip(1)
                                        .take(credits)
                                        .collect::<Vec<_>>()
                                })
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                        credits -= extra.len();
                        plan.take(point);
                        plan.tried += 1;
                        let neighbors = if confirm {
                            Vec::new()
                        } else {
                            plan.untested
                                .iter()
                                .copied()
                                .filter(|p| in_confirmation_neighborhood(*p, point))
                                .collect()
                        };
                        Probe {
                            index,
                            point,
                            confirm,
                            hits: std::mem::take(&mut plan.hits),
                            neighbors,
                            extra,
                        }
                    })
                    .collect();
                send.send(batch)
                    .map_err(|_| "Plus retry worker closed its input".to_string())?;
                batches += 1;
            }
            for (_, receive) in queues.iter().take(batches) {
                for outcome in receive
                    .recv()
                    .map_err(|_| "Plus retry worker closed its output".to_string())?
                {
                    let plan = &mut plans[outcome.index];
                    credits += outcome.unused;
                    plan.tried += outcome.tested.len();
                    for point in outcome.tested {
                        plan.take(point);
                    }
                    plan.hits = outcome.hits;
                    plan.confirmations.extend(outcome.confirmations);

                    for hit in &mut plan.hits {
                        if !hit.reported && hit.spaced() && Plan::novel(hit, base) {
                            hit.reported = true;
                            emit(hit.row.clone(), peaks[outcome.index].sync)?;
                        }
                    }
                }
            }
        }
        debug_assert!(
            !within_budget()
                || SLOT_POINT_BUDGET - credits == plans.iter().map(|p| p.tried).sum::<usize>()
        );
        Ok(())
    })
}
