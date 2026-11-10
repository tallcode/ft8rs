use std::io::BufReader;
use std::process::Command;

use ft8rs::stream::profile::same_signal;
use ft8rs::{SlotTimestamp, StreamDecodedMessage};

use crate::protocol::DecodeRecord;

use super::child::worker_main;
use super::ipc::{event, read_frame, response, write_frame};
use super::*;
use ft8rs::stream::profile::ProfileStreamDecodeSession;
use ft8rs::stream::session::StreamDecodeProvenance;

const HANG_RECOVERY_ROWS: [StreamDecodeProvenance; 5] = [
    StreamDecodeProvenance::Unknown,
    StreamDecodeProvenance::CooperativeRecovery,
    StreamDecodeProvenance::PlusCompatibility,
    StreamDecodeProvenance::CandidateRetry,
    StreamDecodeProvenance::Experiment,
];

#[test]
fn hybrid_supplemental_rows_are_displayed_without_seeding_a7() {
    let config = StreamDecodeConfig {
        profile: DecodeProfile::Hybrid,
        ..Default::default()
    };
    let mut delivery = SlotDelivery::default();
    let mut history = WorkerHistory::default();
    let (events, received) = mpsc::channel();
    let timestamp = SlotTimestamp::parse("261030_120000").unwrap();
    for (message, provenance) in [
        ("CQ K1ABC FN42", StreamDecodeProvenance::Unknown),
        ("CQ W9XYZ EN50", StreamDecodeProvenance::CandidateRetry),
        ("CQ W1AW FN31", StreamDecodeProvenance::PlusCompatibility),
        ("CQ N0CAL EM48", StreamDecodeProvenance::CooperativeRecovery),
        ("CQ K7ABC CN87", StreamDecodeProvenance::Experiment),
    ] {
        delivery
            .forward(
                Response::Decode {
                    timestamp: timestamp.format(),
                    freq: 1_200.0,
                    dt: 0.2,
                    snr: -18.0,
                    sync: 1.0,
                    message: message.into(),
                    tones: vec![0; 79],
                    provenance,
                    stage: DecodeStage::Final,
                },
                &mut history,
                &config,
                &events,
            )
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            EngineEvent::Decode(record) if record.provenance == provenance
        ));
    }
    let snapshot = ron::to_string(history.a7_for_slot(&timestamp)).unwrap();
    assert!(snapshot.contains("CQ K1ABC FN42"));
    for call in ["W9XYZ", "W1AW", "N0CAL", "K7ABC"] {
        assert!(!snapshot.contains(call));
    }
}

fn fixture(mode: &str) -> Result<Worker, String> {
    if mode == "native" {
        if let Some(executable) = std::env::var_os("FT8RS_WORKER_EXECUTABLE") {
            let mut command = Command::new(executable);
            command.arg(WORKER_ARGUMENT);
            return Worker::start_command(command);
        }
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "worker::tests::worker_fixture",
            "--nocapture",
        ])
        .env("FT8RS_WORKER_TEST_FIXTURE", mode);
    Worker::start_command(command)
}

/// Park the fixture after it has emitted a known baseline row so the
/// supervisor can terminate a worker with an unfinished slot.
pub(crate) fn pause_fixture_after_confirmation(timestamp: &SlotTimestamp, message: &str) {
    if timestamp.format() == "230208_140300"
        && std::env::var("FT8RS_WORKER_TEST_FIXTURE")
            .is_ok_and(|mode| mode.strip_prefix("pause-baseline:") == Some(message))
    {
        loop {
            thread::park();
        }
    }
}

