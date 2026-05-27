use std::cell::RefCell;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

mod output;

use ft8rs::input::{
    decode_soundcard_streaming_decodes, decode_wav_file_streaming_decodes,
    infer_start_time_from_path, list_soundcards, FileDecodeOptions, SoundcardFormatInfo,
};
use ft8rs::stream::StreamDecodeConfig;
use ft8rs::SlotTimestamp;

use output::udp::UdpConfig;
use output::Outputs;

#[derive(Parser)]
#[command(name = "ft8rs", about = "FT8 streaming decoder")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Decode a WAV file as a timestamped FT8 stream
    File(FileArgs),
    /// List or monitor live audio input devices
    Monitor(MonitorArgs),
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
struct MonitorArgs {
    /// Input device selector: use the Index shown by `ft8rs monitor`, or the full device name.
    /// Omit this option to list input devices.
    #[arg(long)]
    device: Option<String>,

    /// Stop after this many 15-second slots. Omit to keep listening.
    #[arg(long)]
    slots: Option<usize>,

    /// Send UDP decode reports in the WSJT-X-compatible packet format.
    #[arg(long)]
    udp: bool,

    /// UDP report destination host.
    #[arg(long, default_value = "127.0.0.1")]
    udp_host: String,

    /// UDP report destination port.
    #[arg(long, default_value_t = 2238)]
    udp_port: u16,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        Command::File(args) => run_file(args),
        Command::Monitor(args) => run_monitor(args),
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

    let outputs = RefCell::new(Outputs::new(None)?);
    decode_wav_file_streaming_decodes(
        &args.input,
        FileDecodeOptions { start_time, config },
        |timestamp, row| outputs.borrow_mut().on_decode(timestamp, row),
        |timestamp, count| outputs.borrow_mut().on_slot_complete(timestamp, count),
    )
}

fn run_monitor(args: MonitorArgs) -> Result<(), String> {
    if args.device.is_none() {
        return run_monitor_ls();
    }

    let udp = args.udp.then_some(UdpConfig {
        host: args.udp_host,
        port: args.udp_port,
    });
    let outputs = RefCell::new(Outputs::new(udp)?);
    decode_soundcard_streaming_decodes(
        ft8rs::input::SoundcardDecodeOptions {
            device: args.device,
            config: StreamDecodeConfig::default(),
            max_slots: args.slots,
        },
        |timestamp, row| outputs.borrow_mut().on_decode(timestamp, row),
        |timestamp, count| outputs.borrow_mut().on_slot_complete(timestamp, count),
    )
}

fn run_monitor_ls() -> Result<(), String> {
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
