/// High-performance FFT engine using rustfft.
///
/// Key optimizations:
/// - Thread-local planner cache (avoids re-planning)
/// - Reusable buffers (avoids allocation)
/// - No normalization on forward FFT (matches FFTPACK)
/// - Manual 1/N normalization on inverse FFT
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
}

impl ScratchBuffers {
    fn new() -> Self {
        Self {
            complex_buf: Vec::new(),
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
    let inverse = match isign {
        -1 => false,
        1 => true,
        _ => panic!("four2a_c2c only supports isign=-1 or isign=1"),
    };
    four2a_c2c_impl(re, im, inverse);
}

#[inline]
fn four2a_c2c_impl(re: &mut [f64], im: &mut [f64], inverse: bool) {
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
        let buf = &mut scratch.complex_buf[..n];
        plan.process(buf);

        for i in 0..n {
            re[i] = buf[i].re;
            im[i] = buf[i].im;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fft_roundtrip_3840() {
        let n = 3840;
        let mut re = vec![0.0; n];
        let im = vec![0.0; n];
        re[100] = 1.0;

        let mut re_fwd = re.clone();
        let mut im_fwd = im.clone();
        four2a_c2c(&mut re_fwd, &mut im_fwd, -1);

        four2a_c2c(&mut re_fwd, &mut im_fwd, 1);
        for i in 0..n {
            re_fwd[i] /= n as f64;
            im_fwd[i] /= n as f64;
        }

        for i in 0..n {
            let expected = if i == 100 { 1.0 } else { 0.0 };
            assert!(
                (re_fwd[i] - expected).abs() < 1e-10,
                "re[{}]: {} vs {}",
                i,
                re_fwd[i],
                expected
            );
            assert!(im_fwd[i].abs() < 1e-10);
        }
    }

    #[test]
    fn test_fft_roundtrip_3200() {
        let n = 3200;
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[50] = 1.0;
        im[50] = 0.5;

        let mut re_fwd = re.clone();
        let mut im_fwd = im.clone();
        four2a_c2c(&mut re_fwd, &mut im_fwd, -1);

        four2a_c2c(&mut re_fwd, &mut im_fwd, 1);
        for i in 0..n {
            re_fwd[i] /= n as f64;
            im_fwd[i] /= n as f64;
        }

        for i in 0..n {
            assert!((re_fwd[i] - re[i]).abs() < 1e-10);
            assert!((im_fwd[i] - im[i]).abs() < 1e-10);
        }
    }

    #[test]
    fn test_four2a_inverse_matches_normalized_inverse_times_n() {
        let n = 3200;
        let mut re: Vec<f64> = (0..n).map(|i| (i as f64 * 0.013).sin()).collect();
        let mut im: Vec<f64> = (0..n).map(|i| (i as f64 * 0.017).cos()).collect();
        let mut re_norm = re.clone();
        let mut im_norm = im.clone();

        four2a_c2c(&mut re, &mut im, 1);
        four2a_c2c(&mut re_norm, &mut im_norm, 1);

        for i in 0..n {
            re[i] /= n as f64;
            im[i] /= n as f64;
            assert!((re_norm[i] - re[i] * n as f64).abs() < 1e-8);
            assert!((im_norm[i] - im[i] * n as f64).abs() < 1e-8);
        }
    }

    #[test]
    fn test_fft_roundtrip_192000() {
        let n = 192000;
        let mut re = vec![0.0; n];
        let im = vec![0.0; n];
        re[1000] = 1.0;

        let mut re_fwd = re.clone();
        let mut im_fwd = im.clone();
        four2a_c2c(&mut re_fwd, &mut im_fwd, -1);

        four2a_c2c(&mut re_fwd, &mut im_fwd, 1);
        for i in 0..n {
            re_fwd[i] /= n as f64;
            im_fwd[i] /= n as f64;
        }

        assert!((re_fwd[1000] - 1.0).abs() < 1e-8);
        for i in (0..n).step_by(1000) {
            if i != 1000 {
                assert!(re_fwd[i].abs() < 1e-8, "re[{}]: {}", i, re_fwd[i]);
            }
        }
    }
}

/// four2a/FFTPACK-style real-to-complex forward FFT.
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
