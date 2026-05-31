# ft8rs

`ft8rs` is a WSJT-X-aligned FT8 streaming decoder written in Rust.

It provides:

- a pure FT8 decode core without UI coupling;
- file input that decodes WAV audio by FT8 slots;
- soundcard input that aligns to system time and monitors live FT8 audio;
- CLI output by default;
- optional UDP decode reports compatible with WSJT-X-style consumers.

The default build uses `rustfft @ 3840` and has no FFTW runtime dependency.
An FFTW build is available at compile time for alignment checks.

## Requirements

Install a stable Rust toolchain.

Default build:

```bash
cargo build --release
```

FFTW build:

```bash
cargo build --release --features fftw
```

On macOS, install FFTW first:

```bash
brew install fftw
```

On Linux, install the usual native dependencies:

```bash
sudo apt-get install -y libasound2-dev libfftw3-dev pkg-config
```

The FFT backend is selected at compile time. A built binary cannot switch
between RustFFT and FFTW at runtime.

## CLI

The binary is:

```bash
target/release/ft8rs
```

Help:

```bash
target/release/ft8rs --help
target/release/ft8rs file --help
target/release/ft8rs monitor --help
```

Version:

```bash
target/release/ft8rs --version
```

Release artifacts built from a tag such as `v0.0.1` print that tag version:

```text
ft8rs 0.0.1
```

Local development builds print the default package version plus git metadata,
for example:

```text
ft8rs 0.0.0-dev+3ed8eaa
```

## Decode A WAV File

If the filename contains a WSJT-X timestamp, `ft8rs` infers the slot time:

```bash
target/release/ft8rs file tests/ft8/210703_133430.wav
```

Output format:

```text
HHMMSS SNR DT FREQ MESSAGE
```

Example:

```text
133430  16 +0.3  2571 W1FC F5BZB -08
133430  14 -0.1  2157 WM3PEN EA6VQ -09
133430  -2 -0.8  1197 CQ F5RXL IN94
------ slot done: 21 decodes ------
```

For files without a timestamp in the name, pass `--start-time`.

Accepted formats:

- `YYMMDD_HHMMSS`, for example `230208_140300`
- `HHMMSS`, for example `140300`

```bash
target/release/ft8rs file recording.wav --start-time 230208_140300
target/release/ft8rs file recording.wav --start-time 140300
```

## Monitor A Soundcard

List input devices:

```bash
target/release/ft8rs monitor
```

Monitor by input device index or exact device name:

```bash
target/release/ft8rs monitor --device 0
target/release/ft8rs monitor --device "VB-Cable A"
```

Limit the number of slots:

```bash
target/release/ft8rs monitor --device "VB-Cable A" --slots 2
```

`monitor` aligns capture to the next UTC 15-second FT8 slot and streams decodes
as each decode stage produces new messages.

## UDP Reports

UDP output is off by default. Enable it with `--udp`.

Default destination:

- host: `127.0.0.1`
- port: `2238`

```bash
target/release/ft8rs monitor --device "VB-Cable A" --udp
target/release/ft8rs monitor --device "VB-Cable A" --udp --udp-host 127.0.0.1 --udp-port 2238
```

UDP can be used together with CLI output.

## Decode Options

Common options:

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav \
  --low 200 \
  --high 3000 \
  --depth 3 \
  --max-candidates 1000
```

Short aliases:

| Option | Alias | Meaning |
|---|---:|---|
| `--start-time` | `-s` | file start timestamp |
| `--device` | `-i` | soundcard input index or name |
| `--slots` | `-S` | number of slots to decode |
| `--low` | `-L` | low decode frequency |
| `--high` | `-H` | high decode frequency |
| `--rx-frequency` | `-f` | focused receive frequency |
| `--tx-frequency` | `-T` | transmit/focused AP frequency |
| `--ap-width` | `-A` | AP frequency window |
| `--depth` | `-d` | decode depth |
| `--max-candidates` | `-C` | sync candidates |
| `--no-ap` | `-P` | disable AP decode |
| `--cq-only` | `-O` | CQ-only AP mode |
| `--my-call` | `-c` | local callsign context |
| `--my-grid` | `-G` | local grid context |
| `--his-call` | `-x` | DX callsign context |
| `--his-grid` | `-g` | DX grid context |
| `--qso-progress` | `-Q` | AP QSO progress, `0..=5` |
| `--udp` | `-u` | enable UDP output |
| `--udp-host` | `-o` | UDP destination host |
| `--udp-port` | `-p` | UDP destination port |
| `--fft-threads` | `-m` | FFTW plan threads |
| `--patience` | `-w` | FFTW planning patience |

Context example:

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav \
  --my-call K1ABC \
  --my-grid FN20 \
  --his-call W9XYZ \
  --his-grid EN60 \
  --qso-progress 0
```

FFTW-only options:

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav --fft-threads 3 --patience 1
```

The default RustFFT build accepts only the default FFT settings.

## Test

Decode tests should be run in release mode.

Short file:

```bash
cargo test --release test_stream_decode_short_audio -- --nocapture
```

Long file:

```bash
cargo test --release test_stream_decode_long_audio -- --nocapture
```

FFTW path:

```bash
cargo test --release --features fftw test_stream_decode_short_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture
```

Current expected release summaries:

- short fixture: `21` unique messages;
- long fixture: `425/425` WSJT-X target rows, every slot under `15s`.

To write a long-test diff CSV for investigation:

```bash
FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture
```

## Notes

- Active WAV fixtures are under `tests/ft8`.
- Older diagnostic files are kept under `tests/old`.
- WSJT-X alignment notes for developers are in `WSJTX.md`.
- License: GPL-3.0.
