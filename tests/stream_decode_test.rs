use ft8rs::decode::hybrid::{
    build_passive_evidence_store_from_tagged, divergence_report_from_evidence,
    hybrid_hash_call_opportunity_report, ActiveCallContext, HashCallOpportunityReport,
    HybridDivergenceReport, QsoContextOpportunityReport,
};
use ft8rs::decode::lib_jtdx::JtdxStreamDecodeSession;
use ft8rs::fft_engine_name;
use ft8rs::input::audio::{read_wav_mono_f32, resample_linear};
use ft8rs::stream::session::StreamDecodeProvenance;
use ft8rs::stream::{SlotTimestamp, StreamDecodeConfig, StreamDecodeSession, StreamDecodedMessage};
const SHORT_TARGET_ACCEPTED_FLOOR: usize = 19;
const LONG_TARGET_ACCEPTED_FLOOR: usize = 424;
const JTDX_SHORT_TARGET_ACCEPTED_FLOOR: usize = 20;
const JTDX_LONG_TARGET_ACCEPTED_FLOOR: usize = 430;

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

fn count_supported_by_baseline<'a>(
    rows: impl IntoIterator<Item = &'a StreamDecodedMessage>,
    baseline: &[BaselineRow],
) -> (usize, usize) {
    let mut supported = 0;
    let mut unsupported = 0;
    for row in rows {
        if baseline
            .iter()
            .any(|baseline| baseline.norm_msg == norm(&row.msg))
        {
            supported += 1;
        } else {
            unsupported += 1;
        }
    }
    (supported, unsupported)
}

fn is_same_decoded_signal(a: &StreamDecodedMessage, b: &StreamDecodedMessage) -> bool {
    norm(&a.msg) == norm(&b.msg) && (a.freq - b.freq).abs() <= 5.0 && (a.dt - b.dt).abs() <= 0.3
}

fn unique_new_rows(
    candidates: Vec<StreamDecodedMessage>,
    existing: &[StreamDecodedMessage],
) -> Vec<StreamDecodedMessage> {
    let mut rows = Vec::new();
    for candidate in candidates {
        if existing
            .iter()
            .chain(rows.iter())
            .any(|row| is_same_decoded_signal(row, &candidate))
        {
            continue;
        }
        rows.push(candidate);
    }
    rows
}

fn replay_jtdx_with_qso_hint(
    base_config: &StreamDecodeConfig,
    timestamp: &SlotTimestamp,
    samples: &[f32],
    hint: &ActiveQsoHint,
) -> Vec<StreamDecodedMessage> {
    let mut config = base_config.clone_for_profile_jtdx();
    config.hiscall = Some(hint.hiscall.clone());
    config.nfqso = hint.nfqso;
    config.stophint = false;
    let mut decoder = JtdxStreamDecodeSession::new(config);
    decoder
        .decode_slot_streaming_at(timestamp, samples, |_| Ok(()))
        .unwrap_or_default()
}

fn jtdx_only_regular_seed_rows(
    wsjtx_rows: &[StreamDecodedMessage],
    jtdx_rows: &[ft8rs::stream::session::StreamDecodedWithProvenance],
) -> Vec<StreamDecodedMessage> {
    jtdx_rows
        .iter()
        .filter(|row| row.provenance == StreamDecodeProvenance::Regular)
        .filter(|row| {
            !wsjtx_rows
                .iter()
                .any(|wsjtx| is_same_decoded_signal(wsjtx, &row.decode))
        })
        .map(|row| row.decode.clone())
        .collect()
}

#[derive(Clone, Debug)]
struct ActiveQsoHint {
    hiscall: String,
    nfqso: f64,
}

impl From<&ft8rs::decode::hybrid::QsoContextHint> for ActiveQsoHint {
    fn from(value: &ft8rs::decode::hybrid::QsoContextHint) -> Self {
        Self {
            hiscall: value.hiscall.clone(),
            nfqso: value.nfqso,
        }
    }
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

