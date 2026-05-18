/// Segment decode test with diff CSV output:
///   `-` = missed (baseline but not decoded)
///   `+` = extra (decoded but not in baseline)
///   `?` = frequency mismatch (decoded but freq differs >3Hz from baseline)

use ft8rs::{decode_ft8, DecodeFT8Options};
use std::collections::HashSet;
use std::time::Instant;

const SEGMENT_DURATION: usize = 15;
const DECODE_SAMPLE_RATE: usize = 12_000;

#[derive(Debug, Clone)]
struct BMsg { snr: i32, drift: f64, freq: f64, msg: String }

fn norm(msg: &str) -> String {
    msg.trim().to_uppercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn parse_baseline(path: &str) -> Vec<(usize, BMsg)> {
    let mut rows = Vec::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let l = line.trim().trim_end_matches(',');
        if l.is_empty() || l.starts_with("Date-Time") { continue; }
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 5 { continue; }
        let ts = p[0].trim();
        let snr: i32 = p[1].trim().parse().unwrap_or(0);
        let drift: f64 = p[2].trim().parse().unwrap_or(0.0);
        let freq: f64 = p[3].trim().parse().unwrap_or(0.0);
        let msg = p[4].trim().to_string();
        let seg = if ts.len() >= 13 {
            let t = &ts[ts.len() - 6..];
            let h: usize = t[0..2].parse().unwrap_or(0);
            let m: usize = t[2..4].parse().unwrap_or(0);
            let s: usize = t[4..6].parse().unwrap_or(0);
            (h * 3600 + m * 60 + s - (14 * 3600 + 3 * 60)) / 15
        } else { 0 };
        rows.push((seg, BMsg { snr, drift, freq, msg }));
    }
    rows
}

fn load_wav(path: &str) -> (u32, Vec<f32>) {
    let r = hound::WavReader::open(path).unwrap();
    let spec = r.spec();
    let s: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => r.into_samples::<i32>().map(|v| {
            let v = v.unwrap();
            match spec.bits_per_sample {
                16 => v as f32 / 32768.0, 24 => v as f32 / 8_388_608.0,
                32 => v as f32 / 2_147_483_648.0, _ => panic!(),
            }
        }).collect(),
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

#[test]
fn test_segment_decode_230208() {
    let (sr, all) = load_wav("tests/ft8/230208_140300.wav");
    let dur = all.len() as f64 / sr as f64;
    let nseg = (dur / SEGMENT_DURATION as f64).floor() as usize;

    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let bl_count = baseline.len();

    let s12k = resample(&all, sr, DECODE_SAMPLE_RATE as u32);
    let sps = SEGMENT_DURATION * DECODE_SAMPLE_RATE;

    let mut total_matched = 0;
    let mut total_missed = 0;
    let mut total_extra = 0;
    let mut total_freq_mismatch = 0;

    let mut diff_lines = vec!["Date-Time,SNR,Drift,Freq,Msg,Tag".to_string()];

    for seg in 0..nseg {
        let start = seg * sps;
        let data = &s12k[start..(start + sps).min(s12k.len())];
        if data.len() < DECODE_SAMPLE_RATE * 10 { continue; }

        let t0 = Instant::now();
        let decoded = decode_ft8(data, DecodeFT8Options {
            sample_rate: Some(DECODE_SAMPLE_RATE), freq_low: Some(200.0),
            freq_high: Some(3000.0), sync_min: Some(0.8), depth: Some(3),
            max_candidates: Some(300), hash_call_book: None,
        });
        let elapsed = t0.elapsed();

        let bl: Vec<&BMsg> = baseline.iter().filter(|(s, _)| *s == seg).map(|(_, m)| m).collect();
        let dec_norm: HashSet<String> = decoded.iter().map(|d| norm(&d.msg)).collect();

        // Build decoded freq map
        let dmap: std::collections::HashMap<String, (f64, f64)> = decoded.iter()
            .map(|d| (norm(&d.msg), (d.snr, d.freq))).collect();

        let tot = (14 * 3600 + 3 * 60) as u64 + (seg as f64 * SEGMENT_DURATION as f64) as u64;
        let ts = format!("230208_{:02}{:02}{:02}", tot / 3600, (tot % 3600) / 60, tot % 60);

        let mut matched = 0;
        let mut missed = 0;
        let mut extra = 0;
        let mut freq_mm = 0;

        // Baseline messages: check matched, missed, freq mismatch
        for m in &bl {
            let nm = norm(&m.msg);
            if let Some((dsnr, dfreq)) = dmap.get(&nm) {
                let fdiff = (dfreq - m.freq).abs();
                if fdiff > 3.0 {
                    freq_mm += 1;
                    diff_lines.push(format!(
                        "{},{},{:.1},{:.0},{},?  (decoded @{:.0}Hz, baseline @{:.0}Hz, Δ={:+.0}Hz)",
                        ts, dsnr.round() as i32, 0.0, dfreq.round(), m.msg, dfreq, m.freq, dfreq - m.freq
                    ));
                }
                matched += 1;
            } else {
                missed += 1;
                diff_lines.push(format!(
                    "{},{},{},{},{},-",
                    ts, m.snr, m.drift as i32, m.freq.round() as i32, m.msg
                ));
            }
        }

        // Extra decoded messages
        for d in &decoded {
            let nm = norm(&d.msg);
            if !bl.iter().any(|m| norm(&m.msg) == nm) {
                extra += 1;
                diff_lines.push(format!(
                    "{},{},{:.1},{:.0},{},+",
                    ts, d.snr.round() as i32, 0.0, d.freq.round(), d.msg
                ));
            }
        }

        total_matched += matched;
        total_missed += missed;
        total_extra += extra;
        total_freq_mismatch += freq_mm;

        println!("  Seg {:.0}-{:.0}s: decoded {} | matched {}/{} | missed {} | freq? {} | extra {} | {}ms",
            seg as f64 * 15.0, seg as f64 * 15.0 + 15.0,
            decoded.len(), matched, bl.len(), missed, freq_mm, extra, elapsed.as_millis());
    }

    let rate = total_matched as f64 / bl_count as f64 * 100.0;
    std::fs::write("tests/ft8/230208_140300_diff.csv",
        diff_lines.join("\n") + "\n").unwrap();

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  Segment Decode Test: 230208_140300.wav                      ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║  Baseline: {} messages across {} segments                     ║", bl_count, nseg);
    println!("║  Matched: {} ({:.1}%)                                     ║", total_matched, rate);
    println!("║  Missed (-): {}                                               ║", total_missed);
    println!("║  Extra  (+): {}                                               ║", total_extra);
    println!("║  Freq ? : {}                                              ║", total_freq_mismatch);
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!("📁 Diff: tests/ft8/230208_140300_diff.csv");

    assert!(rate >= 70.0, "Hit rate {:.1}% < 70%", rate);
}
