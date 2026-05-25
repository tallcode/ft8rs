/// FFT engine wrapping libfftw3 to match WSJT-X four2a exactly.
///
/// WSJT-X uses FFTW with arbitrary sizes (NFFT1=3840), not powers of 2.
/// 3840-pt FFT → df=3.125 Hz → 6.25 Hz tone spacing = exactly 2 bins.

const FFTW_ESTIMATE: u32 = 1 << 6;

type PlanHandle = *mut std::ffi::c_void;

extern "C" {
    fn fftw_plan_dft_r2c_1d(n: i32, input: *mut f64, output: *mut f64, flags: u32) -> PlanHandle;
    fn fftw_plan_dft_c2r_1d(n: i32, input: *mut f64, output: *mut f64, flags: u32) -> PlanHandle;
    fn fftw_plan_dft_1d(
        n: i32,
        input: *mut f64,
        output: *mut f64,
        sign: i32,
        flags: u32,
    ) -> PlanHandle;

    fn fftw_execute_dft(plan: PlanHandle, input: *const f64, output: *mut f64);
    fn fftw_execute_dft_r2c(plan: PlanHandle, input: *const f64, output: *mut f64);
    fn fftw_execute_dft_c2r(plan: PlanHandle, input: *const f64, output: *mut f64);

    fn fftw_destroy_plan(plan: PlanHandle);
}

use std::sync::Mutex;

/// Mutex protecting FFTW plan creation (FFTW planner is not thread-safe).
/// Plan *execution* is thread-safe with ESTIMATE flag, but creating plans
/// requires serialization. We use a single global mutex for all planning.
static PLANNING_MUTEX: Mutex<()> = Mutex::new(());

/// One plan + its scratch buffers, created together so the plan's pointers stay valid.
struct PlanAndBuffers {
    plan: PlanHandle,
    buf_in: Vec<f64>,
    buf_out: Vec<f64>,
}

impl Drop for PlanAndBuffers {
    fn drop(&mut self) {
        if !self.plan.is_null() {
            unsafe { fftw_destroy_plan(self.plan) };
        }
    }
}

impl PlanAndBuffers {
    fn r2c(n: usize) -> Self {
        // FFTW planner is not thread-safe — serialize creation
        let _g = PLANNING_MUTEX.lock().unwrap();
        let mut buf_in = vec![0.0f64; n];
        let mut buf_out = vec![0.0f64; n + 2]; // (n/2+1) complex = n+2 reals
        let plan = unsafe {
            fftw_plan_dft_r2c_1d(
                n as i32,
                buf_in.as_mut_ptr(),
                buf_out.as_mut_ptr(),
                FFTW_ESTIMATE,
            )
        };
        if plan.is_null() {
            panic!("FFTW r2c plan failed for n={n}");
        }
        Self {
            plan,
            buf_in,
            buf_out,
        }
    }

    fn c2r(n: usize) -> Self {
        let _g = PLANNING_MUTEX.lock().unwrap();
        let mut buf_in = vec![0.0f64; n + 2];
        let mut buf_out = vec![0.0f64; n];
        let plan = unsafe {
            fftw_plan_dft_c2r_1d(
                n as i32,
                buf_in.as_mut_ptr(),
                buf_out.as_mut_ptr(),
                FFTW_ESTIMATE,
            )
        };
        if plan.is_null() {
            panic!("FFTW c2r plan failed for n={n}");
        }
        Self {
            plan,
            buf_in,
            buf_out,
        }
    }

    fn c2c(n: usize, forward: bool) -> Self {
        let _g = PLANNING_MUTEX.lock().unwrap();
        let len = n * 2;
        let mut buf_in = vec![0.0f64; len];
        let mut buf_out = vec![0.0f64; len];
        let sign = if forward { -1 } else { 1 };
        let plan = unsafe {
            fftw_plan_dft_1d(
                n as i32,
                buf_in.as_mut_ptr(),
                buf_out.as_mut_ptr(),
                sign,
                FFTW_ESTIMATE,
            )
        };
        if plan.is_null() {
            panic!("FFTW c2c plan failed for n={n}");
        }
        Self {
            plan,
            buf_in,
            buf_out,
        }
    }
}