#[test]
#[ignore = "entry point for the subprocess fault fixtures"]
fn worker_fixture() {
    let Ok(mode) = std::env::var("FT8RS_WORKER_TEST_FIXTURE") else {
        return;
    };
    if mode == "native" || mode.starts_with("pause-baseline:") {
        worker_main().unwrap();
        return;
    }
    let mut input = BufReader::new(std::io::stdin());
    assert!(matches!(
        read_frame::<Request>(&mut input).unwrap(),
        Some(Request::Init(_))
    ));
    while let Some(Request::Stage {
        id,
        timestamp,
        stage,
        ..
    }) = read_frame(&mut input).unwrap()
    {
        write_frame(
            &mut std::io::stdout().lock(),
            &Response::Decode {
                timestamp: timestamp.clone(),
                freq: if mode == "cumulative" && stage == SlotStage::N47 {
                    1150.0
                } else {
                    1137.0
                },
                dt: 2.4,
                snr: -8.0,
                sync: 1.0,
                message: "JA8IIK/P SV9GPV -18".into(),
                tones: vec![0; 79],
                provenance: ft8rs::stream::session::StreamDecodeProvenance::Regular,
                stage: DecodeStage::Final,
            },
        )
        .unwrap();
        if timestamp.ends_with("120000") && (mode != "cumulative" || stage == SlotStage::N50) {
            if mode == "hang-multi" {
                for (index, provenance) in HANG_RECOVERY_ROWS.into_iter().enumerate() {
                    write_frame(
                        &mut std::io::stdout().lock(),
                        &Response::Decode {
                            timestamp: timestamp.clone(),
                            freq: 1200.0 + 10.0 * index as f64,
                            dt: 0.1,
                            snr: -8.0,
                            sync: 1.0,
                            message: "CQ K1ABC FN42".into(),
                            tones: vec![0; 79],
                            provenance,
                            stage: DecodeStage::Final,
                        },
                    )
                    .unwrap();
                }
            }
            // An intentionally uncooperative CPU task. It can only stop
            // when its containing process is terminated by the watchdog.
            loop {
                std::hint::black_box(17_u64.wrapping_mul(31));
            }
        }
        if mode == "cumulative" {
            thread::sleep(Duration::from_millis(400));
        }
        if stage == SlotStage::N50 {
            write_frame(
                &mut std::io::stdout().lock(),
                &Response::SlotComplete {
                    timestamp,
                    count: 1,
                },
            )
            .unwrap();
        }
        write_frame(&mut std::io::stdout().lock(), &Response::Done(id)).unwrap();
    }
}

fn stage(time: &str, stage: SlotStage, samples: Vec<f32>) -> DecodeCmd {
    DecodeCmd::Stage {
        timestamp: SlotTimestamp::parse(time).unwrap(),
        stage,
        samples_12k: samples,
        submitted: Instant::now(),
        recovery_deadline: None,
        restore_a7: None,
        restore_jtdx: [None, None],
        supervised: false,
    }
}

#[test]
fn wsjtx_receive_target_change_preserves_native_a7() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ft8rs-core/tests/ft8/230208_140300.wav");
    let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
    let config = StreamDecodeConfig::default();
    let mut retuned = config.clone();
    retuned.nfqso = 1100.0;
    retuned.hiscall = Some("W1AW".into());
    retuned.hisgrid = Some("FN31".into());
    let (tx, commands) = mpsc::channel();
    let (events, results) = mpsc::channel();
    let supervisor = thread::spawn(move || {
        supervise(config, commands, events, || fixture("native"), LIVE_BUDGET);
    });

    let mut recovered = false;
    for (slot, timestamp) in [(0, "230208_140300"), (2, "230208_140330")] {
        let start = slot * 180_000;
        for (kind, len) in [
            (SlotStage::N41, 141_696),
            (SlotStage::N47, 162_432),
            (SlotStage::N50, 180_000),
        ] {
            tx.send(stage(
                timestamp,
                kind,
                audio.samples[start..start + len].to_vec(),
            ))
            .unwrap();
        }
        loop {
            match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                EngineEvent::Decode(record) => {
                    recovered |= slot == 2
                        && record.row.msg == "CQ R6KEE KN75"
                        && record.provenance == StreamDecodeProvenance::A7Memory;
                }
                EngineEvent::SlotComplete { .. } => break,
                EngineEvent::Error(error) => panic!("live worker failed: {error}"),
                _ => {}
            }
        }
        if slot == 0 {
            tx.send(DecodeCmd::Reconfigure {
                config: retuned.clone(),
                reset_dx_target: false,
                reset_dx_operator: false,
            })
            .unwrap();
        }
    }
    tx.send(DecodeCmd::Stop).unwrap();
    supervisor.join().unwrap();
    assert!(recovered, "the receive target change dropped the A7 seed");
}

