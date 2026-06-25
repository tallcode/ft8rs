//! Feature-gated decode profiling (P0 of the hardware-acceleration study).
//!
//! Probes are placed at *call sites* — we wrap a call, never edit the algorithm
//! body — so this stays orchestration-flavored and does not touch decode math.
//!
//! With the `profiling` feature OFF (the default, and what the byte-identical
//! baseline tests build), [`scope`] returns a zero-sized guard with no `Drop`,
//! so every probe compiles to nothing: no float math changes, no overhead,
//! alignment with WSJT-X/JTDX preserved. Turn it ON with `--features profiling`
//! to collect per-stage wall-clock and call counts via lock-free atomics, then
//! print [`report`].
//!
//! The accumulators are process-global atomics, so they aggregate correctly
//! across the dx concurrent-foci worker threads as well as the single-threaded
//! file/soundcard path. Call [`reset`] before a run to zero them.

/// Coarse decode stages timed at their call sites. Discriminants index the
/// atomic accumulator arrays, so keep them contiguous from 0.
#[derive(Clone, Copy)]
pub enum Stage {
    /// Whole-slot decode (`decode_slot_streaming_at`) — the denominator.
    Slot = 0,
    /// Candidate search + spectra (`sync8`).
    Sync = 1,
    /// Per-candidate decode (`ft8b`), inclusive of everything below.
    Ft8b = 2,
    /// LDPC belief propagation (`bpdecode174_91`).
    Bp = 3,
    /// Ordered-statistics decode fallback (`osd174_91`).
    Osd = 4,
    /// Decoded-signal subtraction (`subtractft8`).
    Subtract = 5,
    /// sync8 sub-stage: symbol spectra FFTs (`compute_symbol_spectra`).
    SyncSpectra = 6,
    /// sync8 sub-stage: 2D correlation (`compute_sync2d`).
    Sync2d = 7,
    /// sync8 sub-stage: candidate extraction (`extract_candidates`).
    SyncExtract = 8,
    /// OSD sub-stage: Gaussian elimination.
    OsdElim = 9,
    /// OSD sub-stage: `mrbencode91_into` re-encoding (leaf, spans the searches).
    OsdEncode = 10,
    /// OSD sub-stage: weighted-sum distance (leaf, spans the searches).
    OsdDist = 11,
    /// OSD sub-stage: npre2 box-build loop (`boxit91_pattern` hashing).
    OsdBox = 12,
}

/// Number of stages; also the length of the accumulator arrays.
pub const STAGE_COUNT: usize = 13;

#[cfg(feature = "profiling")]
mod imp {
    use super::{Stage, STAGE_COUNT};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    const STAGE_NAMES: [&str; STAGE_COUNT] = [
        "slot",
        "sync8",
        "ft8b",
        "ldpc-bp",
        "osd",
        "subtract",
        "└spectra",
        "└sync2d",
        "└extract",
        "·osd-elim",
        "·osd-enc",
        "·osd-dist",
        "·osd-box",
    ];

    static NANOS: [AtomicU64; STAGE_COUNT] = [const { AtomicU64::new(0) }; STAGE_COUNT];
    static CALLS: [AtomicU64; STAGE_COUNT] = [const { AtomicU64::new(0) }; STAGE_COUNT];

    /// RAII guard: on drop, adds the elapsed time and one call to its stage.
    pub struct Scope {
        stage: usize,
        t0: Instant,
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            let dt = self.t0.elapsed().as_nanos() as u64;
            NANOS[self.stage].fetch_add(dt, Ordering::Relaxed);
            CALLS[self.stage].fetch_add(1, Ordering::Relaxed);
        }
    }

    #[inline]
    pub fn scope(stage: Stage) -> Scope {
        Scope {
            stage: stage as usize,
            t0: Instant::now(),
        }
    }

    pub fn reset() {
        for i in 0..STAGE_COUNT {
            NANOS[i].store(0, Ordering::Relaxed);
            CALLS[i].store(0, Ordering::Relaxed);
        }
    }

    pub fn report() -> String {
        let nanos: Vec<u64> = NANOS.iter().map(|a| a.load(Ordering::Relaxed)).collect();
        let calls: Vec<u64> = CALLS.iter().map(|a| a.load(Ordering::Relaxed)).collect();
        let slot_ns = nanos[Stage::Slot as usize].max(1);
        let ms = |ns: u64| ns as f64 / 1e6;
        let mut out = String::new();
        out.push_str("=== ft8rs decode profile (--features profiling) ===\n");
        out.push_str(&format!(
            "{:<10} {:>12} {:>10} {:>12} {:>8}\n",
            "stage", "total(ms)", "calls", "avg(us)", "%slot"
        ));
        for i in 0..STAGE_COUNT {
            let avg_us = if calls[i] > 0 {
                (nanos[i] as f64 / calls[i] as f64) / 1e3
            } else {
                0.0
            };
            let pct = nanos[i] as f64 / slot_ns as f64 * 100.0;
            out.push_str(&format!(
                "{:<10} {:>12.2} {:>10} {:>12.2} {:>7.1}%\n",
                STAGE_NAMES[i],
                ms(nanos[i]),
                calls[i],
                avg_us,
                pct
            ));
        }
        let bp = calls[Stage::Bp as usize];
        let osd = calls[Stage::Osd as usize];
        let hit = if bp > 0 {
            osd as f64 / bp as f64 * 100.0
        } else {
            0.0
        };
        out.push_str(&format!(
            "osd fallback rate: {osd}/{bp} bp attempts = {hit:.1}%\n"
        ));
        out
    }
}

#[cfg(not(feature = "profiling"))]
mod imp {
    use super::Stage;

    /// Zero-sized, no-`Drop` guard. Optimizes away entirely.
    pub struct Scope;

    #[inline(always)]
    pub fn scope(_stage: Stage) -> Scope {
        Scope
    }

    pub fn reset() {}

    pub fn report() -> String {
        "profiling disabled (build with --features profiling)".to_string()
    }
}

pub use imp::{report, reset, scope, Scope};
