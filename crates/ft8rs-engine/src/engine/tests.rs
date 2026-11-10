use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant};

use ft8rs::input::audio::resample_linear;
use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig};
use ft8rs::SlotTimestamp;

use crate::protocol::{DecodeStage, EngineEvent};
use crate::soundcard::{PeriodAccumulator, SlotStage};

use super::actor::spawn_decode_actor;
use super::monitor::{capture_stage_submitted, CaptureClock, ClockAction};
use super::DecodeCmd;

const TARGET_SAMPLE_RATE: u32 = 12_000;
use ft8rs::stream::session::StreamDecodeProvenance;

const NZHSYM_STRIDE: usize = 3456;
const SHORT_FIXTURE: &str = "../ft8rs-core/tests/ft8/210703_133430.wav";

fn load_fixture_12k() -> Vec<f32> {
    let audio = ft8rs::input::audio::read_wav_mono_f32(SHORT_FIXTURE)
        .expect("versioned short recording must be readable");
    resample_linear(&audio.samples, audio.sample_rate, TARGET_SAMPLE_RATE)
}

#[test]
fn capture_clock_detects_whole_slot_steps_and_ignores_normal_boundaries() {
    let base = Instant::now();
    for step_ms in [-15_000, 15_000] {
        let mut clock = CaptureClock::default();
        let mut reported = None;
        let mut observe = |elapsed, wall| {
            clock.observe(base + Duration::from_millis(elapsed), wall, &mut reported)
        };
        assert!(matches!(observe(0, 29_990), ClockAction::Accept));
        assert!(matches!(observe(20, 30_010), ClockAction::Accept));
        let stepped = 30_030 + step_ms;
        assert!(matches!(observe(40, stepped), ClockAction::Restart));
        assert!(matches!(observe(60, stepped + 20), ClockAction::Wait));
        assert!(matches!(
            observe(15_010, stepped + 14_970),
            ClockAction::Aligned
        ));
        assert!(matches!(
            observe(15_030, stepped + 14_990),
            ClockAction::Accept
        ));
    }
}

#[test]
fn capture_clock_drops_queued_audio_and_does_not_apply_a_reported_step_twice() {
    let base = Instant::now();
    let start = base + Duration::from_millis(10);
    let complete = base + Duration::from_millis(20);
    let mut clock = CaptureClock::default();
    let mut reported = Some((start, complete));
    assert!(matches!(
        clock.observe(base, 15_005, &mut reported),
        ClockAction::Wait
    ));
    assert!(matches!(
        clock.observe(complete, 15_015, &mut reported),
        ClockAction::Restart
    ));
    assert!(reported.is_none());
    reported = Some((start, complete));
    assert!(matches!(
        clock.observe(complete + Duration::from_millis(1), 15_016, &mut reported),
        ClockAction::Wait
    ));
    assert!(reported.is_none());
}

#[test]
fn first_complete_slot_after_realigning_reproduces_decode_rows() {
    type Row = (String, u64, u64, u64, DecodeStage, StreamDecodeProvenance);
    let source = load_fixture_12k();
    let base = Instant::now();
    let decode = |stepped: bool| {
        let mut clock = CaptureClock::default();
        let mut reported = None;
        if stepped {
            clock.observe(base, 60_000, &mut reported);
            assert!(matches!(
                clock.observe(base + Duration::from_millis(20), 15_020, &mut reported),
                ClockAction::Restart
            ));
        }
        let mut acc = PeriodAccumulator::new(TARGET_SAMPLE_RATE);
        let mut stages = Vec::new();
        for (index, chunk) in source.chunks(240).enumerate() {
            let elapsed = index as u64 * 20;
            let wall = 30_000 + elapsed as i64;
            assert!(matches!(
                clock.observe(
                    base + Duration::from_millis(15_000 + elapsed),
                    wall,
                    &mut reported
                ),
                ClockAction::Accept | ClockAction::Aligned
            ));
            stages.extend(acc.push(chunk, wall).stages);
        }
        stages.extend(acc.flush().stages);

        let (input, output) = spawn_decode_actor(StreamDecodeConfig::default());
        let started = Instant::now();
        for (stage, timestamp, samples_12k) in stages {
            input
                .send(DecodeCmd::Stage {
                    stage,
                    timestamp,
                    samples_12k,
                    submitted: started,
                    recovery_deadline: None,
                    restore_a7: None,
                    restore_jtdx: [None, None],
                    supervised: false,
                })
                .unwrap();
        }
        let mut rows = Vec::<Row>::new();
        let mut first = None;
        loop {
            match output.recv_timeout(Duration::from_secs(10)).unwrap() {
                EngineEvent::Decode(record) => {
                    first.get_or_insert_with(|| started.elapsed());
                    rows.push((
                        record.row.msg,
                        record.row.freq.to_bits(),
                        record.row.snr.to_bits(),
                        record.row.dt.to_bits(),
                        record.stage,
                        record.provenance,
                    ));
                }
                EngineEvent::SlotComplete { .. } => break,
                EngineEvent::Error(err) => panic!("decode: {err}"),
                _ => {}
            }
        }
        eprintln!(
            "clock step {stepped}: first callback {:?}, slot complete {:?}",
            first.unwrap(),
            started.elapsed()
        );
        rows
    };
    let baseline = decode(false);
    assert_eq!(decode(true), baseline);
}

