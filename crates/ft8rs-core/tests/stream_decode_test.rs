use ft8rs::decode::lib_jtdx::JtdxStreamDecodeSession;
use ft8rs::input::audio::{read_wav_mono_f32, resample_linear};
use ft8rs::stream::{
    DecodeProfile, ProfileStreamDecodeSession, SlotTimestamp, StreamDecodeConfig,
    StreamDecodeSession,
};
const SHORT_TARGET_ACCEPTED_FLOOR: usize = 19;
const LONG_TARGET_ACCEPTED_FLOOR: usize = 424;
const JTDX_SHORT_TARGET_ACCEPTED_FLOOR: usize = 20;
const JTDX_LONG_TARGET_ACCEPTED_FLOOR: usize = 430;
const HYBRID_LONG_TARGET_COUNT: usize = 465;

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

impl TimingSummary {
    fn describe(&self) -> String {
        format!(
            "mean={:+.3}s median={:+.3}s p10={:+.3}s p90={:+.3}s n={}",
            self.mean, self.median, self.p10, self.p90, self.count
        )
    }
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
    parse_baseline_with(path, is_ignored_baseline_marker)
}

fn parse_jtdx_baseline(path: &str) -> Vec<BaselineRow> {
    parse_baseline_with(path, is_ignored_jtdx_baseline_marker)
}