    println!(
        "\n[ENGINE={}] [STREAM A8D DECODE] without_a8={} | with_a8={}",
        fft_engine_name(),
        without_a8.len(),
        with_a8.len()
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

    println!(
        "\n[ENGINE={}] [JTDX SHORT DECODE] decoded {} | matched {}/{} | {:.1}s",
        fft_engine_name(),
        results.len(),
        matched,
        target.len(),
        elapsed.as_secs_f64()
    );
    if !misses.is_empty() {
        println!("  Misses:");
        for msg in &misses {
            println!("    {}", msg);
        }
    }
    assert!(
        matched >= JTDX_SHORT_TARGET_ACCEPTED_FLOOR,
        "JTDX SHORT: matched {}/{} < {}",
        matched,
        target.len(),
        JTDX_SHORT_TARGET_ACCEPTED_FLOOR
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

    println!(
        "\n[ENGINE={}] [JTDX LONG DECODE] {} segments, {} target messages",
        fft_engine_name(),
        nseg,
        primary_total
    );
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
        println!(
            "  Seg {}: decoded {} | matched {}/{} | {}ms",
            seg,
            results.len(),
            matched,
            bl.len(),
            elapsed_ms
        );
    }

    println!("\n[JTDX LONG DECODE SUMMARY]");
    println!("  Total matched: {}/{}", total_matched, baseline.len());
    println!(
        "  JTDX baseline matched: {}/{}",
        primary_matched, primary_total
    );
    assert!(
        primary_matched >= JTDX_LONG_TARGET_ACCEPTED_FLOOR,
        "JTDX LONG: {}/{} < {}",
        primary_matched,
        primary_total,
        JTDX_LONG_TARGET_ACCEPTED_FLOOR
    );
}

#[test]
#[ignore = "manual Phase 0 diagnostic; run with --release --ignored"]
fn test_hybrid_phase0_opportunity_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;

    let mut wsjtx = StreamDecodeSession::new(StreamDecodeConfig::default());
    let mut jtdx = JtdxStreamDecodeSession::new(StreamDecodeConfig::default());
    let mut hash_total = HashCallOpportunityReport::default();
    let mut qso_context = ActiveCallContext::new(4);
    let mut qso_total = QsoContextOpportunityReport::default();
    let mut divergence_total = HybridDivergenceReport::default();
    let mut wsjtx_unique_supported = 0usize;
    let mut wsjtx_unique_unsupported = 0usize;
    let mut jtdx_unique_supported = 0usize;
    let mut jtdx_unique_unsupported = 0usize;

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let wsjtx_tagged = wsjtx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let jtdx_tagged = jtdx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let wsjtx_rows: Vec<_> = wsjtx_tagged.iter().map(|row| row.decode.clone()).collect();
        let jtdx_rows: Vec<_> = jtdx_tagged.iter().map(|row| row.decode.clone()).collect();

        let hash_report = hybrid_hash_call_opportunity_report(&wsjtx_rows, &jtdx_rows);
        let evidence = build_passive_evidence_store_from_tagged(&wsjtx_tagged, &jtdx_tagged);
        qso_context.update_from_evidence(&evidence);
        qso_total.observe_slot(qso_context.hints());
        let divergence_report = divergence_report_from_evidence(&evidence);
        let wsjtx_unique_rows: Vec<_> = evidence
            .rows()
            .iter()
            .filter(|row| row.sources == vec![ft8rs::decode::hybrid::DecoderId::WSJTX])
            .map(|row| StreamDecodedMessage {
                freq: row.freq_hz,
                dt: row.dt_sec,
                snr: row.snr_db as f64,
                msg: row.message.clone(),
                sync: 0.0,
                itone: [0; 79],
            })
            .collect();
        let jtdx_unique_rows: Vec<_> = evidence
            .rows()
            .iter()
            .filter(|row| row.sources == vec![ft8rs::decode::hybrid::DecoderId::JTDX])
            .map(|row| StreamDecodedMessage {
                freq: row.freq_hz,
                dt: row.dt_sec,
                snr: row.snr_db as f64,
                msg: row.message.clone(),
                sync: 0.0,
                itone: [0; 79],
            })
            .collect();
        let (slot_wsjtx_supported, slot_wsjtx_unsupported) =
            count_supported_by_baseline(&wsjtx_unique_rows, &baseline);
        let (slot_jtdx_supported, slot_jtdx_unsupported) =
            count_supported_by_baseline(&jtdx_unique_rows, &baseline);
        wsjtx_unique_supported += slot_wsjtx_supported;
        wsjtx_unique_unsupported += slot_wsjtx_unsupported;
        jtdx_unique_supported += slot_jtdx_supported;
        jtdx_unique_unsupported += slot_jtdx_unsupported;

