use std::io::{self, Write};

use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::stream::StreamDecodedMessage;
use ft8rs::SlotTimestamp;

#[derive(Default)]
pub struct CliOutput {
    pub(crate) broken_pipe: bool,
}

impl CliOutput {
    pub fn on_decode(
        &mut self,
        timestamp: SlotTimestamp,
        row: &StreamDecodedMessage,
        provenance: StreamDecodeProvenance,
    ) -> Result<(), String> {
        self.line(format_args!("{}", decode_line(&timestamp, row, provenance)))
    }

    pub fn on_slot_complete(
        &mut self,
        _timestamp: SlotTimestamp,
        count: usize,
    ) -> Result<(), String> {
        self.line(format_args!("------ slot done: {count:>2} decodes ------"))
    }

    fn line(&mut self, line: std::fmt::Arguments<'_>) -> Result<(), String> {
        write_stdout(format_args!("{line}\n")).map_err(|error| {
            self.broken_pipe = error.kind() == io::ErrorKind::BrokenPipe;
            format!("failed to write stdout: {error}")
        })
    }
}

pub(crate) fn write_stdout(value: std::fmt::Arguments<'_>) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(value)?;
    stdout.flush()
}

fn decode_line(
    timestamp: &SlotTimestamp,
    row: &StreamDecodedMessage,
    provenance: StreamDecodeProvenance,
) -> String {
    let suffix = match provenance {
        StreamDecodeProvenance::Experiment => "exp",
        StreamDecodeProvenance::PlusCompatibility => "Pc",
        StreamDecodeProvenance::CandidateRetry => "Pr",
        StreamDecodeProvenance::WsjtxAp(ap_type) => ap_type.annotation(),
        StreamDecodeProvenance::A7Memory => "a7",
        StreamDecodeProvenance::A8List => "a8",
        StreamDecodeProvenance::ApMask => "ap",
        _ => "",
    };
    format!(
        "{} {:>3} {:+.1} {:>5.0} {}{}{}",
        timestamp.format_time(),
        row.snr.round() as i32,
        row.dt,
        row.freq.round(),
        row.msg,
        if suffix.is_empty() { "" } else { " " },
        suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ft8rs::stream::session::WsjtxApType;
    #[test]
    fn provenance_tags_are_suffixes_and_never_change_payload() {
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
        for (provenance, tag) in [
            (StreamDecodeProvenance::PlusCompatibility, "Pc"),
            (StreamDecodeProvenance::CandidateRetry, "Pr"),
            (StreamDecodeProvenance::WsjtxAp(WsjtxApType::Cq), "a1"),
            (StreamDecodeProvenance::WsjtxAp(WsjtxApType::MyCall), "a2"),
            (
                StreamDecodeProvenance::WsjtxAp(WsjtxApType::BothCalls),
                "a3",
            ),
            (StreamDecodeProvenance::WsjtxAp(WsjtxApType::Rrr), "a4"),
            (
                StreamDecodeProvenance::WsjtxAp(WsjtxApType::SeventyThree),
                "a5",
            ),
            (StreamDecodeProvenance::WsjtxAp(WsjtxApType::Rr73), "a6"),
            (StreamDecodeProvenance::A7Memory, "a7"),
            (StreamDecodeProvenance::A8List, "a8"),
            (StreamDecodeProvenance::ApMask, "ap"),
        ] {
            assert_eq!(
                decode_line(&time, &row, provenance),
                format!("{plain} {tag}")
            );
        }
        assert_eq!(row.msg, "CQ K1ABC FN42");
    }
}
