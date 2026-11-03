# ft8rs

A receive-only FT8 decoder library written in Rust, based on the WSJT-X and
JTDX decoders. The `ft8rs::wsjtx` facade provides stateful WSJT-X FT8 decoding
from mono PCM audio, with optional early batch delivery. This is an unofficial
Rust port, independent of the WSJT-X project. Only FT8 reception is supported.

## Install

```toml
[dependencies]
ft8rs = "0.1"
```

Use an optimized build for actual decoding, such as `cargo run --release`.
The default FFT backend is RustFFT; no Fortran compiler or FFTW installation is
required. The optional `fftw` feature requires `fftw3`, `fftw3_threads` and their
native library search paths (normally discovered through `pkg-config`).

## Decode one slot

```rust
use ft8rs::wsjtx::{Audio, Config, Decoder, SlotTimestamp};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = Decoder::new(Config::default())?;
    // Replace with 15 seconds of mono PCM starting at this UTC slot boundary.
    let samples = vec![0.0_f32; 15 * 48_000];
    let messages = decoder.decode(
        SlotTimestamp::parse("261103_120000")?,
        Audio::mono_f32(&samples, 48_000),
    )?;
    for message in messages {
        println!("{} {} dB {} Hz {}", message.slot, message.snr_db,
                 message.frequency_hz, message.text);
    }
    Ok(())
}
```

The input is borrowed mono `f32` PCM with nominal full scale `-1..=1`.
Every sample must be finite. Rates from 12 kHz through 192 kHz are supported;
input is converted to 12 kHz through the existing anti-alias input path.
Each buffer must contain exactly 15 seconds, starting at the supplied UTC
15-second boundary. Invalid input returns a typed `wsjtx::Error` before changing
the session state. Finite audio headroom is preserved without clipping.

Reuse the same `Decoder` for consecutive slots, including silent slots, to
preserve native hash/AP memory. Call `reset()` before changing bands or decoding
an unrelated recording. Explicit station context in `Config` survives reset.
The facade requires no CallDB and has no WAV, soundcard, or transmit API.

## Receive batches promptly

`decode_with_callback` sends each nonempty native-stage batch before continuing
with the remaining work. The callback receives an owned `Vec<wsjtx::Message>`
that can be sent directly to a channel:

```rust
use ft8rs::wsjtx::{Audio, Config, Decoder, SlotTimestamp};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = Decoder::new(Config::default())?;
    let samples = vec![0.0_f32; 180_000];
    // Move the receiver to your UI or consumer in a real application.
    let (sender, _receiver) = std::sync::mpsc::channel();
    decoder.decode_with_callback(
        SlotTimestamp::parse("261103_120000")?,
        Audio::mono_f32(&samples, 12_000),
        |batch| { let _ = sender.send(batch); },
    )?;
    // Returning from the method signals completion of all work for this slot.
    Ok(())
}
```

There are zero, one or two callbacks per slot: an early `nzhsym=41` batch before
subtraction/final decoding, then new final-stage rows including native AP.
Empty stages produce no callback, and early rows are not sent again in the final
batch. Native passes do not expose finer callbacks. Message text, order, SNR,
frequency, DT and sync values are preserved; no AP subtype or independent
confirmation is inferred. Both decode methods are synchronous: run them on a
worker thread, outside a real-time audio callback, and keep batch handlers short.

## Source and tests

The source repository also contains JTDX, Hybrid and DX orchestration and the
CLI/GUI applications. The `wsjtx` facade selects only the existing WSJT-X
session. Native algorithms, thresholds, candidate order and AP scheduling are
unchanged by the facade. Other profiles can require an external CallDB; the
registry package does not bundle one.

The crate includes the small protocol/waveform fixtures needed by its library
unit tests. Run them with `cargo test --release --lib`. Large recordings and
audio integration tests remain in the [source repository](https://github.com/tallcode/ft8rs).
From a checkout, `scripts/test-decoders.sh release --include-ignored` runs the
full serial decoder suite using an isolated calibration database.

## License and attribution

GPL-3.0-only. This crate contains derivative Rust ports of WSJT-X and JTDX;
changing the programming language does not remove their license obligations.
See the bundled [LICENSE](LICENSE) and [NOTICE](NOTICE) for the full license,
upstream copyright notices and attributions. Original source mappings remain
in the ported files. The Rust port and facade are copyright 2026 Tao Xu (BG5ATV).
