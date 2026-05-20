pub mod ft8;
pub mod ft4;
pub mod util;
pub mod stream;

// Re-exports matching ft8ts index.ts
pub use ft8::decode::{decode as decode_ft8, DecodedMessage as DecodedFT8Message, DecodeOptions as DecodeFT8Options, SyncMode};
pub use ft8::encode::encode as encode_ft8;
pub use ft4::decode::{decode as decode_ft4, DecodedMessage as DecodedFT4Message, DecodeOptions as DecodeFT4Options};
pub use ft4::encode::encode as encode_ft4;
pub use util::hashcall::HashCallBook;
pub use util::long_decode::{long_decode, LongDecodeConfig, LongDecodeResult, SegmentResult};