#[test]
fn capture_queue_age_is_included_in_the_decode_deadline() {
    let now = Instant::now();
    assert_eq!(
        capture_stage_submitted(now, 25_000, 10_000),
        now - Duration::from_secs(15)
    );
    assert_eq!(capture_stage_submitted(now, 10_000, 25_000), now);
}

fn feed_slot(cmd_tx: &SyncSender<DecodeCmd>, ts: &SlotTimestamp, s12k: &[f32]) {
    let slice = |n: usize| s12k[..(n * NZHSYM_STRIDE).min(s12k.len())].to_vec();
    for (n, stage) in [
        (41, SlotStage::N41),
        (47, SlotStage::N47),
        (50, SlotStage::N50),
    ] {
        let samples = if n == 50 { s12k.to_vec() } else { slice(n) };
        cmd_tx
            .send(DecodeCmd::Stage {
                stage,
                timestamp: ts.clone(),
                samples_12k: samples,
                submitted: Instant::now(),
                recovery_deadline: None,
                restore_a7: None,
                restore_jtdx: [None, None],
                supervised: false,
            })
            .unwrap();
    }
}

fn drain_until_slot_complete(rx: &Receiver<EngineEvent>) -> (usize, usize) {
    let mut decodes = 0usize;
    loop {
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(EngineEvent::Decode(_)) => decodes += 1,
            Ok(EngineEvent::SlotComplete { count, .. }) => return (decodes, count),
            Ok(EngineEvent::Error(err)) => panic!("decode actor error: {err}"),
            Ok(_) => {}
            Err(err) => panic!("timed out waiting for slot complete: {err}"),
        }
    }
}

#[test]
fn decode_actor_reproduces_wsjtx_short_slot_and_reconfigures() {
    let s12k = load_fixture_12k();
    let ts = SlotTimestamp::parse("210703_133430").unwrap();

    let (cmd_tx, evt_rx) = spawn_decode_actor(StreamDecodeConfig::default());

    // wsjtx staged decode of the short fixture.
    feed_slot(&cmd_tx, &ts, &s12k);
    let (_early_or_final, count) = drain_until_slot_complete(&evt_rx);
    assert!(
        count >= 15,
        "wsjtx short slot decoded too few rows: {count}"
    );

    // Reconfigure to jtdx in place (hash migrated) and decode again — the
    // actor must survive the rebuild and keep producing.
    let jtdx = StreamDecodeConfig {
        profile: DecodeProfile::Jtdx,
        ..Default::default()
    };
    cmd_tx
        .send(DecodeCmd::Reconfigure {
            config: jtdx,
            reset_dx_target: false,
            reset_dx_operator: false,
        })
        .unwrap();
    feed_slot(&cmd_tx, &ts, &s12k);
    let (_d2, count2) = drain_until_slot_complete(&evt_rx);
    assert!(
        count2 >= 15,
        "jtdx short slot decoded too few rows: {count2}"
    );

    cmd_tx.send(DecodeCmd::Stop).unwrap();
}

