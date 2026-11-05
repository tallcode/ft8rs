//! Serial paired replay. Raw bit snapshots include callback order and metadata.
use super::{fft, metrics, session};
use crate::{SlotTimestamp, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage};
use std::{path::PathBuf, time::Instant};

#[derive(serde::Serialize, Debug, PartialEq, Eq)]
struct Row {
    msg: String,
    freq: u64,
    dt: u64,
    snr: u64,
    sync: u64,
    tones: Vec<i32>,
}

impl From<&StreamDecodedMessage> for Row {
    fn from(r: &StreamDecodedMessage) -> Self {
        Self {
            msg: r.msg.clone(),
            freq: r.freq.to_bits(),
            dt: r.dt.to_bits(),
            snr: r.snr.to_bits(),
            sync: r.sync.to_bits(),
            tones: r.itone.to_vec(),
        }
    }
}

#[derive(serde::Serialize)]
struct Slot {
    rows: Vec<Row>,
    callbacks: Vec<Row>,
    callback_compute_ms: Vec<f64>,
    callback_boundary_ms: Vec<f64>,
    stage_ms: [f64; 3],
    total_ms: f64,
}

enum Runner {
    Original(StreamDecodeSession),
    Plus(session::StreamDecodeSession),
}

impl Runner {
    fn new(mode: &str, config: StreamDecodeConfig) -> Self {
        match mode {
            "original" => Self::Original(StreamDecodeSession::new(config)),
            "reference" | "optimized" => Self::Plus(session::StreamDecodeSession::new(config)),
            _ => panic!("unknown plus validation mode: {mode}"),
        }
    }

    fn decode(&mut self, timestamp: &SlotTimestamp, samples: &[f32]) -> Slot {
        // Both paths use identical public stage calls. State allocation is
        // included; input I/O, interstage acquisition waits and reporting aren't.
        macro_rules! run {
            ($session:expr) => {{
                let start = Instant::now();
                let mut state = $session.start_slot_decode();
                let mut callbacks = Vec::new();
                let mut callback_compute_ms = Vec::new();
                let mut callback_boundary_ms = Vec::new();
                let stage_start = Instant::now();
                $session
                    .decode_slot_nzhsym41_at(
                        Some(timestamp),
                        &mut state,
                        &samples[..samples.len().min(41 * 3456)],
                        |row| {
                            callbacks.push(Row::from(row));
                            callback_compute_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                            callback_boundary_ms
                                .push(-3192.0 + start.elapsed().as_secs_f64() * 1000.0);
                            Ok(())
                        },
                    )
                    .unwrap();
                let early = stage_start.elapsed().as_secs_f64() * 1000.0;
                let subtract_start_boundary_ms =
                    (-3192.0 + start.elapsed().as_secs_f64() * 1000.0).max(-1464.0);
                let stage_start = Instant::now();
                $session
                    .subtract_slot_nzhsym47(&mut state, &samples[..samples.len().min(47 * 3456)]);
                let subtract = stage_start.elapsed().as_secs_f64() * 1000.0;
                let final_start_boundary_ms = (subtract_start_boundary_ms + subtract).max(-600.0);
                let stage_start = Instant::now();
                let rows = $session
                    .decode_slot_nzhsym50_and_finish(
                        state,
                        &samples[..samples.len().min(50 * 3456)],
                        |row| {
                            callbacks.push(Row::from(row));
                            callback_compute_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                            callback_boundary_ms.push(
                                final_start_boundary_ms
                                    + stage_start.elapsed().as_secs_f64() * 1000.0,
                            );
                            Ok(())
                        },
                    )
                    .unwrap();
                let final_ms = stage_start.elapsed().as_secs_f64() * 1000.0;
                let total_ms = start.elapsed().as_secs_f64() * 1000.0;
                let rows: Vec<_> = rows.iter().map(Row::from).collect();
                assert_eq!(rows, callbacks, "callback/return order or bits changed");
                assert!(total_ms <= 10000.0, "slot exceeded 10 seconds: {total_ms}");
                Slot {
                    rows,
                    callbacks,
                    callback_compute_ms,
                    callback_boundary_ms,
                    stage_ms: [early, subtract, final_ms],
                    total_ms,
                }
            }};
        }
        match self {
            Self::Original(session) => run!(session),
            Self::Plus(session) => run!(session),
        }
    }
}

struct Case {
    id: String,
    file: PathBuf,
    config: StreamDecodeConfig,
    timestamp: SlotTimestamp,
}

