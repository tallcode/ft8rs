use ft8rs::stream::session::StreamDecodeProvenance;
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
        provenance: StreamDecodeProvenance,
    ) -> Result<(), String> {
        println!("{}", decode_line(&timestamp, row, provenance));
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

fn decode_line(
    timestamp: &SlotTimestamp,
    row: &StreamDecodedMessage,
    provenance: StreamDecodeProvenance,
) -> String {
    let suffix = if provenance == StreamDecodeProvenance::Experiment {
        " exp"
    } else {
        ""
    };
    format!(
        "{} {:>3} {:+.1} {:>5.0} {}{}",
        timestamp.format_time(),
        row.snr.round() as i32,
        row.dt,
        row.freq.round(),
        row.msg,
        suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn experiment_tag_is_a_suffix_and_never_changes_payload() {
        let time = SlotTimestamp::parse("261029_120000").unwrap();
        let row = StreamDecodedMessage {
            freq: 1000.0,
            dt: 0.1,
            snr: -20.0,
            msg: "CQ K1ABC FN42".into(),
            sync: 0.0,
            itone: [0; 79],
        };
        let plain = decode_line(&time, &row, StreamDecodeProvenance::Unknown);
        assert_eq!(plain, "120000 -20 +0.1  1000 CQ K1ABC FN42");
        assert_eq!(
            decode_line(&time, &row, StreamDecodeProvenance::Experiment),
            format!("{plain} exp")
        );
        assert_eq!(row.msg, "CQ K1ABC FN42");
    }
}
