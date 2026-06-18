use ft8rs::decode::dx::DxExposure;
use ft8rs::decode::lib_jtdx::JtdxStreamDecodeSession;
use ft8rs::input::audio::{read_wav_mono_f32, resample_linear};
use ft8rs::stream::{
    DecodeProfile, ProfileStreamDecodeSession, SlotTimestamp, StreamDecodeConfig,
    StreamDecodeSession,
};
use std::path::{Path, PathBuf};
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
    if cfg!(debug_assertions) {
        panic!("stream decode acceptance tests must be run with --release");
    }
}

fn samples_12k_from_wav(path: &str) -> Vec<f32> {
    let audio = read_wav_mono_f32(path).unwrap();
    if audio.sample_rate == 12000 {
        audio.samples
    } else {
        resample_linear(&audio.samples, audio.sample_rate, 12000)
    }
}

fn samples_12k_from_wav_path(path: &Path) -> Vec<f32> {
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
#[ignore = "manual DX real-on-band false-alarm gate; run with --release --ignored"]
fn test_dx_profile_real_on_band_absent_target_no_deep_false_alarm() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps);
    let target = "ZZ1ZZZ";
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        hiscall: Some(target.to_string()),
        nfqso: 1000.0,
        nfa: 900.0,
        nfb: 1100.0,
        dx_deep_experimental_output: true,
        ..Default::default()
    });
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let mut rows = Vec::new();

    for seg in 0..nseg {
        let timestamp = start.add_seconds((seg * 15) as i64);
        let data = slot_samples(&samples, seg * sps, sps);
        for row in decoder.decode_slot_at(&timestamp, &data) {
            rows.push((timestamp.clone(), row));
        }
    }

    assert!(
        rows.is_empty(),
        "DX absent-target real-on-band gate should emit 0 rows for {target}: {:?}",
        rows.iter()
            .map(|(timestamp, row)| format!(
                "{} {:.0} {:+.1} {}",
                timestamp.format_time(),
                row.freq,
                row.dt,
                row.msg
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "manual DX real-on-band false-alarm matrix gate; run with --release --ignored"]
fn test_dx_profile_real_on_band_absent_target_matrix_no_deep_false_alarm() {
    assert_release_mode();
    let samples = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps).min(6);
    let targets = ["ZZ1ZZZ", "QQ9QQQ", "K0ZZZ", "N0ABC"];
    let focuses = [500.0, 1000.0, 1500.0, 2200.0];
    let start = SlotTimestamp::parse("230208_140300").unwrap();
    let mut rows = Vec::new();

    for target in targets {
        for focus in focuses {
            let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
                profile: DecodeProfile::Dx,
                hiscall: Some(target.to_string()),
                nfqso: focus,
                nfa: (focus - 100.0).max(200.0),
                nfb: (focus + 100.0).min(3000.0),
                dx_deep_experimental_output: true,
                ..Default::default()
            });

            for seg in 0..nseg {
                let timestamp = start.add_seconds((seg * 15) as i64);
                let data = slot_samples(&samples, seg * sps, sps);
                for row in decoder.decode_slot_at(&timestamp, &data) {
                    rows.push((target.to_string(), focus, timestamp.clone(), row));
                }
            }
        }
    }

    assert!(
        rows.is_empty(),
        "DX absent-target real-on-band matrix gate should emit 0 rows: {:?}",
        rows.iter()
            .map(|(target, focus, timestamp, row)| format!(
                "target={target} focus={focus:.0} {} {:.0} {:+.1} {}",
                timestamp.format_time(),
                row.freq,
                row.dt,
                row.msg
            ))
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "manual DX G2 external corpus gate; set FT8RS_DX_G2_CORPUS and run with --release --ignored"]
fn test_dx_profile_external_g2_corpus_no_deep_false_alarm() {
    assert_release_mode();
    let Ok(root) = std::env::var("FT8RS_DX_G2_CORPUS") else {
        eprintln!("DX external G2 corpus skipped: FT8RS_DX_G2_CORPUS is not set");
        return;
    };
    let root = PathBuf::from(root);
    assert!(
        root.is_dir(),
        "FT8RS_DX_G2_CORPUS must point to a directory: {}",
        root.display()
    );

    let mut summaries = Vec::new();

    let cases = external_g2_cases(&root);
    for spec in external_g2_specs() {
        let cases_for_spec: Vec<_> = cases
            .iter()
            .filter(|case| case.label == spec.label)
            .collect();
        if cases_for_spec.is_empty() {
            continue;
        }
        let wavs = collect_wavs(&root.join(spec.subdir));
        let summary = run_external_g2_spec(spec, &cases_for_spec, &wavs);
        summaries.push(summary);
    }

    validate_external_g2_summaries(&summaries);

    let totals = external_g2_totals(&summaries);
    let pfa95_slots = rule_of_three_upper(totals.slots);
    let pfa95_focus = rule_of_three_upper(totals.exposure.focus_trials);
    let pfa95_hypothesis = rule_of_three_upper(totals.exposure.hypothesis_trials);
    let pfa95_stack_osd = rule_of_three_upper(totals.exposure.stack_osd_attempts);
    let summary_text = external_g2_summary_text(&summaries);
    eprintln!(
        "DX external G2 corpus: emitted_fabrications={}/{}, pfa95_slots<={pfa95_slots:.6}, pfa95_focus<={pfa95_focus:.6}, pfa95_hypothesis<={pfa95_hypothesis:.6}, pfa95_stack_osd<={pfa95_stack_osd:.6}, slots={} focus_trials={} field_trials={} hypothesis_trials={} stack_osd_candidates={} stack_osd_attempts={} stack_osd_skipped_budget={} deep_rows_emitted={} summaries=[{summary_text}]",
        totals.fabricated,
        totals.slots,
        totals.exposure.slots,
        totals.exposure.focus_trials,
        totals.exposure.field_trials,
        totals.exposure.hypothesis_trials,
        totals.exposure.stack_osd_candidates,
        totals.exposure.stack_osd_attempts,
        totals.exposure.stack_osd_skipped_budget,
        totals.exposure.deep_rows_emitted
    );
    assert_eq!(
        totals.exposure.slots, totals.slots,
        "external G2 exposure slot count must match decoded trial slots"
    );
    assert_eq!(
        totals.fabricated, 0,
        "DX external G2 corpus fabricated target rows: {summaries:?}"
    );
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ExternalG2Totals {
    slots: usize,
    fabricated: usize,
    exposure: DxExposure,
}

#[derive(Clone, Copy, Debug)]
struct ExternalG2Spec {
    label: &'static str,
    subdir: &'static str,
    min_slots: usize,
}

#[derive(Clone, Debug, PartialEq)]
struct ExternalG2Case {
    label: &'static str,
    wav: Option<String>,
    target: String,
    mycall: Option<String>,
    focus: f64,
    nfa: f64,
    nfb: f64,
}

#[derive(Debug)]
struct ExternalG2Summary {
    label: &'static str,
    files: usize,
    slots: usize,
    exposure: DxExposure,
    fabricated: usize,
}

fn external_g2_totals(summaries: &[ExternalG2Summary]) -> ExternalG2Totals {
    let mut totals = ExternalG2Totals::default();
    for summary in summaries {
        totals.slots += summary.slots;
        totals.fabricated += summary.fabricated;
        totals.exposure.accumulate(summary.exposure);
    }
    totals
}

fn external_g2_summary_text(summaries: &[ExternalG2Summary]) -> String {
    summaries
        .iter()
        .map(|summary| {
            format!(
                "{}:files={} slots={} focus_trials={} field_trials={} hypothesis_trials={} stack_osd_candidates={} stack_osd_attempts={} stack_osd_skipped_budget={} deep_rows_emitted={} fabricated={}",
                summary.label,
                summary.files,
                summary.slots,
                summary.exposure.focus_trials,
                summary.exposure.field_trials,
                summary.exposure.hypothesis_trials,
                summary.exposure.stack_osd_candidates,
                summary.exposure.stack_osd_attempts,
                summary.exposure.stack_osd_skipped_budget,
                summary.exposure.deep_rows_emitted,
                summary.fabricated
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_external_g2_summaries(summaries: &[ExternalG2Summary]) {
    assert!(
        summaries.len() == external_g2_specs().len(),
        "FT8RS_DX_G2_CORPUS must contain wav files in every required subdirectory: noise/, wrong_call/, on_band/, hash_collision/; found {summaries:?}"
    );
    for spec in external_g2_specs() {
        let summary = summaries
            .iter()
            .find(|summary| summary.label == spec.label)
            .expect("all external G2 corpus specs must be present");
        assert!(
            summary.slots >= spec.min_slots,
            "DX external G2 {} corpus has {} slots, needs at least {}",
            spec.label,
            summary.slots,
            spec.min_slots
        );
    }
}

fn rule_of_three_upper(exposure: usize) -> f64 {
    3.0 / exposure.max(1) as f64
}

fn external_g2_specs() -> [ExternalG2Spec; 4] {
    [
        ExternalG2Spec {
            label: "noise",
            subdir: "noise",
            min_slots: 5760,
        },
        ExternalG2Spec {
            label: "wrong_call",
            subdir: "wrong_call",
            min_slots: 1000,
        },
        ExternalG2Spec {
            label: "real_on_band",
            subdir: "on_band",
            min_slots: 480,
        },
        ExternalG2Spec {
            label: "hash_collision",
            subdir: "hash_collision",
            min_slots: 50,
        },
    ]
}

fn external_g2_cases(root: &Path) -> Vec<ExternalG2Case> {
    let manifest = root.join("manifest.csv");
    if manifest.exists() {
        parse_external_g2_manifest(&manifest)
    } else {
        external_g2_default_cases()
    }
}

fn external_g2_default_cases() -> Vec<ExternalG2Case> {
    vec![
        ExternalG2Case {
            label: "noise",
            wav: None,
            target: "ZZ1ZZZ".to_string(),
            mycall: None,
            focus: 1000.0,
            nfa: 900.0,
            nfb: 1100.0,
        },
        ExternalG2Case {
            label: "wrong_call",
            wav: None,
            target: "ZZ1ZZZ".to_string(),
            mycall: None,
            focus: 1000.0,
            nfa: 900.0,
            nfb: 1100.0,
        },
        ExternalG2Case {
            label: "real_on_band",
            wav: None,
            target: "ZZ1ZZZ".to_string(),
            mycall: None,
            focus: 1000.0,
            nfa: 900.0,
            nfb: 1100.0,
        },
        ExternalG2Case {
            label: "hash_collision",
            wav: None,
            target: "BG5ATV".to_string(),
            mycall: Some("K1JT".to_string()),
            focus: 1000.0,
            nfa: 900.0,
            nfb: 1100.0,
        },
    ]
}

fn parse_external_g2_manifest(path: &Path) -> Vec<ExternalG2Case> {
    let content = std::fs::read_to_string(path).unwrap_or_else(|err| {
        panic!(
            "failed to read DX external G2 manifest {}: {err}",
            path.display()
        )
    });
    let mut cases = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.to_ascii_lowercase().starts_with("label,") {
            continue;
        }
        let parts: Vec<&str> = line.split(',').map(str::trim).collect();
        assert!(
            parts.len() == 7,
            "DX external G2 manifest {} line {} must have exactly columns label,wav,target,mycall,focus,nfa,nfb",
            path.display(),
            line_no + 1
        );
        let label = external_g2_label(parts[0]).unwrap_or_else(|| {
            panic!(
                "DX external G2 manifest {} line {} has unknown label '{}'",
                path.display(),
                line_no + 1,
                parts[0]
            )
        });
        let wav = parse_manifest_wav_name(path, line_no, parts[1]);
        let target = parts[2].to_ascii_uppercase();
        assert!(
            !target.is_empty(),
            "DX external G2 manifest {} line {} must name an absent target",
            path.display(),
            line_no + 1
        );
        let focus = parse_manifest_f64(path, line_no, "focus", parts[4]);
        let nfa = parse_manifest_f64(path, line_no, "nfa", parts[5]);
        let nfb = parse_manifest_f64(path, line_no, "nfb", parts[6]);
        validate_manifest_frequency_window(path, line_no, focus, nfa, nfb);
        cases.push(ExternalG2Case {
            label,
            wav,
            target,
            mycall: (!parts[3].is_empty()).then(|| parts[3].to_ascii_uppercase()),
            focus,
            nfa,
            nfb,
        });
    }
    assert!(
        !cases.is_empty(),
        "DX external G2 manifest {} did not define any cases",
        path.display()
    );
    cases
}

fn external_g2_label(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "noise" => Some("noise"),
        "wrong_call" | "wrong-call" => Some("wrong_call"),
        "on_band" | "real_on_band" => Some("real_on_band"),
        "hash_collision" => Some("hash_collision"),
        _ => None,
    }
}

fn parse_manifest_wav_name(path: &Path, line_no: usize, value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    assert!(
        !value.contains('/') && !value.contains('\\'),
        "DX external G2 manifest {} line {} wav must be a file name, not a path: '{}'",
        path.display(),
        line_no + 1,
        value
    );
    assert!(
        value.to_ascii_lowercase().ends_with(".wav"),
        "DX external G2 manifest {} line {} wav must name a .wav file: '{}'",
        path.display(),
        line_no + 1,
        value
    );
    Some(value.to_string())
}

fn parse_manifest_f64(path: &Path, line_no: usize, name: &str, value: &str) -> f64 {
    value.parse::<f64>().unwrap_or_else(|err| {
        panic!(
            "DX external G2 manifest {} line {} has invalid {name} '{}': {err}",
            path.display(),
            line_no + 1,
            value
        )
    })
}

fn validate_manifest_frequency_window(path: &Path, line_no: usize, focus: f64, nfa: f64, nfb: f64) {
    assert!(
        focus.is_finite() && nfa.is_finite() && nfb.is_finite(),
        "DX external G2 manifest {} line {} focus/nfa/nfb must be finite",
        path.display(),
        line_no + 1
    );
    assert!(
        (0.0..=5000.0).contains(&nfa) && (0.0..=5000.0).contains(&nfb),
        "DX external G2 manifest {} line {} nfa/nfb must be inside 0..=5000 Hz",
        path.display(),
        line_no + 1
    );
    assert!(
        nfa < nfb,
        "DX external G2 manifest {} line {} must have nfa < nfb",
        path.display(),
        line_no + 1
    );
    assert!(
        focus >= nfa && focus <= nfb,
        "DX external G2 manifest {} line {} focus must be inside nfa..nfb",
        path.display(),
        line_no + 1
    );
}

fn run_external_g2_spec(
    spec: ExternalG2Spec,
    cases: &[&ExternalG2Case],
    wavs: &[PathBuf],
) -> ExternalG2Summary {
    let mut slots = 0usize;
    let mut exposure = DxExposure::default();
    let mut fabricated = Vec::new();
    let selected_wavs = external_g2_selected_wavs(spec, cases, wavs);
    for wav in &selected_wavs {
        let samples = samples_12k_from_wav_path(wav);
        let sps = 15 * 12000;
        let nseg = samples.len().div_ceil(sps);
        let start = timestamp_from_wav_path(wav);
        for case in cases
            .iter()
            .filter(|case| external_g2_case_applies(case, wav))
        {
            let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
                profile: DecodeProfile::Dx,
                hiscall: Some(case.target.clone()),
                mycall: case.mycall.clone(),
                nfqso: case.focus,
                nfa: case.nfa,
                nfb: case.nfb,
                dx_deep_experimental_output: true,
                ..Default::default()
            });

            for seg in 0..nseg {
                slots += 1;
                let timestamp = start.add_seconds((seg * 15) as i64);
                let data = slot_samples(&samples, seg * sps, sps);
                for row in decoder.decode_slot_at(&timestamp, &data) {
                    fabricated.push(format!(
                        "{}:{} target={} focus={:.0} {:.0} {:+.1} {}",
                        wav.display(),
                        timestamp.format_time(),
                        case.target,
                        case.focus,
                        row.freq,
                        row.dt,
                        row.msg
                    ));
                }
            }
            exposure.accumulate(
                decoder
                    .dx_exposure()
                    .expect("external G2 gate must use the DX profile"),
            );
        }
    }
    assert!(
        fabricated.is_empty(),
        "DX external G2 {} corpus fabricated target rows: {:?}",
        spec.label,
        fabricated
    );
    ExternalG2Summary {
        label: spec.label,
        files: selected_wavs.len(),
        slots,
        exposure,
        fabricated: fabricated.len(),
    }
}

fn external_g2_selected_wavs(
    spec: ExternalG2Spec,
    cases: &[&ExternalG2Case],
    wavs: &[PathBuf],
) -> Vec<PathBuf> {
    for case in cases.iter().filter(|case| case.label == spec.label) {
        if let Some(name) = case.wav.as_deref() {
            assert!(
                wavs.iter().any(|wav| {
                    wav.file_name()
                        .and_then(|value| value.to_str())
                        .is_some_and(|file_name| file_name == name)
                }),
                "DX external G2 manifest references missing wav '{}' in {} corpus",
                name,
                spec.subdir
            );
        }
    }

    let mut selected = Vec::new();
    for wav in wavs {
        if cases
            .iter()
            .any(|case| case.label == spec.label && external_g2_case_applies(case, wav))
        {
            selected.push(wav.clone());
        }
    }
    selected.sort();
    selected
}

fn external_g2_case_applies(case: &ExternalG2Case, wav: &Path) -> bool {
    match case.wav.as_deref() {
        None => true,
        Some(name) => wav
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|file_name| file_name == name),
    }
}

