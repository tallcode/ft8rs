use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use ft8rs::util::wav::{parse_wav_buffer, write_mono16_wav_file};
use ft8rs::{decode_ft8, decode_ft4, encode_ft8, DecodeFT8Options, DecodeFT4Options};

const SAMPLE_RATE: usize = 12_000;


#[derive(Parser)]
#[command(name = "ft8rs", about = "FT8/FT4 encoder/decoder")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Decode a WAV file
    Decode {
        /// Input WAV file
        file: PathBuf,
        /// Mode: ft8 (default) or ft4
        #[arg(long, default_value = "ft8")]
        mode: String,
        /// Lower frequency bound (Hz)
        #[arg(long, default_value = "200")]
        low: f64,
        /// Upper frequency bound (Hz)
        #[arg(long, default_value = "3000")]
        high: f64,
        /// Decoding depth (1=fast, 2=normal, 3=deep)
        #[arg(long, default_value = "2")]
        depth: usize,
        /// Max candidate signals
        #[arg(long, default_value = "300")]
        max_candidates: usize,
    },
    /// Encode a message to WAV
    Encode {
        /// Message to encode
        message: String,
        /// Output WAV file
        #[arg(long, default_value = "output.wav")]
        out: PathBuf,
        /// Base frequency in Hz
        #[arg(long, default_value = "1000")]
        df: f64,
    },
}

fn run_decode(file: &PathBuf, mode: &str, low: f64, high: f64, depth: usize, max_candidates: usize) {
    let buf = fs::read(file).expect("Failed to read file");
    let wav = parse_wav_buffer(&buf).expect("Failed to parse WAV");

    println!(
        "WAV: {} Hz, {} samples, {:.1}s",
        wav.sample_rate,
        wav.samples.len(),
        wav.samples.len() as f64 / wav.sample_rate as f64
    );

    let samples_f32: Vec<f32> = wav.samples.iter().map(|&x| x).collect();
    let start = Instant::now();

    if mode == "ft4" {
        let decoded = decode_ft4(&samples_f32, DecodeFT4Options {
            sample_rate: Some(wav.sample_rate as usize),
            freq_low: Some(low),
            freq_high: Some(high),
            depth: Some(depth),
            max_candidates: Some(max_candidates),
            ..Default::default()
        });
        let elapsed = start.elapsed();
        println!("\nDecoded {} messages in {:.2}s:\n", decoded.len(), elapsed.as_secs_f64());
        println!("   dt  snr   freq  message");
        println!("  ---  ---  -----  -------");
        for d in &decoded {
            println!("    {}  {:+3}  {:>5}  {}", format!("{:+.1}", d.dt), d.snr.round() as i32, d.freq.round() as i32, d.msg);
        }
    } else {
        let decoded = decode_ft8(&samples_f32, DecodeFT8Options {
            sample_rate: Some(wav.sample_rate as usize),
            freq_low: Some(low),
            freq_high: Some(high),
            sync_min: Some(1.3),
            depth: Some(depth),
            max_candidates: Some(max_candidates),
            hash_call_book: None,
            mycall: None,
            hiscall: None,
            sync_mode: None,
        });
        let elapsed = start.elapsed();
        println!("\nDecoded {} messages in {:.2}s:\n", decoded.len(), elapsed.as_secs_f64());
        println!("   dt  snr   freq  message");
        println!("  ---  ---  -----  -------");
        for d in &decoded {
            println!("    {}  {:+3}  {:>5}  {}", format!("{:+.1}", d.dt), d.snr.round() as i32, d.freq.round() as i32, d.msg);
        }
    }
}

fn run_encode(message: &str, out_file: &PathBuf, df_hz: f64) {
    let waveform = encode_ft8(message, ft8rs::util::waveform::WaveformOptions {
        sample_rate: Some(SAMPLE_RATE as f64),
        samples_per_symbol: Some(1_920),
        base_frequency: Some(df_hz),
        ..Default::default()
    });

    let mut file = fs::File::create(out_file).expect("Failed to create output file");
    write_mono16_wav_file(&mut file, &waveform, SAMPLE_RATE as u32)
        .expect("Failed to write WAV");

    println!(
        "Wrote {} ({} samples, {:.3} s)",
        out_file.display(),
        waveform.len(),
        waveform.len() as f64 / SAMPLE_RATE as f64
    );
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Decode {
            file,
            mode,
            low,
            high,
            depth,
            max_candidates,
        } => {
            run_decode(&file, &mode, low, high, depth, max_candidates);
        }
        Commands::Encode {
            message,
            out,
            df,
        } => {
            run_encode(&message, &out, df);
        }
    }
}
