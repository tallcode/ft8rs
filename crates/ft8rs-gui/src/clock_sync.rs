//! User-triggered SNTP and platform-specific system clock writes.
mod system;

use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime};

use futures_util::stream::{FuturesUnordered, StreamExt};
use sntpc::{NtpContext, StdTimestampGen};
use sntpc_net_tokio::UdpSocketWrapper;
use tokio::net::{lookup_host, UdpSocket};
use tokio::runtime::Builder;
use tokio::time::{sleep, timeout};

#[cfg(windows)]
pub(crate) use system::run_write_helper;

const POOL_HOST: &str = match option_env!("FT8RS_NTP_POOL_HOST") {
    Some(vendor_zone) => vendor_zone,
    None => "pool.ntp.org",
};
const HOSTS: [&str; 5] = [
    "ntp.aliyun.com",
    "time.cloudflare.com",
    POOL_HOST,
    "time.windows.com",
    "time.cloud.tencent.com",
];
const TIMEOUT: Duration = Duration::from_secs(3);
const VERIFY_DELAY: Duration = Duration::from_secs(4);
const MIN_PROGRESS_TIME: Duration = Duration::from_secs(1);
const CLOCK_WRITE_THRESHOLD_US: u64 = 50_000;

#[derive(Clone, Copy, Debug)]
struct Sample {
    host: &'static str,
    peer: SocketAddr,
    offset_us: i64,
}

pub(crate) enum Update {
    Querying,
    Applying,
    Verifying,
    WithinTolerance {
        offset_us: i64,
    },
    Finished {
        offset_us: i64,
        remaining_us: Option<i64>,
    },
    Failed(Error),
}

#[derive(Debug)]
pub(crate) enum Error {
    NoResponse,
    Cancelled,
    System(String),
}

impl Update {
    pub(crate) fn busy(&self) -> bool {
        matches!(self, Self::Querying | Self::Applying | Self::Verifying)
    }
}

pub(crate) fn start(
    report_write: impl FnOnce(Instant, Instant) + Send + 'static,
    wake: impl Fn() + Send + 'static,
) -> Result<Receiver<Update>, std::io::Error> {
    let (tx, rx) = mpsc::channel();
    let show_progress_until = Instant::now() + MIN_PROGRESS_TIME;
    std::thread::Builder::new()
        .name("ft8rs-clock-sync".into())
        .spawn(move || {
            let update = |event| {
                let sent = tx.send(event).is_ok();
                wake();
                sent
            };
            let run = || {
                let sample =
                    network(first_success(HOSTS.map(query_host)))?.ok_or(Error::NoResponse)?;
                if !needs_clock_write(sample.offset_us) {
                    return Ok(Update::WithinTolerance {
                        offset_us: sample.offset_us,
                    });
                }
                if !update(Update::Applying) {
                    return Err(Error::Cancelled);
                }
                let before = Instant::now();
                system::apply(sample)?;
                report_write(before, Instant::now());
                if !update(Update::Verifying) {
                    return Err(Error::Cancelled);
                }
                let remaining_us = network(async {
                    sleep(VERIFY_DELAY).await;
                    timeout(TIMEOUT, query_peer(sample.host, sample.peer))
                        .await
                        .ok()
                        .flatten()
                        .map(|sample| sample.offset_us)
                })
                .ok()
                .flatten();
                Ok(Update::Finished {
                    offset_us: sample.offset_us,
                    remaining_us,
                })
            };
            let result = run().unwrap_or_else(Update::Failed);
            // Hold the final UI update without delaying the network race or clock write.
            std::thread::sleep(show_progress_until.saturating_duration_since(Instant::now()));
            update(result);
        })?;
    Ok(rx)
}

fn needs_clock_write(offset_us: i64) -> bool {
    offset_us.unsigned_abs() >= CLOCK_WRITE_THRESHOLD_US
}

fn network<T>(query: impl std::future::Future<Output = T>) -> Result<T, Error> {
    let runtime = Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|err| Error::System(err.to_string()))?;
    let result = runtime.block_on(query);
    // Dropping the query closes every losing UDP socket. A getaddrinfo already
    // inside the OS resolver can finish independently without sending a probe.
    runtime.shutdown_background();
    Ok(result)
}

