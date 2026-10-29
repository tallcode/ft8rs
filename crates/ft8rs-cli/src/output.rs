use ft8rs::stream::session::StreamDecodedWithProvenance;
use ft8rs::SlotTimestamp;

use crate::output_cli::CliOutput;
pub use ft8rs_engine::report::{UdpConfig, UdpOutput};

pub struct Outputs {
    cli: CliOutput,
    udp: Option<UdpOutput>,
}

impl Outputs {
    pub fn new(udp: Option<UdpConfig>) -> Result<Self, String> {
        Ok(Self {
            cli: CliOutput::new(),
            udp: udp.map(UdpOutput::new).transpose()?,
        })
    }

    pub fn on_decode(
        &mut self,
        timestamp: SlotTimestamp,
        row: &StreamDecodedWithProvenance,
    ) -> Result<(), String> {
        self.cli
            .on_decode(timestamp.clone(), &row.decode, row.provenance)?;
        if let Some(udp) = &self.udp {
            udp.on_decode(timestamp, &row.decode)?;
        }
        Ok(())
    }

    pub fn on_slot_complete(
        &mut self,
        timestamp: SlotTimestamp,
        count: usize,
    ) -> Result<(), String> {
        self.cli.on_slot_complete(timestamp, count)
    }
}