#[test]
fn test_external_g2_manifest_parses_cases_and_labels() {
    let path = write_temp_manifest(
        "label,wav,target,mycall,focus,nfa,nfb\n\
         noise,noise_0001.wav,ZZ1ZZZ,,1000,900,1100\n\
         wrong_call,wrong_0001.wav,ZZ1ZZZ,,1000,900,1100\n\
         on_band,,QQ9QQQ,K1JT,1500,1400,1600\n\
         hash_collision,hash_0001.wav,BG5ATV,K1JT,1000,900,1100\n",
    );

    let cases = parse_external_g2_manifest(&path);

    assert_eq!(cases.len(), 4);
    assert_eq!(cases[0].label, "noise");
    assert_eq!(cases[0].wav.as_deref(), Some("noise_0001.wav"));
    assert_eq!(cases[0].target, "ZZ1ZZZ");
    assert_eq!(cases[0].mycall, None);
    assert_eq!(cases[1].label, "wrong_call");
    assert_eq!(cases[1].wav.as_deref(), Some("wrong_0001.wav"));
    assert_eq!(cases[2].label, "real_on_band");
    assert_eq!(cases[2].wav, None);
    assert_eq!(cases[2].target, "QQ9QQQ");
    assert_eq!(cases[2].mycall.as_deref(), Some("K1JT"));
    assert_eq!(cases[3].label, "hash_collision");
}