#[test]
fn killed_hybrid_worker_preserves_rows_and_restores_a7_thirty_seconds_later() {
    use ft8rs::stream::session::StreamDecodeProvenance::A7Memory;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ft8rs-core/tests/ft8/230208_140300.wav");
    let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let config = StreamDecodeConfig {
        profile: DecodeProfile::Hybrid,
        cooperative_realtime: true,
        ..Default::default()
    };
    let (tx, commands) = mpsc::channel();
    let (events, results) = mpsc::channel();
    let supervisor = thread::spawn(move || {
        let mut starts = 0;
        supervise(
            config,
            commands,
            events,
            || {
                let mut child = fixture("native")?;
                if starts == 0 {
                    child.kill_at_checkpoint = Some("230208_140300");
                }
                starts += 1;
                Ok(child)
            },
            LIVE_BUDGET,
        );
        starts
    });

    let (slots, _) = audio.samples.as_chunks::<180_000>();
    for (index, samples) in slots.iter().take(3).enumerate() {
        let timestamp = start.add_seconds(index as i64 * 15);
        for (kind, len) in [
            (SlotStage::N41, 141696),
            (SlotStage::N47, 162432),
            (SlotStage::N50, 180000),
        ] {
            tx.send(stage(&timestamp.format(), kind, samples[..len].to_vec()))
                .unwrap();
        }
        let mut rows: Vec<DecodeRecord> = Vec::new();
        loop {
            match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                EngineEvent::Decode(record) => {
                    assert_eq!(record.timestamp, timestamp);
                    assert!(
                        !rows.iter().any(|old| old.row.msg == record.row.msg
                            && (old.row.freq - record.row.freq).abs() <= 5.0
                            && (old.row.dt - record.row.dt).abs() <= 0.3),
                        "duplicate row: {record:?}"
                    );
                    rows.push(record);
                }
                EngineEvent::SlotComplete {
                    timestamp: completed,
                    count,
                } => {
                    assert_eq!(completed, timestamp);
                    assert_eq!(count, rows.len());
                    break;
                }
                EngineEvent::Error(error) => panic!("live worker failed: {error}"),
                _ => {}
            }
        }
        if index == 0 {
            assert!(rows.iter().any(|row| row.row.msg == "PD2RLD R6KEE KN75"));
        }
        if index == 2 {
            assert!(rows
                .iter()
                .any(|row| row.row.msg == "CQ R6KEE KN75" && row.provenance == A7Memory));
        }
    }
    tx.send(DecodeCmd::Stop).unwrap();
    assert_eq!(
        supervisor.join().unwrap(),
        2,
        "one real child must have been replaced"
    );
    assert!(results.try_recv().is_err());
}

