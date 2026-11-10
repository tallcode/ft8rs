//! UTC slot accumulation and staged 12 kHz windows.
use ft8rs::input::audio::downsample_12k;
use ft8rs::SlotTimestamp;

const TARGET_SAMPLE_RATE: u32 = 12_000;
pub(super) const SLOT_SECONDS: u64 = 15;
const NZHSYM_STRIDE: usize = 3456;
const SLOT_MS: i64 = SLOT_SECONDS as i64 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum SlotStage {
    N41,
    N47,
    N50,
}

/// Diagnostics for one just-completed slot — surfaced to the GUI so a single live
/// test reveals how the driver actually delivers audio (block size / count), how
/// full and well-timed the decode window is, and whether the block *arrival* times
/// (`capture_ms`) are smooth or jittery. High `max_jitter_ms` on a driver that
/// still decodes elsewhere is the tell that capture-timestamps are unreliable.
pub(crate) struct SlotDiag {
    pub peak: f32,
    pub samples: usize,     // native samples in the slot's decode window
    pub blocks: u32,        // capture callbacks that fed this slot
    pub avg_block: u32,     // mean block size in samples
    pub max_jitter_ms: i64, // worst deviation of block arrival from smooth real-time
}

/// What a `PeriodAccumulator::push` produced: staged 12 kHz windows to hand the
/// decoder (in order), and, when a slot just finished, its diagnostics.
#[derive(Default)]
pub(crate) struct AccOutput {
    pub boundary_crossed: bool,
    pub stages: Vec<(SlotStage, SlotTimestamp, Vec<f32>)>,
    pub completed: Option<SlotDiag>,
}

/// Whole-block UTC alignment, matching the native once-per-callback boundary
/// check. Samples are appended by count within that slot; callback timestamps
/// are not hardware sampling timestamps and do not split straddling blocks.
pub(crate) struct PeriodAccumulator {
    sample_rate: u32,
    boundary: Option<i64>, // current slot's start (unix seconds, multiple of 15)
    native: Vec<f32>,      // native-rate samples accumulated for the current slot
    stage: u8,             // 0 none, 1 N41 fired, 2 N47 fired, 3 N50 fired
    peak: f32,             // running peak amplitude of the current slot
    // Per-slot capture diagnostics.
    blocks: u32,
    slot_t0_ms: Option<i64>, // delivery time just before the slot's first sample
    max_jitter_ms: i64,
}