        if hash_report.unresolved_hash_rows > 0
            || hash_report.rows_resolvable_by_other_decoder > 0
            || divergence_report.wsjtx_unique_rows > 0
            || divergence_report.jtdx_unique_rows > 0
        {
            println!(
                "  Seg {}: hash_unresolved={} hash_resolvable={} shared={} wsjtx_unique={} jtdx_unique={} qso_hints={}",
                seg,
                hash_report.unresolved_hash_rows,
                hash_report.rows_resolvable_by_other_decoder,
                divergence_report.shared_rows,
                divergence_report.wsjtx_unique_rows,
                divergence_report.jtdx_unique_rows,
                qso_context.hints().len(),
            );
        }

        hash_total.merge(hash_report);
        divergence_total.merge(divergence_report);
    }

    println!(
        "\n[ENGINE={}] [HYBRID PHASE0 OPPORTUNITY]",
        fft_engine_name()
    );
    println!(
        "  HashCallHint: unresolved_hash_rows={} | resolvable_by_other_decoder={} | hash_conflicts={}",
        hash_total.unresolved_hash_rows,
        hash_total.rows_resolvable_by_other_decoder,
        hash_total.hash_conflicts
    );
    println!(
        "  QsoContextHint: slots_with_hints={} | total_hints={} | max_hints_in_slot={}",
        qso_total.slots_with_hints, qso_total.total_hints, qso_total.max_hints_in_slot
    );
    println!(
        "  Divergence: rows={} | shared={} | wsjtx_unique={} | jtdx_unique={} | representation_only_diffs={}",
        divergence_total.total_rows,
        divergence_total.shared_rows,
        divergence_total.wsjtx_unique_rows,
        divergence_total.jtdx_unique_rows,
        divergence_total.representation_only_diffs
    );
    println!(
        "    unique_by_provenance={:?}",
        divergence_total.unique_by_provenance
    );
    println!(
        "    unique_by_message_class={:?}",
        divergence_total.unique_by_message_class
    );
    println!(
        "    unique_by_snr_bucket={:?}",
        divergence_total.unique_by_snr_bucket
    );
    println!(
        "  FalsePositiveCost: wsjtx_unique_supported={} | wsjtx_unique_unsupported={} | jtdx_unique_supported={} | jtdx_unique_unsupported={}",
        wsjtx_unique_supported,
        wsjtx_unique_unsupported,
        jtdx_unique_supported,
        jtdx_unique_unsupported
    );
}

#[test]
#[ignore = "manual Phase 3 diagnostic; run with --release --ignored"]
fn test_hybrid_qso_context_replay_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;
    let base_config = StreamDecodeConfig::default();

    let mut wsjtx = StreamDecodeSession::new(base_config.clone());
    let mut jtdx = JtdxStreamDecodeSession::new(base_config.clone());
    let mut qso_context = ActiveCallContext::new(4);
    let mut attempted_hints = 0usize;
    let mut added_rows = 0usize;
    let mut supported_rows = 0usize;
    let mut unsupported_rows = 0usize;

    for seg in 0..nseg {
        let replay_hints: Vec<ActiveQsoHint> = qso_context
            .hints()
            .iter()
            .map(ActiveQsoHint::from)
            .collect();
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let wsjtx_tagged = wsjtx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let jtdx_tagged = jtdx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let wsjtx_rows: Vec<_> = wsjtx_tagged.iter().map(|row| row.decode.clone()).collect();
        let jtdx_rows: Vec<_> = jtdx_tagged.iter().map(|row| row.decode.clone()).collect();
        let mut existing_rows = wsjtx_rows;
        existing_rows.extend(jtdx_rows);

        let mut replay_added = Vec::new();
        for hint in &replay_hints {
            attempted_hints += 1;
            let replay_rows = replay_jtdx_with_qso_hint(&base_config, &timestamp, &data, hint);
            let new_rows = unique_new_rows(replay_rows, &existing_rows);
            existing_rows.extend(new_rows.iter().cloned());
            replay_added.extend(new_rows);
        }
        let (slot_supported, slot_unsupported) =
            count_supported_by_baseline(&replay_added, &baseline);
        added_rows += replay_added.len();
        supported_rows += slot_supported;
        unsupported_rows += slot_unsupported;
        if !replay_added.is_empty() {
            println!(
                "  Seg {}: replay_hints={} added={} supported={} unsupported={}",
                seg,
                replay_hints.len(),
                replay_added.len(),
                slot_supported,
                slot_unsupported
            );
            for row in &replay_added {
                println!(
                    "    {:>5.0} {:+.1} {:>5.0} {}",
                    row.snr, row.dt, row.freq, row.msg
                );
            }
        }

        let evidence = build_passive_evidence_store_from_tagged(&wsjtx_tagged, &jtdx_tagged);
        qso_context.update_from_evidence(&evidence);
    }

    println!(
        "\n[ENGINE={}] [HYBRID QSO CONTEXT REPLAY] attempted_hints={} | added_rows={} | supported={} | unsupported={}",
        fft_engine_name(),
        attempted_hints,
        added_rows,
        supported_rows,
        unsupported_rows
    );
}

