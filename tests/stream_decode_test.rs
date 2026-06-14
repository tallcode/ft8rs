use ft8rs::fft_engine_name;
use ft8rs::input::audio::{read_wav_mono_f32, resample_linear};
use ft8rs::stream::{SlotTimestamp, StreamDecodeConfig, StreamDecodeSession};
const SHORT_TARGET_ACCEPTED_FLOOR: usize = 19;
const LONG_TARGET_ACCEPTED_FLOOR: usize = 424;

#[derive(Clone, Debug)]
struct BaselineRow {
    seg: usize,
    drift: String,
    msg: String,
    norm_msg: String,
    ignored: bool,
}

#[derive(Default, Debug)]
struct TimingStats {
    offsets: Vec<f64>,
}

impl TimingStats {
    fn push(&mut self, baseline_drift: &str, decoded_dt: f64) {
        if let Ok(baseline_drift) = baseline_drift.parse::<f64>() {
            self.offsets.push(baseline_drift - decoded_dt);
        }
    }

    fn summary(&self) -> Option<TimingSummary> {
        if self.offsets.is_empty() {
            return None;
        }

        let mut sorted = self.offsets.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let n = sorted.len();
        let mean = sorted.iter().sum::<f64>() / n as f64;
        let median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) * 0.5
        };
        let p10 = percentile(&sorted, 0.10);
        let p90 = percentile(&sorted, 0.90);
        Some(TimingSummary {
            count: n,
            mean,
            median,
            p10,
            p90,
        })
    }
}

#[derive(Debug)]
struct TimingSummary {
    count: usize,
    mean: f64,
    median: f64,
    p10: f64,
    p90: f64,
}

