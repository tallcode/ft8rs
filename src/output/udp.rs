use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use ft8rs::stream::{StreamDecodedMessage, StreamSnrSource};
use ft8rs::SlotTimestamp;

const MAGIC: u32 = 0xadbccbda;
const SCHEMA: u32 = 2;
const TYPE_DECODE: u32 = 2;
const CLIENT_ID: &str = "ft8rs";
const MODE: &str = "FT8";

pub struct UdpConfig {
    pub host: String,
    pub port: u16,
}

pub struct UdpOutput {
    socket: UdpSocket,
    destination: SocketAddr,
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
    // The compatible packet has only an integer SNR field. DX deep rows use their
    // JTDX-formula estimate when available; otherwise -99 remains the explicit
    // "unavailable" sentinel.
    let snr = match row.snr_source {
        StreamSnrSource::Decoder | StreamSnrSource::DxDeepEstimated => row.snr.round() as i32,
        StreamSnrSource::DxDeepUnavailable => -99,
    };
    put_i32(&mut packet, snr);
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
    use ft8rs::stream::{StreamDecodedMessage, StreamSnrSource};
    use ft8rs::SlotTimestamp;

    #[test]
    fn builds_decode_packet_header_and_payload_shape() {
        let row = StreamDecodedMessage {
            freq: 1501.7,
            dt: -0.3,
            snr: -17.4,
            snr_source: StreamSnrSource::Decoder,
            deep_confidence: None,
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

    #[test]
    fn dx_deep_unavailable_snr_uses_udp_sentinel() {
        let row = StreamDecodedMessage {
            freq: 1501.7,
            dt: -0.3,
            snr: -99.0,
            snr_source: StreamSnrSource::DxDeepUnavailable,
            deep_confidence: None,
            msg: "CQ TEST PM95".to_string(),
            sync: 0.0,
            itone: [0; 79],
        };
        let packet = build_decode_packet(SlotTimestamp::parse("230208_140300").unwrap(), &row);

        assert_eq!(&packet[26..30], &(-99_i32).to_be_bytes());
    }

    #[test]
    fn dx_deep_estimated_snr_uses_udp_integer_snr() {
        let row = StreamDecodedMessage {
            freq: 1501.7,
            dt: -0.3,
            snr: -18.4,
            snr_source: StreamSnrSource::DxDeepEstimated,
            deep_confidence: None,
            msg: "CQ TEST PM95".to_string(),
            sync: 0.0,
            itone: [0; 79],
        };
        let packet = build_decode_packet(SlotTimestamp::parse("230208_140300").unwrap(), &row);

        assert_eq!(&packet[26..30], &(-18_i32).to_be_bytes());
    }
}
