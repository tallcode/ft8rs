#[cfg(feature = "fftw")]
mod fftw_backend {
    use std::collections::HashMap;
    use std::sync::Mutex;

    const FFTW_ESTIMATE_PATIENT: u32 = 1 << 7;

    type PlanHandle = *mut std::ffi::c_void;

    extern "C" {
        fn fftw_plan_dft_r2c_1d(
            n: i32,
            input: *mut f64,
            output: *mut f64,
            flags: u32,
        ) -> PlanHandle;
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

    static PLANNING_MUTEX: Mutex<()> = Mutex::new(());

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
            let _g = PLANNING_MUTEX.lock().unwrap();
            let mut buf_in = vec![0.0f64; n];
            let mut buf_out = vec![0.0f64; n + 2];
            let plan = unsafe {
                fftw_plan_dft_r2c_1d(
                    n as i32,
                    buf_in.as_mut_ptr(),
                    buf_out.as_mut_ptr(),
                    FFTW_ESTIMATE_PATIENT,
                )
            };
            if plan.is_null() {
                panic!("JTDX four2a FFTW r2c plan failed for n={n}");
            }
            Self {
                plan,
                buf_in,
                buf_out,
            }
        }

        fn c2c(n: usize, forward: bool) -> Self {
            let _g = PLANNING_MUTEX.lock().unwrap();
            let mut buf_in = vec![0.0f64; n * 2];
            let mut buf_out = vec![0.0f64; n * 2];
            let sign = if forward { -1 } else { 1 };
            let plan = unsafe {
                fftw_plan_dft_1d(
                    n as i32,
                    buf_in.as_mut_ptr(),
                    buf_out.as_mut_ptr(),
                    sign,
                    FFTW_ESTIMATE_PATIENT,
                )
            };
            if plan.is_null() {
                panic!("JTDX four2a FFTW c2c plan failed for n={n}");
            }
            Self {
                plan,
                buf_in,
                buf_out,
            }
        }
    }

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

    pub fn c2c(re: &mut [f64], im: &mut [f64], isign: i32) {
        let forward = match isign {
            -1 => true,
            1 => false,
            _ => panic!("four2a_c2c only supports isign=-1 or isign=1"),
        };
        let n = re.len();
        debug_assert_eq!(im.len(), n);

        PC.with_borrow_mut(|pc| {
            let pb = pc.get_c2c(n, forward);
            let plan = pb.plan;
            let in_ptr = pb.buf_in.as_mut_ptr();
            let out_ptr = pb.buf_out.as_mut_ptr();

            unsafe {
                for i in 0..n {
                    *in_ptr.add(2 * i) = re[i];
                    *in_ptr.add(2 * i + 1) = im[i];
                }
                fftw_execute_dft(plan, in_ptr, out_ptr);
                for i in 0..n {
                    re[i] = *out_ptr.add(2 * i);
                    im[i] = *out_ptr.add(2 * i + 1);
                }
            }
        });
    }

    pub fn r2c(re: &mut [f64], im: &mut [f64]) {
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
}

#[cfg(not(feature = "fftw"))]
mod rustfft_backend {
    use rustfft::{num_complex::Complex, Fft, FftDirection, FftPlanner};
    use std::sync::Arc;

    thread_local! {
        static PLANNER: std::cell::RefCell<FftPlanner<f64>> =
            std::cell::RefCell::new(FftPlanner::new());
    }

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
                .or_insert_with(|| {
                    PLANNER.with_borrow_mut(|p| p.plan_fft(n, FftDirection::Forward))
                })
                .clone()
        }

        fn get_inverse(&mut self, n: usize) -> Arc<dyn Fft<f64>> {
            self.inverse
                .entry(n)
                .or_insert_with(|| {
                    PLANNER.with_borrow_mut(|p| p.plan_fft(n, FftDirection::Inverse))
                })
                .clone()
        }
    }

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
        static CACHE: std::cell::RefCell<PlanCache> =
            std::cell::RefCell::new(PlanCache::new());
        static SCRATCH: std::cell::RefCell<ScratchBuffers> =
            std::cell::RefCell::new(ScratchBuffers::new());
    }

    pub fn c2c(re: &mut [f64], im: &mut [f64], isign: i32) {
        let inverse = match isign {
            -1 => false,
            1 => true,
            _ => panic!("four2a_c2c only supports isign=-1 or isign=1"),
        };
        let n = re.len();
        debug_assert_eq!(im.len(), n);

        SCRATCH.with_borrow_mut(|scratch| {
            scratch.ensure_capacity(n);
            let buf = &mut scratch.complex_buf[..n];
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

    pub fn r2c(re: &mut [f64], im: &mut [f64]) {
        let n = re.len();
        debug_assert_eq!(im.len(), n);
        let nh = n / 2 + 1;
        c2c(re, im, -1);
        for i in nh..n {
            re[i] = 0.0;
            im[i] = 0.0;
        }
    }
}

#[inline]
pub fn four2a_c2c(re: &mut [f64], im: &mut [f64], isign: i32) {
    #[cfg(feature = "fftw")]
    {
        fftw_backend::c2c(re, im, isign);
    }
    #[cfg(not(feature = "fftw"))]
    {
        rustfft_backend::c2c(re, im, isign);
    }
}

#[inline]
pub fn four2a_r2c(re: &mut [f64], im: &mut [f64]) {
    #[cfg(feature = "fftw")]
    {
        fftw_backend::r2c(re, im);
    }
    #[cfg(not(feature = "fftw"))]
    {
        rustfft_backend::r2c(re, im);
    }
}