#[test]
#[ignore = "manual Phase 4 diagnostic; run with --release --ignored"]
fn test_hybrid_same_parity_a7_replay_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;
    let base_config = StreamDecodeConfig::default();

    let mut wsjtx = StreamDecodeSession::new(base_config.clone());
    let mut jtdx = JtdxStreamDecodeSession::new(base_config.clone());
    let mut same_parity_seeds: [Vec<StreamDecodedMessage>; 2] = std::array::from_fn(|_| Vec::new());
    let mut attempted_slots = 0usize;
    let mut imported_seeds = 0usize;
    let mut added_rows = 0usize;
    let mut supported_rows = 0usize;
    let mut unsupported_rows = 0usize;

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let parity = ((timestamp.nutc() / 5) % 2) as usize;
        let replay_seeds = same_parity_seeds[parity].clone();

        let wsjtx_tagged = wsjtx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let jtdx_tagged = jtdx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let wsjtx_rows: Vec<_> = wsjtx_tagged.iter().map(|row| row.decode.clone()).collect();
        let jtdx_rows: Vec<_> = jtdx_tagged.iter().map(|row| row.decode.clone()).collect();
        let mut existing_rows = wsjtx_rows.clone();
        existing_rows.extend(jtdx_rows);

        if !replay_seeds.is_empty() {
            attempted_slots += 1;
            let mut replay_wsjtx = StreamDecodeSession::new(base_config.clone());
            imported_seeds +=
                replay_wsjtx.import_same_parity_a7_seed_rows(&timestamp, &replay_seeds);
            let replay_rows = replay_wsjtx
                .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
                .unwrap();
            let replay_a7_rows: Vec<_> = replay_rows
                .into_iter()
                .filter(|row| row.provenance == StreamDecodeProvenance::A7Memory)
                .map(|row| row.decode)
                .collect();
            let replay_added = unique_new_rows(replay_a7_rows, &existing_rows);
            let (slot_supported, slot_unsupported) =
                count_supported_by_baseline(&replay_added, &baseline);
            added_rows += replay_added.len();
            supported_rows += slot_supported;
            unsupported_rows += slot_unsupported;
            if !replay_added.is_empty() {
                println!(
                    "  Seg {}: seeds={} added={} supported={} unsupported={}",
                    seg,
                    replay_seeds.len(),
                    replay_added.len(),
                    slot_supported,
                    slot_unsupported
                );
                for row in &replay_added {
                    println!(
                        "    {:>5.0} {:+.1} {:>5.0} {}",
                        row.snr, row.dt, row.freq, row.msg
                    );
                }
            }
        }

        same_parity_seeds[parity] = jtdx_only_regular_seed_rows(&wsjtx_rows, &jtdx_tagged);
    }

    println!(
        "\n[ENGINE={}] [HYBRID SAME-PARITY A7 REPLAY] attempted_slots={} | imported_seeds={} | added_rows={} | supported={} | unsupported={}",
        fft_engine_name(),
        attempted_slots,
        imported_seeds,
        added_rows,
        supported_rows,
        unsupported_rows
    );
}

