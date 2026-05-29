//! FT8 decoder core.
//!
//! This module owns the WSJT-X-aligned FT8/JT77 protocol, LDPC, AP, sync,
//! subtraction and hash-call logic. Stream/file/soundcard orchestration lives
//! outside this module.

#[path = "lib/ft8/ft8_a7.rs"]
pub mod ap_decode;
#[path = "lib/ft8/constants.rs"]
pub mod constants;
#[path = "lib/ft8/ft8_decode.rs"]
pub mod decode;
#[path = "lib/ft8/decode174_91.rs"]
pub(crate) mod decode174_91;
#[path = "lib/77bit/hashcall.rs"]
pub mod hashcall;
#[path = "lib/indexx.rs"]
pub(crate) mod indexx;
#[path = "lib/ft8/ldpc_174_91_c_parity.rs"]
mod ldpc_tables;
#[path = "lib/77bit/packjt77.rs"]
pub(crate) mod pack_jt77;
#[path = "lib/77bit/protocol.rs"]
pub(crate) mod protocol;
#[path = "lib/ft8/subtractft8.rs"]
pub(crate) mod subtract_ft8;
#[path = "lib/77bit/unpack77.rs"]
pub(crate) mod unpack_jt77;

#[inline]
pub(crate) fn sync8_df() -> f64 {
    crate::util::sync8_df()
}
