use super::*;
use std::collections::BTreeSet;

#[path = "adaptive.rs"]
pub(in crate::decode::lib_wsjtx_plus::ft8_decode) mod adaptive;

#[derive(Clone, Copy)]
struct Point {
    pub bin: i32,
    pub time: isize,
    pub nsync: usize,
    pub quality: f64,
    partial_quality: f64,
}

struct Hit {
    pub row: Ft8bResult,
    supports: Vec<Point>,
    reported: bool,
}

impl Hit {
    pub fn spaced(&self) -> bool {
        self.supports.iter().any(|a| {
            self.supports
                .iter()
                .any(|b| a.time != b.time || (a.bin - b.bin).abs() >= 8)
        })
    }
}

fn check_costas(time: isize, workspace: &DecodeWorkspace, floor: usize) -> (usize, f64) {
    let mut fft = SymbolFft::new();
    let mut correct = 0;
    let mut margin = 0.0;
    let mut observed = 0;
    let mut remaining = 21;
    for (k, &expected) in COSTAS.iter().enumerate() {
        for block in [0, 36, 72] {
            let start = time + ((block + k) * COSTAS_SYMBOL_LEN) as isize;
            let bins =
                extract_symbol_spectrum(&mut fft, &workspace.cd0_re, &workspace.cd0_im, start);
            let strengths = bins.map(|b| wsjtx_cabs(b.re as f32, b.im as f32));
            let (mut peak, mut strength, mut other) = (0, -1.0f32, 0.0f32);
            for (idx, &value) in strengths.iter().enumerate() {
                if value > strength {
                    (peak, strength) = (idx, value);
                }
                if idx != usize::from(expected) {
                    other = other.max(value);
                }
            }
            correct += usize::from(peak == usize::from(expected));
            remaining -= 1;
            if correct + remaining < floor {
                return (correct, 0.0);
            }
            if strength > 0.0 {
                let expected = strengths[usize::from(expected)];
                margin += f64::from((expected - other) / (expected + other));
                observed += 1;
            }
        }
    }
    (correct, margin / f64::from(observed.max(1)))
}

// A few data symbols from each third complement the known sync sequence.
// Smooth contrast distinguishes offsets even when every Costas winner ties.
fn data_contrast(time: isize, workspace: &DecodeWorkspace) -> f64 {
    let mut fft = SymbolFft::new();
    let mut contrast = [0.0; 3];
    let mut observed = [0; 3];
    for data in (3..58).step_by(6) {
        let symbol = if data < 29 { data + 7 } else { data + 14 };
        let start = time + (symbol * COSTAS_SYMBOL_LEN) as isize;
        let bins = extract_symbol_spectrum(&mut fft, &workspace.cd0_re, &workspace.cd0_im, start);
        let (mut first, mut second) = (0.0f32, 0.0f32);
        for b in bins {
            let strength = wsjtx_cabs(b.re as f32, b.im as f32);
            if strength >= first {
                second = first;
                first = strength;
            } else {
                second = second.max(strength);
            }
        }
        if first > 0.0 {
            let group = data / 20;
            contrast[group] += f64::from((first - second) / (first + second));
            observed[group] += 1;
        }
    }
    for (sum, n) in contrast.iter_mut().zip(observed) {
        *sum /= f64::from(n.max(1));
    }
    contrast.sort_by(f64::total_cmp);
    contrast.iter().sum::<f64>() / 3.0
}

fn select_points(
    points: &mut [Point],
    cap: usize,
    fine: (i32, isize),
    coarse: (i32, isize),
) -> Vec<Point> {
    let key = |p: &Point| (p.bin, p.time);
    points.sort_by(|a, b| {
        b.quality
            .total_cmp(&a.quality)
            .then_with(|| b.nsync.cmp(&a.nsync))
    });
    let mut selected = Vec::with_capacity(cap);
    for anchor in [fine, coarse] {
        if let Some(p) = points.iter().find(|p| key(p) == anchor) {
            if !selected.iter().any(|q| key(q) == anchor) && selected.len() < cap {
                selected.push(*p);
            }
        }
    }
    let mut partial = points.to_vec();
    partial.sort_by(|a, b| b.partial_quality.total_cmp(&a.partial_quality));
    let separated = |q: &Point, p: &Point| (q.bin - p.bin).abs() > 8 || (q.time - p.time).abs() > 2;
    let mut balanced = points.iter().copied();
    let mut fading = partial.into_iter();
    while selected.len() < cap {
        let mut advanced = false;
        if let Some(p) = balanced.find(|p| selected.iter().all(|q| separated(q, p))) {
            selected.push(p);
            advanced = true;
        }
        if selected.len() == cap {
            break;
        }
        if let Some(p) = fading.find(|p| selected.iter().all(|q| separated(q, p))) {
            selected.push(p);
            advanced = true;
        }
        if !advanced {
            break;
        }
    }
    for p in points.iter().copied() {
        if selected.len() == cap {
            break;
        }
        if !selected.iter().any(|q| key(q) == key(&p)) {
            selected.push(p);
        }
    }
    selected
}

