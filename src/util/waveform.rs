/// GFSK waveform generation for FT8/FT4.

const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
const MODULATION_INDEX: f64 = 1.0;

#[derive(Default)]
pub struct WaveformOptions {
    pub sample_rate: Option<f64>,
    pub samples_per_symbol: Option<usize>,
    pub bt: Option<f64>,
    pub base_frequency: Option<f64>,
    pub initial_phase: Option<f64>,
}


struct WaveformDefaults {
    sample_rate: f64,
    samples_per_symbol: usize,
    bt: f64,
}

struct WaveformShape {
    include_ramp_symbols: bool,
    full_symbol_ramp: bool,
}

// Abramowitz and Stegun 7.1.26 approximation.
fn erf_approx(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let y = 1.0
        - ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t
            * (-ax * ax).exp();
    sign * y
}

fn gfsk_pulse(bt: f64, tt: f64) -> f64 {
    let scale = std::f64::consts::PI * (2.0 / std::f64::consts::LN_2).sqrt() * bt;
    0.5 * (erf_approx(scale * (tt + 0.5)) - erf_approx(scale * (tt - 0.5)))
}

fn generate_gfsk_waveform(
    tones: &[u8],
    options: WaveformOptions,
    defaults: WaveformDefaults,
    shape: WaveformShape,
) -> Vec<f32> {
    let nsym = tones.len();
    if nsym == 0 {
        return Vec::new();
    }

    let sample_rate = options.sample_rate.unwrap_or(defaults.sample_rate);
    let nsps = options.samples_per_symbol.unwrap_or(defaults.samples_per_symbol);
    let bt = options.bt.unwrap_or(defaults.bt);
    let f0 = options.base_frequency.unwrap_or(0.0);
    let initial_phase = options.initial_phase.unwrap_or(0.0);

    let nwave = if shape.include_ramp_symbols {
        (nsym + 2) * nsps
    } else {
        nsym * nsps
    };

    // GFSK pulse shaping
    let mut pulse = vec![0.0; 3 * nsps];
    for i in 0..pulse.len() {
        let tt = (i as f64 + 1.0 - 1.5 * nsps as f64) / nsps as f64;
        pulse[i] = gfsk_pulse(bt, tt);
    }

    let mut dphi = vec![0.0; (nsym + 2) * nsps];
    let dphi_peak = (TWO_PI * MODULATION_INDEX) / nsps as f64;

    for j in 0..nsym {
        let tone = tones[j] as f64;
        let ib = j * nsps;
        for i in 0..pulse.len() {
            dphi[ib + i] += dphi_peak * pulse[i] * tone;
        }
    }

    let first_tone = tones[0] as f64;
    let last_tone = tones[nsym - 1] as f64;
    let tail_base = nsym * nsps;
    for i in 0..(2 * nsps) {
        dphi[i] += dphi_peak * first_tone * pulse[nsps + i];
        dphi[tail_base + i] += dphi_peak * last_tone * pulse[i];
    }

    let carrier_dphi = TWO_PI * f0 / sample_rate;
    for i in 0..dphi.len() {
        dphi[i] += carrier_dphi;
    }

    // Generate waveform
    let mut wave = vec![0.0f32; nwave];
    let mut phi = initial_phase % TWO_PI;
    if phi < 0.0 {
        phi += TWO_PI;
    }
    let phase_start = if shape.include_ramp_symbols { 0 } else { nsps };

    for k in 0..nwave {
        let j = phase_start + k;
        wave[k] = phi.sin() as f32;
        phi += dphi[j];
        phi %= TWO_PI;
        if phi < 0.0 {
            phi += TWO_PI;
        }
    }

    // Apply ramp
    if shape.full_symbol_ramp {
        for i in 0..nsps {
            let up = (1.0 - (TWO_PI * i as f64) / (2.0 * nsps as f64)).cos() / 2.0;
            wave[i] *= up as f32;
        }

        let tail_start = (nsym + 1) * nsps;
        for i in 0..nsps {
            let down = (1.0 + (TWO_PI * i as f64) / (2.0 * nsps as f64)).cos() / 2.0;
            wave[tail_start + i] *= down as f32;
        }
    } else {
        let nramp = (nsps as f64 / 8.0).round() as usize;
        for i in 0..nramp {
            let up = (1.0 - (TWO_PI * i as f64) / (2.0 * nramp as f64)).cos() / 2.0;
            wave[i] *= up as f32;
        }

        let tail_start = nwave - nramp;
        for i in 0..nramp {
            let down = (1.0 + (TWO_PI * i as f64) / (2.0 * nramp as f64)).cos() / 2.0;
            wave[tail_start + i] *= down as f32;
        }
    }

    wave
}

const FT8_DEFAULT_SAMPLE_RATE: f64 = 12_000.0;
const FT8_DEFAULT_SAMPLES_PER_SYMBOL: usize = 1_920;
const FT8_DEFAULT_BT: f64 = 2.0;

const FT4_DEFAULT_SAMPLE_RATE: f64 = 12_000.0;
const FT4_DEFAULT_SAMPLES_PER_SYMBOL: usize = 576;
const FT4_DEFAULT_BT: f64 = 1.0;

pub fn generate_ft8_waveform(tones: &[u8], options: WaveformOptions) -> Vec<f32> {
    generate_gfsk_waveform(
        tones,
        options,
        WaveformDefaults {
            sample_rate: FT8_DEFAULT_SAMPLE_RATE,
            samples_per_symbol: FT8_DEFAULT_SAMPLES_PER_SYMBOL,
            bt: FT8_DEFAULT_BT,
        },
        WaveformShape {
            include_ramp_symbols: false,
            full_symbol_ramp: false,
        },
    )
}

pub fn generate_ft4_waveform(tones: &[u8], options: WaveformOptions) -> Vec<f32> {
    generate_gfsk_waveform(
        tones,
        options,
        WaveformDefaults {
            sample_rate: FT4_DEFAULT_SAMPLE_RATE,
            samples_per_symbol: FT4_DEFAULT_SAMPLES_PER_SYMBOL,
            bt: FT4_DEFAULT_BT,
        },
        WaveformShape {
            include_ramp_symbols: true,
            full_symbol_ramp: true,
        },
    )
}