#[test]
fn test_external_g2_example_manifest_stays_parseable() {
    let cases = parse_external_g2_manifest(Path::new("tests/ft8/g2_manifest.example.csv"));

    assert_eq!(cases.len(), 4);
    for spec in external_g2_specs() {
        assert!(
            cases.iter().any(|case| case.label == spec.label),
            "example manifest missing {}",
            spec.label
        );
    }
}

#[test]
#[should_panic(expected = "focus must be inside nfa..nfb")]
fn test_external_g2_manifest_rejects_focus_outside_window() {
    let path = write_temp_manifest(
        "label,wav,target,mycall,focus,nfa,nfb\n\
         noise,noise_0001.wav,ZZ1ZZZ,,1200,900,1100\n",
    );

    let _ = parse_external_g2_manifest(&path);
}

#[test]
#[should_panic(expected = "must have nfa < nfb")]
fn test_external_g2_manifest_rejects_reversed_window() {
    let path = write_temp_manifest(
        "label,wav,target,mycall,focus,nfa,nfb\n\
         noise,noise_0001.wav,ZZ1ZZZ,,1000,1100,900\n",
    );

    let _ = parse_external_g2_manifest(&path);
}

#[test]
#[should_panic(expected = "wav must be a file name")]
fn test_external_g2_manifest_rejects_wav_paths() {
    let path = write_temp_manifest(
        "label,wav,target,mycall,focus,nfa,nfb\n\
         noise,subdir/noise_0001.wav,ZZ1ZZZ,,1000,900,1100\n",
    );

    let _ = parse_external_g2_manifest(&path);
}

