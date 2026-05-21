use std::rc::Rc;

use crate::ft8::decode::{decode, DecodeOptions, SyncMode};
use crate::util::hashcall::HashCallBook;

const SAMPLE_RATE: u32 = 12000;

#[derive(Clone)]
pub struct StreamDecodeConfig {
    pub freq_low: f64,
    pub freq_high: f64,
    pub sync_min: f64,
    pub max_candidates: usize,
    pub depth: usize,
}

impl Default for StreamDecodeConfig {
    fn default() -> Self {
        Self { freq_low: 200.0, freq_high: 3000.0, sync_min: 1.3, max_candidates: 600, depth: 3 }
    }
}

#[derive(Clone, Debug)]
pub struct StreamDecodedMessage {
    pub freq: f64,
    pub dt: f64,
    pub snr: f64,
    pub msg: String,
    pub sync: f64,
    pub itone: [i32; 79],
}

pub struct StreamDecoder {
    config: StreamDecodeConfig,
    book: Rc<HashCallBook>,
}

impl StreamDecoder {
    pub fn new(config: StreamDecodeConfig) -> Self {
        Self { config, book: Rc::new(HashCallBook::new()) }
    }

    pub fn decode_slot(&mut self, samples: &[f32]) -> Vec<StreamDecodedMessage> {
        let book = Rc::clone(&self.book);
        let results = decode(samples, DecodeOptions {
            sample_rate: Some(SAMPLE_RATE as usize),
            freq_low: Some(self.config.freq_low),
            freq_high: Some(self.config.freq_high),
            sync_min: Some(self.config.sync_min),
            depth: Some(self.config.depth),
            max_candidates: Some(self.config.max_candidates),
            hash_call_book: Some(book),
            mycall: None,
            hiscall: None,
            sync_mode: Some(SyncMode::Power),
        });

        let mut seen = std::collections::HashSet::new();
        let mut merged = Vec::new();
        for d in &results {
            let key = normal(&d.msg);
            if seen.insert(key) {
                let mut itone = [0i32; 79];
                itone.copy_from_slice(&d.itone[..79]);
                merged.push(StreamDecodedMessage {
                    freq: d.freq, dt: d.dt, snr: d.snr,
                    msg: d.msg.clone(), sync: d.sync, itone,
                });
            }
            for part in d.msg.split_whitespace() {
                let p = part.trim();
                if p.len() >= 3
                    && p.chars().all(|c| c.is_alphanumeric() || c == '/' || c == '<' || c == '>')
                    && p.chars().any(|c| c.is_numeric())
                {
                    self.book.save(p);
                }
            }
        }
        merged
    }
}

fn normal(msg: &str) -> String {
    msg.split_whitespace().map(|w| w.trim().to_uppercase()).collect::<Vec<_>>().join(" ")
}