fn norm(msg: &str) -> String {
    msg.split_whitespace()
        .map(normalize_message_token_for_match)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_message_token_for_match(token: &str) -> String {
    let token = token.trim().to_uppercase();
    if token.starts_with('<') && token.ends_with('>') {
        let inner = &token[1..token.len() - 1];
        if inner != "..." && !inner.is_empty() {
            return inner.to_string();
        }
    }
    token
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = p.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

fn segment_from_timestamp(ts: &str) -> usize {
    if ts.len() >= 13 {
        let t = &ts[ts.len() - 6..];
        let h: i64 = t[0..2].parse().unwrap_or(0);
        let m: i64 = t[2..4].parse().unwrap_or(0);
        let s: i64 = t[4..6].parse().unwrap_or(0);
        ((h * 3600 + m * 60 + s - (14 * 3600 + 3 * 60)) / 15).max(0) as usize
    } else {
        0
    }
}

fn slot_samples(samples: &[f32], start: usize, len: usize) -> Vec<f32> {
    let mut out = vec![0.0; len];
    for (dst, sample) in out.iter_mut().enumerate() {
        if let Some(value) = samples.get(start + dst) {
            *sample = *value;
        }
    }
    out
}

fn parse_baseline(path: &str) -> Vec<BaselineRow> {
    let content = std::fs::read_to_string(path).unwrap();
    let mut results = Vec::new();
    for line in content.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 5 {
            continue;
        }
        let date_time = parts[0].trim().to_string();
        let msg = parts[4].trim().to_string();
        let extra_marker = parts.get(5).map_or("", |value| value.trim()).to_string();
        let ignored = is_ignored_baseline_marker(&extra_marker);
        results.push(BaselineRow {
            seg: segment_from_timestamp(&date_time),
            drift: parts[2].trim().to_string(),
            norm_msg: norm(&msg),
            msg,
            ignored,
        });
    }
    results
}

fn is_ignored_baseline_marker(value: &str) -> bool {
    // Extra column semantics:
    //   blank = multi-verified baseline
    //   W     = WSJT-X-only decode, still part of the WSJT-X target baseline
    //   J/E   = JTDX/other extra decodes, ignored while aligning to WSJT-X
    matches!(value.trim().to_ascii_uppercase().as_str(), "J" | "E")
}

fn assert_release_mode() {
    assert!(
        !cfg!(debug_assertions),
        "stream decode acceptance tests must be run with --release"
    );
}

fn samples_12k_from_wav(path: &str) -> Vec<f32> {
    let audio = read_wav_mono_f32(path).unwrap();
    if audio.sample_rate == 12000 {
        audio.samples
    } else {
        resample_linear(&audio.samples, audio.sample_rate, 12000)
    }
}

#[test]
fn test_stream_decode_short_audio() {
    assert_release_mode();
    let t0 = std::time::Instant::now();
    let samples = samples_12k_from_wav("tests/ft8/210703_133430.wav");

    let mut decoder = StreamDecodeSession::new(StreamDecodeConfig {
        nfa: 100.0,
        ..Default::default()
    });

    let results = decoder.decode_slot(&samples);
    let elapsed = t0.elapsed();
    assert!(
        elapsed.as_secs_f64() < 15.0,
        "Short decode timeout: {:.1}s > 15s",
        elapsed.as_secs_f64()
    );

    let baseline = parse_baseline("tests/ft8/210703_133430.csv");
    let target: Vec<_> = baseline.iter().filter(|row| !row.ignored).collect();
    let mut used_results = vec![false; results.len()];
    let mut matched = 0;
    let mut misses = Vec::new();
    for row in &target {
        if let Some((idx, _)) = results
            .iter()
            .enumerate()
            .find(|(idx, d)| !used_results[*idx] && norm(&d.msg) == row.norm_msg)
        {
            used_results[idx] = true;
            matched += 1;
        } else {
            misses.push(row.msg.clone());
        }
    }
    let plus_count = used_results.iter().filter(|used| !**used).count();

    println!(
        "\n[ENGINE={}] [STREAM SHORT DECODE] decoded {} | matched {}/{} | plus {} | {:.1}s",
        fft_engine_name(),
        results.len(),
        matched,
        target.len(),
        plus_count,
        elapsed.as_secs_f64()
    );
    if !misses.is_empty() {
        println!("  Misses:");
        for msg in &misses {
            println!("    {}", msg);
        }
    }
    assert!(
        matched >= SHORT_TARGET_ACCEPTED_FLOOR,
        "STREAM SHORT: matched {}/{} < {}",
        matched,
        target.len(),
        SHORT_TARGET_ACCEPTED_FLOOR
    );
}

#[test]
fn test_stream_decode_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;

    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let ignored_count = baseline.iter().filter(|row| row.ignored).count();
    println!(
        "\n[ENGINE={}] [STREAM LONG DECODE] {} segments, {} baseline messages ({} J/E ignored in diff), slot_start_offset=+0.000s",
        fft_engine_name(),
        nseg,
        baseline.len(),
        ignored_count
    );

    let config = StreamDecodeConfig {
        ..Default::default()
    };
    let mut decoder = StreamDecodeSession::new(config);

    let mut total_matched = 0;
    let mut primary_matched = 0;
    let primary_total = baseline.iter().filter(|row| !row.ignored).count();
    let accepted_floor = LONG_TARGET_ACCEPTED_FLOOR;
    let severe_floor = accepted_floor.saturating_sub(10);
    let mut timing_stats = TimingStats::default();

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);

        let slot_t0 = std::time::Instant::now();
        let results = decoder.decode_slot_at(&timestamp, &data);
        let elapsed_ms = slot_t0.elapsed().as_millis() as u64;
        assert!(
            elapsed_ms <= 15_000,
            "SLOT {} TIMEOUT: {}ms > 15s",
            seg,
            elapsed_ms
        );

        let bl: Vec<_> = baseline.iter().filter(|row| row.seg == seg).collect();
        let mut used_results = vec![false; results.len()];
        let mut matched = 0;
        for row in &bl {
            if let Some((idx, result)) = results
                .iter()
                .enumerate()
                .find(|(idx, d)| !used_results[*idx] && norm(&d.msg) == row.norm_msg)
            {
                used_results[idx] = true;
                matched += 1;
                if !row.ignored {
                    primary_matched += 1;
                }
                timing_stats.push(&row.drift, result.dt);
            }
        }
        total_matched += matched;
        println!(
            "  Seg {}: decoded {} | matched {}/{} | {}ms",
            seg,
            results.len(),
            matched,
            bl.len(),
            elapsed_ms
        );

        let remaining_baseline = baseline
            .iter()
            .filter(|row| !row.ignored && row.seg > seg)
            .count();
        assert!(
            primary_matched + remaining_baseline >= severe_floor,
            "STREAM LONG sensitivity abort at seg {}: target matched {} + target remaining {} < {}",
            seg,
            primary_matched,
            remaining_baseline,
            severe_floor,
        );
    }

    let rate = total_matched as f64 / baseline.len() as f64 * 100.0;
    println!("\n[STREAM LONG DECODE SUMMARY]");
    println!(
        "  Total matched: {}/{} ({:.1}%)",
        total_matched,
        baseline.len(),
        rate
    );
    println!(
        "  WSJT-X baseline matched: {}/{}",
        primary_matched, primary_total
    );
    if let Some(timing) = timing_stats.summary() {
        println!(
            "  Timing residual: baseline_drift-decoded_dt mean={:+.3}s median={:+.3}s p10={:+.3}s p90={:+.3}s n={}",
            timing.mean, timing.median, timing.p10, timing.p90, timing.count
        );
    }
    assert!(
        primary_matched >= accepted_floor,
        "STREAM LONG WSJT-X target: {}/{} < {}",
        primary_matched,
        primary_total,
        accepted_floor
    );
}
