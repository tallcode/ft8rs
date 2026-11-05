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

    let plan = complex_plan(n, inverse);

    SCRATCH.with_borrow_mut(|scratch| {
        let buf = &mut scratch.complex_buf[..n];
        process_fft(&*plan, buf, &mut scratch.execution);

        for i in 0..n {
            re[i] = buf[i].re;
            im[i] = buf[i].im;
        }
    });
}

fn complex_plan(n: usize, inverse: bool) -> Arc<dyn Fft<f64>> {
    CACHE.with_borrow_mut(|cache| {
        if inverse {
            cache.get_inverse(n)
        } else {
            cache.get_forward(n)
        }
    })
}

fn process_fft(plan: &dyn Fft<f64>, data: &mut [Complex<f64>], scratch: &mut Vec<Complex<f64>>) {
    let needed = plan.get_inplace_scratch_len();
    if scratch.len() < needed {
        scratch.resize(needed, Complex::ZERO);
        super::metrics::scratch_growth(needed * std::mem::size_of::<Complex<f64>>());
    }
    plan.process_with_scratch(data, &mut scratch[..needed]);
}

/// Subtraction already owns interleaved samples and can pass them directly.
pub(super) fn fft_complex_in_place(data: &mut [Complex<f64>], inverse: bool) {
    if mode() == Mode::Reference || cfg!(feature = "fftw") {
        let (mut re, mut im): (Vec<_>, Vec<_>) = data.iter().map(|v| (v.re, v.im)).unzip();
        crate::util::four2a_c2c(&mut re, &mut im, if inverse { 1 } else { -1 });
        for (value, (re, im)) in data.iter_mut().zip(re.into_iter().zip(im)) {
            *value = Complex::new(re, im);
        }
        return;
    }

    let _span = super::metrics::Span::new(super::metrics::Kind::Fft);
    super::metrics::fft(data.len());
    let plan = complex_plan(data.len(), inverse);
    SCRATCH.with_borrow_mut(|scratch| process_fft(&*plan, data, &mut scratch.execution));
}

/// Reuse the same forward plan across a candidate's symbol windows.
pub(super) struct SymbolFft {
    plan: Option<Arc<dyn Fft<f64>>>,
    scratch: Vec<Complex<f64>>,
    profile: bool,
}

impl SymbolFft {
    pub(super) fn new() -> Self {
        let plan = (mode() == Mode::Optimized && !cfg!(feature = "fftw"))
            .then(|| CACHE.with_borrow_mut(|cache| cache.get_forward(32)));
        let scratch = plan.as_ref().map_or_else(Vec::new, |plan| {
            vec![Complex::ZERO; plan.get_inplace_scratch_len()]
        });
        Self {
            plan,
            scratch,
            profile: super::metrics::active(),
        }
    }

    /// Return the eight tone bins. A8 permits partial windows at the edges.
    pub(super) fn tones(&mut self, re: &[f64], im: &[f64], start: isize) -> [Complex<f64>; 8] {
        let first = start.max(0) as usize;
        let offset = start.saturating_neg().clamp(0, 32) as usize;
        let len = re.len().saturating_sub(first).min(32 - offset);
        let mut buffer: [Complex<f64>; 32] = if len == 32 {
            let re = &re[first..first + 32];
            let im = &im[first..first + 32];
            std::array::from_fn(|i| Complex::new(re[i], im[i]))
        } else {
            let mut buffer = [Complex::ZERO; 32];
            for i in 0..len {
                buffer[offset + i] = Complex::new(re[first + i], im[first + i]);
            }
            buffer
        };
        if let Some(plan) = &self.plan {
            let _span = self.profile.then(|| {
                let span = super::metrics::Span::new(super::metrics::Kind::Fft);
                super::metrics::fft(buffer.len());
                span
            });
            plan.process_with_scratch(&mut buffer, &mut self.scratch);
            std::array::from_fn(|i| buffer[i])
        } else {
            let mut re = buffer.map(|value| value.re);
            let mut im = buffer.map(|value| value.im);
            crate::util::four2a_c2c(&mut re, &mut im, -1);
            std::array::from_fn(|i| Complex::new(re[i], im[i]))
        }
    }
}

struct RealPlan {
    fft: Arc<dyn Fft<f64>>,
    twiddles: Vec<Complex<f64>>,
}

