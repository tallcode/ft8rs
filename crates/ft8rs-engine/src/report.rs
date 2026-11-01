use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ft8rs::stream::StreamDecodedMessage;
use ft8rs::SlotTimestamp;

const MAGIC: u32 = 0xadbccbda;
const SCHEMA: u32 = 2;
const TYPE_DECODE: u32 = 2;
const CLIENT_ID: &str = "ft8rs";
const MODE: &str = "FT8";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpConfig {
    pub host: String,
    pub port: u16,
}

pub struct UdpOutput {
    socket: UdpSocket,
    destination: SocketAddr,
    failures: Mutex<SendFailures>,
}

#[derive(Default)]
struct SendFailures {
    count: u64,
    last_log: Option<Instant>,
}

impl SendFailures {
    fn record(&mut self, now: Instant) -> Option<u64> {
        self.count = self.count.saturating_add(1);
        if self
            .last_log
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(30))
        {
            self.last_log = Some(now);
            Some(self.count)
        } else {
            None
        }
    }
}

impl UdpOutput {
    pub fn new(config: UdpConfig) -> Result<Self, String> {
        let destination = resolve_destination(&config.host, config.port)?;
        let bind_addr = if destination.is_ipv6() {
            "[::]:0"
        } else {
            "0.0.0.0:0"
        };
        let socket = UdpSocket::bind(bind_addr)
            .map_err(|err| format!("failed to bind UDP socket: {err}"))?;
        Ok(Self {
            socket,
            destination,
            failures: Mutex::new(SendFailures::default()),
        })
    }

    pub fn on_decode(
        &self,
        timestamp: SlotTimestamp,
        row: &StreamDecodedMessage,
    ) -> Result<(), String> {
        let packet = build_decode_packet(timestamp, row);
        self.socket
            .send_to(&packet, self.destination)
            .map_err(|err| {
                format!(
                    "failed to send UDP decode report to {}: {err}",
                    self.destination
                )
            })?;
        Ok(())
    }

    /// Live output failures never interrupt decoding. Log the first failure and
    /// at most one per 30 seconds thereafter, with the cumulative failure count.
    pub(crate) fn on_decode_best_effort(
        &self,
        timestamp: SlotTimestamp,
        row: &StreamDecodedMessage,
    ) {
        if let Err(error) = self.on_decode(timestamp, row) {
            if let Some(count) = self.failures.lock().unwrap().record(Instant::now()) {
                log::warn!("{error}; UDP send failures since output opened: {count}");
            }
        }
    }
}

fn resolve_destination(host: &str, port: u16) -> Result<SocketAddr, String> {
    (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("invalid UDP report address {host}:{port}: {err}"))?
        .next()
        .ok_or_else(|| format!("invalid UDP report address {host}:{port}"))
}

fn build_decode_packet(timestamp: SlotTimestamp, row: &StreamDecodedMessage) -> Vec<u8> {
    let mut packet = Vec::with_capacity(96 + row.msg.len());
    put_u32(&mut packet, MAGIC);
    put_u32(&mut packet, SCHEMA);
    put_u32(&mut packet, TYPE_DECODE);
    put_byte_array(&mut packet, CLIENT_ID.as_bytes());

    put_bool(&mut packet, true);
    put_qtime(&mut packet, timestamp);
    put_i32(&mut packet, row.snr.round() as i32);
    put_f64(&mut packet, row.dt);
    put_u32(&mut packet, row.freq.round().max(0.0) as u32);
    put_byte_array(&mut packet, MODE.as_bytes());
    put_byte_array(&mut packet, row.msg.as_bytes());
    put_bool(&mut packet, false);
    put_bool(&mut packet, false);
    packet
}

fn put_qtime(out: &mut Vec<u8>, timestamp: SlotTimestamp) {
    put_u32(out, timestamp.milliseconds_since_midnight());
}

fn put_byte_array(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn put_bool(out: &mut Vec<u8>, value: bool) {
    out.push(u8::from(value));
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::build_decode_packet;
    use ft8rs::stream::StreamDecodedMessage;
    use ft8rs::SlotTimestamp;

    #[test]
    fn send_failures_count_every_error_and_rate_limit_diagnostics() {
        let mut failures = super::SendFailures::default();
        let now = std::time::Instant::now();
        assert_eq!(failures.record(now), Some(1));
        for _ in 0..100 {
            assert_eq!(failures.record(now), None);
        }
        assert_eq!(
            failures.record(now + std::time::Duration::from_secs(30)),
            Some(102)
        );
    }

    #[test]
    fn builds_decode_packet_header_and_payload_shape() {
        let row = StreamDecodedMessage {
            freq: 1501.7,
            dt: -0.3,
            snr: -17.4,
            msg: "CQ TEST PM95".to_string(),
            sync: 0.0,
            itone: [0; 79],
        };
        let packet = build_decode_packet(SlotTimestamp::parse("230208_140300").unwrap(), &row);

        assert_eq!(&packet[0..4], &0xadbccbda_u32.to_be_bytes());
        assert_eq!(&packet[4..8], &2_u32.to_be_bytes());
        assert_eq!(&packet[8..12], &2_u32.to_be_bytes());
        assert_eq!(&packet[12..16], &5_u32.to_be_bytes());
        assert_eq!(&packet[16..21], b"ft8rs");
        assert_eq!(packet[21], 1);
        let expected_msecs: u32 = 14 * 3600 * 1000 + 3 * 60 * 1000;
        assert_eq!(&packet[22..26], &expected_msecs.to_be_bytes());
        assert_eq!(&packet[26..30], &(-17_i32).to_be_bytes());

        let mode_len_offset = 30 + 8 + 4;
        assert_eq!(
            &packet[mode_len_offset..mode_len_offset + 4],
            &3_u32.to_be_bytes()
        );
        assert_eq!(&packet[mode_len_offset + 4..mode_len_offset + 7], b"FT8");
    }
}
