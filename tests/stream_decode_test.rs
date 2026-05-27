use ft8rs::fft_engine_name;
use ft8rs::input::audio::{read_wav_mono_f32, resample_linear};
use ft8rs::stream::{StreamDecodeConfig, StreamDecodeSession};
use std::collections::HashSet;

#[derive(Clone, Debug)]
struct BaselineRow {
    seg: usize,
    date_time: String,
    snr: String,
    drift: String,
    freq: String,
    msg: String,
    norm_msg: String,
}

#[derive(Clone, Debug)]
struct DiffRow {
    date_time: String,
    snr: String,
    drift: String,
    freq: String,
    msg: String,
    tag: char,
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
        let h: usize = t[0..2].parse().unwrap_or(0);
        let m: usize = t[2..4].parse().unwrap_or(0);
        let s: usize = t[4..6].parse().unwrap_or(0);
        (h * 3600 + m * 60 + s - (14 * 3600 + 3 * 60)) / 15
    } else {
        0
    }
}

fn timestamp_for_segment(seg: usize) -> String {
    let total = 14 * 3600 + 3 * 60 + seg * 15;
    format!(
        "230208_{:02}{:02}{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

fn slot_with_start_offset(samples: &[f32], start: isize, len: usize) -> Vec<f32> {
    let mut out = vec![0.0; len];
    for (dst, sample) in out.iter_mut().enumerate() {
        let src = start + dst as isize;
        if src >= 0 {
            if let Some(value) = samples.get(src as usize) {
                *sample = *value;
            }
        }
    }
    out
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn diff_row_to_csv(row: &DiffRow) -> String {
    format!(
        "{},{},{},{},{},{}",
        csv_escape(&row.date_time),
        csv_escape(&row.snr),
        csv_escape(&row.drift),
        csv_escape(&row.freq),
        csv_escape(&row.msg),
        row.tag
    )
}

fn write_diff_csv(path: &str, rows: &[DiffRow]) {
    let mut out = String::from("Date-Time,SNR,Drift,Freq,Msg,Tag\n");
    for row in rows {
        out.push_str(&diff_row_to_csv(row));
        out.push('\n');
    }
    std::fs::write(path, out).unwrap();
}

fn parse_baseline(path: &str) -> Vec<BaselineRow> {
    let content = std::fs::read_to_string(path).unwrap();
    let mut results = Vec::new();
    for line in content.lines().skip(1) {
        let line = line.trim().trim_end_matches(',');
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() < 5 {
            continue;
        }
        let date_time = parts[0].trim().to_string();
        let msg = parts[4].trim().to_string();
        results.push(BaselineRow {
            seg: segment_from_timestamp(&date_time),
            date_time,
            snr: parts[1].trim().to_string(),
            drift: parts[2].trim().to_string(),
            freq: parts[3].trim().to_string(),
            norm_msg: norm(&msg),
            msg,
        });
    }
    results
}

fn assert_release_mode() {
    assert!(
        !cfg!(debug_assertions),
        "stream decode acceptance tests must be run with --release"
    );
}

#[test]
fn test_stream_decode_short_audio() {
    assert_release_mode();
    let t0 = std::time::Instant::now();
    let audio = read_wav_mono_f32("tests/ft8/210703_133430.wav").unwrap();
    let samples = resample_linear(&audio.samples, audio.sample_rate, 12000);

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

    let mut seen = HashSet::new();
    let mut unique_msgs: Vec<String> = Vec::new();
    for r in &results {
        let n = norm(&r.msg);
        if !seen.contains(&n) {
            seen.insert(n);
            unique_msgs.push(r.msg.clone());
        }
    }

    println!(
        "\n[ENGINE={}] [STREAM SHORT DECODE] {} unique messages in {:.1}s",
        fft_engine_name(),
        unique_msgs.len(),
        elapsed.as_secs_f64()
    );
    for m in &unique_msgs {
        println!("  {}", m);
    }
    assert!(
        unique_msgs.len() >= 19,
        "STREAM SHORT: {} < 19",
        unique_msgs.len()
    );
}

#[test]
fn test_stream_decode_long_audio() {
    assert_release_mode();
    let audio = read_wav_mono_f32("tests/ft8/230208_140300.wav").unwrap();
    let s12k = resample_linear(&audio.samples, audio.sample_rate, 12000);
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;
    let start_offset_sec = std::env::var("FT8RS_SLOT_START_OFFSET_SEC")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    let start_offset_samples = (start_offset_sec * 12000.0).round() as isize;

    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    println!(
        "\n[ENGINE={}] [STREAM LONG DECODE] {} segments, {} baseline messages, slot_start_offset={:+.3}s",
        fft_engine_name(),
        nseg,
        baseline.len(),
        start_offset_sec
    );

    let config = StreamDecodeConfig {
        ..Default::default()
    };
    let mut decoder = StreamDecodeSession::new(config);

    let mut total_matched = 0;
    let accepted_floor = 422usize;
    let acceptance_enabled = start_offset_samples == 0;
    let severe_floor = if acceptance_enabled {
        accepted_floor.saturating_sub(10)
    } else {
        0
    };
    let mut diff_rows = Vec::new();
    let mut timing_stats = TimingStats::default();

    for seg in 0..nseg {
        let seg_start = seg as isize * sps as isize - start_offset_samples;
        let data = slot_with_start_offset(&s12k, seg_start, sps);

        let slot_t0 = std::time::Instant::now();
        let results = decoder.decode_slot(&data);
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
        let mut missed = Vec::new();
        for row in &bl {
            if let Some((idx, result)) = results
                .iter()
                .enumerate()
                .find(|(idx, d)| !used_results[*idx] && norm(&d.msg) == row.norm_msg)
            {
                used_results[idx] = true;
                matched += 1;
                timing_stats.push(&row.drift, result.dt);
            } else {
                missed.push(row.norm_msg.clone());
                diff_rows.push(DiffRow {
                    date_time: row.date_time.clone(),
                    snr: row.snr.clone(),
                    drift: row.drift.clone(),
                    freq: row.freq.clone(),
                    msg: row.msg.clone(),
                    tag: '-',
                });
            }
        }
        for (idx, result) in results.iter().enumerate() {
            if !used_results[idx] {
                diff_rows.push(DiffRow {
                    date_time: timestamp_for_segment(seg),
                    snr: format!("{:.0}", result.snr.round()),
                    drift: format!("{:.1}", result.dt),
                    freq: format!("{:.0}", result.freq.round()),
                    msg: result.msg.clone(),
                    tag: '+',
                });
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
        if std::env::var("FT8RS_PRINT_MISSES").ok().as_deref() == Some("1") && !missed.is_empty() {
            for msg in missed {
                println!("    MISS {}", msg);
            }
        }

        if acceptance_enabled {
            let remaining_baseline = baseline.iter().filter(|row| row.seg > seg).count();
            assert!(
                total_matched + remaining_baseline >= severe_floor,
                "STREAM LONG sensitivity abort at seg {}: matched {} + remaining {} < {}",
                seg,
                total_matched,
                remaining_baseline,
                severe_floor,
            );
        }
    }

    let rate = total_matched as f64 / baseline.len() as f64 * 100.0;
    println!("\n[STREAM LONG DECODE SUMMARY]");
    println!(
        "  Total matched: {}/{} ({:.1}%)",
        total_matched,
        baseline.len(),
        rate
    );
    if let Some(timing) = timing_stats.summary() {
        println!(
            "  Timing offset estimate: start_offset=baseline_drift-decoded_dt mean={:+.3}s median={:+.3}s p10={:+.3}s p90={:+.3}s n={}",
            timing.mean, timing.median, timing.p10, timing.p90, timing.count
        );
    }
    if std::env::var("FT8RS_WRITE_DIFF").ok().as_deref() == Some("1") {
        write_diff_csv("tests/ft8/230208_140300_diff.csv", &diff_rows);
    }
    if acceptance_enabled {
        assert!(
            total_matched >= accepted_floor,
            "STREAM LONG: {}/{} < {}",
            total_matched,
            baseline.len(),
            accepted_floor
        );
    }
}
