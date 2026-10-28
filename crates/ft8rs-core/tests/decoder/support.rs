use super::adapters::Decoder;
use ft8rs::input::audio::{read_wav_mono_f32, resample_linear};
use ft8rs::{SlotTimestamp, StreamDecodedMessage};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

pub fn audio(path: impl AsRef<Path>) -> Vec<f32> {
    let path = root().join(path);
    let wav = read_wav_mono_f32(&path).unwrap();
    if wav.sample_rate == 12000 {
        wav.samples
    } else {
        resample_linear(&wav.samples, wav.sample_rate, 12000)
    }
}

pub fn normal(message: &str) -> String {
    message
        .split_whitespace()
        .map(|token| {
            if token == "<...>" {
                token.to_owned()
            } else {
                token.trim_matches(['<', '>']).to_ascii_uppercase()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn matches(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    normal(&a.msg) == normal(&b.msg) && (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3
}

pub struct Replay {
    pub rows: Vec<StreamDecodedMessage>,
    pub first_message: Option<Duration>,
    pub last_message: Option<Duration>,
    pub finished: Duration,
}

pub fn replay(decoder: &mut dyn Decoder, timestamp: &SlotTimestamp, audio: &[f32]) -> Replay {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
    let start = Instant::now();
    let mut callbacks = Vec::new();
    let mut first = None;
    let mut last = None;
    let rows = decoder
        .decode(timestamp, audio, &mut |row| {
            let elapsed = start.elapsed();
            first.get_or_insert(elapsed);
            last = Some(elapsed);
            callbacks.push(row.clone());
            Ok(())
        })
        .expect("decoder/callback failure");
    let finished = start.elapsed();
    // One-to-one matching: repeated callbacks cannot hide a missing row.
    assert_eq!(
        callbacks.len(),
        rows.len(),
        "callback/return count mismatch"
    );
    let mut used = vec![false; rows.len()];
    for callback in &callbacks {
        let index = rows
            .iter()
            .enumerate()
            .position(|(i, row)| !used[i] && matches(callback, row))
            .unwrap_or_else(|| panic!("callback missing from return: {}", callback.msg));
        used[index] = true;
    }
    assert!(last.is_none_or(|time| time <= finished));
    Replay {
        rows,
        first_message: first,
        last_message: last,
        finished,
    }
}

impl Replay {
    pub fn describe(&self) -> String {
        format!(
            "rows={} first={:?} last={:?} finished={:?}",
            self.rows.len(),
            self.first_message,
            self.last_message,
            self.finished
        )
    }
}

pub fn noise(seed: u64) -> Vec<f32> {
    let mut noise = super::random::Noise::new(seed);
    (0..180000)
        .map(|_| (noise.gaussian() * 0.03) as f32)
        .collect()
}
