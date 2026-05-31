#[cfg(feature = "fftw")]
pub(crate) mod fft_fftw;
#[cfg(not(feature = "fftw"))]
pub(crate) mod fft_rustfft;

/// Compile-time FFT dispatcher.
///
/// Default: rustfft @ 3840, no external FFTW runtime dependency.
/// Feature `fftw`: FFTW @ 3840, used for FFTW validation.

/// sync8 FFT size: both compile-time engines use 3840 bins.
#[inline]
pub fn sync8_fft_size() -> usize {
    3840
}

/// sync8 frequency resolution
#[inline]
pub fn sync8_df() -> f64 {
    12000.0 / sync8_fft_size() as f64
}

/// Engine name string for logging
#[inline]
pub fn engine_name() -> &'static str {
    #[cfg(feature = "fftw")]
    {
        "FFTW"
    }
    #[cfg(not(feature = "fftw"))]
    {
        "rustfft"
    }
}

/// Configure FFTW plan threads.
///
/// This is meaningful only for the FFTW backend. RustFFT does not expose
/// plan-level internal threading, so `threads > 1` is rejected there instead of
/// silently pretending to honor the option.
pub fn set_fft_threads(threads: usize) -> Result<(), String> {
    if threads == 0 {
        return Err("--fft-threads must be at least 1".to_string());
    }

    #[cfg(feature = "fftw")]
    {
        fft_fftw::set_fft_threads(threads)
    }
    #[cfg(not(feature = "fftw"))]
    {
        if threads == 1 {
            Ok(())
        } else {
            Err("--fft-threads > 1 requires an FFTW build (--features fftw)".to_string())
        }
    }
}

/// Configure FFTW planning patience.
///
/// Patience 0..=4 maps to FFTW ESTIMATE, ESTIMATE_PATIENT, MEASURE, PATIENT
/// and EXHAUSTIVE respectively. RustFFT has no FFTW planning phase, so only
/// the default value is accepted there.
pub fn set_fft_patience(patience: usize) -> Result<(), String> {
    if patience > 4 {
        return Err("--patience must be in 0..=4".to_string());
    }

    #[cfg(feature = "fftw")]
    {
        fft_fftw::set_fft_patience(patience)
    }
    #[cfg(not(feature = "fftw"))]
    {
        if patience == 1 {
            Ok(())
        } else {
            Err("--patience requires an FFTW build (--features fftw)".to_string())
        }
    }
}

// ── four2a dispatch ──

/// four2a/FFTPACK-style complex FFT.
///
/// Mirrors `call four2a(c, n, 1, isign, 1)`: `isign=-1` is forward,
/// `isign=1` is inverse, and neither direction applies normalization.
#[inline]
pub fn four2a_c2c(re: &mut [f64], im: &mut [f64], isign: i32) {
    #[cfg(feature = "fftw")]
    {
        fft_fftw::four2a_c2c(re, im, isign);
    }
    #[cfg(not(feature = "fftw"))]
    {
        fft_rustfft::four2a_c2c(re, im, isign);
    }
}

/// four2a/FFTPACK-style real-to-complex forward FFT.
///
/// Mirrors `call four2a(x, n, 1, -1, 0)`.
#[inline]
pub fn four2a_r2c(re: &mut [f64], im: &mut [f64]) {
    #[cfg(feature = "fftw")]
    {
        fft_fftw::four2a_r2c(re, im);
    }
    #[cfg(not(feature = "fftw"))]
    {
        fft_rustfft::four2a_r2c(re, im);
    }
}
