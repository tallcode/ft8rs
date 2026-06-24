//! Decoder core.
//!
//! This module owns the WSJT-X-aligned FT8/JT77 protocol, LDPC, AP, sync,
//! subtraction and hash-call logic. Stream/file/soundcard orchestration lives
//! outside this module.

pub mod dx;
pub mod hybrid;
pub mod lib_jtdx;
pub mod profile;

#[path = "lib_wsjtx/ft8/bpdecode174_91.rs"]
pub(crate) mod bpdecode174_91;
#[path = "lib_wsjtx/ft8/chkcrc14a.rs"]
pub(crate) mod chkcrc14a;
#[path = "lib_wsjtx/ft8/decode174_91.rs"]
pub(crate) mod decode174_91;
#[path = "lib_wsjtx/ft8/encode174_91.rs"]
pub(crate) mod encode174_91;
#[path = "lib_wsjtx/ft8/ft8_a7.rs"]
pub mod ft8_a7;
#[path = "lib_wsjtx/ft8_decode.rs"]
pub mod ft8_decode;
#[path = "lib_wsjtx/ft8/gen_ft8wave.rs"]
pub(crate) mod gen_ft8wave;
#[path = "lib_wsjtx/ft8/genft8.rs"]
pub(crate) mod genft8;
#[path = "lib_wsjtx/ft8/get_crc14.rs"]
pub(crate) mod get_crc14;
#[path = "lib_wsjtx/indexx.rs"]
pub(crate) mod indexx;
#[path = "lib_wsjtx/ft8/ldpc_174_91_c_generator.rs"]
pub(crate) mod ldpc_174_91_c_generator;
#[path = "lib_wsjtx/ft8/ldpc_174_91_c_parity.rs"]
pub(crate) mod ldpc_174_91_c_parity;
#[path = "lib_wsjtx/nuttal_window.rs"]
pub(crate) mod nuttal_window;
#[path = "lib_wsjtx/ft8/osd174_91.rs"]
pub(crate) mod osd174_91;
#[path = "lib_wsjtx/77bit/packjt77.rs"]
pub(crate) mod packjt77;
#[path = "lib_wsjtx/platanh.rs"]
pub(crate) mod platanh;
#[path = "lib_wsjtx/ft8/subtractft8.rs"]
pub(crate) mod subtractft8;
pub use self::packjt77::HashCallBook;

#[inline]
pub(crate) fn sync8_df() -> f64 {
    crate::util::sync8_df()
}
