use std::path::Path;

use ft8rs::input::audio::{downsample_12k, read_wav_mono_f32, resample_linear};
use ft8rs::wsjtx::{Audio, Config, Decoder, Error, Message, SlotTimestamp};
use ft8rs::{StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage};

const SLOT_LEN: usize = 180_000;

fn require_optimized_build() {
    if cfg!(debug_assertions) {
        panic!("use --profile fast or --release");
    }
}

fn read_audio(relative: &str) -> Vec<f32> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    let audio = read_wav_mono_f32(path).unwrap();
    assert_eq!(audio.sample_rate, 12_000);
    audio.samples
}

fn slot() -> SlotTimestamp {
    SlotTimestamp::parse("261103_120000").unwrap()
}

fn assert_same_rows(actual: &[Message], expected: &[StreamDecodedMessage], time: &SlotTimestamp) {
    assert_eq!(actual.len(), expected.len(), "row count changed at {time}");
    for (actual, expected) in actual.iter().zip(expected) {
        assert_eq!(&actual.slot, time);
        assert_eq!(
            actual.text, expected.msg,
            "row order/content changed at {time}"
        );
        assert_eq!(actual.snr_db.to_bits(), expected.snr.to_bits());
        assert_eq!(actual.dt_seconds.to_bits(), expected.dt.to_bits());
        assert_eq!(actual.frequency_hz.to_bits(), expected.freq.to_bits());
        assert_eq!(actual.sync.to_bits(), expected.sync.to_bits());
    }
}

#[test]
fn historical_recordings_preserve_every_row_and_numeric_bit_across_slots() {
    require_optimized_build();
    for (file, start) in [
        ("210703_133430", "210703_133430"),
        ("230208_140300", "230208_140300"),
    ] {
        let audio = read_audio(&format!("tests/ft8/{file}.wav"));
        assert_eq!(audio.len() % SLOT_LEN, 0);
        let mut decoder = Decoder::new(Config::default()).unwrap();
        let mut native = StreamDecodeSession::new(StreamDecodeConfig::default());
        let start = SlotTimestamp::parse(start).unwrap();
        let before = audio.clone();
        let mut count = 0;
        for (i, samples) in audio.as_chunks::<SLOT_LEN>().0.iter().enumerate() {
            let time = start.add_seconds(i as i64 * 15);
            let native_time = ft8rs::SlotTimestamp::parse(&time.to_string()).unwrap();
            let expected = native.decode_slot_at(&native_time, samples);
            let actual = decoder
                .decode(time.clone(), Audio::mono_f32(samples, 12_000))
                .unwrap();
            assert_same_rows(&actual, &expected, &time);
            count += actual.len();
        }
        assert_eq!(audio, before, "caller audio was changed");
        assert!(count > 0, "historical comparison decoded no messages");
        println!("{file}: all {count} rows preserved with bit-identical measurements");
    }
}

#[test]
fn known_truth_and_antialias_conversion_match_native_input_path() {
    require_optimized_build();
    let original = read_audio("tests/decoder/data/ft8/audio/wsjtx_cq.wav");
    for rate in [12_000, 44_100, 48_000] {
        let samples = resample_linear(&original, 12_000, rate);
        let before = samples.clone();
        let converted = downsample_12k(&samples, rate);
        let time = slot();
        let expected = StreamDecodeSession::new(StreamDecodeConfig::default()).decode_slot_at(
            &ft8rs::SlotTimestamp::parse(&time.to_string()).unwrap(),
            &converted,
        );
        let actual = Decoder::new(Config::default())
            .unwrap()
            .decode(time.clone(), Audio::mono_f32(&samples, rate))
            .unwrap();
        assert_same_rows(&actual, &expected, &time);
        assert!(
            actual.iter().any(|row| row.text == "CQ K1ABC FN42"),
            "{rate} Hz lost known truth"
        );
        assert!(
            actual.iter().all(|row| row.text == "CQ K1ABC FN42"),
            "{rate} Hz produced wrong content"
        );
        assert_eq!(samples, before);
    }
}

