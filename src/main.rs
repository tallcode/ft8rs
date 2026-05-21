use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use hound::WavReader;

use ft8rs::stream::{StreamDecoder, StreamDecodeConfig};

#[derive(Parser)]
#[command(name = "ft8rs", about = "FT8 streaming decoder")]
struct Cli {
    /// Input WAV file
    file: PathBuf,
}

fn load_wav_f32(path: &str) -> (u32, Vec<f32>) {
    let r = WavReader::open(path).expect("Failed to open WAV");
    let spec = r.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            r.into_samples::<i32>().map(|v| {
                match spec.bits_per_sample {
                    16 => v.unwrap() as f32 / 32768.0,
                    24 => v.unwrap() as f32 / 8_388_608.0,
                    32 => v.unwrap() as f32 / 2_147_483_648.0,
                    _ => panic!("unsupported bits"),
                }
            }).collect()
        }
        hound::SampleFormat::Float => r.into_samples::<f32>().map(|v| v.unwrap()).collect(),
    };
    (spec.sample_rate, samples)
}

fn resample(src: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate { return src.to_vec(); }
    let ratio = from_rate as f64 / to_rate as f64;
    let n = ((src.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let s = i as f64 * ratio;
        let lo = s.floor() as usize;
        let fr = s - lo as f64;
        let v0 = *src.get(lo).unwrap_or(&0.0) as f64;
        let v1 = *src.get(lo + 1).unwrap_or(&0.0) as f64;
        out.push((v0 * (1.0 - fr) + v1 * fr) as f32);
    }
    out
}

fn main() {
    let cli = Cli::parse();
    let (sr, samples) = load_wav_f32(&cli.file.to_string_lossy());
    let samples_12k = resample(&samples, sr, 12_000);
    let samples_per_slot = 12_000 * 15;

    let config = StreamDecodeConfig::default();
    let mut decoder = StreamDecoder::new(config);

    let total_slots = samples_12k.len() / samples_per_slot;
    let t0 = Instant::now();

    for slot in 0..total_slots {
        let start = slot * samples_per_slot;
        let end = (start + samples_per_slot).min(samples_12k.len());
        let results = decoder.decode_slot(&samples_12k[start..end]);
        for r in &results {
            println!("{:+.1} {:>3} {:>5.0} {}", r.dt, r.snr.round(), r.freq, r.msg);
        }
    }

    eprintln!("Decoded {} slots in {:.1}s", total_slots, t0.elapsed().as_secs_f64());
}