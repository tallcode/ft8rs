//! Hybrid decode runner.
//!
//! Hybrid shares input samples between the WSJT-X and JTDX decoders, but does
//! not share decoder-private state in phase 1. Only decoded results are merged.

use std::collections::HashMap;
use std::thread;

use crate::decode::lib_jtdx::JtdxStreamDecodeSession;
use crate::stream::session::{StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage};
use crate::stream::time::SlotTimestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeSource {
    Wsjtx,
    Jtdx,
    Both,
}

#[derive(Clone, Debug)]
pub struct HybridDecodedMessage {
    pub decode: StreamDecodedMessage,
    pub source: DecodeSource,
}

pub struct HybridStreamDecodeSession {
    wsjtx: StreamDecodeSession,
    jtdx: JtdxStreamDecodeSession,
}

impl HybridStreamDecodeSession {
    pub fn new(config: StreamDecodeConfig) -> Self {
        Self {
            wsjtx: StreamDecodeSession::new(config.clone_for_profile_wsjt_x()),
            jtdx: JtdxStreamDecodeSession::new(config.clone_for_profile_jtdx()),
        }
    }

    pub fn decode_slot_streaming_at<F>(
        &mut self,
        timestamp: &SlotTimestamp,
        samples: &[f32],
        mut on_decode: F,
    ) -> Result<Vec<StreamDecodedMessage>, String>
    where
        F: FnMut(&StreamDecodedMessage) -> Result<(), String>,
    {
        let (wsjtx_results, jtdx) = thread::scope(|scope| {
            let jtdx_session = &mut self.jtdx;
            let jtdx_handle = scope
                .spawn(|| jtdx_session.decode_slot_streaming_at(timestamp, samples, |_| Ok(())));

            let mut wsjtx_results = Vec::new();
            self.wsjtx
                .decode_slot_streaming_at(timestamp, samples, |decode| {
                    wsjtx_results.push(decode.clone());
                    on_decode(decode)
                })?;

            let jtdx = jtdx_handle
                .join()
                .map_err(|_| "hybrid JTDX worker panicked".to_string())??;

            Ok::<_, String>((wsjtx_results, jtdx))
        })?;

        let merged_with_source = merge_decodes_with_source(&wsjtx_results, &jtdx);
        for row in &merged_with_source {
            if row.source == DecodeSource::Jtdx {
                on_decode(&row.decode)?;
            }
        }
        Ok(merged_with_source
            .into_iter()
            .map(|row| row.decode)
            .collect())
    }
}

pub fn merge_decodes(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> Vec<StreamDecodedMessage> {
    merge_decodes_with_source(wsjtx, jtdx)
        .into_iter()
        .map(|row| row.decode)
        .collect()
}

fn merge_decodes_with_source(
    wsjtx: &[StreamDecodedMessage],
    jtdx: &[StreamDecodedMessage],
) -> Vec<HybridDecodedMessage> {
    let mut rows: Vec<HybridDecodedMessage> = Vec::new();
    let mut by_msg: HashMap<String, Vec<usize>> = HashMap::new();

    for decode in wsjtx {
        let key = normalized_message(&decode.msg);
        by_msg.entry(key).or_default().push(rows.len());
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Wsjtx,
        });
    }

    for decode in jtdx {
        let key = normalized_message(&decode.msg);
        if let Some(idx) = by_msg.get(&key).and_then(|indices| {
            indices
                .iter()
                .copied()
                .find(|&idx| is_same_signal(&rows[idx].decode, decode))
        }) {
            rows[idx].source = DecodeSource::Both;
            continue;
        }
        by_msg.entry(key).or_default().push(rows.len());
        rows.push(HybridDecodedMessage {
            decode: decode.clone(),
            source: DecodeSource::Jtdx,
        });
    }

    rows
}

fn normalized_message(msg: &str) -> String {
    msg.split_whitespace()
        .map(normalized_message_word)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_message_word(word: &str) -> String {
    let word = word.to_ascii_uppercase();
    if word.starts_with('<') && word.ends_with('>') {
        let inner = &word[1..word.len() - 1];
        if !inner.is_empty() && inner != "..." {
            return inner.to_string();
        }
    }
    word
}

fn is_same_signal(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(freq: f64, dt: f64, msg: &str) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq,
            dt,
            snr: 0.0,
            msg: msg.to_string(),
            sync: 0.0,
            itone: [0; 79],
        }
    }

    #[test]
    fn merge_decodes_deduplicates_hash_brace_display_variants() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <RK4FF> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes_with_source(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].source, DecodeSource::Both);
        assert_eq!(merged[0].decode.msg, "EA5/DH0YAH <RK4FF> RR73");
    }

    #[test]
    fn merge_decodes_keeps_unresolved_hash_marker_distinct() {
        let wsjtx = [decoded(1205.0, 0.6, "EA5/DH0YAH <...> RR73")];
        let jtdx = [decoded(1206.0, 0.8, "EA5/DH0YAH RK4FF RR73")];

        let merged = merge_decodes_with_source(&wsjtx, &jtdx);

        assert_eq!(merged.len(), 2);
    }
}
