use std::cell::RefCell;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

mod output;

use ft8rs::input::{
    decode_soundcard_streaming_decodes, decode_wav_file_streaming_decodes,
    infer_start_time_from_path, list_soundcards, FileDecodeOptions, SoundcardFormatInfo,
};
use ft8rs::stream::{DecodeProfile, StreamDecodeConfig};
use ft8rs::SlotTimestamp;

use output::udp::UdpConfig;
use output::Outputs;

const VERSION: &str = env!("FT8RS_VERSION");

#[derive(Parser)]
#[command(name = "ft8rs", version = VERSION, about = "FT8 streaming decoder")]
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

    #[command(flatten, next_help_heading = "Decode context")]
    decode: DecodeArgs,

    /// Timestamp for the first decoded slot, e.g. 230208_140300 or 140300.
    /// If omitted, ft8rs tries to infer it from the file name.
    #[arg(
        short = 's',
        long,
        value_name = "YYMMDD_HHMMSS|HHMMSS",
        help_heading = "Input"
    )]
    start_time: Option<String>,
}

#[derive(Args, Clone, Debug)]
struct DecodeArgs {
    /// Decode profile: wsjtx, jtdx, or hybrid.
    #[arg(long, default_value = "wsjtx", help_heading = "Decode")]
    profile: String,

    /// My callsign, used by AP decode and hash-call unpacking.
    #[arg(short = 'c', long, help_heading = "Decode context")]
    my_call: Option<String>,

    /// My grid locator, retained in the decode config.
    #[arg(short = 'G', long, help_heading = "Decode context")]
    my_grid: Option<String>,

    /// His callsign, used by AP decode and hash-call unpacking.
    #[arg(short = 'x', long, help_heading = "Decode context")]
    his_call: Option<String>,

    /// His grid locator, retained in the decode config.
    #[arg(short = 'g', long, help_heading = "Decode context")]
    his_grid: Option<String>,

    /// QSO progress (0-5), used by AP pass selection.
    #[arg(short = 'Q', long, help_heading = "Decode context")]
    qso_progress: Option<usize>,

    /// Lower decode frequency bound in Hz
    #[arg(short = 'L', long, help_heading = "Frequency")]
    low: Option<f64>,

    /// Upper decode frequency bound in Hz
    #[arg(short = 'H', long, help_heading = "Frequency")]
    high: Option<f64>,

    /// Receive/QSO frequency offset in Hz, used by AP and focused retry logic.
    #[arg(short = 'f', long, help_heading = "Frequency")]
    rx_frequency: Option<f64>,

    /// Transmit frequency offset in Hz, used by AP frequency gating.
    #[arg(short = 'T', long, help_heading = "Frequency")]
    tx_frequency: Option<f64>,

    /// AP frequency gate width in Hz.
    #[arg(short = 'A', long, help_heading = "Frequency")]
    ap_width: Option<f64>,

    /// Decode depth
    #[arg(short = 'd', long, help_heading = "Decode")]
    depth: Option<usize>,

    /// Maximum sync candidates per pass
    #[arg(short = 'C', long, help_heading = "Decode")]
    max_candidates: Option<usize>,

    /// Disable AP decoding
    #[arg(short = 'P', long, help_heading = "Decode")]
    no_ap: bool,

    /// Restrict AP decoding to CQ-style AP.
    #[arg(short = 'O', long, help_heading = "Decode")]
    cq_only: bool,

    /// Enable JTDX SWL mode for profile=jtdx or profile=hybrid.
    #[arg(long, help_heading = "Decode")]
    swl: bool,

    /// Enable JTDX forced sync time-window tracking for profile=jtdx or profile=hybrid.
    #[arg(long, help_heading = "Decode")]
    force_sync: bool,

    /// Enable JTDX Hound AP table for profile=jtdx or profile=hybrid.
    #[arg(long, help_heading = "Decode")]
    hound: bool,

    /// JTDX FT8 band-decode threads: 0=auto, 1..24=user setting.
    #[arg(long, default_value_t = 0, help_heading = "Decode")]
    jtdx_threads: usize,

    /// Number of threads to process large FFTs. Values greater than 1 require an FFTW build.
    #[arg(short = 'm', long, default_value_t = 1, help_heading = "FFTW")]
    fft_threads: usize,

