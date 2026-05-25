pub mod constants;
pub mod crc;
pub mod decode174_91;
pub mod fft_fftw;
pub mod fft_rustfft;
pub mod hashcall;
pub mod ldpc_tables;
pub mod pack_jt77;
pub mod subtract_ft8;
pub mod unpack_jt77;

/// Dual-engine FFT dispatcher.
///
/// Default: FFTW @ 3840 (matches WSJT-X four2a)
/// Override: set `FTRS_FFT=rustfft` env var → rustfft @ 4096
/// CLI: `ft8rs --fft-engine=rustfft file.wav`
///
/// Both engines expose the same public API via this dispatcher.
use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};

/// 0=uninit, 1=FFTW, 2=rustfft
static ENGINE_ID: AtomicU8 = AtomicU8::new(0);

#[inline]
fn engine_id() -> u8 {
    let id = ENGINE_ID.load(Ordering::Relaxed);
    if id != 0 {
        return id;
    }
    let env = std::env::var("FTRS_FFT").unwrap_or_default();
    let id = if env == "rustfft" { 2 } else { 1 };
    ENGINE_ID.store(id, Ordering::SeqCst);
    id
}

/// Returns true when using rustfft engine.
#[inline]
pub fn is_rustfft() -> bool {
    engine_id() == 2
}

/// sync8 FFT size: FFTW→3840, rustfft→4096
#[inline]
pub fn sync8_fft_size() -> usize {
    if is_rustfft() {
        4096
    } else {
        3840
    }
}

/// sync8 frequency resolution
#[inline]
pub fn sync8_df() -> f64 {
    12000.0 / sync8_fft_size() as f64
}

/// Engine name string for logging
#[inline]
pub fn engine_name() -> &'static str {
    if is_rustfft() {
        "rustfft"
    } else {
        "FFTW"
    }
}

// ── FFT dispatch ──

thread_local! {
    static FFT_BUF: RefCell<(Vec<f64>, Vec<f64>)> = RefCell::new((Vec::new(), Vec::new()));
}

/// Complex-to-complex FFT. Forward: no normalization, Inverse: 1/N.
#[inline]
pub fn fft_complex(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let _n = re.len();
    if is_rustfft() {
        fft_rustfft::fft_complex(re, im, inverse);
    } else {
        fft_fftw::fft_complex(re, im, inverse);
    }
}

/// Real-to-complex forward FFT.
#[inline]
pub fn fft_r2c(re: &mut [f64], im: &mut [f64]) {
    if is_rustfft() {
        fft_rustfft::fft_r2c(re, im);
    } else {
        fft_fftw::fft_r2c(re, im);
    }
}

/// Complex-to-real inverse FFT.
#[inline]
pub fn fft_c2r(re: &mut [f64], im: &mut [f64]) {
    if is_rustfft() {
        fft_rustfft::fft_c2r(re, im);
    } else {
        fft_fftw::fft_c2r(re, im);
    }
}

/// Next power of 2.
#[inline]
pub fn next_pow2(n: usize) -> usize {
    if is_rustfft() {
        fft_rustfft::next_pow2(n)
    } else {
        fft_fftw::next_pow2(n)
    }
}
