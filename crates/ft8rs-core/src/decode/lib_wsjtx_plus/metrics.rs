//! Opt-in inclusive timings. Profiling runs are separate from performance runs.
use std::{cell::RefCell, collections::BTreeMap, time::Instant};

#[derive(Clone, Copy, Debug)]
pub(super) enum Kind {
    Fft,
    Sync,
    Demod,
    Fec,
    Bp,
    Osd,
    Subtract,
    A7,
    A8,
}

#[derive(Default, serde::Serialize)]
pub(super) struct Metrics {
    pub calls_ns: BTreeMap<String, (u64, u128)>,
    pub fft_sizes: BTreeMap<usize, u64>,
    pub scratch_grows: u64,
    pub scratch_grown_bytes: usize,
}

thread_local! {
    static METRICS: RefCell<Option<Metrics>> = const { RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn begin() {
    METRICS.with_borrow_mut(|m| {
        assert!(m.is_none(), "nested metrics collection");
        *m = Some(Metrics::default());
    });
}

#[cfg(test)]
pub(super) fn finish() -> Metrics {
    METRICS.with_borrow_mut(|m| m.take().expect("metrics not started"))
}

pub(super) fn active() -> bool {
    METRICS.with_borrow(Option::is_some)
}

pub(super) struct Span(Kind, Option<Instant>);

impl Span {
    pub fn new(kind: Kind) -> Self {
        Self(
            kind,
            METRICS.with_borrow(|m| m.as_ref().map(|_| Instant::now())),
        )
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.1 {
            let ns = start.elapsed().as_nanos();
            METRICS.with_borrow_mut(|m| {
                let value = m
                    .as_mut()
                    .unwrap()
                    .calls_ns
                    .entry(format!("{:?}", self.0))
                    .or_default();
                value.0 += 1;
                value.1 += ns;
            });
        }
    }
}

pub(super) fn fft(n: usize) {
    METRICS.with_borrow_mut(|m| {
        if let Some(m) = m {
            *m.fft_sizes.entry(n).or_default() += 1;
        }
    });
}

pub(super) fn scratch_growth(bytes: usize) {
    METRICS.with_borrow_mut(|m| {
        if let Some(m) = m {
            m.scratch_grows += 1;
            m.scratch_grown_bytes += bytes;
        }
    });
}