#[test]
#[ignore = "manual Phase 0 diagnostic; run with --release --ignored"]
fn test_hybrid_hash_call_opportunity_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;

    let mut wsjtx = StreamDecodeSession::new(StreamDecodeConfig::default());
    let mut jtdx = JtdxStreamDecodeSession::new(StreamDecodeConfig::default());
    let mut total = HashCallOpportunityReport::default();

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let wsjtx_rows = wsjtx.decode_slot_at(&timestamp, &data);
        let jtdx_rows = jtdx
            .decode_slot_streaming_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let report = hybrid_hash_call_opportunity_report(&wsjtx_rows, &jtdx_rows);
        if report.unresolved_hash_rows > 0 || report.rows_resolvable_by_other_decoder > 0 {
            println!(
                "  Seg {}: unresolved_hash_rows={} resolvable_by_other_decoder={} hash_conflicts={}",
                seg,
                report.unresolved_hash_rows,
                report.rows_resolvable_by_other_decoder,
                report.hash_conflicts
            );
        }
        total.merge(report);
    }

    println!(
        "\n[ENGINE={}] [HYBRID HASH OPPORTUNITY] unresolved_hash_rows={} | resolvable_by_other_decoder={} | hash_conflicts={}",
        fft_engine_name(),
        total.unresolved_hash_rows,
        total.rows_resolvable_by_other_decoder,
        total.hash_conflicts
    );
}

#[test]
#[ignore = "manual Phase 0 diagnostic; run with --release --ignored"]
fn test_hybrid_qso_context_opportunity_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;

    let mut wsjtx = StreamDecodeSession::new(StreamDecodeConfig::default());
    let mut jtdx = JtdxStreamDecodeSession::new(StreamDecodeConfig::default());
    let mut context = ActiveCallContext::new(4);
    let mut report = QsoContextOpportunityReport::default();

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let wsjtx_rows = wsjtx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let jtdx_rows = jtdx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let evidence = build_passive_evidence_store_from_tagged(&wsjtx_rows, &jtdx_rows);
        context.update_from_evidence(&evidence);
        report.observe_slot(context.hints());
        if !context.hints().is_empty() {
            println!(
                "  Seg {}: qso_context_hints={} top={:?}",
                seg,
                context.hints().len(),
                context.hints().first()
            );
        }
    }

    println!(
        "\n[ENGINE={}] [HYBRID QSO CONTEXT OPPORTUNITY] slots_with_hints={} | total_hints={} | max_hints_in_slot={}",
        fft_engine_name(),
        report.slots_with_hints,
        report.total_hints,
        report.max_hints_in_slot
    );
}

#[test]
#[ignore = "manual Phase 0 diagnostic; run with --release --ignored"]
fn test_hybrid_divergence_report_long_audio() {
    assert_release_mode();
    let s12k = samples_12k_from_wav("tests/ft8/230208_140300.wav");
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;

    let mut wsjtx = StreamDecodeSession::new(StreamDecodeConfig::default());
    let mut jtdx = JtdxStreamDecodeSession::new(StreamDecodeConfig::default());
    let mut total = HybridDivergenceReport::default();

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let data = slot_samples(&s12k, seg_start, sps);
        let timestamp = SlotTimestamp::parse("230208_140300")
            .unwrap()
            .add_seconds((seg * 15) as i64);
        let wsjtx_rows = wsjtx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let jtdx_rows = jtdx
            .decode_slot_streaming_with_provenance_at(&timestamp, &data, |_| Ok(()))
            .unwrap();
        let evidence = build_passive_evidence_store_from_tagged(&wsjtx_rows, &jtdx_rows);
        let report = divergence_report_from_evidence(&evidence);
        if report.wsjtx_unique_rows > 0 || report.jtdx_unique_rows > 0 {
            println!(
                "  Seg {}: shared={} wsjtx_unique={} jtdx_unique={} repr_diffs={}",
                seg,
                report.shared_rows,
                report.wsjtx_unique_rows,
                report.jtdx_unique_rows,
                report.representation_only_diffs
            );
        }
        total.merge(report);
    }

    println!(
        "\n[ENGINE={}] [HYBRID DIVERGENCE] rows={} | shared={} | wsjtx_unique={} | jtdx_unique={} | representation_only_diffs={}",
        fft_engine_name(),
        total.total_rows,
        total.shared_rows,
        total.wsjtx_unique_rows,
        total.jtdx_unique_rows,
        total.representation_only_diffs
    );
    println!("  unique_by_decoder={:?}", total.unique_by_decoder);
    println!("  unique_by_provenance={:?}", total.unique_by_provenance);
    println!(
        "  unique_by_message_class={:?}",
        total.unique_by_message_class
    );
    println!("  unique_by_snr_bucket={:?}", total.unique_by_snr_bucket);
}
