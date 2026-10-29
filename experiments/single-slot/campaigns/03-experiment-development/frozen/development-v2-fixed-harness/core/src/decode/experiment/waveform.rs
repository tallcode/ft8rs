//! Reference-waveform fit, data-symbol admission and regularized cancellation.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct WaveformFit {
    pub f: f64,
    pub dt: f64,
    pub score: f64,
    pub data_score: f64,
    pub minimum_block: f64,
}
impl WaveformFit {
    pub fn data_supported(self) -> bool {
        self.data_score.is_finite()
            && self.data_score >= 100.0
            && self.minimum_block.is_finite()
            && self.minimum_block >= 5.0
    }
    pub fn supported(self) -> bool {
        self.score.is_finite() && self.score > MIN_SCORE && self.data_supported()
    }
}

pub(super) fn fit_message(audio: &[f64], row: &Row, deadline: Instant) -> Option<WaveformFit> {
    let tones = row.tones?;
    if !valid_tones(&tones) || !row.f.is_finite() || !row.dt.is_finite() {
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
        if Instant::now() >= deadline {
            return None;
        }
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
    let start = (best.2 + 0.5) * 200.0;
    let mut z = [C::new(0.0, 0.0); 3];
    let mut energy = [0.0; 3];
    let mut counts = [0usize; 3];
    let first = start.ceil().max(0.0) as usize;
    let last = (start + re.len() as f64 / 60.0)
        .floor()
        .min(band.observed_end as f64 - 1.0) as usize;
    let rotation = C::from_polar(1.0, TAU * (best.1 - band.center) / 200.0);
    let mut phase = C::from_polar(
        1.0,
        TAU * (best.1 - band.center) * (first as f64 - start) / 200.0,
    );
    for j in first..=last {
        let source = (j as f64 - start) * 60.0;
        let i = source.floor() as usize;
        if i + 1 >= re.len() {
            break;
        }
        let symbol = i / 1920;
        let data_index = match symbol {
            7..=35 => Some(symbol - 7),
            43..=71 => Some(symbol - 43 + 29),
            _ => None,
        };
        if let Some(data) = data_index {
            let group = (data / 20).min(2);
            let x = source - i as f64;
            let v = C::new(
                re[i] * (1.0 - x) + re[i + 1] * x,
                im[i] * (1.0 - x) + im[i + 1] * x,
            ) * phase;
            z[group] += band.samples[j] * v.conj();
            energy[group] += v.norm_sqr();
            counts[group] += 1;
        }
        phase *= rotation;
    }
    let total = z.iter().sum::<C>();
    let data_score = total.norm_sqr() / energy.iter().sum::<f64>().max(1e-15) / n0;
    let direction = total / total.norm().max(1e-15);
    let minimum_block = (0..3)
        .map(|g| {
            let projection = (z[g] * direction.conj()).re;
            if counts[g] < 128 || projection <= 0.0 {
                return 0.0;
            }
            projection.powi(2) / energy[g].max(1e-15) / n0
        })
        .fold(f64::INFINITY, f64::min);
    Some(WaveformFit {
        f: best.1,
        dt: best.2,
        score: best.0 / n0,
        data_score,
        minimum_block,
    })
}
pub(super) fn cancel_smooth(audio: &[f64], row: &Row, f: f64, dt: f64) -> Vec<f64> {
    let tones = row.tones.expect("validated original tones");
    let (re, im) = crate::decode::gen_ft8wave::gen_ft8wave(&tones, f);
    let start = ((dt + 0.5) * FS).round() as isize;
    let mut global = [0.0; 5];
    for i in 0..re.len() {
        let j = start + i as isize;
        if !(0..audio.len() as isize).contains(&j) {
            continue;
        }
        let a = re[i];
        let b = im[i];
        global[0] += a * a;
        global[1] += b * b;
        global[2] += a * b;
        global[3] += audio[j as usize] * a;
        global[4] += audio[j as usize] * b;
    }
    let det = (global[0] * global[1] - global[2] * global[2]).max(1e-15);
    let prior_a = (global[3] * global[1] - global[4] * global[2]) / det;
    let prior_b = (global[4] * global[0] - global[3] * global[2]) / det;
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
        // One symbol of ridge support towards the whole-frame channel.
        // Suppresses rapid fitting of unrelated weak energy into this model.
        let ridge = 960.0;
        aa += ridge;
        bb += ridge;
        ya += ridge * prior_a;
        yb += ridge * prior_b;
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
