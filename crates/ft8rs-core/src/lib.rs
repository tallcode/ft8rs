pub mod about;
pub mod decode;
pub mod input;
pub mod stream;
pub(crate) mod util;

// Test-only adapters can exercise private native routines without widening the
// public API or modifying the mirrored decoder source trees.
#[cfg(test)]
#[path = "../tests/decoder/protocol.rs"]
mod decoder_protocol_tests;

pub use decode::ft8_decode::{decode_with_sbase, DecodeOptions, DecodedMessage};
pub use decode::HashCallBook;
pub use stream::session::{
    DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage,
};
pub use stream::SlotTimestamp;
pub use util::engine_name as fft_engine_name;
pub use util::set_fft_patience;
pub use util::set_fft_threads;
