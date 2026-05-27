#[cfg(feature = "fftw")]
pub(crate) mod fft_fftw;
#[cfg(not(feature = "fftw"))]
pub(crate) mod fft_rustfft;

/// Compile-time FFT dispatcher.
///
/// Default: rustfft @ 3840, no external FFTW runtime dependency.
/// Feature `fftw`: FFTW @ 3840, used for WSJT-X-aligned tests.

/// sync8 FFT size: both compile-time engines use the WSJT-X-aligned 3840 bins.
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

// ── FFT dispatch ──

/// Complex-to-complex FFT. Forward: no normalization, Inverse: 1/N.
#[inline]
pub fn fft_complex(re: &mut [f64], im: &mut [f64], inverse: bool) {
    #[cfg(feature = "fftw")]
    {
        fft_fftw::fft_complex(re, im, inverse);
    }
    #[cfg(not(feature = "fftw"))]
    {
        fft_rustfft::fft_complex(re, im, inverse);
    }
}

/// Real-to-complex forward FFT.
#[inline]
pub fn fft_r2c(re: &mut [f64], im: &mut [f64]) {
    #[cfg(feature = "fftw")]
    {
        fft_fftw::fft_r2c(re, im);
    }
    #[cfg(not(feature = "fftw"))]
    {
        fft_rustfft::fft_r2c(re, im);
    }
}
