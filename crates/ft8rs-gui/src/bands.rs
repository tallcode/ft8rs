//! Band labels for the live decode log, based on WSJT-X UDP dial status.
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::wsjtx_udp::DialStatus;

const HISTORY_CAP: usize = 1024;
const SLOT_MS: u64 = 15_000;
const STATUS_LIFETIME: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct CurrentBand {
    sources: HashMap<(SocketAddr, String), DialSource>,
    history: VecDeque<DialStatus>,
}

struct DialSource {
    frequency_hz: u64,
    received: Instant,
}

impl CurrentBand {
    pub(crate) fn observe(&mut self, mut status: DialStatus) {
        self.expire(status.received);
        let key = (status.peer, status.id.clone());
        match status.frequency_hz {
            None => {
                if let Some(source) = self.sources.get_mut(&key) {
                    source.received = status.received;
                    status.frequency_hz = Some(source.frequency_hz);
                } else {
                    return;
                }
            }
            Some(0) => {
                self.sources.remove(&key);
            }
            Some(frequency_hz) => {
                self.sources.insert(
                    key,
                    DialSource {
                        frequency_hz,
                        received: status.received,
                    },
                );
            }
        }
        if self.history.len() == HISTORY_CAP {
            self.history.pop_front();
        }
        self.history.push_back(status);
    }

    fn expire(&mut self, now: Instant) {
        self.sources
            .retain(|_, source| now.saturating_duration_since(source.received) < STATUS_LIFETIME);
    }

    pub(crate) fn clear(&mut self) {
        self.sources.clear();
        self.history.clear();
    }

    pub(crate) fn name_at_slot(
        &self,
        slot: u64,
        clock_step: Option<Instant>,
    ) -> Option<&'static str> {
        let start_ms = slot * SLOT_MS;
        let mut seen = HashSet::new();
        let mut frequencies = self.history.iter().rev().filter_map(|status| {
            if status.received_ms > start_ms
                || clock_step.is_some_and(|step| status.received < step)
                || start_ms - status.received_ms >= STATUS_LIFETIME.as_millis() as u64
                || !seen.insert((status.peer, status.id.as_str()))
            {
                return None;
            }
            status.frequency_hz.filter(|&hz| hz != 0)
        });
        let hz = frequencies.next()?;
        frequencies
            .all(|other| other == hz)
            .then(|| band_name(hz))
            .flatten()
    }

    #[cfg(test)]
    fn name(&self) -> Option<&'static str> {
        let mut sources = self.sources.values();
        let hz = sources.next()?.frequency_hz;
        if sources.all(|source| source.frequency_hz == hz) {
            band_name(hz)
        } else {
            None
        }
    }
}

