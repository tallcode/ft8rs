//! Blocking CLI live monitoring and its in-process decode worker.
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use ft8rs::stream::profile::{ProfileSlotState, ProfileStreamDecodeSession};
use ft8rs::stream::session::{DecodeProfile, StreamDecodeConfig, StreamDecodedWithProvenance};
use ft8rs::stream::{SlotTimestamp, StreamDecodeSession, StreamDecodedMessage};

use super::capture::{start_input_stream, AudioCapture, CaptureRecvError, InputChannel};
use super::slots::{PeriodAccumulator, SlotStage};
use super::CAPTURE_TIMEOUT;

const CLI_STAGE_QUEUE_CAP: usize = 8;
const CLI_STAGE_MAX_AGE: Duration = Duration::from_secs(10);
const CAPTURE_POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct SoundcardDecodeOptions {
    pub device: Option<String>,
    pub channel: InputChannel,
    pub config: StreamDecodeConfig,
    pub max_slots: Option<usize>,
}

pub fn decode_soundcard_streaming_decodes<F, G>(
    options: SoundcardDecodeOptions,
    mut on_decode: F,
    on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedMessage) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    decode_soundcard_streaming_tagged(
        options,
        |time, row| on_decode(time, &row.decode),
        on_slot_complete,
    )
}

pub fn decode_soundcard_streaming_tagged<F, G>(
    options: SoundcardDecodeOptions,
    mut on_decode: F,
    mut on_slot_complete: G,
) -> Result<(), String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedWithProvenance) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    if options.max_slots == Some(0) {
        return Err("max_slots must be at least 1".to_owned());
    }
    let AudioCapture {
        stream, rx, format, ..
    } = start_input_stream(options.device.as_deref(), options.channel)?;
    let sample_rate = format.sample_rate;
    let (cmd_tx, event_rx) = start_decode_worker(options.config);

    // Feed each callback on the original UTC grid. If decoding falls behind,
    // fail explicitly rather than accumulate stale audio or skip partial stages.
    let mut acc = PeriodAccumulator::new(sample_rate);
    let mut slots_done = 0usize;
    let mut last_audio = Instant::now();
    loop {
        // Surface any decodes the worker has finished.
        drain_available_decode_events(&event_rx, &mut on_decode, &mut on_slot_complete)?;

        let chunk = match rx.recv_timeout(CAPTURE_POLL) {
            Ok(block) => block,
            Err(CaptureRecvError::Timeout) if last_audio.elapsed() < CAPTURE_TIMEOUT => continue,
            Err(CaptureRecvError::Timeout) => return Err("No audio received from input".to_owned()),
            Err(CaptureRecvError::Disconnected) => {
                return Err("Audio input disconnected".to_owned())
            }
            Err(CaptureRecvError::Failed(error)) => return Err(error),
        };
        last_audio = Instant::now();
        let out = acc.push(&chunk.samples, chunk.capture_ms);
        for (stage, timestamp, samples_12k) in out.stages {
            send_worker_command(&cmd_tx, worker_command(stage, timestamp, samples_12k))?;
        }
        if out.completed.is_some() {
            slots_done += 1;
            if options.max_slots.is_some_and(|max| slots_done >= max) {
                break;
            }
        }
    }

    // Flush the in-progress slot, then wait for the worker to drain and finish.
    for (stage, timestamp, samples_12k) in acc.flush().stages {
        send_worker_command(&cmd_tx, worker_command(stage, timestamp, samples_12k))?;
    }
    drop(stream);
    drop(cmd_tx);
    while let Ok(event) = event_rx.recv() {
        handle_decode_event(event, &mut on_decode, &mut on_slot_complete)?;
    }
    Ok(())
}

fn worker_command(
    stage: SlotStage,
    timestamp: SlotTimestamp,
    samples_12k: Vec<f32>,
) -> DecodeWorkerCommand {
    match stage {
        SlotStage::N41 => DecodeWorkerCommand::Nzhsym41 {
            timestamp,
            samples_12k,
        },
        SlotStage::N47 => DecodeWorkerCommand::Nzhsym47 {
            timestamp,
            samples_12k,
        },
        SlotStage::N50 => DecodeWorkerCommand::Nzhsym50 {
            timestamp,
            samples_12k,
        },
    }
}

