# ft8rs

FT8/FT4 encoder and decoder in pure Rust. A port of the TypeScript implementation [ft8ts](https://github.com/e04/ft8ts), which is itself based on the Fortran reference from [WSJT-X](https://wsjt.sourceforge.io/wsjtx.html) v2.7.0.

## Overview

FT8 and FT4 are digital amateur radio modes designed for weak-signal communication, developed by Joe Taylor (K1JT) and Steve Franke (K9AN). FT8 transmits 77-bit messages in 15-second frames using 8-GFSK modulation, with LDPC error correction enabling reliable decoding at signal-to-noise ratios as low as -24 dB.

This library provides pure Rust implementations of both encoding and decoding for FT8 and FT4, suitable for use as a command-line tool or integrated into other Rust projects.

## Features

- **FT8/FT4 decode** — Decode WAV files with configurable frequency range and decoding depth
- **FT8 encode** — Generate WAV audio from FT8 messages
- **CLI tool** — Simple command-line interface for decode and encode
- **Pure Rust** — Zero FFI, no external C/Fortran dependencies
- **High performance** — Efficient FFT and LDPC implementation in idiomatic Rust

## Installation

```bash
cargo install --path .
```

## Usage

### CLI

```bash
# Decode a WAV file with deep decoding
ft8rs decode tests/ft8/210703_133430.wav --depth 3

# Decode with custom frequency range
ft8rs decode recording.wav --low 200 --high 3000 --depth 2

# Decode FT4 mode
ft8rs decode recording.wav --mode ft4

# Encode a message to WAV
ft8rs encode "CQ JK1IFA PM95" --out output.wav --df 1000
```

### Library

```rust
use ft8rs::{decode_ft8, DecodeFT8Options};

let samples: Vec<f32> = load_wav("recording.wav");
let decoded = decode_ft8(&samples, DecodeFT8Options {
    sample_rate: Some(12000),
    freq_low: Some(200.0),
    freq_high: Some(3000.0),
    depth: Some(3),
    ..Default::default()
});

for d in &decoded {
    println!("{} Hz  SNR {} dB  dt {}s  {}", d.freq, d.snr, d.dt, d.msg);
}
```

### Decode Options

| Option | Default | Description |
|--------|---------|-------------|
| `sample_rate` | 12000 | Input audio sample rate (Hz) |
| `freq_low` | 200 | Lower frequency bound (Hz) |
| `freq_high` | 3000 | Upper frequency bound (Hz) |
| `sync_min` | 1.2 | Minimum sync threshold |
| `depth` | 2 | Decoding depth: 1=fast, 2=normal, 3=deep |
| `max_candidates` | 300 | Maximum candidates to process |

## License

GPL-3.0

## Acknowledgements

- **[WSJT-X](https://wsjt.sourceforge.io/wsjtx.html)** — The original Fortran implementation by Joe Taylor (K1JT), Steve Franke (K9AN), and the WSJT development team. Licensed under GPL v3.

- **[ft8ts](https://github.com/e04/ft8ts)** — A clean TypeScript reference implementation that this Rust port is based on. Its well-structured code and thorough test cases made this port possible.

- **[PyFT8](https://github.com/G1OJS/PyFT8)** — A pure Python FT8 implementation.

- **[ft8_lib](https://github.com/kgoba/ft8_lib)** — A lightweight C implementation of FT8/FT4.
