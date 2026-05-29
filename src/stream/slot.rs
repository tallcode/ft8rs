use crate::stream::session::{StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage};
use crate::stream::time::SlotTimestamp;

const SAMPLE_RATE: u32 = 12_000;
const SLOT_SECONDS: usize = 15;

#[derive(Clone, Debug)]
pub struct TimestampedDecode {
    pub timestamp: SlotTimestamp,
    pub decode: StreamDecodedMessage,
}

pub fn decode_12k_slots(
    samples_12k: &[f32],
    start_time: SlotTimestamp,
    config: StreamDecodeConfig,
) -> Vec<TimestampedDecode> {
    collect_12k_slots(samples_12k, start_time, config)
}

pub fn decode_12k_slots_streaming<F>(
    samples_12k: &[f32],
    start_time: SlotTimestamp,
    config: StreamDecodeConfig,
    mut on_slot: F,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, Vec<StreamDecodedMessage>) -> Result<(), String>,
{
    let samples_per_slot = SAMPLE_RATE as usize * SLOT_SECONDS;
    let total_slots = samples_12k.len().div_ceil(samples_per_slot);
    let mut decoder = StreamDecodeSession::new(config);

    for slot in 0..total_slots {
        let start = slot * samples_per_slot;
        let end = (start + samples_per_slot).min(samples_12k.len());
        let timestamp = start_time.add_seconds((slot * SLOT_SECONDS) as i64);
        let results = decoder.decode_slot_at(&timestamp, &samples_12k[start..end]);
        on_slot(timestamp, results)?;
    }

    Ok(())
}

pub fn decode_12k_slots_streaming_decodes<F, G>(
    samples_12k: &[f32],
    start_time: SlotTimestamp,
    config: StreamDecodeConfig,
    mut on_decode: F,
    mut on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let samples_per_slot = SAMPLE_RATE as usize * SLOT_SECONDS;
    let total_slots = samples_12k.len().div_ceil(samples_per_slot);
    let mut decoder = StreamDecodeSession::new(config);

    for slot in 0..total_slots {
        let start = slot * samples_per_slot;
        let end = (start + samples_per_slot).min(samples_12k.len());
        let timestamp = start_time.add_seconds((slot * SLOT_SECONDS) as i64);
        let results =
            decoder.decode_slot_streaming_at(&timestamp, &samples_12k[start..end], |decode| {
                on_decode(timestamp.clone(), decode)
            })?;
        on_slot_complete(timestamp, results.len())?;
    }

    Ok(())
}

fn collect_12k_slots(
    samples_12k: &[f32],
    start_time: SlotTimestamp,
    config: StreamDecodeConfig,
) -> Vec<TimestampedDecode> {
    let mut out = Vec::new();
    decode_12k_slots_streaming(samples_12k, start_time, config, |timestamp, results| {
        out.extend(results.into_iter().map(|decode| TimestampedDecode {
            timestamp: timestamp.clone(),
            decode,
        }));
        Ok(())
    })
    .expect("collecting in-memory decode slots cannot fail");
    out
}