#[test]
fn killed_in_base_search_keeps_confirmations_and_restores_native_history() {
    use StreamDecodeProvenance::{A7Memory, JtdxDeep};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ft8rs-core/tests/ft8/230208_140300.wav");
    let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    for (profile, seed, recovered, provenance) in [
        (
            DecodeProfile::Wsjtx,
            "PD2RLD R6KEE KN75",
            "CQ R6KEE KN75",
            A7Memory,
        ),
        (
            DecodeProfile::Hybrid,
            "PD2RLD R6KEE KN75",
            "CQ R6KEE KN75",
            A7Memory,
        ),
        (
            DecodeProfile::Jtdx,
            "CQ D1DX KN87",
            "CQ D1DX KN87",
            JtdxDeep,
        ),
    ] {
        let config = StreamDecodeConfig {
            profile,
            cooperative_realtime: true,
            ..Default::default()
        };
        let mut cold = ProfileStreamDecodeSession::new(config.clone());
        assert!(
            !cold
                .decode_slot_at(&start.add_seconds(30), &audio.samples[360_000..540_000])
                .iter()
                .any(|row| row.msg == recovered),
            "{profile:?} control unexpectedly decoded {recovered}"
        );
        let (tx, commands) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let supervisor = thread::spawn(move || {
            let mut starts = 0;
            supervise(
                config,
                commands,
                events,
                || {
                    let child = if starts == 0 {
                        let mut child = fixture(&format!("pause-baseline:{seed}"))?;
                        child.kill_at_baseline_row = Some(seed);
                        child
                    } else {
                        fixture("native")?
                    };
                    starts += 1;
                    // The first child parks inside the actual confirmation callback.
                    // The supervisor always terminates and reaps it.
                    Ok(child)
                },
                LIVE_BUDGET,
            );
            starts
        });

        let (slots, _) = audio.samples.as_chunks::<180_000>();
        for (index, samples) in slots.iter().take(3).enumerate() {
            let timestamp = start.add_seconds(index as i64 * 15);
            for (kind, len) in [
                (SlotStage::N41, 141696),
                (SlotStage::N47, 162432),
                (SlotStage::N50, 180000),
            ] {
                let input = if index == 0 && kind == SlotStage::N41 {
                    // Ensure the classic seed is confirmed in the N50 search.
                    vec![0.0; len]
                } else {
                    samples[..len].to_vec()
                };
                tx.send(stage(&timestamp.format(), kind, input)).unwrap();
            }
            let mut rows: Vec<DecodeRecord> = Vec::new();
            loop {
                match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                    EngineEvent::Decode(record) => {
                        assert_eq!(record.timestamp, timestamp);
                        assert!(!rows.iter().any(|old| same_signal(&old.row, &record.row)));
                        rows.push(record);
                    }
                    EngineEvent::SlotComplete {
                        timestamp: completed,
                        count,
                    } => {
                        assert_eq!(completed, timestamp);
                        assert_eq!(count, rows.len());
                        break;
                    }
                    EngineEvent::Error(error) => panic!("{profile:?}: {error}"),
                    _ => {}
                }
            }
            if index == 0 {
                assert!(rows
                    .iter()
                    .any(|row| row.row.msg == seed && row.stage == DecodeStage::Final));
            }
            if index == 2 {
                assert!(
                    rows.iter()
                        .any(|row| row.row.msg == recovered && row.provenance == provenance),
                    "{profile:?} lost history for {recovered}"
                );
            }
        }
        tx.send(DecodeCmd::Stop).unwrap();
        assert_eq!(
            supervisor.join().unwrap(),
            2,
            "{profile:?} did not replace exactly one worker"
        );
        assert!(results.try_recv().is_err());
    }
}

