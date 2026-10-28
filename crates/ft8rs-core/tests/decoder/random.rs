//! Fixed, test-only PRNG and Gaussian samples. No system entropy or time seeds.
pub struct Noise {
    state: u64,
    spare: Option<f64>,
}

impl Noise {
    pub fn new(seed: u64) -> Self {
        assert_ne!(seed, 0);
        Self {
            state: seed,
            spare: None,
        }
    }

    fn uniform(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        (((self.state.wrapping_mul(2685821657736338717) >> 11) as f64) + 0.5) / (1u64 << 53) as f64
    }

    pub fn gaussian(&mut self) -> f64 {
        if let Some(value) = self.spare.take() {
            return value;
        }
        let radius = (-2.0 * self.uniform().ln()).sqrt();
        let angle = std::f64::consts::TAU * self.uniform();
        self.spare = Some(radius * angle.sin());
        radius * angle.cos()
    }
}