/// The hybrid decode actor (staged N41/N47/N50), over the LONG fixture where
/// hybrid genuinely exceeds the WSJT-X baseline (jtdx-unique rows exist), must
/// emit the same total as a one-shot hybrid decode — i.e. it must not drop
/// jtdx-unique rows. The short fixture can't catch this (there hybrid==wsjtx,
/// jtdx fully overlaps). Slow; run with --ignored.
#[test]
#[ignore = "slow long-fixture actor gate; run with --release --ignored"]
fn decode_actor_hybrid_long_emits_jtdx_unique() {
    use ft8rs::stream::profile::ProfileStreamDecodeSession;
    const LONG_FIXTURE: &str = "../ft8rs-core/tests/ft8/230208_140300.wav";
    let audio = ft8rs::input::audio::read_wav_mono_f32(LONG_FIXTURE).unwrap();
    let s12k = resample_linear(&audio.samples, audio.sample_rate, TARGET_SAMPLE_RATE);
    let sps = 15 * TARGET_SAMPLE_RATE as usize;
    let nseg = s12k.len().div_ceil(sps);
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let hyb = || StreamDecodeConfig {
        profile: DecodeProfile::Hybrid,
        ..Default::default()
    };

    let mut oneshot = ProfileStreamDecodeSession::new(hyb());
    let mut oneshot_total = 0usize;
    for seg in 0..nseg {
        let ts = start.add_seconds((seg * 15) as i64);
        let data = &s12k[seg * sps..((seg + 1) * sps).min(s12k.len())];
        oneshot_total += oneshot.decode_slot_at(&ts, data).len();
    }

    let (cmd_tx, evt_rx) = spawn_decode_actor(hyb());
    let mut actor_emitted = 0usize;
    for seg in 0..nseg {
        let ts = start.add_seconds((seg * 15) as i64);
        let data = &s12k[seg * sps..((seg + 1) * sps).min(s12k.len())];
        feed_slot(&cmd_tx, &ts, data);
        let (emitted, _count) = drain_until_slot_complete(&evt_rx);
        actor_emitted += emitted;
    }
    cmd_tx.send(DecodeCmd::Stop).unwrap();

    assert!(
        oneshot_total > 424,
        "sanity: long hybrid should exceed wsjtx baseline (jtdx-unique exists), got {oneshot_total}"
    );
    assert_eq!(
        actor_emitted, oneshot_total,
        "hybrid actor emitted {actor_emitted} rows over the long fixture but one-shot has {oneshot_total} (jtdx-unique dropped in the staged engine path?)"
    );
}

/// The hybrid decode actor (staged N41/N47/N50) must emit the same rows as a
/// one-shot hybrid decode of the same slot — in particular it must not drop
/// JTDX-unique rows. Guards the monitor early-decode staging.
#[test]
fn decode_actor_hybrid_matches_oneshot() {
    use ft8rs::stream::profile::ProfileStreamDecodeSession;
    let s12k = load_fixture_12k();
    let ts = SlotTimestamp::parse("210703_133430").unwrap();
    let hybrid_cfg = || StreamDecodeConfig {
        profile: DecodeProfile::Hybrid,
        ..Default::default()
    };

    // Reference: one-shot hybrid decode (file path) of the whole slot.
    let mut oneshot = ProfileStreamDecodeSession::new(hybrid_cfg());
    let oneshot_count = oneshot.decode_slot_at(&ts, &s12k).len();

    // Actor: staged N41/N47/N50 exactly as the monitor engine drives it.
    let (cmd_tx, evt_rx) = spawn_decode_actor(hybrid_cfg());
    feed_slot(&cmd_tx, &ts, &s12k);
    let (emitted, count) = drain_until_slot_complete(&evt_rx);
    cmd_tx.send(DecodeCmd::Stop).unwrap();

    assert_eq!(
        count, oneshot_count,
        "hybrid actor slot count {count} != one-shot {oneshot_count} (jtdx-unique dropped?)"
    );
    assert_eq!(
        emitted, oneshot_count,
        "hybrid actor emitted {emitted} rows but slot count is {oneshot_count}"
    );
}
