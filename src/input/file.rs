use std::path::Path;

use crate::input::audio::{read_wav_mono_f32, resample_linear};
use crate::stream::session::{StreamDecodeConfig, StreamDecodedMessage};
use crate::stream::slot::{
    decode_12k_slots, decode_12k_slots_streaming, decode_12k_slots_streaming_decodes,
    TimestampedDecode,
};
use crate::stream::time::SlotTimestamp;

const SAMPLE_RATE: u32 = 12_000;

#[derive(Clone, Debug)]
pub struct FileDecodeOptions {
    pub start_time: SlotTimestamp,
    pub config: StreamDecodeConfig,
}

pub fn decode_wav_file(
    path: impl AsRef<Path>,
    options: FileDecodeOptions,
) -> Result<Vec<TimestampedDecode>, String> {
    let samples_12k = read_wav_12k(path)?;
    Ok(decode_12k_slots(
        &samples_12k,
        options.start_time,
        options.config,
    ))
}

pub fn decode_wav_file_streaming<F>(
    path: impl AsRef<Path>,
    options: FileDecodeOptions,
    on_slot: F,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, Vec<StreamDecodedMessage>) -> Result<(), String>,
{
    let samples_12k = read_wav_12k(path)?;
    decode_12k_slots_streaming(&samples_12k, options.start_time, options.config, on_slot)
}

pub fn decode_wav_file_streaming_decodes<F, G>(
    path: impl AsRef<Path>,
    options: FileDecodeOptions,
    on_decode: F,
    on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let samples_12k = read_wav_12k(path)?;
    decode_12k_slots_streaming_decodes(
        &samples_12k,
        options.start_time,
        options.config,
        on_decode,
        on_slot_complete,
    )
}

pub fn infer_start_time_from_path(path: impl AsRef<Path>) -> Option<SlotTimestamp> {
    let stem = path.as_ref().file_stem()?.to_string_lossy();
    let stem = stem.as_ref();
    let suffix = stem
        .as_bytes()
        .windows(13)
        .rposition(|window| is_timestamp_bytes(window))
        .map(|idx| &stem[idx..idx + 13])?;
    SlotTimestamp::parse(suffix).ok()
}

fn read_wav_12k(path: impl AsRef<Path>) -> Result<Vec<f32>, String> {
    let audio = read_wav_mono_f32(path)?;
    Ok(resample_linear(
        &audio.samples,
        audio.sample_rate,
        SAMPLE_RATE,
    ))
}

fn is_timestamp_bytes(window: &[u8]) -> bool {
    window.len() == 13
        && window[6] == b'_'
        && window[..6].iter().all(u8::is_ascii_digit)
        && window[7..].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::infer_start_time_from_path;

    #[test]
    fn infers_start_time_from_wsjtx_filename() {
        let ts = infer_start_time_from_path("tests/ft8/230208_140300.wav").unwrap();
        assert_eq!(ts.format(), "230208_140300");
    }
}
