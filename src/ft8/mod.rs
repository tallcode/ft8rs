//! FT8 decoder core.
//!
//! This module owns the WSJT-X-aligned FT8/JT77 protocol, LDPC, AP, sync,
//! subtraction and hash-call logic. Stream/file/soundcard orchestration lives
//! outside this module.

#[path = "lib/ft8/ft8_a7.rs"]
pub mod ap_decode;
#[path = "lib/ft8/bpdecode174_91.rs"]
pub(crate) mod bpdecode174_91;
#[path = "lib/ft8/chkcrc14a.rs"]
pub(crate) mod chkcrc14a;
#[path = "lib/ft8_decode.rs"]
pub mod decode;
#[path = "lib/ft8/decode174_91.rs"]
pub(crate) mod decode174_91;
#[path = "lib/ft8/get_crc14.rs"]
pub(crate) mod get_crc14;
#[path = "lib/indexx.rs"]
pub(crate) mod indexx;
#[path = "lib/ft8/ldpc_174_91_c_generator.rs"]
pub(crate) mod ldpc_174_91_c_generator;
#[path = "lib/ft8/ldpc_174_91_c_parity.rs"]
mod ldpc_tables;
#[path = "lib/nuttal_window.rs"]
pub(crate) mod nuttal_window;
#[path = "lib/ft8/osd174_91.rs"]
pub(crate) mod osd174_91;
#[path = "lib/77bit/packjt77.rs"]
pub(crate) mod pack_jt77;
#[path = "lib/platanh.rs"]
pub(crate) mod platanh;
#[path = "lib/ft8/subtractft8.rs"]
pub(crate) mod subtract_ft8;
pub(crate) use self::pack_jt77 as unpack_jt77;
pub use self::pack_jt77::HashCallBook;

#[inline]
pub(crate) fn sync8_df() -> f64 {
    crate::util::sync8_df()
}
