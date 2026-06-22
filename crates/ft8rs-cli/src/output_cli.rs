use ft8rs::stream::StreamDecodedMessage;
use ft8rs::SlotTimestamp;

pub struct CliOutput;

impl CliOutput {
    pub fn new() -> Self {
        Self
    }

    pub fn on_decode(
        &mut self,
        timestamp: SlotTimestamp,
        row: &StreamDecodedMessage,
    ) -> Result<(), String> {
        println!(
            "{} {:>3} {:+.1} {:>5.0} {}",
            timestamp.format_time(),
            row.snr.round() as i32,
            row.dt,
            row.freq.round(),
            row.msg
        );
        flush_stdout()
    }

    pub fn on_slot_complete(
        &mut self,
        _timestamp: SlotTimestamp,
        count: usize,
    ) -> Result<(), String> {
        println!("------ slot done: {count:>2} decodes ------");
        flush_stdout()
    }
}

fn flush_stdout() -> Result<(), String> {
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush stdout: {err}"))
}
