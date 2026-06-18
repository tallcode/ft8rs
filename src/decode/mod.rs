//! Decoder core.
//!
//! This module owns the WSJT-X-aligned FT8/JT77 protocol, LDPC, AP, sync,
//! subtraction and hash-call logic. Stream/file/soundcard orchestration lives
//! outside this module.

pub mod dx;
pub mod hybrid;
pub mod lib_jtdx;

// The modules below mirror WSJT-X source structure and arithmetic closely.
// Keep clippy style rewrites out of this zone so future source audits can
// compare the Rust blocks against the corresponding Fortran.
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/bpdecode174_91.rs"]
pub(crate) mod bpdecode174_91;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/chkcrc14a.rs"]
pub(crate) mod chkcrc14a;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/decode174_91.rs"]
pub(crate) mod decode174_91;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/encode174_91.rs"]
pub(crate) mod encode174_91;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/ft8_a7.rs"]
pub mod ft8_a7;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8_decode.rs"]
pub mod ft8_decode;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/gen_ft8wave.rs"]
pub(crate) mod gen_ft8wave;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/genft8.rs"]
pub(crate) mod genft8;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/get_crc14.rs"]
pub(crate) mod get_crc14;
#[allow(clippy::all)]
#[path = "lib_wsjtx/indexx.rs"]
pub(crate) mod indexx;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/ldpc_174_91_c_generator.rs"]
pub(crate) mod ldpc_174_91_c_generator;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/ldpc_174_91_c_parity.rs"]
pub(crate) mod ldpc_174_91_c_parity;
#[allow(clippy::all)]
#[path = "lib_wsjtx/nuttal_window.rs"]
pub(crate) mod nuttal_window;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/osd174_91.rs"]
pub(crate) mod osd174_91;
#[allow(clippy::all)]
#[path = "lib_wsjtx/77bit/packjt77.rs"]
pub(crate) mod packjt77;
#[allow(clippy::all)]
#[path = "lib_wsjtx/platanh.rs"]
pub(crate) mod platanh;
#[allow(clippy::all)]
#[path = "lib_wsjtx/ft8/subtractft8.rs"]
pub(crate) mod subtractft8;
pub use self::packjt77::HashCallBook;

#[inline]
pub(crate) fn sync8_df() -> f64 {
    crate::util::sync8_df()
}