#[test]
#[should_panic(expected = "must have exactly columns")]
fn test_external_g2_manifest_rejects_extra_columns() {
    let path = write_temp_manifest(
        "label,wav,target,mycall,focus,nfa,nfb\n\
         noise,noise_0001.wav,ZZ1ZZZ,,1000,900,1100,unexpected\n",
    );

    let _ = parse_external_g2_manifest(&path);
}

#[test]
fn test_external_g2_manifest_selects_specific_and_wildcard_wavs() {
    let wildcard = ExternalG2Case {
        label: "noise",
        wav: None,
        target: "ZZ1ZZZ".to_string(),
        mycall: None,
        focus: 1000.0,
        nfa: 900.0,
        nfb: 1100.0,
    };
    let specific = ExternalG2Case {
        label: "noise",
        wav: Some("b.wav".to_string()),
        target: "QQ9QQQ".to_string(),
        mycall: None,
        focus: 1500.0,
        nfa: 1400.0,
        nfb: 1600.0,
    };
    let wavs = [PathBuf::from("a.wav"), PathBuf::from("b.wav")];

    assert_eq!(
        external_g2_selected_wavs(external_g2_specs()[0], &[&specific], &wavs),
        vec![PathBuf::from("b.wav")]
    );
    assert_eq!(
        external_g2_selected_wavs(external_g2_specs()[0], &[&wildcard], &wavs),
        vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")]
    );
}

