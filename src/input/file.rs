use std::path::Path;

use crate::input::audio::{read_wav_mono_f32, resample_linear};
use crate::stream::session::{StreamDecodeConfig, StreamDecodedMessage};
use crate::stream::slot::{decode_12k_slots, decode_12k_slots_streaming, TimestampedDecode};
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

fn read_wav_12k(path: impl AsRef<Path>) -> Result<Vec<f32>, String> {
    let audio = read_wav_mono_f32(path)?;
    Ok(resample_linear(
        &audio.samples,
        audio.sample_rate,
        SAMPLE_RATE,
    ))
}
