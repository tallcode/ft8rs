# ft8rs

[![crates.io](https://img.shields.io/crates/v/ft8rs.svg)](https://crates.io/crates/ft8rs)
[![docs.rs](https://docs.rs/ft8rs/badge.svg)](https://docs.rs/ft8rs/latest/ft8rs/)

`ft8rs::wsjtx` provides stateful WSJT-X FT8 decoding from mono PCM audio.
This unofficial Rust port preserves the existing
WSJT-X session, decode stages, native calculations and cross-slot memory.
Only FT8 reception is supported by this facade.

[API reference](https://docs.rs/ft8rs/latest/ft8rs/wsjtx/) |
[Core source](https://github.com/tallcode/ft8rs/tree/main/crates/ft8rs-core) |
[Repository documentation](https://github.com/tallcode/ft8rs/blob/main/docs/README.md)

## Install

```toml
[dependencies]
ft8rs = "0.1"
```

The direct dependencies are `serde`, `rustfft`, `hound` and `num-bigint`, plus
their transitive dependencies. The published crate has no GUI or audio-capture
dependency.

Use an optimized build for actual decoding, such as `cargo run --release`.
The default FFT backend is RustFFT; no Fortran compiler or FFTW installation is
required. The optional `fftw` feature requires `fftw3`, `fftw3_threads` and their
native library search paths (normally discovered through `pkg-config`).

## Core execution model

```text
Complete UTC-aligned PCM slot
    -> validate input and convert to 12 kHz
    -> early native decode (nzhsym=41) -> optional message batch
    -> native signal subtraction (nzhsym=47)
    -> final native decode and AP (nzhsym=50) -> optional new-message batch
    -> slot completion
```

Synchronization, message decoding, FEC and subtraction use the existing native
Rust port. The facade supplies input validation, audio conversion and result
mapping around that session. It preserves native thresholds, candidate order,
AP scheduling, message text and numerical results. The early stage is skipped
at shallow decode depth; any stage with no new messages emits no batch.

## Public interface

| Method | Result | Delivery |
|---|---|---|
| `Decoder::decode` | `Result<Vec<Message>, Error>` | Returns all messages after the slot finishes |
| `Decoder::decode_with_callback` | `Result<(), Error>` | Sends each nonempty native-stage batch to the callback; return signals completion |
| `Decoder::reset` | `()` | Clears learned history while retaining configuration |

Both decode methods accept one complete slot. They do not accept arbitrary
capture chunks or perform wall-clock slot alignment. Supply the first sample
at the UTC slot boundary and assemble capture chunks in your application.

## Decode one slot

```rust
use ft8rs::wsjtx::{Audio, Config, Decoder, SlotTimestamp};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = Decoder::new(Config::default())?;
    // Replace this silent buffer with mono PCM starting at the UTC slot boundary.
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

The silent example above returns no messages. For a receiving application,
replace the buffer with your captured PCM samples.

## Audio and session contract

| Input | Requirement |
|---|---|
| Samples | Borrowed mono `f32` PCM with nominal full scale `-1..=1`; all values must be finite |
| Sample rate | 12,000-192,000 Hz, supplied explicitly |
| Buffer length | Exactly `sample_rate * 15` samples |
| Start time | The first sample corresponds to the supplied UTC 15-second boundary |
| Timestamp format | `YYMMDD_HHMMSS`, `YYYYMMDD_HHMMSS`, or UTC `HHMMSS` |

12 kHz audio is used directly; other rates use the existing anti-alias input
conversion. Finite headroom is preserved without normalization or clipping.
Invalid input returns a typed `wsjtx::Error` before changing session state or
invoking a batch callback. A successful decode with no messages is a normal
result, not an error.

Reuse the same `Decoder` for consecutive slots, including silent slots, to
preserve native hash/AP memory. Call `reset()` before changing bands, after a
gap in the recording, or before decoding unrelated audio. Reset retains the
configuration and explicitly configured station context. Each decoder instance
owns its session state.

The facade requires no CallDB and has no WAV, soundcard, or transmit API.
Optional call/grid context in `Config` supplies native lookup/AP hints; it is
not independent evidence of reception. See the
[configuration reference](https://docs.rs/ft8rs/latest/ft8rs/wsjtx/struct.Config.html)
for defaults and allowed ranges.

## Receive batches promptly

`decode_with_callback` sends each nonempty native-stage batch before continuing
with the remaining work. The callback receives an owned `Vec<wsjtx::Message>`
that can be sent directly to a channel:

```rust
use ft8rs::wsjtx::{Audio, Config, Decoder, SlotTimestamp};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut decoder = Decoder::new(Config::default())?;
    // Replace this silent buffer with 15 seconds of captured mono PCM.
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
batch. Native passes do not expose finer callbacks. Message text, order, SNR in
dB, audio frequency in Hz, DT in seconds and native sync values are preserved;
no AP subtype or independent confirmation is inferred. Both decode methods are
synchronous: run them on a worker thread, outside a real-time audio callback,
and keep batch handlers short.

## Source and tests

The `wsjtx` entry point selects the WSJT-X session. This overview covers its
audio-to-message contract; lower-level modules and other decoder profiles in
the repository have separate implementation notes.

The crate includes the small protocol/waveform fixtures needed by its library
unit tests. Run them with `cargo test --release --lib`. Large recordings and
audio integration tests remain in the [source repository](https://github.com/tallcode/ft8rs).
From a source checkout, run the WSJT-X facade regressions with an optimized
build:

```sh
cargo test --profile fast -p ft8rs --test wsjtx_api_test -- --test-threads=1
```

These checks compare each historical-recording message and numerical field
against the existing WSJT-X session. They also cover input conversion, invalid
input, session isolation, reset behavior and native-stage batch delivery.

## License and attribution

GPL-3.0-only. This crate contains derivative Rust ports of WSJT-X and JTDX;
changing the programming language does not remove their license obligations.
See the bundled [LICENSE](https://github.com/tallcode/ft8rs/blob/main/LICENSE)
and [NOTICE](https://github.com/tallcode/ft8rs/blob/main/NOTICE) for the full license,
upstream copyright notices and attributions. Original source mappings remain
in the ported files. The Rust port and facade are copyright 2026 Tao Xu (BG5ATV).
