//! Worker request/response schema, RON framing, and event conversion.
use std::io::{BufRead, Write};
use std::time::SystemTime;

use ft8rs::stream::profile::{A7Snapshot, JtdxBranch, JtdxCheckpoint, JtdxRestore};
use ft8rs::{SlotTimestamp, StreamDecodeConfig, StreamDecodedMessage};
use serde::{Deserialize, Serialize};

use crate::protocol::{DecodeRecord, DecodeStage, DxContextSnapshot, EngineEvent};
use crate::soundcard::SlotStage;

const PREFIX: &str = "FT8RS-WORKER ";

#[derive(Serialize, Deserialize)]
pub(super) enum Request {
    Init(StreamDecodeConfig),
    Stage {
        id: u64,
        stage: SlotStage,
        timestamp: String,
        samples: Vec<f32>,
        recovery_deadline: Option<SystemTime>,
        restore_a7: Option<A7Snapshot>,
        restore_jtdx: [Option<JtdxRestore>; 2],
    },
    Reconfigure {
        id: u64,
        config: StreamDecodeConfig,
        target: bool,
        operator: bool,
    },
}

#[derive(Serialize, Deserialize)]
pub(super) enum Response {
    Decode {
        timestamp: String,
        freq: f64,
        dt: f64,
        snr: f64,
        sync: f64,
        message: String,
        tones: Vec<i32>,
        provenance: ft8rs::stream::session::StreamDecodeProvenance,
        stage: DecodeStage,
    },
    SlotComplete {
        timestamp: String,
        count: usize,
    },
    A7Checkpoint {
        timestamp: String,
        snapshot: A7Snapshot,
    },
    JtdxCheckpoint {
        timestamp: String,
        branch: JtdxBranch,
        update: JtdxCheckpoint,
        publish: bool,
    },
    Dx(DxContextSnapshot),
    Error(String),
    Log(String),
    Done(u64),
}

pub(super) fn write_frame(output: &mut impl Write, value: &impl Serialize) -> Result<(), String> {
    let encoded = ron::to_string(value).map_err(|e| e.to_string())?;
    writeln!(output, "{PREFIX}{encoded}")
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}

pub(super) fn read_frame<T: for<'a> Deserialize<'a>>(
    input: &mut impl BufRead,
) -> Result<Option<T>, String> {
    loop {
        let mut line = String::new();
        input.read_line(&mut line).map_err(|e| e.to_string())?;
        if !line.ends_with('\n') {
            return Ok(None);
        }
        if let Some(frame) = line.strip_prefix(PREFIX) {
            return ron::from_str(frame.trim_end())
                .map(Some)
                .map_err(|e| e.to_string());
        }
    }
}

pub(super) fn response(event: EngineEvent) -> Option<Response> {
    Some(match event {
        EngineEvent::Decode(record) => Response::Decode {
            timestamp: record.timestamp.format(),
            freq: record.row.freq,
            dt: record.row.dt,
            snr: record.row.snr,
            sync: record.row.sync,
            message: record.row.msg,
            tones: record.row.itone.to_vec(),
            provenance: record.provenance,
            stage: record.stage,
        },
        EngineEvent::SlotComplete { timestamp, count } => Response::SlotComplete {
            timestamp: timestamp.format(),
            count,
        },
        EngineEvent::A7Checkpoint {
            timestamp,
            snapshot,
        } => Response::A7Checkpoint {
            timestamp: timestamp.format(),
            snapshot,
        },
        EngineEvent::JtdxCheckpoint {
            timestamp,
            branch,
            update,
            publish,
        } => Response::JtdxCheckpoint {
            timestamp: timestamp.format(),
            branch,
            update,
            publish,
        },
        EngineEvent::DxContext(dx) => Response::Dx(dx),
        EngineEvent::Error(error) => Response::Error(error),
        _ => return None,
    })
}

pub(super) fn event(response: Response) -> Result<Option<EngineEvent>, String> {
    Ok(Some(match response {
        Response::Decode {
            timestamp,
            freq,
            dt,
            snr,
            sync,
            message,
            tones,
            provenance,
            stage,
        } => EngineEvent::Decode(DecodeRecord {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            row: StreamDecodedMessage {
                freq,
                dt,
                snr,
                sync,
                msg: message,
                itone: tones
                    .try_into()
                    .map_err(|_| "worker returned invalid FT8 tone count")?,
            },
            provenance,
            stage,
        }),
        Response::SlotComplete { timestamp, count } => EngineEvent::SlotComplete {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            count,
        },
        Response::A7Checkpoint {
            timestamp,
            snapshot,
        } => EngineEvent::A7Checkpoint {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            snapshot,
        },
        Response::JtdxCheckpoint {
            timestamp,
            branch,
            update,
            publish,
        } => EngineEvent::JtdxCheckpoint {
            timestamp: SlotTimestamp::parse(&timestamp)?,
            branch,
            update,
            publish,
        },
        Response::Dx(dx) => EngineEvent::DxContext(dx),
        Response::Error(error) => EngineEvent::Error(error),
        Response::Log(message) => {
            log::info!(target: "ft8rs_engine::worker", "decoder worker: {message}");
            return Ok(None);
        }
        Response::Done(_) => return Ok(None),
    }))
}
