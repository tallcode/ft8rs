pub mod cli;
pub mod udp;

use ft8rs::stream::StreamDecodedMessage;
use ft8rs::SlotTimestamp;

use cli::CliOutput;
use udp::{UdpConfig, UdpOutput};

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
        row: &StreamDecodedMessage,
    ) -> Result<(), String> {
        self.cli.on_decode(timestamp.clone(), row)?;
        if let Some(udp) = &self.udp {
            udp.on_decode(timestamp, row)?;
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
