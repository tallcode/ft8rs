use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use ft8rs::input::{
    decode_soundcard_streaming, decode_wav_file_streaming, infer_start_time_from_path,
    list_soundcards, FileDecodeOptions, SoundcardFormatInfo,
};
use ft8rs::stream::StreamDecodeConfig;
use ft8rs::SlotTimestamp;

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum FftEngine {
    #[default]
    Fftw,
    Rustfft,
}

#[derive(Parser)]
#[command(name = "ft8rs", about = "FT8 streaming decoder")]
struct Cli {
    /// FFT engine to use
    #[arg(long, global = true, value_enum, default_value_t = FftEngine::Fftw,
        help = "FFT engine: fftw (3840-pt, WSJT-X aligned) or rustfft (4096-pt)")]
    fft_engine: FftEngine,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a WAV file as a timestamped FT8 stream
    File(FileArgs),
    /// List or listen to live audio input devices
    Soundcard(SoundcardArgs),
}

#[derive(Args)]
struct FileArgs {
    /// Input WAV file
    input: PathBuf,

    /// Timestamp for the first decoded slot, e.g. 230208_140300 or 140300.
    /// If omitted, ft8rs tries to infer it from the file name.
    #[arg(long)]
    start_time: Option<String>,

    /// Lower decode frequency bound in Hz
    #[arg(long)]
    low: Option<f64>,

    /// Upper decode frequency bound in Hz
    #[arg(long)]
    high: Option<f64>,

    /// WSJT-X decode depth
    #[arg(long)]
    depth: Option<usize>,

    /// Maximum sync candidates per pass
    #[arg(long)]
    max_candidates: Option<usize>,

    /// Disable AP decoding
    #[arg(long)]
    no_ap: bool,
}

#[derive(Args)]
struct SoundcardArgs {
    /// Input device selector: use the Index shown by `ft8rs soundcard`, or the full device name.
    /// Omit this option to list input devices.
    #[arg(long)]
    device: Option<String>,

    /// Stop after this many 15-second slots. Omit to keep listening.
    #[arg(long)]
    slots: Option<usize>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.fft_engine {
        FftEngine::Rustfft => std::env::set_var("FTRS_FFT", "rustfft"),
        FftEngine::Fftw => std::env::set_var("FTRS_FFT", "fftw"),
    }

    match cli.command {
        Command::File(args) => run_file(args),
        Command::Soundcard(args) => run_soundcard(args),
    }
}

fn run_file(args: FileArgs) -> Result<(), String> {
    let start_time = match args.start_time {
        Some(value) => SlotTimestamp::parse(&value)?,
        None => infer_start_time_from_path(&args.input).ok_or_else(|| {
            format!(
                "could not infer start time from {}; pass --start-time YYMMDD_HHMMSS",
                args.input.display()
            )
        })?,
    };

    let mut config = StreamDecodeConfig::default();
    if let Some(low) = args.low {
        config.nfa = low;
    }
    if let Some(high) = args.high {
        config.nfb = high;
    }
    if let Some(depth) = args.depth {
        config.ndepth = depth;
    }
    if let Some(max_candidates) = args.max_candidates {
        config.ncand = max_candidates;
    }
    if args.no_ap {
        config.lft8apon = false;
    }

    decode_wav_file_streaming(
        &args.input,
        FileDecodeOptions { start_time, config },
        |timestamp, rows| print_slot_rows(timestamp, &rows),
    )
}

fn run_soundcard(args: SoundcardArgs) -> Result<(), String> {
    if args.device.is_none() {
        return run_soundcard_ls();
    }

    decode_soundcard_streaming(
        ft8rs::input::SoundcardDecodeOptions {
            device: args.device,
            config: StreamDecodeConfig::default(),
            max_slots: args.slots,
        },
        |timestamp, rows| print_slot_rows(timestamp, &rows),
    )
}

fn print_slot_rows(
    timestamp: SlotTimestamp,
    rows: &[ft8rs::stream::StreamDecodedMessage],
) -> Result<(), String> {
    for row in rows {
        println!(
            "{} {:>3} {:+.1} {:>5.0} {}",
            timestamp,
            row.snr.round() as i32,
            row.dt,
            row.freq.round(),
            row.msg
        );
    }
    println!(
        "==================== slot complete: {} decodes ====================",
        rows.len()
    );
    use std::io::Write;
    std::io::stdout()
        .flush()
        .map_err(|err| format!("failed to flush stdout: {err}"))
}

fn run_soundcard_ls() -> Result<(), String> {
    let devices = list_soundcards()?;
    if devices.is_empty() {
        println!("No audio input devices found.");
        return Ok(());
    }

    println!(
        "{:<5} {:<12} {:<40} {:<10} Default input format",
        "Index", "Host", "Name", "Default"
    );
    for device in devices {
        let default_mark = if device.is_default_input { "yes" } else { "-" };
        let format = format_soundcard_format(&device.input);

        println!(
            "{:<5} {:<12} {:<40} {:<10} {}",
            device.index, device.host, device.name, default_mark, format
        );
    }

    Ok(())
}

fn format_soundcard_format(format: &SoundcardFormatInfo) -> String {
    format!(
        "{}ch/{}Hz/{}",
        format.channels, format.sample_rate, format.sample_format
    )
}
