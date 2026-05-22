pub mod constants;
pub mod ldpc_tables;
pub mod crc;
pub mod fft_fftw;
pub mod fft_rustfft;
pub mod pack_jt77;
pub mod unpack_jt77;
pub mod decode174_91;
pub mod hashcall;
pub mod subtract_ft8;

/// Dual-engine FFT re-export.
///
/// Currently defaults to FFTW for WSJT-X alignment work.
/// Switch to rustfft by changing `pub use` below.
///
/// Both engines expose: `fft_complex`, `fft_r2c`, `fft_c2r`, `next_pow2`
pub use fft_fftw as fft;
// pub use fft_rustfft as fft;