use std::collections::HashMap;

/// Per-thread plan cache.
struct PlanCache {
    r2c: HashMap<usize, PlanAndBuffers>,
    c2r: HashMap<usize, PlanAndBuffers>,
    c2c_fwd: HashMap<usize, PlanAndBuffers>,
    c2c_bwd: HashMap<usize, PlanAndBuffers>,
}

unsafe impl Send for PlanCache {}
unsafe impl Sync for PlanCache {}

impl PlanCache {
    fn new() -> Self {
        Self {
            r2c: HashMap::new(),
            c2r: HashMap::new(),
            c2c_fwd: HashMap::new(),
            c2c_bwd: HashMap::new(),
        }
    }

    fn get_r2c(&mut self, n: usize) -> &mut PlanAndBuffers {
        self.r2c.entry(n).or_insert_with(|| PlanAndBuffers::r2c(n))
    }

    fn get_c2r(&mut self, n: usize) -> &mut PlanAndBuffers {
        self.c2r.entry(n).or_insert_with(|| PlanAndBuffers::c2r(n))
    }

    fn get_c2c(&mut self, n: usize, forward: bool) -> &mut PlanAndBuffers {
        if forward {
            self.c2c_fwd
                .entry(n)
                .or_insert_with(|| PlanAndBuffers::c2c(n, true))
        } else {
            self.c2c_bwd
                .entry(n)
                .or_insert_with(|| PlanAndBuffers::c2c(n, false))
        }
    }
}

thread_local! {
    static PC: std::cell::RefCell<PlanCache> = std::cell::RefCell::new(PlanCache::new());
}

// ──────────────────────────────── Public API ────────────────────────────────

/// Complex-to-complex FFT (forward or inverse).
/// Forward: no normalization. Inverse: scales by 1/N (matches rustfft / FFTPACK four2a).
#[inline]
pub fn fft_complex(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    debug_assert_eq!(im.len(), n);

    PC.with_borrow_mut(|pc| {
        let pb = pc.get_c2c(n, !inverse);
        let plan = pb.plan;
        let in_ptr = pb.buf_in.as_mut_ptr();
        let out_ptr = pb.buf_out.as_mut_ptr();

        unsafe {
            // Pack split → interleaved
            for i in 0..n {
                *in_ptr.add(2 * i) = re[i];
                *in_ptr.add(2 * i + 1) = im[i];
            }
            fftw_execute_dft(plan, in_ptr, out_ptr);
            // Unpack
            let scale = if inverse { 1.0 / n as f64 } else { 1.0 };
            for i in 0..n {
                re[i] = *out_ptr.add(2 * i) * scale;
                im[i] = *out_ptr.add(2 * i + 1) * scale;
            }
        }
    });
}

/// Real-to-complex forward FFT (r2c).
/// Input: `re[..n]` real data; output: `re[..nh]`/`im[..nh]` complex bins (nh=n/2+1).
#[inline]
pub fn fft_r2c(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(im.len(), n);
    let nh = n / 2 + 1;

    PC.with_borrow_mut(|pc| {
        let pb = pc.get_r2c(n);
        let plan = pb.plan;
        let in_ptr = pb.buf_in.as_mut_ptr();
        let out_ptr = pb.buf_out.as_mut_ptr();

        unsafe {
            std::ptr::copy_nonoverlapping(re.as_ptr(), in_ptr, n);
            fftw_execute_dft_r2c(plan, in_ptr, out_ptr);
            for i in 0..nh {
                re[i] = *out_ptr.add(2 * i);
                im[i] = *out_ptr.add(2 * i + 1);
            }
        }
    });
}

