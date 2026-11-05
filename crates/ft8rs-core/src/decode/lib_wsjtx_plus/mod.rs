//! Independently owned classic receiver for wsjtx+ development.
//!
//! No Hybrid/DX caller or public product profile uses this module. The plus path
//! owns FFT/BP/OSD; protocol/CRC helpers are shared, decoder state stays private.

#[path = "ft8/ft8_a7.rs"]
pub(crate) mod ft8_a7;
pub(crate) mod ft8_decode;
#[path = "ft8/subtractft8.rs"]
pub(crate) mod subtractft8;

#[path = "ft8/bpdecode174_91.rs"]
mod bpdecode174_91;
#[path = "ft8/decode174_91.rs"]
mod decode174_91;

mod fft;
mod metrics;
#[path = "ft8/osd174_91.rs"]
mod osd174_91;
mod session;
mod validation;