#[test]
#[should_panic(expected = "references missing wav")]
fn test_external_g2_manifest_rejects_specific_wav_not_present() {
    let specific = ExternalG2Case {
        label: "noise",
        wav: Some("missing.wav".to_string()),
        target: "ZZ1ZZZ".to_string(),
        mycall: None,
        focus: 1000.0,
        nfa: 900.0,
        nfb: 1100.0,
    };
    let wavs = [PathBuf::from("present.wav")];

    let _ = external_g2_selected_wavs(external_g2_specs()[0], &[&specific], &wavs);
}

#[test]
fn test_external_g2_default_cases_cover_required_categories() {
    let cases = external_g2_default_cases();
    for spec in external_g2_specs() {
        assert!(
            cases.iter().any(|case| case.label == spec.label),
            "missing default external G2 case for {}",
            spec.label
        );
    }
}

#[test]
fn test_external_g2_summary_preserves_exposure_denominators() {
    let summaries = vec![
        ExternalG2Summary {
            label: "noise",
            files: 2,
            slots: 10,
            exposure: DxExposure {
                slots: 10,
                focus_trials: 20,
                field_trials: 18,
                hypothesis_trials: 900,
                stack_osd_candidates: 4,
                stack_osd_attempts: 3,
                stack_osd_skipped_budget: 1,
                deep_rows_emitted: 0,
            },
            fabricated: 0,
        },
        ExternalG2Summary {
            label: "real_on_band",
            files: 1,
            slots: 5,
            exposure: DxExposure {
                slots: 5,
                focus_trials: 7,
                field_trials: 6,
                hypothesis_trials: 300,
                stack_osd_candidates: 2,
                stack_osd_attempts: 1,
                stack_osd_skipped_budget: 1,
                deep_rows_emitted: 0,
            },
            fabricated: 0,
        },
    ];

    let totals = external_g2_totals(&summaries);
    assert_eq!(totals.slots, 15);
    assert_eq!(totals.fabricated, 0);
    assert_eq!(totals.exposure.slots, 15);
    assert_eq!(totals.exposure.focus_trials, 27);
    assert_eq!(totals.exposure.field_trials, 24);
    assert_eq!(totals.exposure.hypothesis_trials, 1200);
    assert_eq!(totals.exposure.stack_osd_candidates, 6);
    assert_eq!(totals.exposure.stack_osd_attempts, 4);
    assert_eq!(totals.exposure.stack_osd_skipped_budget, 2);

    let text = external_g2_summary_text(&summaries);
    assert!(text.contains("noise:files=2 slots=10 focus_trials=20"));
    assert!(text.contains("hypothesis_trials=900"));
    assert!(text.contains("stack_osd_candidates=4"));
    assert!(text.contains("stack_osd_attempts=3"));
    assert!(text.contains("stack_osd_skipped_budget=1"));
    assert!(text.contains("deep_rows_emitted=0"));
    assert!(text.contains("real_on_band:files=1 slots=5 focus_trials=7"));
}