fn attenuate(workspace: &mut DecodeWorkspace, group: usize) {
    let start = group * 60;
    let end = (start + 60).min(N_LDPC);
    for metrics in [
        &mut workspace.bmeta,
        &mut workspace.bmetb,
        &mut workspace.bmetc,
        &mut workspace.bmetd,
        &mut workspace.bmete,
    ] {
        for soft in &mut metrics[start..end] {
            *soft *= 0.25;
        }
    }
}

struct CandidateSearch<'a> {
    cx_re: &'a [f64],
    cx_im: &'a [f64],
    sbase: &'a [f64],
    depth: usize,
    ap_options: &'a Ft8bApOptions,
    book: &'a Option<HashCallBook>,
    workspace: &'a mut DecodeWorkspace,
    loaded_bin: Option<i32>,
    hits: Vec<Hit>,
}

impl CandidateSearch<'_> {
    fn load(&mut self, bin: i32) {
        if self.loaded_bin != Some(bin) {
            let f = f64::from(bin) * f64::from(super::super::DOWNSAMPLE_DF);
            ft8_downsample(self.cx_re, self.cx_im, f, self.workspace);
            self.loaded_bin = Some(bin);
        }
    }

    fn decode(&mut self, point: Point, confirmation_only: bool) {
        self.load(point.bin);
        let f = f64::from(point.bin) * f64::from(super::super::DOWNSAMPLE_DF);
        extract_soft_symbols(point.time, self.workspace);
        if cfg!(debug_assertions) {
            assert_eq!(compute_nsync(&self.workspace.s8), point.nsync);
        }
        let (group, _, _) = data_group_confidence(&self.workspace.s8);
        build_bit_metrics(self.workspace, 1);
        for downweighted in [false, true] {
            if downweighted {
                attenuate(self.workspace, group);
            }
            let Some(row) = decode_metrics(
                f,
                point.time,
                self.sbase,
                self.depth,
                false,
                self.ap_options,
                self.book,
                self.workspace,
                point.nsync,
            ) else {
                continue;
            };
            if let Some(hit) = self.hits.iter_mut().find(|hit| hit.row.itone == row.itone) {
                if !hit
                    .supports
                    .iter()
                    .any(|p| (p.bin, p.time) == (point.bin, point.time))
                {
                    hit.supports.push(point);
                }
            } else if !confirmation_only {
                self.hits.push(Hit {
                    row,
                    supports: vec![point],
                    reported: false,
                });
            }
        }
    }

    fn screen(
        &mut self,
        candidate: &super::super::Candidate,
    ) -> (Vec<Point>, (i32, isize), (i32, isize)) {
        let (fine_f, fine_t) = align_candidate(
            self.cx_re,
            self.cx_im,
            candidate.freq,
            candidate.dt,
            self.workspace,
        );
        let coarse_t = nint_wsjtx_f32((candidate.dt + 0.5) * FS2) + 1;
        let mut grid = BTreeSet::new();
        for (f, t) in [(fine_f, fine_t), (candidate.freq, coarse_t)] {
            let bin = (f / f64::from(super::super::DOWNSAMPLE_DF)).round() as i32;
            for df in (-40..=40).step_by(8) {
                for dt in -10..=10 {
                    grid.insert((bin + df, t + dt));
                }
            }
        }
        const MIN_SYNC: usize = 7;
        let mut points = Vec::new();
        for (bin, time) in grid {
            self.load(bin);
            let (nsync, quality) = check_costas(time, self.workspace, MIN_SYNC);
            if nsync < MIN_SYNC {
                continue;
            }

            // Give uneven frames their own ranking, based on the contrast
            // between the weaker data third and the two supported thirds.
            extract_soft_symbols(time, self.workspace);
            let (_, weak, supported) = data_group_confidence(&self.workspace.s8);
            let contrast = |ratio: f64| 1.0 - 1.0 / ratio.max(1.0);
            let (weak, supported) = (contrast(weak), contrast(supported));
            let gap = (supported - weak).max(0.0);
            let balanced = data_contrast(time, self.workspace);
            points.push(Point {
                bin,
                time,
                nsync,
                quality: 0.5 * quality + balanced,
                partial_quality: 0.25 * quality + 2.0 * (supported * gap).sqrt(),
            });
        }
        let bin = |f: f64| (f / f64::from(super::super::DOWNSAMPLE_DF)).round() as i32;
        (
            points,
            (bin(fine_f), fine_t),
            (bin(candidate.freq), coarse_t),
        )
    }
}
