//! Debug bundle serialization and atomic publication.
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ft8rs::SlotTimestamp;
use serde_json::{json, Value};

use super::{slot_timestamp, Request, Slot, AUDIO_RATE, SLOT_MS};

pub(super) struct Export {
    pub(super) request: Request,
    pub(super) cancelled: Arc<Mutex<bool>>,
}

impl Export {
    pub(super) fn write(self, root: &Path) -> io::Result<Option<PathBuf>> {
        if *self.cancelled.lock().unwrap() {
            return Ok(None);
        }
        fs::create_dir_all(root)?;
        let time = SlotTimestamp::from_unix_seconds_utc(self.request.requested_ms / 1000);
        let base = format!("{time}Z");
        let mut index = 1;
        let (destination, mut staging) = loop {
            let name = if index == 1 {
                base.clone()
            } else {
                format!("{base}_{index}")
            };
            let destination = root.join(&name);
            if !destination.try_exists()? {
                let path = root.join(format!(".{name}.partial"));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        break (
                            destination,
                            Staging {
                                path,
                                published: false,
                            },
                        )
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(err) => return Err(err),
                }
            }
            index += 1;
        };
        self.write_files(&staging.path)?;
        // Linearize publication with unchecking the capture option. Files are
        // closed before this lock, and cancellation removes only staging.
        let cancelled = self.cancelled.lock().unwrap();
        if *cancelled {
            return Ok(None);
        }
        fs::rename(&staging.path, &destination)?;
        staging.published = true;
        Ok(Some(destination))
    }

    fn write_files(&self, path: &Path) -> io::Result<()> {
        let audio_dir = path.join("audio");
        fs::create_dir(&audio_dir)?;
        let mut local = BufWriter::new(File::create(path.join("local-decodes.jsonl"))?);
        let mut udp = BufWriter::new(File::create(path.join("udp-decodes.jsonl"))?);
        let mut logs = BufWriter::new(File::create(path.join("app.log"))?);
        let mut slots = Vec::new();
        let mut cutoff_ms = 0;
        let mut tail_dropped_logs = 0;
        let empty = Slot::default();
        for segment in &self.request.segments {
            let window = segment.window;
            cutoff_ms = window.cutoff_ms();
            tail_dropped_logs += segment
                .slots
                .get(&(window.last + 1))
                .map_or(0, |slot| slot.dropped_logs);
            if segment.id != 0 {
                writeln!(logs, "--- clock segment {} ---", segment.id)?;
            }
            for slot in window.first..=window.last + 1 {
                if *self.cancelled.lock().unwrap() {
                    return Ok(());
                }
                let data = segment.slots.get(&slot).map(Arc::as_ref).unwrap_or(&empty);
                for log in data.logs.iter().filter(|log| log.at_ms <= cutoff_ms) {
                    writeln!(logs, "{}", log.value)?;
                }
                if slot > window.last {
                    break;
                }
                let timestamp = slot_timestamp(slot).format();
                for entry in data.local.iter().filter(|row| row.at_ms <= cutoff_ms) {
                    let record = &entry.value;
                    write_json_line(
                        &mut local,
                        &json!({
                            "slot_utc": timestamp, "clock_segment": segment.id, "forwarded_at_unix_ms": entry.at_ms,
                            "snr_db": record.row.snr, "dt_seconds": record.row.dt,
                            "frequency_hz": record.row.freq, "message": record.row.msg,
                            "stage": record.stage, "provenance": record.provenance,
                        }),
                    )?;
                }
                for entry in data.udp.iter().filter(|row| row.at_ms <= cutoff_ms) {
                    let (peer, record) = &entry.value;
                    write_json_line(
                        &mut udp,
                        &json!({
                            "slot_utc": timestamp, "clock_segment": segment.id, "received_at_unix_ms": entry.at_ms,
                            "packet_ms_since_midnight": record.ms_since_midnight, "peer": peer,
                            "snr_db": record.snr, "dt_seconds": record.dt,
                            "frequency_hz": record.freq, "message": record.message,
                        }),
                    )?;
                }
                let audio = if let Some(audio) = &data.audio {
                    let name = if segment.id == 0 {
                        format!("{timestamp}.wav")
                    } else {
                        format!("g{}_{timestamp}.wav", segment.id)
                    };
                    write_wav(&audio_dir.join(&name), &audio.samples)?;
                    json!({
                        "file": format!("audio/{name}"),
                        "samples": audio.samples.len(), "sample_rate": AUDIO_RATE,
                        "channels": 1, "sample_format": "float32",
                        "short": audio.samples.len() < AUDIO_RATE as usize * 15,
                        "device": audio.device_name, "channel": audio.channel,
                        "native_sample_rate": audio.format.sample_rate,
                        "native_channels": audio.format.channels,
                        "native_sample_format": audio.format.sample_format,
                        "native_samples": audio.native_samples, "capture_xruns": audio.xruns,
                        "capture_jitter_ms": audio.jitter_ms, "decode_config": audio.config,
                    })
                } else {
                    Value::Null
                };
                slots.push(json!({
                "slot_utc": timestamp, "clock_segment": segment.id, "slot_start_unix_ms": slot * SLOT_MS, "audio": audio,
                "dropped_local": data.dropped_local, "dropped_udp": data.dropped_udp,
                "dropped_log_lines": data.dropped_logs,
            }));
            }
        }
        local.flush()?;
        udp.flush()?;
        logs.flush()?;
        let mut manifest = BufWriter::new(File::create(path.join("manifest.json"))?);
        serde_json::to_writer_pretty(
            &mut manifest,
            &json!({
                "format_version": 1, "application_version": env!("FT8RS_VERSION"),
                "requested_at_unix_ms": self.request.requested_ms,
                "last_observation_unix_ms": cutoff_ms,
                "udp_listener_at_save": self.request.udp_listener, "slots": slots,
                "tail_dropped_log_lines": tail_dropped_logs,
            }),
        )?;
        manifest.flush()
    }
}

struct Staging {
    path: PathBuf,
    published: bool,
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn write_json_line(output: &mut impl Write, row: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, row)?;
    writeln!(output)
}

fn write_wav(path: &Path, samples: &[f32]) -> io::Result<()> {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: AUDIO_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .map_err(io::Error::other)?;
    for &sample in samples {
        writer.write_sample(sample).map_err(io::Error::other)?;
    }
    writer.finalize().map_err(io::Error::other)
}
