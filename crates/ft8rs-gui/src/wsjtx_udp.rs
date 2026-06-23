//! WSJT-X UDP listener (GUI-only) for the decoder-comparison feature.
//!
//! Binds a UDP socket and parses WSJT-X's "Decode" (type 2) messages from its
//! UDP broadcast, forwarding them to the GUI for merge/compare with the local
//! decoder. Unicast only (e.g. 127.0.0.1:2237). This never touches the decode
//! path — it is purely an external result feed.
//!
//! The wire format mirrors `ft8rs-engine`'s outbound reporter: a big-endian
//! QDataStream with magic `0xadbccbda`, then for a Decode message:
//! `id:utf8, is_new:bool, time:u32(ms), snr:i32, dt:f64, freq:u32, mode:utf8,
//! message:utf8, low_confidence:bool, off_air:bool`.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const MAGIC: u32 = 0xadbccbda;
const TYPE_DECODE: u32 = 2;
const NULL_STRING: u32 = 0xffff_ffff;

/// A decode received from WSJT-X over UDP.
#[derive(Clone, Debug)]
pub struct ExternalDecode {
    pub ms_since_midnight: u32,
    pub snr: i32,
    pub dt: f64,
    pub freq: u32,
    pub message: String,
}

/// A running listener thread. Dropping it stops the thread.
pub struct UdpIn {
    pub host: String,
    pub port: u16,
    stop: Arc<AtomicBool>,
    rx: Receiver<ExternalDecode>,
    handle: Option<JoinHandle<()>>,
}

impl UdpIn {
    /// Bind `host:port` and start receiving. Errors if the bind fails (e.g. the
    /// port is already in use).
    pub fn spawn(host: &str, port: u16) -> Result<Self, String> {
        let socket = UdpSocket::bind((host, port))
            .map_err(|err| format!("UDP listen on {host}:{port} failed: {err}"))?;
        // Periodic timeout so the loop can observe the stop flag.
        let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !stop_thread.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        if let Some(decode) = parse_decode(&buf[..n]) {
                            if tx.send(decode).is_err() {
                                break; // receiver dropped
                            }
                        }
                    }
                    Err(err) => match err.kind() {
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => continue,
                        _ => thread::sleep(Duration::from_millis(50)),
                    },
                }
            }
        });

        Ok(Self {
            host: host.to_string(),
            port,
            stop,
            rx,
            handle: Some(handle),
        })
    }

    /// Drain one pending decode, if any.
    pub fn try_recv(&self) -> Option<ExternalDecode> {
        self.rx.try_recv().ok()
    }
}

impl Drop for UdpIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Parse a WSJT-X Decode (type 2) datagram. Returns None for other message
/// types, replays (`is_new == false`), or malformed packets.
fn parse_decode(buf: &[u8]) -> Option<ExternalDecode> {
    let mut p = Cursor::new(buf);
    if p.u32()? != MAGIC {
        return None;
    }
    let _schema = p.u32()?;
    if p.u32()? != TYPE_DECODE {
        return None;
    }
    p.skip_string()?; // client id
    let is_new = p.u8()? != 0;
    if !is_new {
        return None;
    }
    let ms_since_midnight = p.u32()?;
    let snr = p.i32()?;
    let dt = p.f64()?;
    let freq = p.u32()?;
    p.skip_string()?; // mode
    let message = p.string()?;
    Some(ExternalDecode {
        ms_since_midnight,
        snr,
        dt,
        freq,
        message,
    })
}

/// Minimal big-endian reader over a byte slice; every read is bounds-checked.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes(b.try_into().unwrap()))
    }

    fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|b| i32::from_be_bytes(b.try_into().unwrap()))
    }

    fn f64(&mut self) -> Option<f64> {
        self.take(8).map(|b| f64::from_be_bytes(b.try_into().unwrap()))
    }

    /// A WSJT-X utf8 string: u32 length (0xffffffff = null) + bytes.
    fn string(&mut self) -> Option<String> {
        let len = self.u32()?;
        if len == NULL_STRING {
            return Some(String::new());
        }
        let bytes = self.take(len as usize)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn skip_string(&mut self) -> Option<()> {
        let len = self.u32()?;
        if len == NULL_STRING {
            return Some(());
        }
        self.take(len as usize).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_string(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn decode_packet(is_new: bool, ms: u32, snr: i32, dt: f64, freq: u32, msg: &str) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&MAGIC.to_be_bytes());
        p.extend_from_slice(&2u32.to_be_bytes()); // schema
        p.extend_from_slice(&TYPE_DECODE.to_be_bytes());
        put_string(&mut p, "WSJT-X");
        p.push(u8::from(is_new));
        p.extend_from_slice(&ms.to_be_bytes());
        p.extend_from_slice(&snr.to_be_bytes());
        p.extend_from_slice(&dt.to_be_bytes());
        p.extend_from_slice(&freq.to_be_bytes());
        put_string(&mut p, "FT8");
        put_string(&mut p, msg);
        p.push(0); // low_confidence
        p.push(0); // off_air
        p
    }

    #[test]
    fn parses_decode_message() {
        let pkt = decode_packet(true, 50_115_000, -8, 0.2, 1501, "CQ BG5ATV PM00");
        let d = parse_decode(&pkt).expect("decode");
        assert_eq!(d.ms_since_midnight, 50_115_000);
        assert_eq!(d.snr, -8);
        assert!((d.dt - 0.2).abs() < 1e-9);
        assert_eq!(d.freq, 1501);
        assert_eq!(d.message, "CQ BG5ATV PM00");
    }

    #[test]
    fn ignores_replays_and_other_types() {
        let replay = decode_packet(false, 0, 0, 0.0, 1000, "CQ TEST");
        assert!(parse_decode(&replay).is_none());

        let mut status = Vec::new();
        status.extend_from_slice(&MAGIC.to_be_bytes());
        status.extend_from_slice(&2u32.to_be_bytes());
        status.extend_from_slice(&1u32.to_be_bytes()); // type 1 = Status
        assert!(parse_decode(&status).is_none());
    }

    #[test]
    fn rejects_truncated_and_bad_magic() {
        assert!(parse_decode(&[0, 1, 2]).is_none());
        let mut bad = decode_packet(true, 0, 0, 0.0, 1000, "CQ");
        bad[0] = 0; // corrupt magic
        assert!(parse_decode(&bad).is_none());
    }

    #[test]
    fn receives_over_real_socket() {
        // Grab a free UDP port, then bind the listener to it.
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let udp = UdpIn::spawn("127.0.0.1", port).expect("spawn listener");

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let pkt = decode_packet(true, 1000, -5, 0.1, 1200, "CQ TEST AA00");
        client.send_to(&pkt, ("127.0.0.1", port)).unwrap();

        // Poll until it arrives (instant on loopback; bounded so it can't hang).
        let mut got = None;
        for _ in 0..200 {
            if let Some(d) = udp.try_recv() {
                got = Some(d);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let d = got.expect("received decode over socket");
        assert_eq!(d.freq, 1200);
        assert_eq!(d.message, "CQ TEST AA00");
    }
}
