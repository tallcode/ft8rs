/// Long-form WAV segment decode test:
/// Splits 230208_140300.wav into 15s segments, decodes each,
/// compares against baseline CSV, outputs diff CSV.
/// Skip <...> messages; no HashCallBook for speed.

use ft8rs::{decode_ft8, DecodeFT8Options};
use std::collections::HashSet;
use std::time::Instant;

const SEGMENT_DURATION: usize = 15;
const DECODE_SAMPLE_RATE: usize = 12_000;

#[derive(Debug, Clone)]
struct BaselineMsg { snr: i32, freq: f64, msg: String }

#[derive(Debug, Clone)]
struct ExtraMsg { snr: f64, freq: f64, msg: String }

struct SegResult {
    start_s: f64,
    elapsed_ms: u128,
    decoded_count: usize,
    baseline_count: usize,
    matched: Vec<String>,
    missed: Vec<BaselineMsg>,
    extra: Vec<ExtraMsg>,
}

fn norm(msg: &str) -> String {
    msg.trim().to_uppercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_hash_callsign(msg: &str) -> bool { msg.contains("<...>") }

fn parse_baseline(path: &str) -> Vec<(usize, BaselineMsg)> {
    let mut rows = Vec::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let line = line.trim().trim_end_matches(',');
        if line.is_empty() || line.starts_with("Date-Time") { continue; }
        let p: Vec<&str> = line.split(',').collect();
        if p.len() < 5 { continue; }
        let ts = p[0].trim();
        let snr: i32 = p[1].trim().parse().unwrap_or(0);
        let freq: f64 = p[3].trim().parse().unwrap_or(0.0);
        let msg = p[4].trim().to_string();
        let seg = if ts.len() >= 13 {
            let t = &ts[ts.len()-6..];
            let h: usize = t[0..2].parse().unwrap_or(0);
            let m: usize = t[2..4].parse().unwrap_or(0);
            let s: usize = t[4..6].parse().unwrap_or(0);
            (h*3600 + m*60 + s - (14*3600 + 3*60)) / 15
        } else { 0 };
        rows.push((seg, BaselineMsg { snr, freq, msg }));
    }
    rows
}

fn load_wav(path: &str) -> (u32, Vec<f32>) {
    let r = hound::WavReader::open(path).unwrap();
    let spec = r.spec();
    let samples = match spec.sample_format {
        hound::SampleFormat::Int => r.into_samples::<i32>().map(|s| {
            let v = s.unwrap();
            match spec.bits_per_sample {
                16 => v as f32 / 32768.0, 24 => v as f32 / 8_388_608.0,
                32 => v as f32 / 2_147_483_648.0, _ => panic!("bits"),
            }
        }).collect(),
        hound::SampleFormat::Float => r.into_samples::<f32>().map(|s| s.unwrap()).collect(),
    };
    (spec.sample_rate, samples)
}

fn resample(src: &[f32], from: u32, to: u32) -> Vec<f32> {
    let ratio = from as f64 / to as f64;
    let n = ((src.len() as f64)/ratio).ceil() as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let s = i as f64 * ratio;
        let lo = s.floor() as usize;
        let f = s - lo as f64;
        let v0 = src.get(lo).copied().unwrap_or(0.0) as f64;
        let v1 = src.get(lo+1).copied().unwrap_or(0.0) as f64;
        out.push((v0*(1.0-f) + v1*f) as f32);
    }
    out
}