#[test]
fn hybrid_uses_a_jtdx_only_confirmation_as_a7_seed() {
    use ft8rs::stream::profile::SlotRecovery;

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ft8rs-core/tests/ft8/230208_140300.wav");
    let audio = ft8rs::input::audio::read_wav_mono_f32(path).unwrap();
    let seed_slot = SlotTimestamp::parse("230208_140300").unwrap();
    let next = seed_slot.add_seconds(30);
    let config = StreamDecodeConfig {
        profile: DecodeProfile::Hybrid,
        cooperative_realtime: true,
        ..Default::default()
    };
    let mut peer = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Jtdx,
        ..config.clone()
    });
    let mut history = WorkerHistory::default();
    let mut delivery = SlotDelivery::default();
    let (events, received) = mpsc::channel();
    let mut checkpoint = |branch, update, publish| {
        if matches!(&update, JtdxCheckpoint::Row(entry)
            if entry.row.decode.msg == "PD2RLD R6KEE KN75")
        {
            delivery
                .forward(
                    Response::JtdxCheckpoint {
                        timestamp: seed_slot.format(),
                        branch,
                        update,
                        publish,
                    },
                    &mut history,
                    &config,
                    &events,
                )
                .unwrap();
        }
    };
    let state = peer.start_slot();
    peer.finish_slot_nzhsym50(
        &seed_slot,
        state,
        &audio.samples[..180_000],
        SlotRecovery {
            jtdx_checkpoint: Some(&mut checkpoint),
            ..Default::default()
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(received.try_iter().count(), 1);

    let mut hybrid = ProfileStreamDecodeSession::new(config);
    hybrid.restore_a7(&next, history.a7[parity(&next)].take().unwrap().1);
    let mut decoded = false;
    hybrid
        .decode_slot_streaming_with_provenance_at(&next, &audio.samples[360_000..540_000], |row| {
            decoded |= row.decode.msg == "CQ R6KEE KN75"
                && row.provenance == StreamDecodeProvenance::A7Memory;
            Ok(())
        })
        .unwrap();
    assert!(
        decoded,
        "the WSJT-X branch must be able to use a JTDX-only seed"
    );
}

#[test]
fn deadline_silently_kills_cpu_work_preserves_callbacks_and_recovers_next_slot() {
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    let thread = thread::spawn(move || {
        supervise(
            StreamDecodeConfig::default(),
            rx,
            events,
            || fixture("hang-multi"),
            Duration::from_millis(700),
        )
    });
    let start = Instant::now();
    tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
        .unwrap();
    let mut decoded = Vec::new();
    loop {
        match results.recv_timeout(Duration::from_secs(3)).unwrap() {
            EngineEvent::Decode(record) => decoded.push(record.provenance),
            EngineEvent::Error(message) => {
                panic!("normal deadline must not show a GUI error: {message}");
            }
            EngineEvent::SlotComplete { count, .. } => {
                assert_eq!(count, 1 + HANG_RECOVERY_ROWS.len());
                break;
            }
            _ => {}
        }
    }
    assert_eq!(decoded[0], StreamDecodeProvenance::Regular);
    assert_eq!(decoded[1..], HANG_RECOVERY_ROWS);
    assert!(start.elapsed() < Duration::from_secs(2));
    tx.send(stage("261030_120015", SlotStage::N50, vec![0.0; 32]))
        .unwrap();
    loop {
        match results.recv_timeout(Duration::from_secs(3)).unwrap() {
            EngineEvent::Decode(record) => assert_eq!(record.timestamp.format_time(), "120015"),
            EngineEvent::SlotComplete { timestamp, .. } => {
                assert_eq!(timestamp.format_time(), "120015");
                break;
            }
            EngineEvent::Error(message) => panic!("next slot failed: {message}"),
            _ => {}
        }
    }
    tx.send(DecodeCmd::Stop).unwrap();
    thread.join().unwrap();
}

#[test]
fn stop_preempts_an_uncooperative_decoder() {
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    let thread = thread::spawn(move || {
        supervise(
            StreamDecodeConfig::default(),
            rx,
            events,
            || fixture("hang"),
            LIVE_BUDGET,
        )
    });
    tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
        .unwrap();
    assert!(matches!(
        results.recv_timeout(Duration::from_secs(3)).unwrap(),
        EngineEvent::Decode(_)
    ));
    let start = Instant::now();
    tx.send(DecodeCmd::Stop).unwrap();
    thread.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn budget_is_cumulative_across_early_and_final_stages() {
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    let thread = thread::spawn(move || {
        supervise(
            StreamDecodeConfig::default(),
            rx,
            events,
            || fixture("cumulative"),
            Duration::from_millis(1200),
        )
    });
    for kind in [SlotStage::N41, SlotStage::N47] {
        tx.send(stage("261030_120000", kind, vec![0.0; 32]))
            .unwrap();
        assert!(matches!(
            results.recv_timeout(Duration::from_secs(3)).unwrap(),
            EngineEvent::Decode(_)
        ));
        // Each fake stage works for 400 ms. Submit the next capture stage
        // only after the previous one has finished, excluding queue age.
        thread::sleep(Duration::from_millis(450));
    }
    let final_start = Instant::now();
    tx.send(stage("261030_120000", SlotStage::N50, vec![0.0; 32]))
        .unwrap();
    loop {
        match results.recv_timeout(Duration::from_secs(3)).unwrap() {
            EngineEvent::SlotComplete { .. } => break,
            EngineEvent::Error(message) => {
                panic!("cumulative deadline must be silent: {message}")
            }
            _ => {}
        }
    }
    assert!(
        final_start.elapsed() < Duration::from_millis(800),
        "early-stage work must reduce the final-stage budget"
    );
    tx.send(DecodeCmd::Stop).unwrap();
    thread.join().unwrap();
}

#[test]
fn expired_queued_slots_are_skipped_without_spawning_native_work() {
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    tx.send(DecodeCmd::Stage {
        timestamp: SlotTimestamp::parse("261030_120000").unwrap(),
        stage: SlotStage::N41,
        samples_12k: vec![],
        submitted: Instant::now() - Duration::from_secs(2),
        recovery_deadline: None,
        restore_a7: None,
        restore_jtdx: [None, None],
        supervised: false,
    })
    .unwrap();
    tx.send(DecodeCmd::Stop).unwrap();
    supervise(
        StreamDecodeConfig::default(),
        rx,
        events,
        || panic!("expired work must not start a child"),
        Duration::from_millis(100),
    );
    let results: Vec<_> = results.try_iter().collect();
    assert!(!results
        .iter()
        .any(|event| matches!(event, EngineEvent::Error(_))));
    assert!(results
        .iter()
        .any(|event| matches!(event, EngineEvent::SlotComplete { count: 0, .. })));
}

#[test]
fn actual_worker_failures_still_reach_the_gui() {
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    tx.send(stage("261030_120000", SlotStage::N50, vec![]))
        .unwrap();
    tx.send(DecodeCmd::Stop).unwrap();
    supervise(
        StreamDecodeConfig::default(),
        rx,
        events,
        || Err("could not start decoder worker".into()),
        LIVE_BUDGET,
    );
    assert!(results.try_iter().any(|event| matches!(event,
        EngineEvent::Error(message) if message == "could not start decoder worker")));
}

#[test]
fn ipc_roundtrip_preserves_every_config_and_decode_field() {
    let config = StreamDecodeConfig {
        hybrid_experiment: true,
        mycall: Some("BG5ATV".into()),
        nfa: 201.123456789,
        nfb: 2999.123456789,
        ..Default::default()
    };
    let mut frame = Vec::new();
    write_frame(&mut frame, &Request::Init(config.clone())).unwrap();
    let Some(Request::Init(restored)) = read_frame(&mut std::io::Cursor::new(frame)).unwrap()
    else {
        panic!("wrong frame");
    };
    assert_eq!(format!("{config:?}"), format!("{restored:?}"));
    let record = DecodeRecord {
        timestamp: SlotTimestamp::parse("261030_120000").unwrap(),
        row: StreamDecodedMessage {
            freq: 1000.1234567890123,
            dt: -0.1234567890123,
            snr: -20.1234567890123,
            sync: 0.1234567890123,
            msg: "CQ K1ABC FN42".into(),
            itone: [7; 79],
        },
        provenance: ft8rs::stream::session::StreamDecodeProvenance::Experiment,
        stage: DecodeStage::Final,
    };
    let expected = format!("{record:?}");
    let mut frame = Vec::new();
    write_frame(&mut frame, &response(EngineEvent::Decode(record)).unwrap()).unwrap();
    let restored: Response = read_frame(&mut std::io::Cursor::new(frame))
        .unwrap()
        .unwrap();
    let Some(EngineEvent::Decode(restored)) = event(restored).unwrap() else {
        panic!("wrong event");
    };
    assert_eq!(expected, format!("{restored:?}"));
}

#[test]
#[ignore = "serial optimized 285-second recording comparison through the production worker"]
fn native_process_preserves_each_long_recording_message() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ft8rs-core/tests/ft8/230208_140300.wav");
    let samples = ft8rs::input::audio::read_wav_mono_f32(path)
        .unwrap()
        .samples;
    let config = StreamDecodeConfig {
        profile: ft8rs::DecodeProfile::Hybrid,
        ..Default::default()
    };
    let mut original = ProfileStreamDecodeSession::new(config.clone());
    let (tx, rx) = mpsc::channel();
    let (events, results) = mpsc::channel();
    let thread =
        thread::spawn(move || supervise(config, rx, events, || fixture("native"), LIVE_BUDGET));
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let mut retained = 0;
    for (index, samples) in samples.as_chunks::<180000>().0.iter().enumerate() {
        let timestamp = start.add_seconds(index as i64 * 15);
        let mut state = original.start_slot();
        let mut expected = Vec::new();
        original
            .decode_slot_nzhsym41_streaming_with_provenance(
                &timestamp,
                &mut state,
                &samples[..141696],
                |row| {
                    expected.push(format!("{row:?}"));
                    Ok(())
                },
            )
            .unwrap();
        original.subtract_slot_nzhsym47(&mut state, &samples[..162432]);
        original
            .decode_slot_nzhsym50_streaming_with_provenance(
                &timestamp,
                state,
                expected.len(),
                samples,
                |row| {
                    expected.push(format!("{row:?}"));
                    Ok(())
                },
            )
            .unwrap();
        for (kind, len) in [
            (SlotStage::N41, 141696),
            (SlotStage::N47, 162432),
            (SlotStage::N50, 180000),
        ] {
            tx.send(stage(&timestamp.format(), kind, samples[..len].to_vec()))
                .unwrap();
        }
        let mut actual = Vec::new();
        loop {
            match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                EngineEvent::Decode(record) => actual.push(format!(
                    "{:?}",
                    ft8rs::stream::session::StreamDecodedWithProvenance {
                        decode: record.row,
                        provenance: record.provenance
                    }
                )),
                EngineEvent::SlotComplete { .. } => break,
                EngineEvent::Error(error) => panic!("slot {index} failed: {error}"),
                _ => {}
            }
        }
        expected.sort();
        actual.sort();
        assert_eq!(
            actual, expected,
            "slot {index} lost or changed native output"
        );
        retained += actual.len();
    }
    tx.send(DecodeCmd::Stop).unwrap();
    thread.join().unwrap();
    assert!(
        retained >= 476,
        "long recording retained only {retained} rows"
    );
    eprintln!("production worker retained all {retained} native rows with exact fields");
}