async fn first_success<T, const N: usize>(
    queries: [impl std::future::Future<Output = Option<T>>; N],
) -> Option<T> {
    let mut queries: FuturesUnordered<_> = queries.into_iter().collect();
    timeout(TIMEOUT, async {
        while let Some(result) = queries.next().await {
            if result.is_some() {
                return result;
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

async fn query_host(host: &'static str) -> Option<Sample> {
    let peers = lookup_host((host, 123)).await.ok()?;
    // Try the first address of each family so an unusable IPv6 route cannot
    // hide the same provider's IPv4 service.
    let mut families = [None, None];
    for peer in peers {
        families[usize::from(peer.is_ipv6())].get_or_insert(peer);
    }
    first_success(families.map(|peer| async move { query_peer(host, peer?).await })).await
}

async fn query_peer(host: &'static str, peer: SocketAddr) -> Option<Sample> {
    let bind = if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind).await.ok()?;
    let socket = UdpSocketWrapper::new(socket);
    let wall = SystemTime::now();
    // The library's std timestamp source expects a representable Unix time.
    wall.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let monotonic = Instant::now();
    let sample =
        match sntpc::get_time(peer, &socket, NtpContext::new(StdTimestampGen::default())).await {
            Ok(sample) => sample,
            Err(error) => {
                log::debug!("SNTP {host} ({peer}): {error:?}");
                return None;
            }
        };
    let elapsed = SystemTime::now().duration_since(wall).ok()?;
    if elapsed.abs_diff(monotonic.elapsed()) > Duration::from_millis(50) {
        return None;
    }
    Some(Sample {
        host,
        peer,
        offset_us: sample.offset(),
    })
}

#[cfg(windows)]
fn refresh_peer(peer: SocketAddr) -> Result<Sample, Error> {
    network(async {
        timeout(TIMEOUT, query_peer("selected peer", peer))
            .await
            .ok()
            .flatten()
    })?
    .ok_or(Error::NoResponse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::future::pending;

    #[test]
    fn clock_write_threshold_applies_to_both_directions() {
        for offset in [0, -49_999, 49_999] {
            assert!(!needs_clock_write(offset));
        }
        for offset in [-50_000, 50_000, i64::MIN, i64::MAX] {
            assert!(needs_clock_write(offset));
        }
    }

    #[test]
    fn first_valid_result_drops_all_other_futures() {
        struct CountDrop<'a>(&'a Cell<usize>);
        impl Drop for CountDrop<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        let started = Cell::new(0);
        let dropped = Cell::new(0);
        let queries = [0, 1, 2, 3, 4].map(|index| {
            let (started, dropped) = (&started, &dropped);
            async move {
                let _drop = CountDrop(dropped);
                started.set(started.get() + 1);
                match index {
                    0 => None,
                    3 => {
                        while started.get() < 5 {
                            tokio::task::yield_now().await;
                        }
                        Some(index)
                    }
                    _ => pending().await,
                }
            }
        });
        assert_eq!(network(first_success(queries)).unwrap(), Some(3));
        assert_eq!(started.get(), 5);
        assert_eq!(dropped.get(), 5);
    }

    #[test]
    fn unsynchronized_ntp_reply_cannot_win_udp_race() {
        let winner = network(async {
            let bad = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let good = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let bad_peer = bad.local_addr().unwrap();
            let good_peer = good.local_addr().unwrap();
            let respond = |socket: UdpSocket, li: u8| async move {
                let mut request = [0; 48];
                let (_, client) = socket.recv_from(&mut request).await.unwrap();
                if li == 0 {
                    sleep(Duration::from_millis(25)).await;
                }
                let mut reply = [0; 48];
                reply[0] = (li << 6) | (4 << 3) | 4;
                reply[1] = 1;
                for field in reply[24..48].as_chunks_mut::<8>().0 {
                    field.copy_from_slice(&request[40..48]);
                }
                socket.send_to(&reply, client).await.unwrap();
            };
            let bad = tokio::spawn(respond(bad, 3));
            let good = tokio::spawn(respond(good, 0));
            let result = first_success(
                [("bad", bad_peer), ("good", good_peer)].map(|(name, peer)| query_peer(name, peer)),
            )
            .await;
            bad.abort();
            good.abort();
            result
        })
        .unwrap()
        .unwrap();
        assert_eq!(winner.host, "good");
    }
}