thread_local! {
    static REAL_PLANS: std::cell::RefCell<std::collections::HashMap<usize, RealPlan>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Unnormalized real forward transform using one half-length complex FFT.
/// Keep the split output layout, including DC, Nyquist and the zeroed tail.
pub(super) fn four2a_r2c_packed(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(im.len(), n);
    if cfg!(feature = "fftw") || mode() == Mode::Reference || n < 2 || n & 1 != 0 {
        four2a_r2c(re, im);
        return;
    }

    let _span = super::metrics::Span::new(super::metrics::Kind::Fft);
    super::metrics::fft(n);
    let half = n / 2;
    REAL_PLANS.with_borrow_mut(|plans| {
        let plan = plans.entry(n).or_insert_with(|| RealPlan {
            fft: CACHE.with_borrow_mut(|cache| cache.get_forward(half)),
            twiddles: (1..half)
                .map(|k| Complex::from_polar(1.0, -std::f64::consts::TAU * k as f64 / n as f64))
                .collect(),
        });
        SCRATCH.with_borrow_mut(|scratch| {
            scratch.ensure_capacity(half);
            let packed = &mut scratch.complex_buf[..half];
            for (value, pair) in packed.iter_mut().zip(re.as_chunks::<2>().0) {
                *value = Complex::new(pair[0], pair[1]);
            }
            process_fft(&*plan.fft, packed, &mut scratch.execution);

            re[0] = packed[0].re + packed[0].im;
            re[half] = packed[0].re - packed[0].im;
            im[0] = 0.0;
            im[half] = 0.0;
            for (k, &twiddle) in (1..half).zip(&plan.twiddles) {
                // Recover the spectra of the even and odd real samples.
                let a = packed[k];
                let b = packed[half - k].conj();
                let even = (a + b) * 0.5;
                let difference = a - b;
                let odd = Complex::new(0.5 * difference.im, -0.5 * difference.re);
                let bin = even + twiddle * odd;
                re[k] = bin.re;
                im[k] = bin.im;
            }
        });
    });
    re[half + 1..].fill(0.0);
    im[half + 1..].fill(0.0);
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

#[cfg(test)]
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

#[test]
fn symbol_fft_matches_split_transform_at_window_edges() {
    let re: Vec<_> = (0..43).map(|i| (0.23 * i as f64).sin()).collect();
    let im: Vec<_> = (0..43).map(|i| (0.11 * i as f64).cos()).collect();
    let mut fft = SymbolFft::new();
    for start in [-35isize, -7, 0, 11, 12, 42, 43] {
        let mut expected_re = [0.0; 32];
        let mut expected_im = [0.0; 32];
        for i in 0..32 {
            let sample = start + i as isize;
            if (0..re.len() as isize).contains(&sample) {
                expected_re[i] = re[sample as usize];
                expected_im[i] = im[sample as usize];
            }
        }
        four2a_c2c(&mut expected_re, &mut expected_im, -1);
        let bins = fft.tones(&re, &im, start);
        for (i, bin) in bins.iter().enumerate() {
            assert_eq!(
                bin.re.to_bits(),
                expected_re[i].to_bits(),
                "start={start} bin={i}"
            );
            assert_eq!(
                bin.im.to_bits(),
                expected_im[i].to_bits(),
                "start={start} bin={i}"
            );
        }
    }
}

#[test]
fn packed_real_fft_preserves_scale_and_positive_frequency_layout() {
    for n in [2, 32, sync8_fft_size()] {
        let signal: Vec<_> = (0..n)
            .map(|i| 2.0 + (0.13 * i as f64).sin() + if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let mut expected_re = signal.clone();
        let mut expected_im = vec![0.0; n];
        four2a_r2c(&mut expected_re, &mut expected_im);
        let mut re = signal;
        let mut im = vec![0.0; n];
        four2a_r2c_packed(&mut re, &mut im);
        for i in 0..=n / 2 {
            assert!(
                (re[i] - expected_re[i]).abs() < 1.0e-9,
                "n={n} real bin={i}"
            );
            assert!(
                (im[i] - expected_im[i]).abs() < 1.0e-9,
                "n={n} imag bin={i}"
            );
        }
        assert!(re[n / 2 + 1..]
            .iter()
            .chain(&im[n / 2 + 1..])
            .all(|v| *v == 0.0));
    }
}