#[test]
fn test_external_g2_summary_validation_accepts_complete_budget() {
    let summaries = external_g2_budget_summaries(0);

    validate_external_g2_summaries(&summaries);
}

#[test]
#[should_panic(expected = "must contain wav files in every required subdirectory")]
fn test_external_g2_summary_validation_rejects_missing_category() {
    let mut summaries = external_g2_budget_summaries(0);
    summaries.pop();

    validate_external_g2_summaries(&summaries);
}

#[test]
#[should_panic(expected = "needs at least")]
fn test_external_g2_summary_validation_rejects_short_budget() {
    let summaries = external_g2_budget_summaries(1);

    validate_external_g2_summaries(&summaries);
}

#[test]
fn test_external_g2_rule_of_three_uses_requested_denominator() {
    assert_eq!(rule_of_three_upper(0), 3.0);
    assert_eq!(rule_of_three_upper(1), 3.0);
    assert!((rule_of_three_upper(1000) - 0.003).abs() < f64::EPSILON);
}

fn external_g2_budget_summaries(shortfall: usize) -> Vec<ExternalG2Summary> {
    external_g2_specs()
        .into_iter()
        .map(|spec| {
            let slots = spec.min_slots.saturating_sub(shortfall);
            ExternalG2Summary {
                label: spec.label,
                files: 1,
                slots,
                exposure: DxExposure {
                    slots,
                    focus_trials: slots,
                    field_trials: slots,
                    hypothesis_trials: slots,
                    stack_osd_candidates: 0,
                    stack_osd_attempts: 0,
                    stack_osd_skipped_budget: 0,
                    deep_rows_emitted: 0,
                },
                fabricated: 0,
            }
        })
        .collect()
}

