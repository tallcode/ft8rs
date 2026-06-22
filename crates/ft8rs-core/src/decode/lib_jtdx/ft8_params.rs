//! Mirrors JTDX `lib/ft8_params.f90`.

pub const KK: usize = 91;
pub const ND: usize = 58;
pub const NS: usize = 21;
pub const NN: usize = 79;
pub const NSPS: usize = 1920;
pub const NZ: usize = 151_680;
pub const NMAX: usize = 180_000;
pub const NFFT1: usize = 3840;
pub const NH1: usize = 1920;
pub const NSTEP: usize = 480;
pub const NHSYM: usize = 372;
pub const NDOWN: usize = 60;

pub const SAMPLE_RATE: usize = 12_000;
pub const NFFT1_LONG: usize = 192_000;
pub const NFFT2: usize = 3200;
pub const NP2: usize = 2812;
pub const FS2: f64 = SAMPLE_RATE as f64 / NDOWN as f64;
pub const DT2: f64 = 1.0 / FS2;
pub const TWO_PI: f64 = std::f64::consts::PI * 2.0;