#[test]
fn test_segment_decode_230208() {
    let wav_path = "tests/ft8/230208_140300.wav";
    let csv_path = "tests/ft8/230208_140300.csv";

    let (sr, all) = load_wav(wav_path);
    let dur = all.len() as f64 / sr as f64;
    let nseg = (dur / SEGMENT_DURATION as f64).floor() as usize;

    println!("\nWAV: {}Hz {}samples {:.1}s → {} segments", sr, all.len(), dur, nseg);
    println!("Decode: freq=200-3100Hz sync_min=0.7 depth=3");

    let bl_all = parse_baseline(csv_path);
    let bl_all_n = bl_all.len();
    let baseline: Vec<(usize, BaselineMsg)> = bl_all.into_iter()
        .filter(|(s,m)| *s < nseg && !has_hash_callsign(&m.msg)).collect();
    println!("Baseline: {} (skipped {} <...>)\n", baseline.len(), bl_all_n - baseline.len());

    let s12k = resample(&all, sr, DECODE_SAMPLE_RATE as u32);
    let sps = SEGMENT_DURATION * DECODE_SAMPLE_RATE;

    let mut results = Vec::new();

    for seg in 0..nseg {
        let start = seg * sps;
        let end = (start + sps).min(s12k.len());
        let data = &s12k[start..end];
        if data.len() < DECODE_SAMPLE_RATE * 10 { continue; }

        let t0 = Instant::now();
        let decoded = decode_ft8(data, DecodeFT8Options {
            sample_rate: Some(DECODE_SAMPLE_RATE), freq_low: Some(200.0), freq_high: Some(3100.0),
            sync_min: Some(0.7), depth: Some(3), max_candidates: Some(300), hash_call_book: None,
        });
        let elapsed = t0.elapsed();

        let bl: Vec<&BaselineMsg> = baseline.iter().filter(|(s,_)| *s == seg).map(|(_,m)| m).collect();
        let dec_norm: HashSet<String> = decoded.iter().map(|d| norm(&d.msg)).collect();

        let matched: Vec<String> = bl.iter().filter(|m| dec_norm.contains(&norm(&m.msg))).map(|m| m.msg.clone()).collect();
        let missed: Vec<BaselineMsg> = bl.iter().filter(|m| !dec_norm.contains(&norm(&m.msg))).map(|m| (*m).clone()).collect();
        let extra: Vec<ExtraMsg> = decoded.iter().filter(|d|
            !baseline.iter().filter(|(s,_)| *s == seg).any(|(_,bm)| norm(&bm.msg) == norm(&d.msg))
        ).map(|d| ExtraMsg { snr: d.snr, freq: d.freq, msg: d.msg.clone() }).collect();

        println!("  Seg {:.0}-{:.0}s: decoded {} | matched {}/{} | {}ms",
            seg as f64 * SEGMENT_DURATION as f64,
            seg as f64 * SEGMENT_DURATION as f64 + SEGMENT_DURATION as f64,
            decoded.len(), matched.len(), bl.len(), elapsed.as_millis());

        results.push(SegResult {
            start_s: seg as f64 * SEGMENT_DURATION as f64,
            elapsed_ms: elapsed.as_millis(),
            decoded_count: decoded.len(),
            baseline_count: bl.len(),
            matched, missed, extra,
        });
    }

    let total_matched: usize = results.iter().map(|r| r.matched.len()).sum();
    let total_bl: usize = results.iter().map(|r| r.baseline_count).sum();
    let total_extra: usize = results.iter().map(|r| r.extra.len()).sum();
    let total_missed: usize = total_bl - total_matched;

    println!("\n╔═══════════════════╦══════════╦══════════╦══════════╗");
    println!("║  Segment          ║ Baseline ║ Matched  ║   Rate   ║");
    println!("╠═══════════════════╬══════════╬══════════╬══════════╣");
    for r in &results {
        let ts = format!("{:>5.0}–{:>5.0}s", r.start_s, r.start_s + SEGMENT_DURATION as f64);
        let rn = if r.baseline_count > 0 { format!("{:.0}%", r.matched.len() as f64 / r.baseline_count as f64 * 100.0) } else { "N/A".into() };
        println!("║  {} ║ {:>6}   ║ {:>6}   ║  {:>5} ║", ts, r.baseline_count, r.matched.len(), rn);
    }
    println!("╚═══════════════════╩══════════╩══════════╩══════════╝");

    println!("\n📊  Summary");
    println!("   Matched: {} / {} ({:.1}%)", total_matched, total_bl, total_matched as f64 / total_bl as f64 * 100.0);
    println!("   Missed: {}", total_missed);
    println!("   Extra: {}", total_extra);

    // Write diff CSV
    write_diff_csv("tests/ft8/230208_140300_diff.csv", &results);

    let rate = total_matched as f64 / total_bl as f64;
    assert!(rate >= 0.70, "Hit rate {:.1}% < 70%", rate * 100.0);
}

fn write_diff_csv(path: &str, results: &[SegResult]) {
    let mut lines = vec!["Date-Time,SNR,Drift,Frq,Msg,Diff,".to_string()];
    for r in results {
        let tot = (14*3600 + 3*60) as u64 + (r.start_s as u64);
        let ts = format!("230208_{:02}{:02}{:02}", tot/3600, (tot%3600)/60, tot%60);
        for m in &r.missed {
            lines.push(format!("{},{},{},{},{},-", ts, m.snr, 0, m.freq.round() as i32, m.msg));
        }
        for e in &r.extra {
            lines.push(format!("{},{},{},{},{},+", ts, e.snr.round() as i32, 0, e.freq.round() as i32, e.msg));
        }
    }
    std::fs::write(path, lines.join("\n")+"\n").unwrap();
    let nm: usize = results.iter().map(|r| r.missed.len()).sum();
    let ne: usize = results.iter().map(|r| r.extra.len()).sum();
    println!("📁 {} ({} missed -, {} extra +)", path, nm, ne);
}
