use std::cell::RefCell;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

mod output;
mod output_cli;

use ft8rs::input::{
    decode_wav_file_streaming_tagged, infer_start_time_from_path, FileDecodeOptions,
};
use ft8rs::stream::{DecodeProfile, StreamDecodeConfig};
use ft8rs::SlotTimestamp;
use ft8rs_engine::{
    decode_soundcard_streaming_tagged, list_soundcards, InputChannel, SoundcardDecodeOptions,
    SoundcardFormatInfo,
};

use output::{Outputs, UdpConfig};

const VERSION: &str = env!("FT8RS_VERSION");
const DX_MONITOR_WATCHDOG_MS: u64 = 12_000;

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
    /// Show copyright, license, and attribution information
    License,
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
    /// Decode profile: wsjtx, jtdx, hybrid, or dx.
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

    /// Lower decode frequency bound in Hz
    #[arg(short = 'L', long, help_heading = "Frequency")]
    low: Option<f64>,

    /// Upper decode frequency bound in Hz
    #[arg(short = 'H', long, help_heading = "Frequency")]
    high: Option<f64>,

    /// Receive/QSO frequency offset in Hz, used by AP and focused retry logic.
    #[arg(short = 'f', long, help_heading = "Frequency")]
    rx_frequency: Option<f64>,

    /// Enable JTDX SWL mode (both JTDX paths for profile=hybrid).
    /// The dx profile enables its own SWL listen pass automatically.
    #[arg(long, help_heading = "Decode")]
    swl: bool,

    /// Logical JTDX partitions (default 1); pass 0 for CPU-dependent automatic
    /// selection. Applies to both JTDX paths in Hybrid, not FFT threads.
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=24), help_heading = "Decode")]
    jtdx_threads: Option<u8>,

    /// Enable JTDX "decode again" deep mode (nagainfil): OSD ndeep=5 plus a
    /// focused nfqso+/-25 Hz window. Combine with --swl for max sensitivity.
    /// The dx profile enables nagain only for its own focused passes.
    #[arg(long, help_heading = "Decode")]
    nagain: bool,

    /// Disable Hybrid's bounded cross-decoder cancellation round.
    #[arg(long, help_heading = "Decode")]
    no_hybrid_sic: bool,
    /// Enable the experimental weak-signal/interference branch (Hybrid only).
    #[arg(long, help_heading = "Decode")]
    hybrid_experiment: bool,
}

#[derive(Args)]
struct MonitorArgs {
    /// Input device selector: use the Index shown by `ft8rs monitor`, or the full device name.
    /// Omit this option to list input devices.
    #[arg(short = 'i', long, help_heading = "Input")]
    device: Option<String>,

    /// Input channel: mono (default, requests mono or uses channel 1), left,
    /// right, or mix (explicitly averages all channels).
    #[arg(long, default_value = "mono", help_heading = "Input")]
    channel: String,

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
    if ft8rs_engine::run_decode_worker_if_requested() {
        return;
    }
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
        Command::License => {
            print!("{}", ft8rs::about::notice(VERSION));
            Ok(())
        }
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
    decode_wav_file_streaming_tagged(
        &args.input,
        FileDecodeOptions { start_time, config },
        |timestamp, row| outputs.borrow_mut().on_decode(timestamp, row),
        |timestamp, count| outputs.borrow_mut().on_slot_complete(timestamp, count),
    )
}

fn stream_decode_config(args: &DecodeArgs) -> Result<StreamDecodeConfig, String> {
    validate_decode_args(args)?;
    // FFT threads/patience are no longer CLI-configurable; initialize the FFT
    // backend with its defaults (only the FFTW build honors non-default values).
    ft8rs::set_fft_threads(1)?;
    ft8rs::set_fft_patience(1)?;

    let mut config = StreamDecodeConfig {
        profile: DecodeProfile::parse(&args.profile)?,
        ..Default::default()
    };
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
    config.swl = args.swl;
    if let Some(partitions) = args.jtdx_threads {
        config.jtdx_threads = usize::from(partitions);
    }
    config.nagain = args.nagain;
    config.hybrid_sic = !args.no_hybrid_sic;
    if args.hybrid_experiment && config.profile != DecodeProfile::Hybrid {
        return Err("--hybrid-experiment requires --profile hybrid".to_string());
    }
    config.hybrid_experiment = args.hybrid_experiment;
    if config.profile == DecodeProfile::Dx && config.hiscall.is_none() {
        return Err("--profile dx requires --his-call CALL".to_string());
    }
    Ok(config)
}

fn validate_decode_args(args: &DecodeArgs) -> Result<(), String> {
    if let (Some(low), Some(high)) = (args.low, args.high) {
        if low >= high {
            return Err("--low must be less than --high".to_string());
        }
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

    let channel = InputChannel::parse(&args.channel).ok_or_else(|| {
        format!(
            "invalid --channel '{}': use mono, left, right, or mix",
            args.channel
        )
    })?;
    let udp = args.udp.then_some(UdpConfig {
        host: args.udp_host,
        port: args.udp_port,
    });
    let outputs = RefCell::new(Outputs::new(udp)?);
    let mut config = stream_decode_config(&args.decode)?;
    config.cooperative_realtime = true;
    if config.profile == DecodeProfile::Dx {
        config.dx_monitor_watchdog_ms = Some(DX_MONITOR_WATCHDOG_MS);
    }
    decode_soundcard_streaming_tagged(
        SoundcardDecodeOptions {
            device: args.device,
            channel,
            config,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experiment_is_opt_in_and_hybrid_only() {
        for profile in ["wsjtx", "jtdx", "hybrid", "dx"] {
            let mut argv = vec!["ft8rs", "file", "recording.wav", "--profile", profile];
            if profile == "dx" {
                argv.extend(["--his-call", "K1ABC"]);
            }
            let cli = Cli::try_parse_from(&argv).unwrap();
            let Command::File(args) = cli.command else {
                unreachable!()
            };
            assert!(
                !stream_decode_config(&args.decode)
                    .unwrap()
                    .hybrid_experiment
            );
            argv.push("--hybrid-experiment");
            let cli = Cli::try_parse_from(argv).unwrap();
            let Command::File(args) = cli.command else {
                unreachable!()
            };
            let config = stream_decode_config(&args.decode);
            if profile == "hybrid" {
                assert!(config.unwrap().hybrid_experiment);
            } else {
                assert!(config.is_err());
            }
        }
    }

    #[test]
    fn partition_default_and_explicit_auto_reach_the_decoder_config() {
        for profile in ["jtdx", "hybrid"] {
            for (option, expected) in [(None, 1), (Some("0"), 0), (Some("4"), 4)] {
                let mut argv = vec!["ft8rs", "file", "recording.wav", "--profile", profile];
                if let Some(partitions) = option {
                    argv.extend(["--jtdx-threads", partitions]);
                }
                let cli = Cli::try_parse_from(argv).unwrap();
                let Command::File(args) = cli.command else {
                    unreachable!()
                };
                assert_eq!(
                    stream_decode_config(&args.decode).unwrap().jtdx_threads,
                    expected
                );
            }
        }
        assert!(
            Cli::try_parse_from(["ft8rs", "file", "recording.wav", "--jtdx-threads", "25"])
                .is_err()
        );
    }
}
