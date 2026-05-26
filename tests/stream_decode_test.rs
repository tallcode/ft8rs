use ft8rs::stream::{StreamDecodeConfig, StreamDecoder};
use ft8rs::util::engine_name;
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

fn norm(msg: &str) -> String {
    msg.split_whitespace()
        .map(|w| w.trim().to_uppercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn load_wav(path: &str) -> (u32, Vec<f32>) {
    let r = hound::WavReader::open(path).unwrap();
    let spec = r.spec();
    let s: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => r
            .into_samples::<i32>()
            .map(|v| {
                let v = v.unwrap();
                match spec.bits_per_sample {
                    16 => v as f32 / 32768.0,
                    24 => v as f32 / 8_388_608.0,
                    32 => v as f32 / 2_147_483_648.0,
                    _ => panic!(),
                }
            })
            .collect(),
        hound::SampleFormat::Float => r.into_samples::<f32>().map(|v| v.unwrap()).collect(),
    };
    (spec.sample_rate, s)
}

fn resample(src: &[f32], f: u32, t: u32) -> Vec<f32> {
    let ratio = f as f64 / t as f64;
    let n = ((src.len() as f64) / ratio).ceil() as usize;
    let mut o = Vec::with_capacity(n);
    for i in 0..n {
        let s = i as f64 * ratio;
        let lo = s.floor() as usize;
        let fr = s - lo as f64;
        let v0 = *src.get(lo).unwrap_or(&0.0) as f64;
        let v1 = *src.get(lo + 1).unwrap_or(&0.0) as f64;
        o.push((v0 * (1.0 - fr) + v1 * fr) as f32);
    }
    o
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
    assert_eq!(
        engine_name(),
        "FFTW",
        "stream decode acceptance tests must use FFTW@3840"
    );
}

#[test]
fn test_stream_decode_short_audio() {
    assert_release_mode();
    let t0 = std::time::Instant::now();
    let (_sr, samples) = load_wav("tests/ft8/210703_133430.wav");

    let mut decoder = StreamDecoder::new(StreamDecodeConfig {
        freq_low: 100.0,
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
        engine_name(),
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
    let (sr, all) = load_wav("tests/ft8/230208_140300.wav");
    let s12k = resample(&all, sr, 12000);
    let sps = 15 * 12000;
    let dur_12k = s12k.len() as f64 / 12000.0;
    let nseg = (dur_12k / 15.0).ceil() as usize;

    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    println!(
        "\n[ENGINE={}] [STREAM LONG DECODE] {} segments, {} baseline messages",
        engine_name(),
        nseg,
        baseline.len()
    );

    let config = StreamDecodeConfig {
        ..Default::default()
    };
    let mut decoder = StreamDecoder::new(config);

    let mut total_matched = 0;
    let accepted_floor = 420usize;
    let severe_floor = accepted_floor.saturating_sub(10);
    let mut diff_rows = Vec::new();

    for seg in 0..nseg {
        let seg_start = seg * sps;
        let seg_end = ((seg + 1) * sps).min(s12k.len());
        let data = &s12k[seg_start..seg_end];

        let slot_t0 = std::time::Instant::now();
        let results = decoder.decode_slot(data);
        let elapsed_ms = slot_t0.elapsed().as_millis() as u64;
        assert!(
            elapsed_ms <= 15_000,
            "SLOT {} TIMEOUT: {}ms > 15s",
            seg,
            elapsed_ms
        );

        let bl: Vec<_> = baseline.iter().filter(|row| row.seg == seg).collect();
        let mut matched = 0;
        let mut missed = Vec::new();
        for row in &bl {
            if results.iter().any(|d| norm(&d.msg) == row.norm_msg) {
                matched += 1;
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

    let rate = total_matched as f64 / baseline.len() as f64 * 100.0;
    println!("\n[STREAM LONG DECODE SUMMARY]");
    println!(
        "  Total matched: {}/{} ({:.1}%)",
        total_matched,
        baseline.len(),
        rate
    );
    if std::env::var("FT8RS_WRITE_DIFF").ok().as_deref() == Some("1") {
        write_diff_csv("tests/ft8/230208_140300_diff.csv", &diff_rows);
    }
    assert!(
        total_matched >= accepted_floor,
        "STREAM LONG: {}/{} < {}",
        total_matched,
        baseline.len(),
        accepted_floor
    );
}
