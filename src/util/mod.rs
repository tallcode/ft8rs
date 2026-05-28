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

// ── WSJT-X four2a dispatch ──

/// WSJT-X/FFTPACK-style complex FFT.
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

/// WSJT-X/FFTPACK-style real-to-complex forward FFT.
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
