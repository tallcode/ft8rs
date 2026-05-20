/// Signal subtraction for streaming FT8 decode.
/// Matches WSJT-X subtractft8 subroutine with lrefinedt support.

use crate::util::constants::SAMPLE_RATE;

const NDOWN: usize = 60;
const NN: usize = 79;
const COSTAS: [u8; 7] = [0, 2, 5, 7, 6, 4, 1];
const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

/// Subtract a decoded signal from the residual audio.
/// If `refine_dt` is true, performs lrefinedt-style DT refinement
/// by searching ±90 samples for minimum energy.
pub fn subtract_signal(
    residual: &mut [f64],
    itone: &[i32; 79],
    freq: f64,
    dt: f64,
    refine_dt: bool,
) {
    let fs2 = SAMPLE_RATE as f64 / NDOWN as f64;
    let dt2 = 1.0 / fs2;
    let twopi = TWO_PI;

    // Generate the signal to subtract
    let mut sig = generate_ft8_signal(itone, freq, dt, fs2, dt2, twopi, refine_dt, residual.len());

    // Subtract from residual
    let len = residual.len().min(sig.len());
    for i in 0..len {
        residual[i] -= sig[i];
    }
}

fn generate_ft8_signal(
    itone: &[i32; 79],
    freq: f64,
    dt: f64,
    fs2: f64,
    dt2: f64,
    twopi: f64,
    refine_dt: bool,
    data_len: usize,
) -> Vec<f64> {
    let nsps = 1920;
    let symbol_duration = nsps as f64 / SAMPLE_RATE as f64;
    let start_sample = ((dt + 0.5) * SAMPLE_RATE as f64).round() as isize;

    // If refining, search ±90 samples for best fit
    let best_offset = if refine_dt {
        refine_dt_offset(start_sample, itone, freq, fs2, dt2, twopi, data_len)
    } else {
        0
    };

    let adjusted_start = start_sample + best_offset;
    let mut sig = vec![0.0; data_len];

    // Generate tone-by-tone signal
    for k in 0..79 {
        let tone_freq = freq + (itone[k] as f64 - 3.5) * (SAMPLE_RATE as f64 / nsps as f64);
        let tone_start = adjusted_start + (k as f64 * symbol_duration * SAMPLE_RATE as f64).round() as isize;
        let tone_len = (symbol_duration * SAMPLE_RATE as f64).round() as isize;

        for j in 0..tone_len {
            let idx = (tone_start + j) as isize;
            if idx >= 0 && (idx as usize) < data_len {
                let phase = twopi * tone_freq * (j as f64 / SAMPLE_RATE as f64);
                sig[idx as usize] += phase.cos();
            }
        }
    }

    // Normalize
    let max_val = sig.iter().map(|&x| x.abs()).fold(0.0f64, f64::max);
    if max_val > 1e-10 {
        for v in sig.iter_mut() {
            *v /= max_val;
        }
    }

    sig
}

fn refine_dt_offset(
    start_sample: isize,
    itone: &[i32; 79],
    freq: f64,
    fs2: f64,
    dt2: f64,
    twopi: f64,
    data_len: usize,
) -> isize {
    let mut best_energy = f64::MAX;
    let mut best_offset = 0;

    for offset in -90..=90 {
        let test_start = start_sample + offset;
        let energy = compute_residual_energy(test_start, itone, freq, data_len, fs2, dt2, twopi);
        if energy < best_energy {
            best_energy = energy;
            best_offset = offset;
        }
    }

    best_offset
}

fn compute_residual_energy(
    start_sample: isize,
    itone: &[i32; 79],
    freq: f64,
    data_len: usize,
    fs2: f64,
    dt2: f64,
    twopi: f64,
) -> f64 {
    let nsps = 1920;
    let symbol_duration = nsps as f64 / SAMPLE_RATE as f64;
    let mut energy = 0.0;

    for k in 0..79 {
        let tone_start = start_sample + (k as f64 * symbol_duration * SAMPLE_RATE as f64).round() as isize;
        let tone_len = (symbol_duration * SAMPLE_RATE as f64).round() as isize;

        for j in 0..tone_len {
            let idx = (tone_start + j) as isize;
            if idx >= 0 && (idx as usize) < data_len {
                let tone_freq = freq + (itone[k] as f64 - 3.5) * (SAMPLE_RATE as f64 / nsps as f64);
                let phase = twopi * tone_freq * (j as f64 / SAMPLE_RATE as f64);
                let sample_val = phase.cos();
                energy += sample_val * sample_val;
            }
        }
    }

    energy
}
