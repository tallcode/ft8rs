use ft8rs::stream::{StreamDecodedMessage, StreamSnrSource};
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
            "{} {} {:+.1} {:>5.0} {}",
            timestamp.format_time(),
            format_snr(row),
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

fn format_snr(row: &StreamDecodedMessage) -> String {
    match row.snr_source {
        StreamSnrSource::Decoder | StreamSnrSource::DxDeepEstimated => {
            format!("{:>3}", row.snr.round() as i32)
        }
        StreamSnrSource::DxDeepUnavailable => " DX".to_string(),
    }
}

fn flush_stdout() -> Result<(), String> {
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush stdout: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(snr: f64, snr_source: StreamSnrSource) -> StreamDecodedMessage {
        StreamDecodedMessage {
            freq: 1000.0,
            dt: 0.0,
            snr,
            snr_source,
            deep_confidence: None,
            msg: "K1JT BG5ATV -10".to_string(),
            sync: 0.0,
            itone: [0; 79],
        }
    }

    #[test]
    fn formats_decoder_snr_as_db_integer() {
        assert_eq!(format_snr(&row(-17.4, StreamSnrSource::Decoder)), "-17");
    }

    #[test]
    fn formats_dx_deep_unavailable_snr_as_marker() {
        assert_eq!(
            format_snr(&row(-99.0, StreamSnrSource::DxDeepUnavailable)),
            " DX"
        );
    }

    #[test]
    fn formats_dx_deep_estimated_snr_as_db_integer() {
        assert_eq!(
            format_snr(&row(-18.4, StreamSnrSource::DxDeepEstimated)),
            "-18"
        );
    }
}