    /// FFTW3 planning patience (0-4). Values other than 1 require an FFTW build.
    #[arg(short = 'w', long, default_value_t = 1, help_heading = "FFTW")]
    patience: usize,
}

#[derive(Args)]
struct MonitorArgs {
    /// Input device selector: use the Index shown by `ft8rs monitor`, or the full device name.
    /// Omit this option to list input devices.
    #[arg(short = 'i', long, help_heading = "Input")]
    device: Option<String>,

    #[command(flatten, next_help_heading = "Decode context")]
    decode: DecodeArgs,

    /// Stop after this many 15-second slots. Omit to keep listening.
    #[arg(short = 'S', long, help_heading = "Input")]
    slots: Option<usize>,

    /// Send UDP decode reports in the compatible packet format.
    #[arg(short = 'u', long, help_heading = "Output")]
    udp: bool,

    /// UDP report destination host.
    #[arg(
        short = 'o',
        long,
        default_value = "127.0.0.1",
        help_heading = "Output"
    )]
    udp_host: String,

    /// UDP report destination port.
    #[arg(short = 'p', long, default_value_t = 2238, help_heading = "Output")]
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

    let config = stream_decode_config(&args.decode)?;

    let outputs = RefCell::new(Outputs::new(None)?);
    decode_wav_file_streaming_decodes(
        &args.input,
        FileDecodeOptions { start_time, config },
        |timestamp, row| outputs.borrow_mut().on_decode(timestamp, row),
        |timestamp, count| outputs.borrow_mut().on_slot_complete(timestamp, count),
    )
}

fn stream_decode_config(args: &DecodeArgs) -> Result<StreamDecodeConfig, String> {
    validate_decode_args(args)?;
    ft8rs::set_fft_patience(args.patience)?;
    ft8rs::set_fft_threads(args.fft_threads)?;

    let mut config = StreamDecodeConfig::default();
    config.profile = DecodeProfile::parse(&args.profile)?;
    if let Some(value) = normalized_nonempty(&args.my_call) {
        config.mycall = Some(value);
    }
    if let Some(value) = normalized_nonempty(&args.my_grid) {
        config.mygrid = Some(value);
    }
    if let Some(value) = normalized_nonempty(&args.his_call) {
        config.hiscall = Some(value);
    }
    if let Some(value) = normalized_nonempty(&args.his_grid) {
        config.hisgrid = Some(value);
    }
    if let Some(low) = args.low {
        config.nfa = low;
    }
    if let Some(high) = args.high {
        config.nfb = high;
    }
    if let Some(rx_frequency) = args.rx_frequency {
        config.nfqso = rx_frequency;
    }
    if let Some(tx_frequency) = args.tx_frequency {
        config.nftx = tx_frequency;
    }
    if let Some(qso_progress) = args.qso_progress {
        config.nQSOProgress = qso_progress;
    }
    if let Some(ap_width) = args.ap_width {
        config.napwid = ap_width;
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
    if args.cq_only {
        config.lapcqonly = true;
    }
    config.swl = args.swl;
    config.lforcesync = args.force_sync;
    config.lhound = args.hound;
    config.jtdx_threads = args.jtdx_threads;
    Ok(config)
}

fn validate_decode_args(args: &DecodeArgs) -> Result<(), String> {
    if let Some(qso_progress) = args.qso_progress {
        if qso_progress > 5 {
            return Err("--qso-progress must be in 0..=5".to_string());
        }
    }
    if let Some(depth) = args.depth {
        if !(1..=3).contains(&depth) {
            return Err("--depth must be in 1..=3".to_string());
        }
    }
    if let Some(max_candidates) = args.max_candidates {
        if max_candidates == 0 {
            return Err("--max-candidates must be at least 1".to_string());
        }
    }
    if let Some(ap_width) = args.ap_width {
        if ap_width <= 0.0 {
            return Err("--ap-width must be greater than 0".to_string());
        }
    }
    if let (Some(low), Some(high)) = (args.low, args.high) {
        if low >= high {
            return Err("--low must be less than --high".to_string());
        }
    }
    if args.jtdx_threads > 24 {
        return Err("--jtdx-threads must be in 0..=24".to_string());
    }
    Ok(())
}

fn normalized_nonempty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_uppercase())
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
            config: stream_decode_config(&args.decode)?,
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
