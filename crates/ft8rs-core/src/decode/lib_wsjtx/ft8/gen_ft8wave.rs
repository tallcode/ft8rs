//! FT8 Gaussian-filtered waveform generation.
//!
//! Source mapping:
//! - `wsjtx/lib/ft8/gen_ft8wave.f90`

use std::f64::consts::PI;
use std::sync::OnceLock;

pub(crate) const NFRAME: usize = 1920 * 79;
const SAMPLE_RATE: f64 = 12000.0;
const NSPS_WAVE: usize = 1920;

/// Cached GFSK pulse: computed once, reused across all gen_ft8wave calls.
fn gsfk_pulse_cached() -> &'static Vec<f64> {
    static PULSE: OnceLock<Vec<f64>> = OnceLock::new();
    PULSE.get_or_init(|| {
        let bt = 2.0f64;
        let nsps = NSPS_WAVE;
        (0..3 * nsps)
            .map(|i| {
                let tt = (i as f64 + 1.0 - 1.5 * nsps as f64) / nsps as f64;
                let c = PI * (2.0 / std::f64::consts::LN_2).sqrt();
                0.5 * (erf_approx(c * bt * (tt + 0.5)) - erf_approx(c * bt * (tt - 0.5)))
            })
            .collect()
    })
}

/// WSJT-X `gen_ft8wave` caches `ctab(0:NTAB-1)` after the first call.
fn complex_phase_table_cached() -> &'static Vec<(f64, f64)> {
    static CTAB: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    CTAB.get_or_init(|| {
        let ntab = 65536usize;
        let twopi_over_ntab = 2.0 * PI / ntab as f64;
        (0..ntab)
            .map(|i| {
                let phi = i as f64 * twopi_over_ntab;
                (phi.cos(), phi.sin())
            })
            .collect()
    })
}

/// Abramowitz & Stegun 7.1.26 erf approximation.
fn erf_approx(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let ax = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * ax);
    let y = 1.0
        - (((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t
            + 0.254829592)
            * t)
            * (-ax * ax).exp();
    sign * y
}

/// Generate complex FT8 reference waveform with `icmplx=1`.
pub(crate) fn gen_ft8wave(itone: &[i32; 79], f0: f64) -> (Vec<f64>, Vec<f64>) {
    let nsps: usize = NSPS_WAVE;
    let nsym: usize = 79;
    let twopi = 2.0 * PI;
    let dt = 1.0 / SAMPLE_RATE;
    let ntab: usize = 65536;
    let twopi_over_ntab = twopi / ntab as f64;

    let pulse = gsfk_pulse_cached();
    let pulse_len = pulse.len();
    let ctab = complex_phase_table_cached();

    let dphi_len = (nsym + 2) * nsps;
    let mut dphi = vec![0.0f64; dphi_len];
    let dphi_peak = twopi / nsps as f64;

    for j in 0..nsym {
        let ib = j * nsps;
        let tone = itone[j] as f64;
        for i in 0..pulse_len {
            dphi[ib + i] += dphi_peak * pulse[i] * tone;
        }
    }
    let first_tone = itone[0] as f64;
    for i in 0..2 * nsps {
        dphi[i] += dphi_peak * first_tone * pulse[nsps + i];
    }
    let last_tone = itone[nsym - 1] as f64;
    let tail_base = nsym * nsps;
    for i in 0..2 * nsps {
        dphi[tail_base + i] += dphi_peak * last_tone * pulse[i];
    }

    let carrier_dphi = twopi * f0 * dt;
    for slot in &mut dphi {
        *slot += carrier_dphi;
    }

    let nwave = nsym * nsps;
    debug_assert_eq!(nwave, NFRAME);
    let mut cwave_re = vec![0.0f64; nwave];
    let mut cwave_im = vec![0.0f64; nwave];
    let mut phi = 0.0f64;

    for j in nsps..(nsps + nwave) {
        let k = j - nsps;
        let idx = ((phi / twopi_over_ntab) as usize) % ntab;
        let (re, im) = ctab[idx];
        cwave_re[k] = re;
        cwave_im[k] = im;
        phi += dphi[j];
        while phi >= twopi {
            phi -= twopi;
        }
    }

    let nramp = (nsps as f64 / 8.0).round() as usize;
    for i in 0..nramp {
        let env = (1.0 - ((twopi * i as f64) / (2.0 * nramp as f64)).cos()) / 2.0;
        cwave_re[i] *= env;
        cwave_im[i] *= env;
    }
    let k1 = nsym * nsps - nramp;
    for i in 0..nramp {
        let env = (1.0 + ((twopi * i as f64) / (2.0 * nramp as f64)).cos()) / 2.0;
        cwave_re[k1 + i] *= env;
        cwave_im[k1 + i] *= env;
    }

    (cwave_re, cwave_im)
}
