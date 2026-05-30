/// FFT engine wrapping libfftw3 to match WSJT-X four2a exactly.
///
/// WSJT-X uses FFTW with arbitrary sizes (NFFT1=3840), not powers of 2.
/// 3840-pt FFT → df=3.125 Hz → 6.25 Hz tone spacing = exactly 2 bins.

const FFTW_MEASURE: u32 = 0;
const FFTW_EXHAUSTIVE: u32 = 1 << 3;
const FFTW_PATIENT: u32 = 1 << 5;
const FFTW_ESTIMATE: u32 = 1 << 6;
const FFTW_ESTIMATE_PATIENT: u32 = 1 << 7;

type PlanHandle = *mut std::ffi::c_void;

extern "C" {
    fn fftw_init_threads() -> i32;
    fn fftw_plan_with_nthreads(nthreads: i32);
    fn fftw_plan_dft_r2c_1d(n: i32, input: *mut f64, output: *mut f64, flags: u32) -> PlanHandle;
    fn fftw_plan_dft_1d(
        n: i32,
        input: *mut f64,
        output: *mut f64,
        sign: i32,
        flags: u32,
    ) -> PlanHandle;

    fn fftw_execute_dft(plan: PlanHandle, input: *const f64, output: *mut f64);
    fn fftw_execute_dft_r2c(plan: PlanHandle, input: *const f64, output: *mut f64);

    fn fftw_destroy_plan(plan: PlanHandle);
}

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Mutex protecting FFTW plan creation (FFTW planner is not thread-safe).
/// Plan *execution* is thread-safe with ESTIMATE flag, but creating plans
/// requires serialization. We use a single global mutex for all planning.
static PLANNING_MUTEX: Mutex<()> = Mutex::new(());
static FFT_THREADS: AtomicUsize = AtomicUsize::new(1);
static FFT_PATIENCE: AtomicUsize = AtomicUsize::new(1);
static PLANS_CREATED: AtomicBool = AtomicBool::new(false);
static THREAD_INIT: OnceLock<bool> = OnceLock::new();

pub fn set_fft_threads(threads: usize) -> Result<(), String> {
    if threads == 0 {
        return Err("--fft-threads must be at least 1".to_string());
    }
    if threads > i32::MAX as usize {
        return Err(format!("--fft-threads is too large: {threads}"));
    }

    let previous = FFT_THREADS.load(Ordering::SeqCst);
    if PLANS_CREATED.load(Ordering::SeqCst) && previous != threads {
        return Err(
            "--fft-threads must be configured before the first FFT plan is created".to_string(),
        );
    }

    ensure_fftw_threads_initialized()?;
    FFT_THREADS.store(threads, Ordering::SeqCst);
    Ok(())
}

pub fn set_fft_patience(patience: usize) -> Result<(), String> {
    if patience > 4 {
        return Err("--patience must be in 0..=4".to_string());
    }

    let previous = FFT_PATIENCE.load(Ordering::SeqCst);
    if PLANS_CREATED.load(Ordering::SeqCst) && previous != patience {
        return Err(
            "--patience must be configured before the first FFT plan is created".to_string(),
        );
    }

    FFT_PATIENCE.store(patience, Ordering::SeqCst);
    Ok(())
}

fn ensure_fftw_threads_initialized() -> Result<(), String> {
    let ok = *THREAD_INIT.get_or_init(|| unsafe { fftw_init_threads() != 0 });
    if ok {
        Ok(())
    } else {
        Err("fftw_init_threads failed".to_string())
    }
}

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
        prepare_plan_threads();
        let mut buf_in = vec![0.0f64; n];
        let mut buf_out = vec![0.0f64; n + 2]; // (n/2+1) complex = n+2 reals
        let plan = unsafe {
            fftw_plan_dft_r2c_1d(
                n as i32,
                buf_in.as_mut_ptr(),
                buf_out.as_mut_ptr(),
                planning_flags(),
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

    fn c2c(n: usize, forward: bool) -> Self {
        let _g = PLANNING_MUTEX.lock().unwrap();
        prepare_plan_threads();
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
                planning_flags(),
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

fn planning_flags() -> u32 {
    match FFT_PATIENCE.load(Ordering::SeqCst) {
        0 => FFTW_ESTIMATE,
        1 => FFTW_ESTIMATE_PATIENT,
        2 => FFTW_MEASURE,
        3 => FFTW_PATIENT,
        4 => FFTW_EXHAUSTIVE,
        _ => FFTW_ESTIMATE_PATIENT,
    }
}

fn prepare_plan_threads() {
    ensure_fftw_threads_initialized().expect("fftw_init_threads failed");
    let threads = FFT_THREADS.load(Ordering::SeqCst).max(1);
    unsafe { fftw_plan_with_nthreads(threads as i32) };
    PLANS_CREATED.store(true, Ordering::SeqCst);
}

use std::collections::HashMap;

/// Per-thread plan cache.
struct PlanCache {
    r2c: HashMap<usize, PlanAndBuffers>,
    c2c_fwd: HashMap<usize, PlanAndBuffers>,
    c2c_bwd: HashMap<usize, PlanAndBuffers>,
}

unsafe impl Send for PlanCache {}
unsafe impl Sync for PlanCache {}

impl PlanCache {
    fn new() -> Self {
        Self {
            r2c: HashMap::new(),
            c2c_fwd: HashMap::new(),
            c2c_bwd: HashMap::new(),
        }
    }

    fn get_r2c(&mut self, n: usize) -> &mut PlanAndBuffers {
        self.r2c.entry(n).or_insert_with(|| PlanAndBuffers::r2c(n))
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

/// WSJT-X/FFTPACK-style complex FFT without normalization in either direction.
///
/// Used by WSJT-X-aligned call sites that apply the Fortran `fac` explicitly.
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
            for i in 0..n {
                re[i] = *out_ptr.add(2 * i);
                im[i] = *out_ptr.add(2 * i + 1);
            }
        }
    });
}

/// Real-to-complex forward FFT (r2c).
/// Input: `re[..n]` real data; output: `re[..nh]`/`im[..nh]` complex bins (nh=n/2+1).
#[inline]
pub fn four2a_r2c(re: &mut [f64], im: &mut [f64]) {
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
        four2a_c2c(&mut re, &mut im, -1);
        four2a_c2c(&mut re, &mut im, 1);
        for i in 0..n {
            re[i] /= n as f64;
            im[i] /= n as f64;
        }
        assert!((re[100] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn four2a_inverse_matches_normalized_inverse_times_n() {
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
    fn r2c_dc() {
        let n = 3840;
        let mut re = vec![1.0; n];
        let mut im = vec![0.0; n];
        four2a_r2c(&mut re, &mut im);
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
        four2a_c2c(&mut re1, &mut im1, -1);
        let mut re2 = re2;
        four2a_r2c(&mut re2, &mut im2);
        let nh = n / 2 + 1;
        for i in 0..nh {
            assert!((re1[i] - re2[i]).abs() < 1e-9, "re@{i}");
            assert!((im1[i] - im2[i]).abs() < 1e-9, "im@{i}");
        }
    }

    #[test]
    fn large_192k() {
        let n = 192_000;
        let mut re = vec![0.0; n];
        let mut im = vec![0.0; n];
        re[1000] = 1.0;
        four2a_c2c(&mut re, &mut im, -1);
        four2a_c2c(&mut re, &mut im, 1);
        for i in 0..n {
            re[i] /= n as f64;
            im[i] /= n as f64;
        }
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
    four2a_r2c(&mut re, &mut im);
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
