use std::fs::File;

use serde_json::{json, Value};

use super::*;
use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::StreamDecodedMessage;
use ft8rs_engine::protocol::DecodeStage;
use ft8rs_engine::soundcard::InputChannel;

const MIDNIGHT: i64 = 20_766 * SLOTS_PER_DAY;
static TEST_DIR_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn record_audio(recorder: &Recorder, slot: i64, samples: &[f32]) {
    recorder.audio(
        slot,
        (slot + 1) * SLOT_MS,
        CapturedSlot {
            timestamp: &slot_timestamp(slot),
            samples_12k: samples,
            config: &StreamDecodeConfig::default(),
            device_name: "test input",
            format: &SoundcardFormatInfo {
                channels: 2,
                sample_rate: 48_000,
                sample_format: "F32".into(),
            },
            channel: InputChannel::Left,
            native_samples: samples.len() * 4,
            xruns: 2,
            jitter_ms: 3,
        },
    );
}

struct TestDirectory {
    path: PathBuf,
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn test_directory() -> TestDirectory {
    let path = std::env::temp_dir().join(format!(
        "ft8rs-debug-test-{}-{}-{}",
        std::process::id(),
        now_ms(),
        TEST_DIR_ID.fetch_add(1, Ordering::Relaxed),
    ));
    fs::create_dir(&path).unwrap();
    TestDirectory { path }
}

#[test]
fn capture_is_station_gated_and_reenable_starts_with_an_empty_window() {
    assert!(eligible(" bg5atv "));
    assert!(!eligible("BG5ATV/P"));
    assert!(!eligible(""));

    let recorder = Recorder::default();
    let slot = MIDNIGHT;
    record_audio(&recorder, slot, &[0.25]);
    assert!(recorder.session.lock().unwrap().is_none());
    recorder.set_active(true, slot * SLOT_MS + 1000);
    record_audio(&recorder, slot, &[0.25]);
    assert!(recorder
        .session
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .window()
        .is_none());
    record_audio(&recorder, slot + 1, &[0.25]);
    recorder.request((slot + 2) * SLOT_MS, None);
    assert!(recorder.ready(Instant::now()).is_none());
    recorder.set_active(false, (slot + 2) * SLOT_MS);
    assert!(recorder.session.lock().unwrap().is_none());

    recorder.set_active(true, (slot + 2) * SLOT_MS);
    assert!(recorder
        .ready(Instant::now() + Duration::from_secs(11))
        .is_none());
    let session = recorder.session.lock().unwrap();
    let session = session.as_ref().unwrap();
    assert!(session.slots.is_empty());
    assert!(session.window().is_none());
}

#[test]
fn rolling_window_keeps_eight_and_freezes_selection_at_save_click() {
    let recorder = Recorder::default();
    recorder.set_active(true, MIDNIGHT * SLOT_MS - 1);
    for slot in MIDNIGHT..MIDNIGHT + 9 {
        record_audio(&recorder, slot, &[0.125]);
    }
    {
        let guard = recorder.session.lock().unwrap();
        let session = guard.as_ref().unwrap();
        assert!(!session.slots.contains_key(&MIDNIGHT));
        assert_eq!(
            session
                .slots
                .values()
                .filter(|slot| slot.audio.is_some())
                .count(),
            8
        );
    }
    recorder.request((MIDNIGHT + 9) * SLOT_MS, None);
    record_audio(&recorder, MIDNIGHT + 9, &[0.25]);
    let export = recorder
        .ready(Instant::now() + Duration::from_secs(11))
        .unwrap();
    let segment = &export.request.segments[0];
    assert_eq!(segment.window.first, MIDNIGHT + 1);
    assert_eq!(segment.window.last, MIDNIGHT + 8);
    assert!(segment.slots[&(MIDNIGHT + 1)].audio.is_some());
}

#[test]
fn backwards_clock_step_keeps_completed_audio_with_distinct_slot_names() {
    let recorder = Recorder::default();
    let slot_ms = MIDNIGHT * SLOT_MS;
    recorder.set_active(true, slot_ms - 1);
    record_audio(&recorder, MIDNIGHT, &[0.125]);
    recorder.request(slot_ms + SLOT_MS, None);
    recorder.record(|session| session.clock_changed(slot_ms - 1));

    let original = recorder.ready(Instant::now()).unwrap();
    assert_eq!(original.request.segments.len(), 1);
    assert_eq!(original.request.segments[0].id, 0);

    record_audio(&recorder, MIDNIGHT, &[0.25]);
    recorder.request(slot_ms + 2 * SLOT_MS, None);
    let directory = test_directory();
    let exported = recorder
        .ready(Instant::now())
        .unwrap()
        .write(&directory.path)
        .unwrap()
        .unwrap();
    let timestamp = slot_timestamp(MIDNIGHT);
    let audio = exported.join("audio");
    let samples = |name: String| {
        ft8rs::input::audio::read_wav_mono_f32(audio.join(name))
            .unwrap()
            .samples
    };
    assert_eq!(samples(format!("{timestamp}.wav")), [0.125]);
    assert_eq!(samples(format!("g1_{timestamp}.wav")), [0.25]);
}

#[test]
fn export_round_trips_audio_and_keeps_late_decodes_and_logs_for_their_slot() {
    let recorder = Recorder::default();
    let slot = MIDNIGHT - 1;
    let slot_ms = slot * SLOT_MS;
    let boundary_ms = MIDNIGHT * SLOT_MS;
    let directory = test_directory();
    let root = directory.path.join("debug-info");
    recorder.set_active(true, slot_ms - 1);
    let samples = [0.125, -0.75, 0.33];
    record_audio(&recorder, slot, &samples);
    recorder.log(slot_ms - 1, format_args!("before capture"));
    recorder.log(slot_ms + 1, format_args!("in slot"));
    assert!(!root.exists());

    recorder.request(boundary_ms + 1, Some("127.0.0.1:2237".into()));
    let message = "CQ K1ABC FN42";
    recorder.local(
        boundary_ms + 1100,
        &DecodeRecord {
            timestamp: slot_timestamp(slot),
            row: StreamDecodedMessage {
                freq: 1200.0,
                dt: 0.3,
                snr: -17.0,
                sync: 1.0,
                msg: message.into(),
                itone: [0; 79],
            },
            provenance: StreamDecodeProvenance::Experiment,
            stage: DecodeStage::Final,
        },
    );
    let udp = ExternalDecode {
        ms_since_midnight: slot_timestamp(slot).milliseconds_since_midnight(),
        snr: -19,
        dt: 0.4,
        freq: 1201,
        message: message.into(),
    };
    let peer = "127.0.0.1:2238".parse().unwrap();
    recorder.record_udp(boundary_ms + 1900, peer, &udp);
    recorder.record_udp(boundary_ms + GRACE_MS + 1, peer, &udp);
    recorder.log(boundary_ms + 2000, format_args!("last-slot worker done"));
    recorder.log(
        boundary_ms + GRACE_MS + 1,
        format_args!("outside export window"),
    );

    let export = recorder
        .ready(Instant::now() + Duration::from_secs(11))
        .unwrap();
    let saved = export.write(&root).unwrap().unwrap();
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    let audio_path = saved
        .join("audio")
        .join(format!("{}.wav", slot_timestamp(slot)));
    assert_eq!(
        ft8rs::input::file::infer_start_time_from_path(&audio_path),
        Some(slot_timestamp(slot))
    );
    let audio = ft8rs::input::audio::read_wav_mono_f32(&audio_path).unwrap();
    assert_eq!(audio.samples, samples);
    assert_eq!(audio.sample_rate, AUDIO_RATE);

    let read_record = |name| -> Value {
        let content = fs::read_to_string(saved.join(name)).unwrap();
        assert_eq!(content.lines().count(), 1);
        serde_json::from_str(&content).unwrap()
    };
    let local = read_record("local-decodes.jsonl");
    assert_eq!(local["message"], message);
    assert_eq!(local["provenance"], "Experiment");
    let external = read_record("udp-decodes.jsonl");
    assert_eq!(external["slot_utc"], slot_timestamp(slot).format());
    assert_eq!(external["received_at_unix_ms"], boundary_ms + 1900);
    assert_eq!(external["peer"], "127.0.0.1:2238");
    let logs = fs::read_to_string(saved.join("app.log")).unwrap();
    assert_eq!(logs, "in slot\nlast-slot worker done\n");
    let manifest: Value =
        serde_json::from_reader(File::open(saved.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["slots"][0]["audio"]["device"], "test input");
    assert_eq!(manifest["slots"][0]["audio"]["capture_xruns"], 2);
    assert_eq!(manifest["udp_listener_at_save"], "127.0.0.1:2237");
}

#[test]
fn failed_export_preserves_capture_and_cancelled_export_never_publishes() {
    let directory = test_directory();
    let root = directory.path.join("debug-info");
    let recorder = Recorder::default();
    recorder.set_active(true, MIDNIGHT * SLOT_MS - 1);
    record_audio(&recorder, MIDNIGHT, &[0.125]);
    let at_ms = (MIDNIGHT + 2) * SLOT_MS;

    File::create(&root).unwrap();
    recorder.request(at_ms, None);
    assert!(recorder
        .ready(Instant::now())
        .unwrap()
        .write(&root)
        .is_err());
    fs::remove_file(&root).unwrap();
    recorder.request(at_ms, None);
    let saved = recorder
        .ready(Instant::now())
        .unwrap()
        .write(&root)
        .unwrap()
        .unwrap();
    recorder.request(at_ms, None);
    let cancelled = recorder.ready(Instant::now()).unwrap();
    recorder.set_active(false, at_ms);
    assert_eq!(cancelled.write(&root).unwrap(), None);
    assert!(saved.join("manifest.json").is_file());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn exported_recording_replays_the_same_decode_rows() {
    use ft8rs::input::file::{decode_wav_file, FileDecodeOptions};

    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ft8rs-core/tests/ft8/210703_133430.wav");
    let original = ft8rs::input::audio::read_wav_mono_f32(&fixture).unwrap();
    assert_eq!(original.sample_rate, AUDIO_RATE);
    let recorder = Recorder::default();
    recorder.set_active(true, MIDNIGHT * SLOT_MS - 1);
    record_audio(&recorder, MIDNIGHT, &original.samples);
    recorder.request((MIDNIGHT + 2) * SLOT_MS, None);
    let directory = test_directory();
    let saved = recorder
        .ready(Instant::now())
        .unwrap()
        .write(&directory.path)
        .unwrap()
        .unwrap();
    let timestamp = slot_timestamp(MIDNIGHT);
    let exported = saved.join("audio").join(format!("{timestamp}.wav"));
    let options = FileDecodeOptions {
        start_time: timestamp,
        config: StreamDecodeConfig::default(),
    };
    let rows = |path| {
        decode_wav_file(path, options.clone())
            .unwrap()
            .into_iter()
            .map(|row| json!([row.timestamp.format(), row.decode]))
            .collect::<Vec<_>>()
    };
    let baseline = rows(fixture);
    assert!(!baseline.is_empty());
    assert_eq!(rows(exported), baseline);
}