#[test]
fn callback_batches_follow_native_stages_and_arrive_before_completion() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    require_optimized_build();
    let samples = read_audio("tests/ft8/210703_133430.wav");
    let time = slot();
    let native_time = ft8rs::SlotTimestamp::parse(&time.to_string()).unwrap();
    let mut native = StreamDecodeSession::new(StreamDecodeConfig::default());
    let mut state = native.start_slot_decode();
    let mut early = Vec::new();
    native
        .decode_slot_nzhsym41_at(Some(&native_time), &mut state, &samples, |row| {
            early.push(row.clone());
            Ok(())
        })
        .unwrap();
    assert!(!early.is_empty(), "fixture must exercise early delivery");
    native.subtract_slot_nzhsym47(&mut state, &samples);
    let mut late = Vec::new();
    let all = native
        .decode_slot_nzhsym50_and_finish(state, &samples, |row| {
            late.push(row.clone());
            Ok(())
        })
        .unwrap();
    assert!(!late.is_empty(), "fixture must exercise a later batch");

    let (batch_tx, batch_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let start = Instant::now();
    let mut decoder = Decoder::new(Config::default()).unwrap();
    std::thread::scope(|scope| {
        let samples = &samples;
        let worker_time = time.clone();
        scope.spawn(move || {
            let mut first = true;
            decoder
                .decode_with_callback(worker_time, Audio::mono_f32(samples, 12_000), |batch| {
                    batch_tx.send((start.elapsed(), batch)).unwrap();
                    if first {
                        first = false;
                        release_rx.recv_timeout(Duration::from_secs(60)).unwrap();
                    }
                })
                .unwrap();
            done_tx.send(start.elapsed()).unwrap();
        });
        let (first_at, first) = batch_rx.recv_timeout(Duration::from_secs(60)).unwrap();
        assert!(
            done_rx.try_recv().is_err(),
            "completion must be separate from the first batch"
        );
        // Release the worker before assertions so a failure cannot leave it blocked.
        release_tx.send(()).unwrap();
        assert_same_rows(&first, &early, &time);
        let (last_at, last) = batch_rx.recv_timeout(Duration::from_secs(60)).unwrap();
        assert_same_rows(&last, &late, &time);
        let finished_at = done_rx.recv_timeout(Duration::from_secs(60)).unwrap();
        assert!(first_at <= last_at && last_at <= finished_at);
        assert!(batch_rx.recv().is_err(), "unexpected duplicate batch");
        let combined: Vec<_> = first.into_iter().chain(last).collect();
        assert_same_rows(&combined, &all, &time);
        println!("native batches: early={} final={}; first={first_at:?}, last={last_at:?}, completion={finished_at:?}", early.len(), late.len());
    });
}

#[test]
fn callback_skips_empty_stages_and_invalid_audio() {
    require_optimized_build();
    let mut decoder = Decoder::new(Config::default()).unwrap();
    let mut batches = Vec::new();
    let silence = vec![0.0; SLOT_LEN];
    decoder
        .decode_with_callback(slot(), Audio::mono_f32(&silence, 12_000), |batch| {
            batches.push(batch)
        })
        .unwrap();
    assert!(batches.is_empty());
    let error = decoder.decode_with_callback(slot(), Audio::mono_f32(&[], 12_000), |batch| {
        batches.push(batch)
    });
    assert!(matches!(error, Err(Error::InvalidAudioLength { .. })));
    assert!(batches.is_empty());

    let signal = read_audio("tests/decoder/data/ft8/audio/wsjtx_cq.wav");
    let config = Config {
        decode_depth: 1,
        ..Config::default()
    };
    let expected = Decoder::new(config.clone())
        .unwrap()
        .decode(slot(), Audio::mono_f32(&signal, 12_000))
        .unwrap();
    Decoder::new(config)
        .unwrap()
        .decode_with_callback(slot(), Audio::mono_f32(&signal, 12_000), |batch| {
            batches.push(batch)
        })
        .unwrap();
    assert_eq!(
        batches.len(),
        1,
        "shallow depth skips the early native stage"
    );
    assert_eq!(batches.pop().unwrap(), expected);
    assert!(expected.iter().any(|row| row.text == "CQ K1ABC FN42"));
}

#[test]
fn configured_ap_context_reset_and_independent_instances_match_native() {
    require_optimized_build();
    let config = Config {
        frequency_min_hz: 200.0,
        frequency_max_hz: 3000.0,
        decode_depth: 2,
        max_candidates: 500,
        ap_enabled: true,
        ap_cq_only: false,
        qso_progress: 2,
        contest_mode: 0,
        qso_frequency_hz: 1323.0,
        ap_width_hz: 40.0,
        my_call: Some("K1JT".into()),
        his_call: Some("BG5ATV".into()),
        his_grid: Some("PM00".into()),
    };
    let native_config = StreamDecodeConfig {
        nfa: 200.0,
        nfb: 3000.0,
        ndepth: 2,
        ncand: 500,
        lft8apon: true,
        lapcqonly: false,
        nQSOProgress: 2,
        ncontest: 0,
        nfqso: 1323.0,
        napwid: 40.0,
        mycall: Some("K1JT".into()),
        hiscall: Some("BG5ATV".into()),
        hisgrid: Some("PM00".into()),
        ..StreamDecodeConfig::default()
    };
    let mut decoder = Decoder::new(config.clone()).unwrap();
    let mut native = StreamDecodeSession::new(native_config.clone());
    let mut independent = Decoder::new(Config::default()).unwrap();
    let mut independent_native = StreamDecodeSession::new(StreamDecodeConfig::default());
    let signal = read_audio("tests/ft8/a8d_k1jt_bg5atv_pm00.wav");
    let historical = read_audio("tests/ft8/210703_133430.wav");
    let silence = vec![0.0; SLOT_LEN];
    for (i, samples) in [&signal, &historical, &silence, &signal]
        .into_iter()
        .enumerate()
    {
        let time = slot().add_seconds(i as i64 * 15);
        let native_time = ft8rs::SlotTimestamp::parse(&time.to_string()).unwrap();
        let expected = native.decode_slot_at(&native_time, samples);
        let actual = decoder
            .decode(time.clone(), Audio::mono_f32(samples, 12_000))
            .unwrap();
        assert_same_rows(&actual, &expected, &time);
        let expected = independent_native.decode_slot_at(&native_time, &historical);
        let actual = independent
            .decode(time.clone(), Audio::mono_f32(&historical, 12_000))
            .unwrap();
        assert_same_rows(&actual, &expected, &time);
    }
    decoder.reset();
    let time = slot();
    let expected = StreamDecodeSession::new(native_config).decode_slot_at(
        &ft8rs::SlotTimestamp::parse(&time.to_string()).unwrap(),
        &signal,
    );
    let actual = decoder
        .decode(time.clone(), Audio::mono_f32(&signal, 12_000))
        .unwrap();
    assert_same_rows(&actual, &expected, &time);
}