#[test]
fn native_process_streaming_matches_in_process_hybrid() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ft8rs-core/tests/ft8/210703_133430.wav");
    let samples = ft8rs::input::audio::read_wav_mono_f32(path)
        .unwrap()
        .samples;
    for realtime in [false, true] {
        let config = StreamDecodeConfig {
            profile: ft8rs::DecodeProfile::Hybrid,
            cooperative_realtime: realtime,
            ..Default::default()
        };
        let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
        let mut session = ProfileStreamDecodeSession::new(config.clone());
        let mut state = session.start_slot();
        let mut expected = Vec::new();
        session
            .decode_slot_nzhsym41_streaming_with_provenance(
                &timestamp,
                &mut state,
                &samples[..141696],
                |row| {
                    expected.push(row.clone());
                    Ok(())
                },
            )
            .unwrap();
        session.subtract_slot_nzhsym47(&mut state, &samples[..162432]);
        session
            .decode_slot_nzhsym50_streaming_with_provenance(
                &timestamp,
                state,
                expected.len(),
                &samples,
                |row| {
                    expected.push(row.clone());
                    Ok(())
                },
            )
            .unwrap();
        let (tx, rx) = mpsc::channel();
        let (events, results) = mpsc::channel();
        let thread =
            thread::spawn(move || supervise(config, rx, events, || fixture("native"), LIVE_BUDGET));
        let mut actual = Vec::new();
        for (kind, len) in [
            (SlotStage::N41, 141696),
            (SlotStage::N47, 162432),
            (SlotStage::N50, 180000),
        ] {
            tx.send(stage("210703_133430", kind, samples[..len].to_vec()))
                .unwrap();
            // Do not serialize later-stage capture time as if it was old queued
            // work. Native capture actually submits stages at different times.
            if kind != SlotStage::N50 {
                thread::sleep(Duration::from_millis(40));
            }
        }
        loop {
            match results.recv_timeout(Duration::from_secs(15)).unwrap() {
                EngineEvent::Decode(record) => {
                    actual.push(ft8rs::stream::session::StreamDecodedWithProvenance {
                        decode: record.row,
                        provenance: record.provenance,
                    })
                }
                EngineEvent::SlotComplete { .. } => break,
                EngineEvent::Error(error) => panic!("native child failed: {error}"),
                _ => {}
            }
        }
        tx.send(DecodeCmd::Stop).unwrap();
        thread.join().unwrap();
        let sorted = |rows: Vec<ft8rs::stream::session::StreamDecodedWithProvenance>| {
            let mut rows: Vec<_> = rows.into_iter().map(|row| format!("{row:?}")).collect();
            rows.sort();
            rows
        };
        if realtime {
            // The first branch to report a signal chooses the representative.
            // IPC can affect scheduling, so compare unique signals here;
            // the wire test separately checks every encoded field.
            assert_eq!(actual.len(), expected.len());
            for wanted in &expected {
                assert!(
                    actual
                        .iter()
                        .any(|row| same_signal(&row.decode, &wanted.decode)),
                    "lost native signal: {:?}",
                    wanted.decode.msg
                );
            }
        } else {
            assert_eq!(sorted(actual), sorted(expected));
        }
    }
}
