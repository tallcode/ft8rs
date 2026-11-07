pub mod profile;
pub mod session;
pub mod slot;
pub mod time;
pub mod validation;

pub use profile::ProfileStreamDecodeSession;
pub use session::{DecodeProfile, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage};
pub use slot::{
    decode_12k_slots, decode_12k_slots_streaming, decode_12k_slots_streaming_decodes,
    TimestampedDecode,
};
pub use time::SlotTimestamp;

#[cfg(test)]
mod configured_hash_tests;

pub(crate) mod fixed_array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer, const N: usize>(
        values: &[T; N],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        values.as_slice().serialize(s)
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>, const N: usize>(
        d: D,
    ) -> Result<[T; N], D::Error> {
        Vec::<T>::deserialize(d)?
            .try_into()
            .map_err(|_| serde::de::Error::custom("invalid fixed array length"))
    }
}