#[test]
fn malformed_audio_is_rejected_without_advancing_session() {
    require_optimized_build();
    let samples = read_audio("tests/ft8/210703_133430.wav");
    let mut decoder = Decoder::new(Config::default()).unwrap();
    decoder
        .decode(slot(), Audio::mono_f32(&samples, 12_000))
        .unwrap();
    let mut control = Decoder::new(Config::default()).unwrap();
    control
        .decode(slot(), Audio::mono_f32(&samples, 12_000))
        .unwrap();
    for rate in [0, 8_000, 192_001, u32::MAX] {
        assert_eq!(
            decoder.decode(slot(), Audio::mono_f32(&samples, rate)),
            Err(Error::UnsupportedSampleRate(rate))
        );
    }
    for length in [0, SLOT_LEN - 1, SLOT_LEN + 1] {
        let bad = vec![0.0; length];
        assert_eq!(
            decoder.decode(slot(), Audio::mono_f32(&bad, 12_000)),
            Err(Error::InvalidAudioLength {
                expected: SLOT_LEN,
                actual: length
            })
        );
    }
    assert_eq!(
        decoder.decode(slot().add_seconds(1), Audio::mono_f32(&samples, 12_000)),
        Err(Error::UnalignedSlot)
    );
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bad = samples.clone();
        bad[123] = value;
        assert_eq!(
            decoder.decode(slot(), Audio::mono_f32(&bad, 12_000)),
            Err(Error::NonFiniteSample { index: 123 })
        );
    }
    let next = slot().add_seconds(30);
    let actual = decoder
        .decode(next.clone(), Audio::mono_f32(&samples, 12_000))
        .unwrap();
    let expected = control
        .decode(next, Audio::mono_f32(&samples, 12_000))
        .unwrap();
    assert_eq!(
        actual, expected,
        "rejected inputs changed learned session state"
    );
}

#[test]
fn invalid_configuration_and_dates_return_standard_errors() {
    fn standard_error(_: &dyn std::error::Error) {}
    for config in [
        Config {
            frequency_min_hz: f64::NAN,
            ..Config::default()
        },
        Config {
            frequency_max_hz: 5001.0,
            ..Config::default()
        },
        Config {
            frequency_min_hz: 3000.0,
            ..Config::default()
        },
        Config {
            decode_depth: 0,
            ..Config::default()
        },
        Config {
            decode_depth: 4,
            ..Config::default()
        },
        Config {
            max_candidates: 0,
            ..Config::default()
        },
        Config {
            qso_progress: 6,
            ..Config::default()
        },
        Config {
            contest_mode: 9,
            ..Config::default()
        },
        Config {
            qso_frequency_hz: f64::INFINITY,
            ..Config::default()
        },
        Config {
            ap_width_hz: -1.0,
            ..Config::default()
        },
        Config {
            my_call: Some(char::REPLACEMENT_CHARACTER.to_string()),
            ..Config::default()
        },
        Config {
            his_call: Some("K1ABC\n".into()),
            ..Config::default()
        },
        Config {
            his_grid: Some("PM00\t".into()),
            ..Config::default()
        },
    ] {
        let error = Decoder::new(config)
            .err()
            .expect("invalid config was accepted");
        assert!(matches!(error, Error::InvalidConfig(_)));
        standard_error(&error);
        assert!(!error.to_string().is_empty());
    }
    for invalid in ["", "261103_240000", "261131_120000", "21000229_000000"] {
        let error = SlotTimestamp::parse(invalid).unwrap_err();
        assert!(matches!(error, Error::InvalidTimestamp(_)));
        standard_error(&error);
    }
    assert_eq!(
        SlotTimestamp::parse("20261231_235945")
            .unwrap()
            .add_seconds(15)
            .to_string(),
        "20270101_000000"
    );
    assert_eq!(
        SlotTimestamp::from_unix_seconds_utc(0).to_string(),
        "700101_000000"
    );
}