fn band_name(hz: u64) -> Option<&'static str> {
    // ADIF names and inclusive frequency limits used by WSJT-X's Bands model.
    Some(match hz {
        135_700..=137_800 => "2190m",
        472_000..=479_000 => "630m",
        501_000..=504_000 => "560m",
        1_800_000..=2_000_000 => "160m",
        3_500_000..=4_000_000 => "80m",
        5_060_000..=5_450_000 => "60m",
        7_000_000..=7_300_000 => "40m",
        10_100_000..=10_150_000 => "30m",
        14_000_000..=14_350_000 => "20m",
        18_068_000..=18_168_000 => "17m",
        21_000_000..=21_450_000 => "15m",
        24_890_000..=24_990_000 => "12m",
        28_000_000..=29_700_000 => "10m",
        40_000_000..=45_000_000 => "8m",
        50_000_000..=54_000_000 => "6m",
        54_000_001..=69_900_000 => "5m",
        70_000_000..=71_000_000 => "4m",
        144_000_000..=148_000_000 => "2m",
        222_000_000..=225_000_000 => "1.25m",
        420_000_000..=450_000_000 => "70cm",
        902_000_000..=928_000_000 => "33cm",
        1_240_000_000..=1_300_000_000 => "23cm",
        2_300_000_000..=2_450_000_000 => "13cm",
        3_300_000_000..=3_500_000_000 => "9cm",
        5_650_000_000..=5_925_000_000 => "6cm",
        10_000_000_000..=10_500_000_000 => "3cm",
        24_000_000_000..=24_250_000_000 => "1.25cm",
        47_000_000_000..=47_200_000_000 => "6mm",
        75_500_000_000..=81_000_000_000 => "4mm",
        119_980_000_000..=125_020_000_000 => "2.5mm",
        133_000_000_000..=149_000_000_000 => "2mm",
        241_000_000_000..=250_000_000_000 => "1mm",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(received: Instant, id: &str, frequency_hz: u64) -> DialStatus {
        DialStatus {
            peer: "127.0.0.1:2237".parse().unwrap(),
            id: id.into(),
            frequency_hz: Some(frequency_hz),
            received,
            received_ms: 0,
        }
    }

    #[test]
    fn the_latest_dial_status_replaces_the_previous_frequency() {
        let mut band = CurrentBand::default();
        let now = Instant::now();
        assert_eq!(band.name(), None);
        band.observe(status(now, "WSJT-X", 14_074_000));
        assert_eq!(band.name(), Some("20m"));
        band.observe(status(now + Duration::from_secs(15), "WSJT-X", 7_074_000));
        assert_eq!(band.name(), Some("40m"));
        band.clear();
        assert_eq!(band.name(), None);
    }

    #[test]
    fn conflicting_dial_frequencies_suppress_the_band_until_they_agree() {
        let mut band = CurrentBand::default();
        let now = Instant::now();
        band.observe(status(now, "WSJT-X", 14_074_000));
        band.observe(status(now, "other", 14_074_000));
        assert_eq!(band.name(), Some("20m"));

        let later = now + Duration::from_secs(15);
        band.observe(status(later, "other", 14_080_000));
        assert_eq!(band.name(), None);
        band.observe(status(later, "WSJT-X", 14_080_000));
        assert_eq!(band.name(), Some("20m"));
    }

    #[test]
    fn closing_one_of_two_conflicting_senders_with_the_same_id_restores_the_band() {
        let mut band = CurrentBand::default();
        let now = Instant::now();
        let other_peer = "127.0.0.1:2238".parse().unwrap();
        band.observe(status(now, "WSJT-X", 14_074_000));
        let mut other = status(now, "WSJT-X", 7_074_000);
        other.peer = other_peer;
        band.observe(other);
        assert_eq!(band.name(), None);

        let mut close = status(now + Duration::from_secs(15), "WSJT-X", 0);
        close.peer = other_peer;
        band.observe(close);
        assert_eq!(band.name(), Some("20m"));
    }

    #[test]
    fn a_stale_source_stops_blocking_the_band_of_another_active_source() {
        let mut band = CurrentBand::default();
        let now = Instant::now();
        band.observe(status(now, "WSJT-X", 14_074_000));
        band.observe(status(now, "other", 7_074_000));
        let mut heartbeat = status(now + Duration::from_secs(45), "other", 0);
        heartbeat.frequency_hz = None;
        band.observe(heartbeat);

        band.expire(now + STATUS_LIFETIME);
        assert_eq!(band.name(), Some("40m"));
    }

    #[test]
    fn a_late_decode_uses_the_dial_status_at_its_slot_start() {
        let mut band = CurrentBand::default();
        let now = Instant::now();
        let mut old = status(now, "WSJT-X", 14_074_000);
        old.received_ms = 149_900;
        band.observe(old);
        let retune = now + Duration::from_millis(15_200);
        let mut new = status(retune, "WSJT-X", 7_074_000);
        new.received_ms = 165_100;
        band.observe(new);
        assert_eq!(band.name_at_slot(9, None), None);
        assert_eq!(band.name_at_slot(10, None), Some("20m"));
        assert_eq!(band.name_at_slot(11, None), Some("20m"));
        assert_eq!(band.name_at_slot(12, None), Some("40m"));
        assert_eq!(band.name_at_slot(15, None), Some("40m"));
        assert_eq!(band.name_at_slot(16, None), None);
        assert_eq!(band.name_at_slot(10, Some(retune)), None);
    }

    #[test]
    fn band_names_use_wsjtx_frequency_limits() {
        assert_eq!(band_name(14_000_000), Some("20m"));
        assert_eq!(band_name(14_350_000), Some("20m"));
        assert_eq!(band_name(14_350_001), None);
        assert_eq!(band_name(5_760_000_000), Some("6cm"));
    }

    #[test]
    fn heartbeat_keeps_an_idle_dial_alive_until_close() {
        let mut band = CurrentBand::default();
        let now = Instant::now();
        band.observe(status(now, "WSJT-X", 14_074_000));
        let mut heartbeat = status(now + Duration::from_secs(45), "WSJT-X", 0);
        heartbeat.frequency_hz = None;
        band.observe(heartbeat);
        band.expire(now + Duration::from_secs(75));
        assert_eq!(band.name(), Some("20m"));
        band.observe(status(now + Duration::from_secs(90), "WSJT-X", 0));
        assert_eq!(band.name(), None);
    }
}
