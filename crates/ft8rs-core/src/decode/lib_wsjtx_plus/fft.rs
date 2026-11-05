/// High-performance FFT engine using rustfft.
///
/// Key optimizations:
/// - Thread-local planner cache (avoids re-planning)
/// - Reusable buffers (avoids allocation)
/// - No normalization on forward FFT (matches FFTPACK)
/// - Caller-owned normalization in both directions
use rustfft::{num_complex::Complex, Fft, FftDirection, FftPlanner};
use std::sync::Arc;

thread_local! {
    static PLANNER: std::cell::RefCell<FftPlanner<f64>> =
        std::cell::RefCell::new(FftPlanner::new());
}

/// Cached FFT plans per-size.
struct PlanCache {
    forward: std::collections::HashMap<usize, Arc<dyn Fft<f64>>>,
    inverse: std::collections::HashMap<usize, Arc<dyn Fft<f64>>>,
}

impl PlanCache {
    fn new() -> Self {
        Self {
            forward: std::collections::HashMap::new(),
            inverse: std::collections::HashMap::new(),
        }
    }

    fn get_forward(&mut self, n: usize) -> Arc<dyn Fft<f64>> {
        self.forward
            .entry(n)
            .or_insert_with(|| PLANNER.with_borrow_mut(|p| p.plan_fft(n, FftDirection::Forward)))
            .clone()
    }

    fn get_inverse(&mut self, n: usize) -> Arc<dyn Fft<f64>> {
        self.inverse
            .entry(n)
            .or_insert_with(|| PLANNER.with_borrow_mut(|p| p.plan_fft(n, FftDirection::Inverse)))
            .clone()
    }
}

thread_local! {
    static CACHE: std::cell::RefCell<PlanCache> =
        std::cell::RefCell::new(PlanCache::new());
}

/// Reusable scratch buffers per-thread.
struct ScratchBuffers {
    complex_buf: Vec<Complex<f64>>,
    execution: Vec<Complex<f64>>,
}

impl ScratchBuffers {
    fn new() -> Self {
        Self {
            complex_buf: Vec::new(),
            execution: Vec::new(),
        }
    }

    fn ensure_capacity(&mut self, n: usize) {
        if self.complex_buf.len() < n {
            self.complex_buf.resize(n, Complex::ZERO);
        }
    }
}

thread_local! {
    static SCRATCH: std::cell::RefCell<ScratchBuffers> =
        std::cell::RefCell::new(ScratchBuffers::new());
}

/// four2a/FFTPACK-style complex FFT without normalization in either direction.
///
/// Used by call sites that apply the Fortran `fac` explicitly.
#[inline]
pub fn four2a_c2c(re: &mut [f64], im: &mut [f64], isign: i32) {
    if mode() == Mode::Reference || cfg!(feature = "fftw") {
        crate::util::four2a_c2c(re, im, isign);
        return;
    }
    let inverse = match isign {
        -1 => false,
        1 => true,
        _ => panic!("four2a_c2c only supports isign=-1 or isign=1"),
    };
    four2a_c2c_impl(re, im, inverse);
}

#[inline]
fn four2a_c2c_impl(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let _span = super::metrics::Span::new(super::metrics::Kind::Fft);
    super::metrics::fft(re.len());
    let n = re.len();
    debug_assert_eq!(im.len(), n);

    SCRATCH.with_borrow_mut(|scratch| {
        scratch.ensure_capacity(n);
        let buf = &mut scratch.complex_buf[..n];

        // Pack into complex buffer
        for i in 0..n {
            buf[i] = Complex::new(re[i], im[i]);
        }
    });

    let plan = if inverse {
        CACHE.with_borrow_mut(|c| c.get_inverse(n))
    } else {
        CACHE.with_borrow_mut(|c| c.get_forward(n))
    };

    SCRATCH.with_borrow_mut(|scratch| {
        let needed = plan.get_inplace_scratch_len();
        if scratch.execution.len() < needed {
            scratch.execution.resize(needed, Complex::ZERO);
            super::metrics::scratch_growth(needed * std::mem::size_of::<Complex<f64>>());
        }
        let buf = &mut scratch.complex_buf[..n];
        plan.process_with_scratch(buf, &mut scratch.execution[..needed]);

        for i in 0..n {
            re[i] = buf[i].re;
            im[i] = buf[i].im;
        }
    });
}

/// four2a/FFTPACK-style real-to-complex forward FFT.
#[cfg(not(feature = "fftw"))]
#[inline]
pub fn four2a_r2c(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(im.len(), n);
    let nh = n / 2 + 1;
    four2a_c2c(re, im, -1);
    for i in nh..n {
        re[i] = 0.0;
        im[i] = 0.0;
    }
}

// Optional FFTW validation must retain its native real transform, including its
// numerical path; the scratch optimization above concerns the default backend.
#[cfg(feature = "fftw")]
#[inline]
pub fn four2a_r2c(re: &mut [f64], im: &mut [f64]) {
    crate::util::four2a_r2c(re, im);
}

pub(super) use crate::util::sync8_fft_size;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Reference,
    Optimized,
}

thread_local! {
    static MODE: std::cell::Cell<Mode> = const { std::cell::Cell::new(Mode::Optimized) };
}

pub(super) fn mode() -> Mode {
    MODE.with(|m| m.get())
}

pub(super) fn with_mode<T>(mode: Mode, f: impl FnOnce() -> T) -> T {
    struct Restore(Mode);
    impl Drop for Restore {
        fn drop(&mut self) {
            MODE.with(|m| m.set(self.0));
        }
    }
    let _restore = Restore(MODE.with(|m| m.replace(mode)));
    f()
}

#[test]
fn scratch_fft_preserves_every_output_bit() {
    for n in [32, 3200, 3840, 180000, 192000] {
        for sign in [-1, 1] {
            let re: Vec<_> = (0..n).map(|i| (i as f64 * 0.013).sin()).collect();
            let im: Vec<_> = (0..n).map(|i| (i as f64 * 0.017).cos()).collect();
            let mut old_re = re.clone();
            let mut old_im = im.clone();
            crate::util::four2a_c2c(&mut old_re, &mut old_im, sign);
            for _ in 0..2 {
                let mut new_re = re.clone();
                let mut new_im = im.clone();
                with_mode(Mode::Optimized, || {
                    four2a_c2c(&mut new_re, &mut new_im, sign)
                });
                assert!(
                    old_re
                        .iter()
                        .zip(&new_re)
                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                    "real n={n} sign={sign}"
                );
                assert!(
                    old_im
                        .iter()
                        .zip(&new_im)
                        .all(|(a, b)| a.to_bits() == b.to_bits()),
                    "imag n={n} sign={sign}"
                );
            }
        }
    }
}