fn parse_baseline_with(path: &str, is_ignored: fn(&str) -> bool) -> Vec<BaselineRow> {
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
        let ignored = is_ignored(&extra_marker);
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

fn is_ignored_jtdx_baseline_marker(value: &str) -> bool {
    // Extra column semantics for JTDX:
    //   blank = multi-verified baseline
    //   J     = JTDX-only decode, still part of the JTDX target baseline
    //   W/E   = WSJT-X/other extra decodes, ignored while aligning to JTDX
    matches!(value.trim().to_ascii_uppercase().as_str(), "W" | "E")
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

    assert!(
        matched >= SHORT_TARGET_ACCEPTED_FLOOR,
        "STREAM SHORT: decoded {} | matched {}/{} | plus {} | misses {:?} | {:.1}s < floor {}",
        results.len(),
        matched,
        target.len(),
        plus_count,
        misses,
        elapsed.as_secs_f64(),
        SHORT_TARGET_ACCEPTED_FLOOR
    );
}

#[test]
fn test_stream_decode_a8d_audio() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/a8d_k1jt_bg5atv_pm00.wav");
    let target = "K1JT BG5ATV PM00";

    let without_a8 = StreamDecodeSession::new(StreamDecodeConfig {
        lft8apon: false,
        nfa: 900.0,
        nfb: 1100.0,
        nfqso: 1000.0,
        ..Default::default()
    })
    .decode_slot(&samples);
    assert!(
        !without_a8.iter().any(|d| norm(&d.msg) == target),
        "a8 fixture should not decode without a8d context: {:?}",
        without_a8
            .iter()
            .map(|d| d.msg.as_str())
            .collect::<Vec<_>>()
    );

    let with_a8 = StreamDecodeSession::new(StreamDecodeConfig {
        lft8apon: true,
        nfa: 900.0,
        nfb: 1100.0,
        nfqso: 1000.0,
        mycall: Some("K1JT".to_string()),
        hiscall: Some("BG5ATV".to_string()),
        hisgrid: Some("PM00".to_string()),
        ..Default::default()
    })
    .decode_slot(&samples);
    assert!(
        with_a8.iter().any(|d| norm(&d.msg) == target),
        "a8 fixture should decode with a8d context: {:?}",
        with_a8.iter().map(|d| d.msg.as_str()).collect::<Vec<_>>()
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

    assert!(
        primary_matched >= accepted_floor,
        "STREAM LONG WSJT-X target: primary {}/{} < {}, total matched {}/{}, timing {:?}",
        primary_matched,
        primary_total,
        accepted_floor,
        total_matched,
        baseline.len(),
        timing_stats
            .summary()
            .map(|summary| summary.describe())
            .unwrap_or_else(|| "n/a".to_string())
    );
}

#[test]
#[ignore = "manual JTDX profile gate; run with --release --ignored"]
fn test_jtdx_profile_short_audio() {
    assert_release_mode();
    let t0 = std::time::Instant::now();
    let samples = samples_12k_from_wav("tests/ft8/210703_133430.wav");
    let mut decoder = JtdxStreamDecodeSession::new(StreamDecodeConfig {
        nfa: 100.0,
        ..Default::default()
    });
    let timestamp = SlotTimestamp::parse("210703_133430").unwrap();
    let results = decoder
        .decode_slot_streaming_at(&timestamp, &samples, |_| Ok(()))
        .unwrap();
    let elapsed = t0.elapsed();

    let baseline = parse_jtdx_baseline("tests/ft8/210703_133430.csv");
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

    assert!(
        matched >= JTDX_SHORT_TARGET_ACCEPTED_FLOOR,
        "JTDX SHORT: decoded {} | matched {}/{} < {} | misses {:?} | {:.1}s",
        results.len(),
        matched,
        target.len(),
        JTDX_SHORT_TARGET_ACCEPTED_FLOOR,
        misses,
        elapsed.as_secs_f64()
    );
}

#[test]
#[ignore = "manual DX profile gate; run with --release --ignored"]
fn test_dx_profile_synthetic_ua3qna() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/dx_synth_ua3qna.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps);
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        mycall: Some("F1MLZ".to_string()),
        hiscall: Some("UA3QNA".to_string()),
        ..Default::default()
    });
    let start = SlotTimestamp::parse("230208_140630").unwrap();
    let mut rows = Vec::new();

    for seg in 0..nseg {
        let timestamp = start.add_seconds((seg * 15) as i64);
        let data = slot_samples(&samples, seg * sps, sps);
        let slot_rows = decoder.decode_slot_at(&timestamp, &data);
        for row in slot_rows {
            rows.push((timestamp.clone(), row));
        }
    }

    assert!(
        rows.iter()
            .all(|(_, row)| norm(&row.msg).contains("UA3QNA")),
        "DX profile should emit only target rows: {:?}",
        rows.iter()
            .map(|(_, row)| row.msg.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        rows.iter()
            .any(|(timestamp, row)| timestamp.format_time() == "140700"
                && norm(&row.msg) == "F1MLZ UA3QNA -04"),
        "DX synthetic fixture should recover the weak 140700 target row: {:?}",
        rows.iter()
            .map(|(timestamp, row)| format!("{} {}", timestamp.format_time(), row.msg))
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "manual DX profile gate; run with --release --ignored"]
fn test_dx_profile_long_ua3qna() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps);
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        mycall: Some("F1MLZ".to_string()),
        hiscall: Some("UA3QNA".to_string()),
        ..Default::default()
    });
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let baseline = parse_jtdx_baseline("tests/ft8/230208_140300.csv");
    let mut rows = Vec::new();

    for seg in 0..nseg {
        let timestamp = start.add_seconds((seg * 15) as i64);
        let data = slot_samples(&samples, seg * sps, sps);
        let slot_rows = decoder.decode_slot_at(&timestamp, &data);
        for row in slot_rows {
            rows.push((timestamp.clone(), row));
        }
    }

    let unsupported = rows
        .iter()
        .filter(|(timestamp, row)| {
            let seg = segment_from_timestamp(&timestamp.format());
            !baseline
                .iter()
                .any(|baseline| baseline.seg == seg && baseline.norm_msg == norm(&row.msg))
        })
        .count();
    assert_eq!(unsupported, 0, "DX UA3QNA FP budget should be 0 rows");
    assert!(
        rows.iter()
            .all(|(_, row)| norm(&row.msg).contains("UA3QNA")),
        "DX profile should emit only target rows: {:?}",
        rows.iter()
            .map(|(_, row)| row.msg.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        rows.iter()
            .any(|(timestamp, row)| timestamp.format_time() == "140700"
                && norm(&row.msg) == "F1MLZ UA3QNA -04"),
        "DX long fixture should recover the weak 140700 target row: {:?}",
        rows.iter()
            .map(|(timestamp, row)| format!("{} {}", timestamp.format_time(), row.msg))
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "manual DX profile gate; run with --release --ignored"]
fn test_dx_profile_synthetic_reproducible() {
    assert_release_mode();
    let first = decode_dx_synthetic_ua3qna_keys();
    let second = decode_dx_synthetic_ua3qna_keys();

    assert_eq!(
        first, second,
        "DX file-mode output should be reproducible across repeated runs"
    );
}

fn decode_dx_synthetic_ua3qna_keys() -> Vec<String> {
    let samples = samples_12k_from_wav("tests/ft8/dx_synth_ua3qna.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps);
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        mycall: Some("F1MLZ".to_string()),
        hiscall: Some("UA3QNA".to_string()),
        ..Default::default()
    });
    let start = SlotTimestamp::parse("230208_140630").unwrap();
    let mut keys = Vec::new();
    for seg in 0..nseg {
        let timestamp = start.add_seconds((seg * 15) as i64);
        let data = slot_samples(&samples, seg * sps, sps);
        let rows = decoder.decode_slot_at(&timestamp, &data);
        keys.extend(rows.into_iter().map(|row| {
            format!(
                "{}|{:.0}|{:+.1}|{}",
                timestamp.format_time(),
                row.freq,
                row.dt,
                norm(&row.msg)
            )
        }));
    }
    keys
}