impl PeriodAccumulator {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            boundary: None,
            native: Vec::new(),
            stage: 0,
            peak: 0.0,
            blocks: 0,
            slot_t0_ms: None,
            max_jitter_ms: 0,
        }
    }

    /// Feed one captured block. Returns any staged windows now ready plus (once
    /// per slot) the completed-slot marker.
    ///
    /// Attributes the *whole* block to the UTC slot of its capture time and
    /// appends its samples by count — exactly like WSJT-X, whose `writeData` keys a
    /// once-per-block wall-clock check to decide only *whether* it crossed the slot
    /// boundary (resetting `kin`), and otherwise packs samples densely by index. It
    /// never uses a per-block timestamp to position individual samples. That coarse
    /// decision tolerates block-delivery-time jitter (as on cpal's WASAPI capture);
    /// a finer sub-sample split does not, and smears the window on jittery drivers.
    pub(crate) fn push(&mut self, chunk: &[f32], capture_ms: i64) -> AccOutput {
        let mut out = AccOutput::default();
        if chunk.is_empty() {
            return out;
        }
        let slot = capture_ms.div_euclid(SLOT_MS) * SLOT_SECONDS as i64;
        match self.boundary {
            None => self.boundary = Some(slot),
            Some(b) if slot > b => {
                // Crossed into a new slot: finish the old one, start fresh.
                self.finalize(&mut out);
                self.reset(slot);
                out.boundary_crossed = true;
            }
            // Clock stepped back into an already-finished slot: ignore this block.
            Some(b) if slot < b => return out,
            _ => {}
        }
        // Capture diagnostics: measure how far this block's arrival time deviates
        // from smooth real-time delivery (t0 + samples/rate). Small = clean timing;
        // large = bursty/jittery driver, i.e. `capture_ms` can't be trusted for fine
        // sample placement (the reason we grid whole blocks, not sub-samples).
        let n = chunk.len() as i64;
        let rate = self.sample_rate as i64;
        self.blocks += 1;
        match self.slot_t0_ms {
            None => self.slot_t0_ms = Some(capture_ms - n * 1000 / rate),
            Some(t0) => {
                let expected = t0 + (self.native.len() as i64 + n) * 1000 / rate;
                self.max_jitter_ms = self.max_jitter_ms.max((capture_ms - expected).abs());
            }
        }
        self.append(chunk, &mut out);
        out
    }

    /// Flush the in-progress slot (e.g. on shutdown), emitting its final stage.
    pub(crate) fn flush(&mut self) -> AccOutput {
        let mut out = AccOutput::default();
        self.finalize(&mut out);
        self.stage = 3;
        out
    }

    fn append(&mut self, samples: &[f32], out: &mut AccOutput) {
        self.peak = self.peak.max(peak_level(samples));
        self.native.extend_from_slice(samples);
        // Stream the early stages as soon as enough audio has arrived.
        if self.stage == 0 && self.native.len() >= native_samples_for_nzhsym(self.sample_rate, 41) {
            out.stages.push((
                SlotStage::N41,
                self.ts(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 1;
        }
        if self.stage == 1 && self.native.len() >= native_samples_for_nzhsym(self.sample_rate, 47) {
            out.stages.push((
                SlotStage::N47,
                self.ts(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 2;
        }
    }

    fn finalize(&mut self, out: &mut AccOutput) {
        if self.stage == 0 {
            // Never reached the early-decode point: a runt slot (monitoring started
            // mid-slot, or a capture gap) — nothing worth decoding, drop it.
            return;
        }
        let ts = self.ts();
        if self.stage == 1 {
            out.stages.push((
                SlotStage::N47,
                ts.clone(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 2;
        }
        if self.stage == 2 {
            out.stages.push((
                SlotStage::N50,
                ts.clone(),
                downsample_12k(&self.native, self.sample_rate),
            ));
            self.stage = 3;
        }
        out.completed = Some(SlotDiag {
            peak: self.peak,
            samples: self.native.len(),
            blocks: self.blocks,
            avg_block: self.native.len() as u32 / self.blocks.max(1),
            max_jitter_ms: self.max_jitter_ms,
        });
    }

    fn reset(&mut self, boundary: i64) {
        self.boundary = Some(boundary);
        self.native.clear();
        self.stage = 0;
        self.peak = 0.0;
        self.blocks = 0;
        self.slot_t0_ms = None;
        self.max_jitter_ms = 0;
    }

    fn ts(&self) -> SlotTimestamp {
        SlotTimestamp::from_unix_seconds_utc(self.boundary.unwrap())
    }
}

/// Peak absolute amplitude of a sample buffer (0.0 when empty) — feeds the GUI
/// input-level readout so the operator can tell silence (dead capture) from signal.
pub(crate) fn peak_level(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

fn native_samples_for_nzhsym(sample_rate: u32, nzhsym: usize) -> usize {
    let samples_12k = nzhsym * NZHSYM_STRIDE;
    ((samples_12k as u64 * sample_rate as u64) + TARGET_SAMPLE_RATE as u64 - 1) as usize
        / TARGET_SAMPLE_RATE as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 12_000; // downsample_12k is a passthrough here, keeping buffers small

    #[test]
    fn discarded_partial_slot_still_reports_reconfiguration_boundary() {
        let mut acc = PeriodAccumulator::new(SR);
        for ms in (10_000..15_000).step_by(100) {
            assert!(!acc.push(&[0.0; 1200], ms).boundary_crossed);
        }
        let out = acc.push(&[0.0; 1200], 15_000);
        assert!(out.boundary_crossed);
        assert!(out.completed.is_none());
        assert!(out.stages.is_empty());
    }

    /// Feed `chunks` blocks of `chunk_len` samples each, at real-time capture
    /// timestamps starting from `start_ms`, collecting every emitted stage and the
    /// completed-slot markers.
    fn drive(
        acc: &mut PeriodAccumulator,
        chunk_len: usize,
        chunks: usize,
        start_ms: i64,
    ) -> (Vec<(SlotStage, SlotTimestamp, usize)>, Vec<SlotTimestamp>) {
        let mut stages = Vec::new();
        let mut done = Vec::new();
        let block = vec![0.1f32; chunk_len];
        for c in 0..chunks {
            let capture_ms = start_ms + ((c as i64 + 1) * chunk_len as i64 * 1000) / SR as i64;
            let out = acc.push(&block, capture_ms);
            for (st, ts, s) in out.stages {
                // A slot finishes when its N50 window is emitted.
                if st == SlotStage::N50 {
                    done.push(ts.clone());
                }
                stages.push((st, ts, s.len()));
            }
        }
        (stages, done)
    }

    #[test]
    fn whole_capture_blocks_can_exceed_fifteen_seconds() {
        const B: i64 = 1_700_000_010;
        let mut acc = PeriodAccumulator::new(SR);
        // 1024-sample driver blocks don't divide a 15 s period. In a continuous
        // stream some slots contain one extra block; this is valid capture.
        let (stages, _) = drive(&mut acc, 1024, SR as usize * 61 / 1024, B * 1000);
        let lengths: Vec<_> = stages
            .iter()
            .filter(|(stage, _, _)| *stage == SlotStage::N50)
            .map(|(_, _, len)| *len)
            .collect();
        assert!(lengths.iter().any(|&len| len > 180_000), "{lengths:?}");
    }

    #[test]
    fn full_slot_emits_all_three_stages_with_near_full_window() {
        const B: i64 = 1_700_000_010; // a multiple of 15
        let mut acc = PeriodAccumulator::new(SR);
        // 32 half-second blocks = 16 s, so slot B fills and the next one begins.
        let (stages, done) = drive(&mut acc, SR as usize / 2, 32, B * 1000);

        let kinds: Vec<_> = stages.iter().map(|(k, _, _)| *k).collect();
        assert_eq!(kinds, vec![SlotStage::N41, SlotStage::N47, SlotStage::N50]);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        assert!(stages.iter().all(|(_, ts, _)| *ts == ts_b));
        assert_eq!(done, vec![ts_b]);
        // N50 window covers the slot up to the block that crossed the boundary — at
        // least the ~14.4 s (172 800 @ 12 kHz) the decoder consumes.
        let (_, _, n50_len) = *stages.last().unwrap();
        assert!(
            (172_800..=180_000).contains(&n50_len),
            "N50 window should be a near-full slot, got {n50_len}"
        );
    }

    #[test]
    fn each_whole_block_grids_to_its_slot_and_both_decode() {
        const B: i64 = 1_700_000_010;
        let mut acc = PeriodAccumulator::new(SR);
        // Run through slot B and well into slot B+15 so both finalize.
        let (stages, done) = drive(&mut acc, SR as usize / 2, 62, B * 1000);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        let ts_b1 = SlotTimestamp::from_unix_seconds_utc(B + 15);
        assert_eq!(done, vec![ts_b, ts_b1.clone()]);
        // Each slot produced its own N41/N47/N50.
        let for_b1: Vec<_> = stages
            .iter()
            .filter(|(_, ts, _)| *ts == ts_b1)
            .map(|(k, _, _)| *k)
            .collect();
        assert_eq!(for_b1, vec![SlotStage::N41, SlotStage::N47, SlotStage::N50]);
    }

    #[test]
    fn runt_first_slot_is_dropped_not_decoded() {
        const B: i64 = 1_700_000_010;
        let mut acc = PeriodAccumulator::new(SR);
        // Start 12 s into slot B: only ~3 s of it captured -> below the N41 point.
        let start = (B * 1000) + 12_000;
        let (stages, done) = drive(&mut acc, SR as usize / 2, 40, start);
        let ts_b = SlotTimestamp::from_unix_seconds_utc(B);
        // Slot B is a runt: no stages, not marked done.
        assert!(stages.iter().all(|(_, ts, _)| *ts != ts_b));
        assert!(!done.contains(&ts_b));
        // The next full slot decodes normally.
        let ts_b1 = SlotTimestamp::from_unix_seconds_utc(B + 15);
        assert!(done.contains(&ts_b1));
    }

    // Synthetic opposite-phase input: averaging cancels it; Mono must retain
    // signal. This is a regression case, not a claim about every DAX driver.
}
