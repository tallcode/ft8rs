use std::rc::Rc;
/// Segment decode test with cumulative HashCallBook and long_decode utility.
use ft8rs::{decode_ft8, long_decode, DecodeFT8Options, LongDecodeConfig};
use ft8rs::util::hashcall::HashCallBook;
use std::time::Instant;

const SEGMENT_DURATION: usize = 15;
const DECODE_SAMPLE_RATE: u32 = 12_000;

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

fn extract_and_save_callsigns(msg: &str, book: &HashCallBook) {
    let skip_tokens = [
        "CQ", "DX", "TEST", "TU", "73", "RR73", "RRR", "GL",
        "R+", "R-", "+00", "+01", "+02", "+03", "+04", "+05", "+06", "+07", "+08", "+09", "+10",
        "-01", "-02", "-03", "-04", "-05", "-06", "-07", "-08", "-09", "-10",
        "-11", "-12", "-13", "-14", "-15", "-16", "-17", "-18", "-19", "-20",
        "-21", "-22", "-23", "-24", "-25", "-26", "-27", "-28",
        "+11", "+12", "+13", "+14", "+15", "+16", "+17", "+18", "+19", "+20",
        "+21", "+22", "+23", "+24", "+25", "+26", "+27", "+28",
        "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10",
        "11", "12", "13", "14", "15", "16", "17", "18", "19", "20",
        "21", "22", "23", "24", "25", "26", "27", "28", "29", "30",
    ];
    for word in msg.split_whitespace() {
        let mut w = word.trim().to_uppercase();
        if w.starts_with('<') { w = w.trim_start_matches('<').to_string(); }
        if w.ends_with('>') { w = w.trim_end_matches('>').to_string(); }
        if w.len() < 3 { continue; }
        if skip_tokens.contains(&w.as_str()) { continue; }
        if w.len() == 4 && w.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) {
            let digits: usize = w.chars().filter(|c| c.is_ascii_digit()).count();
            if digits == 2 { continue; }
        }
        if (w.starts_with('+') || w.starts_with('-')) && w[1..].chars().all(|c| c.is_ascii_digit()) { continue; }
        if w.starts_with("R+") || w.starts_with("R-") { continue; }
        book.save(&w);
    }
}

#[test]
fn test_segment_decode_with_hashcallbook() {
    let (sr, all) = load_wav("tests/ft8/230208_140300.wav");
    let dur = all.len() as f64 / sr as f64;
    let nseg = (dur / SEGMENT_DURATION as f64).floor() as usize;
    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let bl_count = baseline.len();
    let s12k = resample(&all, sr, DECODE_SAMPLE_RATE);
    let sps = SEGMENT_DURATION * DECODE_SAMPLE_RATE as usize;

    let book = Rc::new(HashCallBook::new());

    let mut total_matched = 0;
    let mut total_missed = 0;
    let mut total_extra = 0;
    let mut total_hash_resolved = 0;
    let mut diff_lines = vec!["Date-Time,SNR,Drift,Freq,Msg,Tag".to_string()];

    for seg in 0..nseg {
        let seg_start = (seg as isize * sps as isize - DECODE_SAMPLE_RATE as isize).max(0) as usize;
        let seg_end = ((seg + 1) as isize * sps as isize + DECODE_SAMPLE_RATE as isize).min(s12k.len() as isize) as usize;
        let data = &s12k[seg_start..seg_end];
        if data.len() < DECODE_SAMPLE_RATE as usize * 10 { continue; }

        let bk = Rc::clone(&book);
        let t0 = Instant::now();
        let decoded = decode_ft8(data, DecodeFT8Options {
            sample_rate: Some(DECODE_SAMPLE_RATE as usize), freq_low: Some(200.0),
            freq_high: Some(3000.0), sync_min: Some(0.8), depth: Some(3),
            max_candidates: Some(500), hash_call_book: Some(bk),
            mycall: None,
            hiscall: None,
            sync_mode: None,
        });
        let elapsed = t0.elapsed();

        for d in &decoded {
            extract_and_save_callsigns(&d.msg, &book);
            if d.msg.contains('<') { total_hash_resolved += 1; }
        }

        let bl: Vec<&BMsg> = baseline.iter().filter(|(s, _)| *s == seg).map(|(_, m)| m).collect();
        let tot = (14 * 3600 + 3 * 60) as u64 + (seg as f64 * SEGMENT_DURATION as f64) as u64;
        let ts = format!("230208_{:02}{:02}{:02}", tot / 3600, (tot % 3600) / 60, tot % 60);

        let mut matched = 0; let mut missed = 0; let mut extra = 0;

        for m in &bl {
            let nm = norm(&m.msg);
            if let Some(d) = decoded.iter().find(|d| norm(&d.msg) == nm) {
                let fdiff = (d.freq - m.freq).abs();
                if fdiff > 3.0 {
                    diff_lines.push(format!("{},{},{},{},{}? (decoded @{}Hz, baseline @{}Hz)", ts, d.snr.round() as i32, d.freq.round(), m.freq.round(), m.msg, d.freq, m.freq));
                }
                matched += 1;
            } else {
                missed += 1;
                diff_lines.push(format!("{},{},{},{},{}-", ts, m.snr, m.drift as i32, m.freq.round() as i32, m.msg));
            }
        }

        for d in &decoded {
            let nm = norm(&d.msg);
            if !bl.iter().any(|m| norm(&m.msg) == nm) {
                extra += 1;
                diff_lines.push(format!("{},{},{},{},+", ts, d.snr.round() as i32, d.freq.round(), d.msg));
            }
        }

        total_matched += matched; total_missed += missed;
        total_extra += extra;

        println!("  Seg {:.0}-{:.0}s: decoded {} | matched {}/{} | missed {} | extra {} | book={} | {}ms",
            seg as f64 * 15.0, seg as f64 * 15.0 + 15.0,
            decoded.len(), matched, bl.len(), missed, extra, book.size(), elapsed.as_millis());
    }

    let rate = total_matched as f64 / bl_count as f64 * 100.0;
    std::fs::write("tests/ft8/230208_140300_diff.csv", diff_lines.join("\n") + "\n").unwrap();

    println!("\nMatched: {} / {} ({:.1}%) | Missed: {} | Extra: {} | Hash resolved: {}", total_matched, bl_count, rate, total_missed, total_extra, total_hash_resolved);
    assert!(rate >= 70.0, "Hit rate {:.1}% < 70%", rate);
}