#[test]
fn test_dx_profile_a8d_fixture() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/a8d_k1jt_bg5atv_pm00.wav");
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        nfa: 900.0,
        nfb: 1100.0,
        nfqso: 1000.0,
        mycall: Some("K1JT".to_string()),
        hiscall: Some("BG5ATV".to_string()),
        hisgrid: Some("PM00".to_string()),
        ..Default::default()
    });
    let timestamp = SlotTimestamp::parse("230208_140300").unwrap();
    let rows = decoder.decode_slot_at(&timestamp, &samples);

    assert!(
        rows.iter().any(|row| norm(&row.msg) == "K1JT BG5ATV PM00"),
        "DX profile should recover the a8d fixture: {:?}",
        rows.iter().map(|row| row.msg.as_str()).collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "manual hybrid profile gate; run with --release --ignored"]
fn test_hybrid_profile_long_audio_count() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps);
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Hybrid,
        ..Default::default()
    });
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let mut total = 0usize;

    for seg in 0..nseg {
        let timestamp = start.add_seconds((seg * 15) as i64);
        let data = slot_samples(&samples, seg * sps, sps);
        total += decoder.decode_slot_at(&timestamp, &data).len();
    }

    assert_eq!(
        total, HYBRID_LONG_TARGET_COUNT,
        "HYBRID LONG decoded count changed"
    );
}

/// The staged (monitor) hybrid path must emit exactly the same row set as the
/// one-shot (file) path — only the timing differs (WSJT-X early rows stream at
/// nzhsym=41 instead of nzhsym=50). This guards the monitor early-decode change.
#[test]
#[ignore = "manual hybrid staged-vs-one-shot equivalence gate; run with --release --ignored"]
fn test_hybrid_staged_matches_oneshot() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps);
    let new_hybrid = || {
        ProfileStreamDecodeSession::new(StreamDecodeConfig {
            profile: DecodeProfile::Hybrid,
            ..Default::default()
        })
    };
    let mut oneshot = new_hybrid();
    let mut staged = new_hybrid();
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let key = |row: &ft8rs::stream::StreamDecodedMessage| {
        format!("{}|{}|{:.2}|{:.1}", row.msg, row.snr, row.dt, row.freq)
    };

    for seg in 0..nseg {
        let timestamp = start.add_seconds((seg * 15) as i64);
        let data = slot_samples(&samples, seg * sps, sps);

        let mut a = Vec::new();
        oneshot
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |row| {
                a.push(key(&row.decode));
                Ok(())
            })
            .unwrap();

        let mut b = Vec::new();
        let mut state = staged.start_slot();
        let n41 = &data[..(41 * 3456).min(data.len())];
        let early = staged
            .decode_slot_nzhsym41_streaming_with_provenance(&timestamp, &mut state, n41, |row| {
                b.push(key(&row.decode));
                Ok(())
            })
            .unwrap();
        let n47 = &data[..(47 * 3456).min(data.len())];
        staged.subtract_slot_nzhsym47(&mut state, n47);
        staged
            .decode_slot_nzhsym50_streaming_with_provenance(
                &timestamp,
                state,
                early,
                &data,
                |row| {
                    b.push(key(&row.decode));
                    Ok(())
                },
            )
            .unwrap();

        a.sort();
        b.sort();
        assert_eq!(a, b, "slot {seg}: staged emit set differs from one-shot");
    }
}

#[test]
#[ignore = "manual JTDX profile gate; run with --release --ignored"]
fn test_jtdx_profile_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;
    let baseline = parse_jtdx_baseline("tests/ft8/230208_140300.csv");
    let primary_total = baseline.iter().filter(|row| !row.ignored).count();
    let mut decoder = JtdxStreamDecodeSession::new(StreamDecodeConfig::default());
    let mut primary_matched = 0;
    let mut total_matched = 0;

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let slot_t0 = std::time::Instant::now();
        let results = decoder
            .decode_slot_streaming_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let elapsed_ms = slot_t0.elapsed().as_millis() as u64;
        assert!(
            elapsed_ms <= 15_000,
            "JTDX SLOT {} TIMEOUT: {}ms > 15s",
            seg,
            elapsed_ms
        );

        let bl: Vec<_> = baseline.iter().filter(|row| row.seg == seg).collect();
        let mut used_results = vec![false; results.len()];
        let mut matched = 0;
        for row in &bl {
            if let Some((idx, _)) = results
                .iter()
                .enumerate()
                .find(|(idx, d)| !used_results[*idx] && norm(&d.msg) == row.norm_msg)
            {
                used_results[idx] = true;
                matched += 1;
                if !row.ignored {
                    primary_matched += 1;
                }
            }
        }
        total_matched += matched;
    }

    assert!(
        primary_matched >= JTDX_LONG_TARGET_ACCEPTED_FLOOR,
        "JTDX LONG: primary {}/{} < {}, total matched {}/{}",
        primary_matched,
        primary_total,
        JTDX_LONG_TARGET_ACCEPTED_FLOOR,
        total_matched,
        baseline.len()
    );
}
