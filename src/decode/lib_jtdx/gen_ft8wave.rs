//! Mirrors JTDX `lib/gen_ft8wave.f90`.

use std::f64::consts::PI;
use std::sync::OnceLock;

pub(crate) const NFRAME: usize = 1920 * 79;
const NSPS: usize = 1920;
const SAMPLE_RATE: f64 = 12000.0;
const NTAB: usize = 65536;

pub(crate) fn gen_ft8wave(itone: &[i32; 79], f0: f64) -> (Vec<f64>, Vec<f64>) {
    let pulse = pulse_cached();
    let ctab = ctab_cached();
    let twopi = 2.0 * PI;
    let mut dphi = vec![0.0f64; (79 + 2) * NSPS];
    let dphi_peak = twopi / NSPS as f64;

    for j in 0..79 {
        let ib = j * NSPS;
        for i in 0..(3 * NSPS) {
            dphi[ib + i] += dphi_peak * pulse[i] * itone[j] as f64;
        }
    }
    for i in 0..(2 * NSPS) {
        dphi[i] += dphi_peak * itone[0] as f64 * pulse[NSPS + i];
        dphi[79 * NSPS + i] += dphi_peak * itone[78] as f64 * pulse[i];
    }

    let carrier = twopi * f0 / SAMPLE_RATE;
    for value in &mut dphi {
        *value += carrier;
    }

    let mut cwave_re = vec![0.0f64; NFRAME];
    let mut cwave_im = vec![0.0f64; NFRAME];
    let twopi_over_ntab = twopi / NTAB as f64;
    let mut phi = 0.0f64;
    for j in NSPS..(NSPS + NFRAME) {
        let k = j - NSPS;
        let idx = ((phi / twopi_over_ntab) as usize) % NTAB;
        let (re, im) = ctab[idx];
        cwave_re[k] = re;
        cwave_im[k] = im;
        phi = (phi + dphi[j]) % twopi;
    }

    let nramp = (NSPS as f64 / 8.0).round() as usize;
    for i in 0..nramp {
        let env = (1.0 - (twopi * i as f64 / (2.0 * nramp as f64)).cos()) / 2.0;
        cwave_re[i] *= env;
        cwave_im[i] *= env;
    }
    let k1 = 79 * NSPS - nramp;
    for i in 0..nramp {
        let env = (1.0 + (twopi * i as f64 / (2.0 * nramp as f64)).cos()) / 2.0;
        cwave_re[k1 + i] *= env;
        cwave_im[k1 + i] *= env;
    }

    (cwave_re, cwave_im)
}

fn pulse_cached() -> &'static Vec<f64> {
    static PULSE: OnceLock<Vec<f64>> = OnceLock::new();
    PULSE.get_or_init(|| {
        let bt = 2.0f64;
        (1..=3 * NSPS)
            .map(|i| {
                let tt = (i as f64 - 1.5 * NSPS as f64) / NSPS as f64;
                gfsk_pulse(bt, tt)
            })
            .collect()
    })
}

fn ctab_cached() -> &'static Vec<(f64, f64)> {
    static CTAB: OnceLock<Vec<(f64, f64)>> = OnceLock::new();
    CTAB.get_or_init(|| {
        let twopi = 2.0 * PI;
        (0..NTAB)
            .map(|i| {
                let phi = i as f64 * twopi / NTAB as f64;
                (phi.cos(), phi.sin())
            })
            .collect()
    })
}

fn gfsk_pulse(bt: f64, tt: f64) -> f64 {
    let c = PI * (2.0 / std::f64::consts::LN_2).sqrt();
    0.5 * (erf_approx(c * bt * (tt + 0.5)) - erf_approx(c * bt * (tt - 0.5)))
}

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
