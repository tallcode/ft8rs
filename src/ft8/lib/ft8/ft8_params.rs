// WSJT-X lib/ft8/ft8_params.f90
pub(crate) const SAMPLE_RATE: usize = 12_000;
pub(crate) const NSPS: usize = 1920;
pub(crate) const NFFT1: usize = 2 * NSPS; // 3840
pub(crate) const NSTEP: usize = NSPS / 4; // 480
pub(crate) const NMAX: usize = 15 * 12_000; // 180000
pub(crate) const NHSYM: usize = NMAX / NSTEP - 3; // 372
pub(crate) const NDOWN: usize = 60;
pub(crate) const NN: usize = 79;

// WSJT-X ft8_downsample.f90
pub(crate) const NFFT1_LONG: usize = 192000;
pub(crate) const NFFT2: usize = 3200;
pub(crate) const NP2: usize = 2812;
pub(crate) const COSTAS_BLOCKS: usize = 7;
pub(crate) const COSTAS_SYMBOL_LEN: usize = 32;
pub(crate) const TAPER_SIZE: usize = 101;
pub(crate) const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
pub(crate) const TWO_PI_F32: f32 = std::f32::consts::PI * 2.0;
pub(crate) const PI_F32: f32 = std::f32::consts::PI;

pub(crate) const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
pub(crate) const DT2: f64 = 1.0 / FS2;
pub(crate) const DOWNSAMPLE_DF: f32 = SAMPLE_RATE as f32 / NFFT1_LONG as f32;
pub(crate) const DOWNSAMPLE_BAUD: f32 = SAMPLE_RATE as f32 / NSPS as f32;
/// WSJT-X ft8_downsample.f90:
/// `fac=1.0/sqrt(float(NFFT1)*NFFT2)` after the unnormalized inverse FFT.
/// `float(NFFT1)` makes this a default-REAL calculation in WSJT-X, so keep the
/// scale as f32 before storing the downsampled symbols back into f64 buffers.
pub(crate) const DOWNSAMPLE_FAC: f32 = 0.00004034357698401436f32;