fn cases() -> Vec<Case> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut cases = Vec::new();
    for stem in ["210703_133430", "230208_140300"] {
        let config = StreamDecodeConfig {
            nfa: if stem.starts_with("21") { 100.0 } else { 200.0 },
            ..Default::default()
        };
        cases.push(Case {
            id: stem.into(),
            file: root.join(format!("ft8/{stem}.wav")),
            config: config.clone(),
            timestamp: SlotTimestamp::parse(stem).unwrap(),
        });
        for row in include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/decoder/data/ft8/degraded/recordings.tsv"
        ))
        .lines()
        .filter(|s| !s.starts_with('#') && !s.is_empty())
        {
            let fields: Vec<_> = row.split('\t').collect();
            if fields[2] == stem {
                cases.push(Case {
                    id: fields[0].into(),
                    file: root.join("decoder/data/ft8/degraded").join(fields[1]),
                    config: config.clone(),
                    timestamp: SlotTimestamp::parse(stem).unwrap(),
                });
            }
        }
    }
    for source in ["wsjtx", "jtdx"] {
        let table =
            std::fs::read_to_string(root.join(format!("decoder/data/ft8/channels/{source}.tsv")))
                .unwrap();
        for row in table
            .lines()
            .filter(|s| !s.starts_with('#') && !s.is_empty())
        {
            let fields: Vec<_> = row.split('\t').collect();
            cases.push(Case {
                id: format!("{source}-{}", fields[0]),
                file: root.join("decoder/data/ft8/channels").join(fields[1]),
                config: StreamDecodeConfig {
                    hiscall: Some("K1ABC".into()),
                    nfqso: 1000.0,
                    ..Default::default()
                },
                timestamp: SlotTimestamp::parse("261028_120000").unwrap(),
            });
        }
    }
    cases.push(Case {
        id: "rr73-current".into(),
        file: root.join("decoder/data/ft8/waveforms/rr73-current.wav"),
        config: StreamDecodeConfig::default(),
        timestamp: SlotTimestamp::parse("261028_120000").unwrap(),
    });
    cases.push(Case {
        id: "a8-context".into(),
        file: root.join("ft8/a8d_k1jt_bg5atv_pm00.wav"),
        config: StreamDecodeConfig {
            mycall: Some("K1JT".into()),
            hiscall: Some("BG5ATV".into()),
            hisgrid: Some("PM00".into()),
            nfqso: 1000.0,
            ..Default::default()
        },
        timestamp: SlotTimestamp::parse("261028_120000").unwrap(),
    });
    cases
}

#[derive(serde::Serialize)]
struct Replay {
    case: String,
    input: String,
    config: StreamDecodeConfig,
    repeat: usize,
    slots: Vec<Slot>,
    metrics: Option<metrics::Metrics>,
}

#[test]
#[ignore = "serial performance/retention run; use scripts/test-wsjtx-plus.py"]
fn paired_corpus_replay() {
    if cfg!(debug_assertions) {
        panic!("use optimized builds");
    }
    let Ok(mode) = std::env::var("FT8RS_PLUS_MODE") else {
        // The repository's normal --include-ignored run must still perform a
        // real check without requiring a report destination or a benchmark run.
        for case in cases()
            .into_iter()
            .filter(|c| matches!(c.id.as_str(), "rr73-current" | "a8-context"))
        {
            let wav = crate::input::audio::read_wav_mono_f32(&case.file).unwrap();
            let old =
                Runner::new("original", case.config.clone()).decode(&case.timestamp, &wav.samples);
            for mode in [fft::Mode::Reference, fft::Mode::Optimized] {
                let new = fft::with_mode(mode, || {
                    Runner::new("optimized", case.config.clone())
                        .decode(&case.timestamp, &wav.samples)
                });
                assert_eq!(old.rows, new.rows, "{} {mode:?}", case.id);
                assert_eq!(old.callbacks, new.callbacks, "{} {mode:?}", case.id);
            }
        }
        return;
    };
    let repeats: usize = std::env::var("FT8RS_PLUS_REPEATS")
        .unwrap_or_else(|_| "3".into())
        .parse()
        .unwrap();
    assert!(repeats > 0);
    let profile = std::env::var("FT8RS_PLUS_PROFILE").is_ok_and(|v| v == "1");
    assert!(
        !profile || mode != "original",
        "profiling is private to plus"
    );
    let filter = std::env::var("FT8RS_PLUS_CASE").unwrap_or_default();
    let output = PathBuf::from(std::env::var_os("FT8RS_PLUS_OUTPUT").expect("choose fresh output"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    let mut count = 0;
    fft::with_mode(
        if mode == "reference" {
            fft::Mode::Reference
        } else {
            fft::Mode::Optimized
        },
        || {
            for case in cases()
                .into_iter()
                .filter(|c| filter.is_empty() || c.id == filter)
            {
                let wav = crate::input::audio::read_wav_mono_f32(&case.file).unwrap();
                assert_eq!(wav.sample_rate, 12000);
                for repeat in 0..repeats {
                    if profile {
                        metrics::begin();
                    }
                    let mut runner = Runner::new(&mode, case.config.clone());
                    let mut slots = Vec::new();
                    for (i, samples) in wav.samples.chunks(180000).enumerate() {
                        slots.push(
                            runner.decode(&case.timestamp.add_seconds(i as i64 * 15), samples),
                        );
                    }
                    let result = Replay {
                        case: case.id.clone(),
                        input: case.file.display().to_string(),
                        config: case.config.clone(),
                        repeat,
                        slots,
                        metrics: profile.then(metrics::finish),
                    };
                    serde_json::to_writer(&mut file, &result).unwrap();
                    use std::io::Write;
                    writeln!(file).unwrap();
                    eprintln!(
                        "plus {mode} {} repeat={repeat} slots={} rows={} total={:.3}s",
                        case.id,
                        result.slots.len(),
                        result.slots.iter().map(|s| s.rows.len()).sum::<usize>(),
                        result.slots.iter().map(|s| s.total_ms).sum::<f64>() / 1000.0
                    );
                    count += 1;
                }
            }
        },
    );
    assert!(count > 0, "empty corpus selection");
}

#[test]
fn session_hash_state_is_independent() {
    let mut first = session::StreamDecodeSession::new(StreamDecodeConfig::default());
    let second = session::StreamDecodeSession::new(StreamDecodeConfig::default());
    first.import_hash_calls(&["K1ABC".into()]);
    // Configured/imported hints must not become regularly decoded evidence.
    assert!(first.export_regular_hash_calls().is_empty());
    assert!(second.export_regular_hash_calls().is_empty());
    // The session's own tests additionally exercise per-parity AP ownership.
}
