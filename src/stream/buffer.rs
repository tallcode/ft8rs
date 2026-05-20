/// Audio buffer management for streaming FT8 decode.
/// Matches WSJT-X nzhsym accumulation mechanism.
use std::f64;

const SAMPLE_RATE: u32 = 12000;
const NSPS: usize = 1920;
const NSTEP: usize = NSPS / 4; // 480
const NMAX_15S: usize = 15 * 12000; // 180000

/// Decode stage based on accumulated symbols (nzhsym).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecodeStage {
    /// nzhsym=41, ~11s, syncmin=2.0, strong signals only
    Early,
    /// nzhsym=47, ~13s, subtract strong signals
    Subtract,
    /// nzhsym=50, 15s, full decode
    Full,
    /// Not enough data yet
    Insufficient,
}

impl DecodeStage {
    pub fn from_nzhsym(nzhsym: usize) -> Self {
        if nzhsym >= 50 {
            DecodeStage::Full
        } else if nzhsym >= 47 {
            DecodeStage::Subtract
        } else if nzhsym >= 41 {
            DecodeStage::Early
        } else {
            DecodeStage::Insufficient
        }
    }

    pub fn sync_min(self) -> f64 {
        match self {
            DecodeStage::Early => 2.0,
            DecodeStage::Subtract => 2.0,
            DecodeStage::Full => 1.3,
            DecodeStage::Insufficient => 999.0,
        }
    }
}

/// Buffer that accumulates audio samples and tracks decode stage.
pub struct AudioBuffer {
    samples: Vec<f64>,
    sample_rate: u32,
    nzhsym: usize,
    /// Total samples for 15s window
    max_samples: usize,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32) -> Self {
        let max_samples = (sample_rate as f64 * 15.0).ceil() as usize;
        Self {
            samples: Vec::with_capacity(max_samples),
            sample_rate,
            nzhsym: 0,
            max_samples,
        }
    }

    /// Push a chunk of audio samples (f32, will be converted to f64).
    pub fn push(&mut self, chunk: &[f32]) {
        let current_len = self.samples.len();
        let new_len = (current_len + chunk.len()).min(self.max_samples);

        if new_len <= self.max_samples {
            if current_len < self.max_samples {
                let to_push = chunk.len().min(self.max_samples - current_len);
                for &s in &chunk[..to_push] {
                    self.samples.push(s as f64);
                }
            }
        }

        self.update_nzhsym();
    }

    /// Push f64 samples directly.
    pub fn push_f64(&mut self, chunk: &[f64]) {
        let current_len = self.samples.len();
        let new_len = (current_len + chunk.len()).min(self.max_samples);

        if current_len < self.max_samples {
            let to_push = chunk.len().min(self.max_samples - current_len);
            self.samples.extend_from_slice(&chunk[..to_push]);
        }

        self.update_nzhsym();
    }

    /// Reset buffer for new slot.
    pub fn reset(&mut self) {
        self.samples.clear();
        self.nzhsym = 0;
    }

    /// Current decode stage.
    pub fn stage(&self) -> DecodeStage {
        DecodeStage::from_nzhsym(self.nzhsym)
    }

    /// Current nzhsym value (matching WSJT-X semantics).
    pub fn nzhsym(&self) -> usize {
        self.nzhsym
    }

    /// Whether we have enough data for the given stage.
    pub fn has_enough_for(&self, stage: DecodeStage) -> bool {
        self.nzhsym >= match stage {
            DecodeStage::Early => 41,
            DecodeStage::Subtract => 47,
            DecodeStage::Full => 50,
            DecodeStage::Insufficient => 0,
        }
    }

    /// Get a reference to all accumulated samples.
    pub fn samples(&self) -> &[f64] {
        &self.samples
    }

    /// Get samples as owned Vec<f32> for passing to decode functions.
    pub fn samples_f32(&self) -> Vec<f32> {
        self.samples.iter().map(|&x| x as f32).collect()
    }

    /// Get number of accumulated samples.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Number of seconds of audio accumulated.
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / self.sample_rate as f64
    }

    /// Compute nzhsym from current samples (matching WSJT-X calculation).
    /// We map elapsed time to nzhsym to match WSJT-X behavior:
    ///   ~10-11s: nzhsym ≈ 41 (early decode begins)
    ///   ~12-13s: nzhsym ≈ 47 (subtract pass)
    ///   ~15s:    nzhsym = 50 (full decode)
    fn update_nzhsym(&mut self) {
        let total_samples = self.samples.len();
        if total_samples == 0 {
            self.nzhsym = 0;
            return;
        }

        // Linear mapping: nzhsym = floor(seconds * 56 / 15)
        // This gives: 10s→37, 10.5s→39, 11s→41, 12.5s→46, 13s→48, 15s→56
        // Use min to cap at 50
        let seconds = self.seconds();
        self.nzhsym = ((seconds * 50.0 / 15.0).floor() as usize).min(50);
    }

    /// Sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