enum DecodeWorkerCommand {
    Nzhsym41 {
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
    Nzhsym47 {
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
    Nzhsym50 {
        timestamp: SlotTimestamp,
        samples_12k: Vec<f32>,
    },
}

#[expect(
    clippy::large_enum_variant,
    reason = "Keep decoded messages inline to avoid an extra allocation per worker event"
)]
enum DecodeWorkerEvent {
    Decode {
        timestamp: SlotTimestamp,
        decode: StreamDecodedWithProvenance,
    },
    SlotComplete {
        timestamp: SlotTimestamp,
        count: usize,
    },
    Error(String),
}

fn start_decode_worker(
    config: StreamDecodeConfig,
) -> (SyncSender<QueuedDecode>, Receiver<DecodeWorkerEvent>) {
    let (cmd_tx, cmd_rx) = mpsc::sync_channel(CLI_STAGE_QUEUE_CAP);
    let (event_tx, event_rx) = mpsc::channel();
    thread::spawn(move || {
        if config.profile != DecodeProfile::Wsjtx {
            run_profile_worker(config, cmd_rx, event_tx);
            return;
        }
        let mut decoder = StreamDecodeSession::new(config);
        let mut slot_state = None;
        while let Some(command) = next_decode_command(&cmd_rx, &event_tx) {
            let result = match command {
                DecodeWorkerCommand::Nzhsym41 {
                    timestamp,
                    samples_12k,
                } => {
                    let mut state = decoder.start_slot_decode();
                    let event_tx = event_tx.clone();
                    let stage_result = decoder.decode_slot_nzhsym41_tagged(
                        Some(&timestamp),
                        &mut state,
                        &samples_12k,
                        |decode| {
                            event_tx
                                .send(DecodeWorkerEvent::Decode {
                                    timestamp: timestamp.clone(),
                                    decode: decode.clone(),
                                })
                                .map_err(|err| err.to_string())
                        },
                    );
                    if stage_result.is_ok() {
                        slot_state = Some(state);
                    }
                    stage_result.map(|_| None)
                }
                DecodeWorkerCommand::Nzhsym47 {
                    timestamp,
                    samples_12k,
                } => {
                    if let Some(state) = slot_state.as_mut() {
                        decoder.subtract_slot_nzhsym47(state, &samples_12k);
                        Ok(None)
                    } else {
                        Err(format!("{timestamp}: received nzhsym=47 before nzhsym=41"))
                    }
                }
                DecodeWorkerCommand::Nzhsym50 {
                    timestamp,
                    samples_12k,
                } => {
                    if let Some(state) = slot_state.take() {
                        let event_tx = event_tx.clone();
                        decoder
                            .decode_slot_nzhsym50_tagged(state, &samples_12k, |decode| {
                                event_tx
                                    .send(DecodeWorkerEvent::Decode {
                                        timestamp: timestamp.clone(),
                                        decode: decode.clone(),
                                    })
                                    .map_err(|err| err.to_string())
                            })
                            .map(|count| Some(DecodeWorkerEvent::SlotComplete { timestamp, count }))
                    } else {
                        Err(format!("{timestamp}: received nzhsym=50 before nzhsym=41"))
                    }
                }
            };

            match result {
                Ok(Some(event)) => {
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    let _ = event_tx.send(DecodeWorkerEvent::Error(err));
                    break;
                }
            }
        }
    });
    (cmd_tx, event_rx)
}

fn run_profile_worker(
    config: StreamDecodeConfig,
    cmd_rx: Receiver<QueuedDecode>,
    event_tx: Sender<DecodeWorkerEvent>,
) {
    let mut decoder = ProfileStreamDecodeSession::new(config);
    // Staged so hybrid streams early WSJT-X rows before the slot boundary (jtdx/dx
    // are stateless no-ops at nzhsym=41/47 and decode fully at nzhsym=50).
    let mut slot_state: Option<(ProfileSlotState, usize)> = None;
    while let Some(command) = next_decode_command(&cmd_rx, &event_tx) {
        let result = match command {
            DecodeWorkerCommand::Nzhsym41 {
                timestamp,
                samples_12k,
            } => {
                let mut state = decoder.start_slot();
                let event_tx = event_tx.clone();
                match decoder.decode_slot_nzhsym41_streaming_with_provenance(
                    &timestamp,
                    &mut state,
                    &samples_12k,
                    |row| {
                        event_tx
                            .send(DecodeWorkerEvent::Decode {
                                timestamp: timestamp.clone(),
                                decode: row.clone(),
                            })
                            .map_err(|err| err.to_string())
                    },
                ) {
                    Ok(early_count) => {
                        slot_state = Some((state, early_count));
                        Ok(None)
                    }
                    Err(err) => Err(err),
                }
            }
            DecodeWorkerCommand::Nzhsym47 {
                timestamp: _,
                samples_12k,
            } => {
                if let Some((state, _)) = slot_state.as_mut() {
                    decoder.subtract_slot_nzhsym47(state, &samples_12k);
                }
                Ok(None)
            }
            DecodeWorkerCommand::Nzhsym50 {
                timestamp,
                samples_12k,
            } => {
                if let Some((state, early_count)) = slot_state.take() {
                    let event_tx = event_tx.clone();
                    decoder
                        .decode_slot_nzhsym50_streaming_with_provenance(
                            &timestamp,
                            state,
                            early_count,
                            &samples_12k,
                            |row| {
                                event_tx
                                    .send(DecodeWorkerEvent::Decode {
                                        timestamp: timestamp.clone(),
                                        decode: row.clone(),
                                    })
                                    .map_err(|err| err.to_string())
                            },
                        )
                        .map(|count| Some(DecodeWorkerEvent::SlotComplete { timestamp, count }))
                } else {
                    Ok(None)
                }
            }
        };

        match result {
            Ok(Some(event)) => {
                if event_tx.send(event).is_err() {
                    break;
                }
            }
            Ok(None) => {}
            Err(err) => {
                let _ = event_tx.send(DecodeWorkerEvent::Error(err));
                break;
            }
        }
    }
}