fn write_temp_manifest(content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ft8rs_g2_manifest_{}_{}.csv",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, content).unwrap();
    path
}

fn collect_wavs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut wavs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("wav"))
        })
        .collect();
    wavs.sort();
    wavs
}

fn timestamp_from_wav_path(path: &Path) -> SlotTimestamp {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    for token in stem.split(|ch: char| !(ch.is_ascii_digit() || ch == '_')) {
        if (token.len() == 13 || token.len() == 6)
            && token.chars().all(|ch| ch.is_ascii_digit() || ch == '_')
        {
            if let Ok(timestamp) = SlotTimestamp::parse(token) {
                return timestamp;
            }
        }
    }
    SlotTimestamp::parse("230208_140300").unwrap()
}

#[test]
#[ignore = "manual DX profile gate; run with --release --ignored"]
fn test_dx_profile_synthetic_reproducible() {
    assert_release_mode();
    let first = decode_dx_synthetic_ua3qna_keys(true, 3);
    let second = decode_dx_synthetic_ua3qna_keys(true, 3);

    assert_eq!(
        first, second,
        "DX experimental deep output should be reproducible across repeated runs"
    );
}

fn decode_dx_synthetic_ua3qna_keys(
    dx_deep_experimental_output: bool,
    max_segments: usize,
) -> Vec<String> {
    let samples = samples_12k_from_wav("tests/ft8/dx_synth_ua3qna.wav");
    let sps = 15 * 12000;
    let nseg = samples.len().div_ceil(sps).min(max_segments);
    let mut decoder = ProfileStreamDecodeSession::new(StreamDecodeConfig {
        profile: DecodeProfile::Dx,
        mycall: Some("F1MLZ".to_string()),
        hiscall: Some("UA3QNA".to_string()),
        dx_deep_experimental_output,
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
