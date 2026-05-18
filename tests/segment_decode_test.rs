/// Segment decode test with frequency-corrected baseline output.
/// Decodes 230208_140300.wav in 15s segments, writes frequency-corrected CSV.

use ft8rs::{decode_ft8, DecodeFT8Options};
use std::collections::HashSet;
use std::time::Instant;

const SEGMENT_DURATION: usize = 15;
const DECODE_SAMPLE_RATE: usize = 12_000;

#[derive(Debug, Clone)]
struct BMsg { ts: String, snr: i32, freq: f64, msg: String }

fn norm(msg: &str) -> String {
    msg.trim().to_uppercase().split_whitespace().collect::<Vec<_>>().join(" ")
}
fn has_hash(msg: &str) -> bool { msg.contains("<...>") }

fn parse_baseline(path: &str) -> Vec<(usize, BMsg)> {
    let mut rows = Vec::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let l = line.trim().trim_end_matches(',');
        if l.is_empty() || l.starts_with("Date-Time") { continue; }
        let p: Vec<&str> = l.split(',').collect();
        if p.len() < 5 { continue; }
        let ts = p[0].to_string();
        let snr: i32 = p[1].parse().unwrap_or(0);
        let freq: f64 = p[3].parse().unwrap_or(0.0);
        let msg = p[4].trim().to_string();
        let seg = if ts.len() >= 13 {
            let t = &ts[ts.len()-6..];
            let h: usize = t[0..2].parse().unwrap_or(0);
            let m: usize = t[2..4].parse().unwrap_or(0);
            let s: usize = t[4..6].parse().unwrap_or(0);
            (h*3600 + m*60 + s - (14*3600 + 3*60)) / 15
        } else { 0 };
        rows.push((seg, BMsg { ts, snr, freq, msg }));
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
    let r = f as f64 / t as f64;
    let n = ((src.len() as f64)/r).ceil() as usize;
    let mut o = Vec::with_capacity(n);
    for i in 0..n {
        let s = i as f64 * r;
        let lo = s.floor() as usize;
        let fr = s - lo as f64;
        let v0 = *src.get(lo).unwrap_or(&0.0) as f64;
        let v1 = *src.get(lo+1).unwrap_or(&0.0) as f64;
        o.push((v0*(1.0-fr) + v1*fr) as f32);
    }
    o
}

#[test]
fn test_corrected_baseline() {
    let (sr, all) = load_wav("tests/ft8/230208_140300.wav");
    let dur = all.len() as f64 / sr as f64;
    let nseg = (dur / SEGMENT_DURATION as f64).floor() as usize;
    
    let bl_all = parse_baseline("tests/ft8/230208_140300.csv");
    let bl_n = bl_all.len();
    let baseline: Vec<(usize, BMsg)> = bl_all.into_iter()
        .filter(|(s,m)| *s < nseg && !has_hash(&m.msg)).collect();
    
    let s12k = resample(&all, sr, DECODE_SAMPLE_RATE as u32);
    let sps = SEGMENT_DURATION * DECODE_SAMPLE_RATE;
    
    let mut corrected_lines: Vec<String> = vec![
        "Date-Time,SNR,Drift,Frq,Msg,Tag,OriginalFrq".to_string()
    ];
    
    for seg in 0..nseg {
        let start = seg * sps;
        let data = &s12k[start..(start + sps).min(s12k.len())];
        if data.len() < DECODE_SAMPLE_RATE * 10 { continue; }
        
        let t0 = Instant::now();
        let decoded = decode_ft8(data, DecodeFT8Options {
            sample_rate: Some(DECODE_SAMPLE_RATE), freq_low: Some(200.0),
            freq_high: Some(3100.0), sync_min: Some(0.7), depth: Some(3),
            max_candidates: Some(300), hash_call_book: None,
        });
        let _elapsed = t0.elapsed();
        
        let bl: Vec<&BMsg> = baseline.iter().filter(|(s,_)| *s == seg).map(|(_,m)| m).collect();
        let dec_norm: HashSet<String> = decoded.iter().map(|d| norm(&d.msg)).collect();
        
        // Build decoded freq map (norm → freq)
        let mut dmap: std::collections::HashMap<String, (f64, f64)> = std::collections::HashMap::new();
        for d in &decoded {
            dmap.insert(norm(&d.msg), (d.snr, d.freq));
        }
        
        let tot = (14*3600 + 3*60) as u64 + (seg as f64 * SEGMENT_DURATION as f64) as u64;
        let ts = format!("230208_{:02}{:02}{:02}", tot/3600, (tot%3600)/60, tot%60);
        
        // For each baseline message in this segment
        for m in &bl {
            let nm = norm(&m.msg);
            if let Some((dsnr, dfreq)) = dmap.get(&nm) {
                // Matched: use DECODED frequency and SNR
                let orig_freq = m.freq.round() as i32;
                let new_freq = dfreq.round() as i32;
                let tag = if (orig_freq - new_freq).abs() > 3 { "FIXED" } else { "" };
                corrected_lines.push(format!("{},{},{},{},{},{},{}",
                    ts, dsnr.round() as i32, 0, new_freq, m.msg, tag, orig_freq));
            } else {
                // Missed: keep original baseline
                corrected_lines.push(format!("{},{},{},{},{},{},{}",
                    ts, m.snr, 0, m.freq.round() as i32, m.msg, "?", m.freq.round() as i32));
            }
        }
        
        // Extra decoded messages
        for d in &decoded {
            let nm = norm(&d.msg);
            if !bl.iter().any(|m| norm(&m.msg) == nm) {
                corrected_lines.push(format!("{},{},{},{},{},NEW,{}",
                    ts, d.snr.round() as i32, 0, d.freq.round() as i32, d.msg, d.freq.round() as i32));
            }
        }
        
        let matched = bl.iter().filter(|m| dec_norm.contains(&norm(&m.msg))).count();
        println!("  Seg {:.0}-{:.0}s: decoded {} | matched {}/{} | {}ms",
            seg as f64*15.0, seg as f64*15.0+15.0, decoded.len(), matched, bl.len(), _elapsed.as_millis());
    }
    
    std::fs::write("tests/ft8/230208_140300_corrected.csv",
        corrected_lines.join("\n") + "\n").unwrap();
    
    let total = corrected_lines.len() - 1;
    let fixed = corrected_lines.iter().filter(|l| l.contains(",FIXED,")).count();
    let missed = corrected_lines.iter().filter(|l| l.contains(",?,")).count();
    let news = corrected_lines.iter().filter(|l| l.contains(",NEW,")).count();
    let matched = total - missed - news;
    
    println!("\n📊 Corrected baseline: {}", "tests/ft8/230208_140300_corrected.csv");
    println!("   Total lines: {}", total);
    println!("   Matched (freq corrected): {}", matched);
    println!("   Freq fixed (>3Hz): {}", fixed);
    println!("   Missed (?): {}", missed);
    println!("   New (extra): {}", news);
}