struct QueuedDecode {
    command: DecodeWorkerCommand,
    queued_at: Instant,
}

fn send_worker_command(
    tx: &SyncSender<QueuedDecode>,
    command: DecodeWorkerCommand,
) -> Result<(), String> {
    tx.try_send(QueuedDecode {
        command,
        queued_at: Instant::now(),
    })
    .map_err(|error| match error {
        TrySendError::Full(_) => "Soundcard decode backlog full: 1 stage rejected".to_owned(),
        TrySendError::Disconnected(_) => "Soundcard decode worker stopped".to_owned(),
    })
}

fn next_decode_command(
    rx: &Receiver<QueuedDecode>,
    events: &Sender<DecodeWorkerEvent>,
) -> Option<DecodeWorkerCommand> {
    let queued = rx.recv().ok()?;
    if queued.queued_at.elapsed() >= CLI_STAGE_MAX_AGE {
        let _ = events.send(DecodeWorkerEvent::Error(
            "Soundcard decode backlog exceeded 10 seconds: 1 stage expired".to_owned(),
        ));
        return None;
    }
    Some(queued.command)
}

fn handle_decode_event<F, G>(
    event: DecodeWorkerEvent,
    on_decode: &mut F,
    on_slot_complete: &mut G,
) -> Result<bool, String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedWithProvenance) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    match event {
        DecodeWorkerEvent::Decode { timestamp, decode } => {
            on_decode(timestamp, &decode)?;
            Ok(false)
        }
        DecodeWorkerEvent::SlotComplete { timestamp, count } => {
            on_slot_complete(timestamp, count)?;
            Ok(true)
        }
        DecodeWorkerEvent::Error(err) => Err(err),
    }
}

fn drain_available_decode_events<F, G>(
    event_rx: &Receiver<DecodeWorkerEvent>,
    on_decode: &mut F,
    on_slot_complete: &mut G,
) -> Result<bool, String>
where
    F: FnMut(SlotTimestamp, &StreamDecodedWithProvenance) -> Result<(), String>,
    G: FnMut(SlotTimestamp, usize) -> Result<(), String>,
{
    let mut slot_complete = false;
    while let Ok(event) = event_rx.try_recv() {
        slot_complete |= handle_decode_event(event, on_decode, on_slot_complete)?;
    }
    Ok(slot_complete)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cli_stage_backlog_rejects_overflow_and_expires_before_decoding() {
        use super::*;
        let (tx, rx) = mpsc::sync_channel(1);
        let (events, received) = mpsc::channel();
        let command = || {
            worker_command(
                SlotStage::N41,
                SlotTimestamp::parse("120000").unwrap(),
                vec![],
            )
        };
        send_worker_command(&tx, command()).unwrap();
        assert_eq!(
            send_worker_command(&tx, command()).unwrap_err(),
            "Soundcard decode backlog full: 1 stage rejected"
        );
        assert!(next_decode_command(&rx, &events).is_some());
        assert!(tx
            .send(QueuedDecode {
                command: command(),
                queued_at: Instant::now() - CLI_STAGE_MAX_AGE,
            })
            .is_ok());
        assert!(next_decode_command(&rx, &events).is_none());
        assert!(matches!(
            received.try_recv(),
            Ok(DecodeWorkerEvent::Error(message)) if message.contains("1 stage expired")
        ));
    }
}