/// Complex-to-real inverse FFT (c2r).
/// Input: `re[..nh]`/`im[..nh]` complex bins; output: `re[..n]` real data.
#[inline]
pub fn fft_c2r(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(im.len(), n);
    let nh = n / 2 + 1;

    PC.with_borrow_mut(|pc| {
        let pb = pc.get_c2r(n);
        let plan = pb.plan;
        let in_ptr = pb.buf_in.as_mut_ptr();
        let out_ptr = pb.buf_out.as_mut_ptr();

        unsafe {
            for i in 0..nh {
                *in_ptr.add(2 * i) = re[i];
                *in_ptr.add(2 * i + 1) = im[i];
            }
            fftw_execute_dft_c2r(plan, in_ptr, out_ptr);
            std::ptr::copy_nonoverlapping(out_ptr, re.as_mut_ptr(), n);
        }
    });
}

/// Next power of 2 (kept for compatibility).
#[inline]
pub fn next_pow2(n: usize) -> usize {
    if n <= 1 {
        return 1;
    }
    1 << (usize::BITS - n.leading_zeros())
}

// ──────────────────────────────── Tests ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c2c_roundtrip_3840() {
        let n = 3840;
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[100] = 1.0;
        fft_complex(&mut re, &mut im, false);
        fft_complex(&mut re, &mut im, true);
        // Inverse now auto-scales by 1/N
        assert!((re[100] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn r2c_dc() {
        let n = 3840;
        let mut re = vec![1.0; n];
        let mut im = vec![0.0; n];
        fft_r2c(&mut re, &mut im);
        assert!((re[0] - 3840.0).abs() < 1e-6, "re[0]={}", re[0]);
        for i in 1..n / 2 + 1 {
            assert!(re[i].abs() < 1e-6 && im[i].abs() < 1e-6, "bin {i}");
        }
    }

    #[test]
    fn c2c_vs_r2c() {
        let n = 3840;
        let mut re1: Vec<f64> = (0..n).map(|i| (i as f64 * 0.1).sin()).collect();
        let mut im1 = vec![0.0; n];
        let re2 = re1.clone();
        let mut im2 = vec![0.0; n];
        fft_complex(&mut re1, &mut im1, false);
        let mut re2 = re2;
        fft_r2c(&mut re2, &mut im2);
        let nh = n / 2 + 1;
        for i in 0..nh {
            assert!((re1[i] - re2[i]).abs() < 1e-9, "re@{i}");
            assert!((im1[i] - im2[i]).abs() < 1e-9, "im@{i}");
        }
    }

    #[test]
    fn c2r_roundtrip() {
        let n = 3840;
        let mut re: Vec<f64> = (0..n).map(|i| (i as f64 * 0.05).sin()).collect();
        let mut im = vec![0.0; n];
        let orig = re.clone();
        fft_r2c(&mut re, &mut im);
        fft_c2r(&mut re, &mut im);
        let s = 1.0 / n as f64;
        for i in 0..n {
            re[i] *= s;
        }
        for i in 0..n {
            assert!((re[i] - orig[i]).abs() < 1e-9, "{i}");
        }
    }

    #[test]
    fn large_192k() {
        let n = 192_000;
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[1000] = 1.0;
        fft_complex(&mut re, &mut im, false);
        fft_complex(&mut re, &mut im, true);
        // Inverse now auto-scales by 1/N
        assert!((re[1000] - 1.0).abs() < 1e-7);
    }
}

#[test]
fn fftw_3840_sanity() {
    // Verify FFTW produces sane power spectrum for known input
    let n = 3840;
    let mut re: Vec<f64> = (0..n)
        .map(|i| (i as f64 * 2.0 * std::f64::consts::PI * 50.0 / n as f64).cos())
        .collect();
    let mut im = vec![0.0; n];
    fft_r2c(&mut re, &mut im);
    // Check peak is at bin 50 (50 Hz signal)
    let max_bin: usize = (0..n / 2 + 1)
        .max_by(|&a, &b| {
            (re[a] * re[a] + im[a] * im[a])
                .partial_cmp(&(re[b] * re[b] + im[b] * im[b]))
                .unwrap()
        })
        .unwrap();
    assert_eq!(max_bin, 50, "Peak should be at bin 50, got {}", max_bin);
    assert!(
        re[50].abs() > n as f64 * 0.3,
        "Peak magnitude too low: {}",
        re[50]
    );
}
