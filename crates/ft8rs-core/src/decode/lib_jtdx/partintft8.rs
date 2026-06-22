//! Mirrors JTDX `lib/partintft8.f90`.
#![allow(dead_code)]

const MAX_DELAY_SECONDS: usize = 120;
const SAMPLES_PER_DELAY_UNIT: usize = 1_200;
const DD8_LEN: usize = 180_000;

pub(crate) fn partintft8(dd8: &mut [f32], ndelay: usize) -> usize {
    let ndelay = ndelay.min(MAX_DELAY_SECONDS);
    let numsamp = ndelay * SAMPLES_PER_DELAY_UNIT;
    if numsamp == 0 || dd8.is_empty() {
        return 0;
    }

    let nmax = dd8.len().min(DD8_LEN);
    let shift = numsamp.min(nmax);
    for i in (shift..nmax).rev() {
        dd8[i] = dd8[i - shift];
    }

    let mut rng = PartIntNoise::new(ndelay as u64);
    for sample in dd8.iter_mut().take(shift) {
        *sample = 10.0 * rng.next_f32() - 5.0;
    }
    shift
}

struct PartIntNoise {
    state: u64,
}

impl PartIntNoise {
    fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn next_f32(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        ((self.state >> 40) as f32) / ((1u64 << 24) as f32)
    }
}