/// Quick smoke test: validates long_decode compiles and produces results.
#[test]
fn test_segment_decode_long_quick() {
    let (sr, all) = load_wav("tests/ft8/230208_140300.wav");
    let baseline = parse_baseline("tests/ft8/230208_140300.csv");

    // Only resample what we need for 3 segments (~50s of 48kHz audio → ~12s of 12kHz)
    let sps_48k = SEGMENT_DURATION * sr as usize;
    let needed_48k_samples = (3 * sps_48k + sr as usize * 2).min(all.len());
    let partial_48k = &all[..needed_48k_samples];
    let s12k = resample(partial_48k, sr, 12000);

    // Only test first 3 segments
    let sps = SEGMENT_DURATION * DECODE_SAMPLE_RATE as usize;
    let dur_samples = (3 * sps + DECODE_SAMPLE_RATE as usize * 2).min(s12k.len());
    let partial = &s12k[..dur_samples];

    let config = LongDecodeConfig {
        freq_low: 200.0, freq_high: 3000.0, sync_min: 0.8,
        max_candidates: 300, depth: 3, n_cycles: 1,
        smoothing: false, cross_segment_memory: true,
        mycall: None, hiscall: None,
    };

    let t0 = Instant::now();
    let result = long_decode(partial, 12000, &config);
    let elapsed = t0.elapsed();

    let mut matched = 0u32;
    for seg_result in &result.segments {
        let seg = seg_result.segment;
        let bl: Vec<&BMsg> = baseline.iter().filter(|(s, _)| *s == seg).map(|(_, m)| m).collect();
        for m in &bl {
            let nm = norm(&m.msg);
            if seg_result.decoded.iter().any(|d| norm(d) == nm) { matched += 1; }
        }
        println!("  Seg {}: {} decoded, {} matches, {}ms",
            seg, seg_result.decoded.len(), matched, seg_result.elapsed_ms);
    }

    println!("long_decode quick: {} matches in {:.1}s", matched, elapsed.as_secs_f64());
    assert!(result.segments.len() > 0, "long_decode produced no segments");
    assert!(matched > 0, "long_decode should decode at least 1 message");
}

/// Full long_decode with 2-cycle decoding for maximum sensitivity.
/// Takes ~6-8 minutes. Run manually: cargo test test_segment_decode_long -- --ignored --nocapture
#[test]
#[ignore]
fn test_segment_decode_long() {
    let (sr, all) = load_wav("tests/ft8/230208_140300.wav");
    let baseline = parse_baseline("tests/ft8/230208_140300.csv");
    let bl_count = baseline.len();

    let config = LongDecodeConfig {
        freq_low: 200.0, freq_high: 3000.0, sync_min: 0.8,
        max_candidates: 500, depth: 3, n_cycles: 3,
        smoothing: true, cross_segment_memory: true,
        mycall: None, hiscall: None,
    };

    let t0 = Instant::now();
    let result = long_decode(&all, sr, &config);
    let total_elapsed = t0.elapsed();

    let mut total_matched = 0u32;
    let mut total_missed = 0u32;
    let mut total_extra = 0u32;

    for seg_result in &result.segments {
        let seg = seg_result.segment;
        let bl: Vec<&BMsg> = baseline.iter().filter(|(s, _)| *s == seg).map(|(_, m)| m).collect();

        let mut matched = 0u32; let mut missed = 0u32;
        for m in &bl {
            let nm = norm(&m.msg);
            if seg_result.decoded.iter().any(|d| norm(d) == nm) { matched += 1; }
            else { missed += 1; }
        }
        let extra = seg_result.decoded.len() as u32 - matched;

        total_matched += matched; total_missed += missed; total_extra += extra;
        println!("  Seg {}: decoded {} | matched {}/{} | missed {} | extra {} | {}ms",
            seg, seg_result.decoded.len(), matched, bl.len(), missed, extra, seg_result.elapsed_ms);
    }

    let rate = total_matched as f64 / bl_count as f64 * 100.0;
    println!("\nLong decode full:");
    println!("Matched: {} / {} ({:.1}%) | Missed: {} | Extra: {}", total_matched, bl_count, rate, total_missed, total_extra);
    println!("Total elapsed: {:.1}s", total_elapsed.as_secs_f64());
    assert!(rate >= 75.0, "Hit rate {:.1}% < 75%", rate);
}
