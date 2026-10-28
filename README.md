# ft8rs

`ft8rs` is a receive-only FT8 decoder written in Rust, with a graphical live
monitor, a command-line interface and a UI-independent decode core. It supports
live soundcard input, timestamped WAV recordings and optional UDP decode reports.

Four profiles are available: **WSJT-X**, **JTDX**, **Hybrid** and **DX**. The
default build uses RustFFT with the native 3840-point synchronization FFT and
has no FFTW runtime dependency. The product supports FT8 reception only.

## Build and start

Run commands from the repository root with a stable Rust toolchain:

```sh
# Graphical live monitor
cargo build --release -p ft8rs-gui
target/release/ft8rs-gui

# Command-line decoder
cargo build --release -p ft8rs-cli
target/release/ft8rs --help
```

On Windows the executables have an `.exe` suffix. Linux CLI builds need ALSA
headers and `pkg-config` (`libasound2-dev` and `pkg-config` on Debian/Ubuntu).
The repository's Windows/Linux x86-64 target configuration requires x86-64-v3.
See [build and development](docs/DEVELOPMENT.md) for the workspace, the optional
FFTW backend, GUI development and macOS app packaging.

Keep `ALLCALL7.TXT` beside the executable for every profile that uses JTDX,
including Hybrid and DX. Builds and release packages supply the repository's
runtime copy. The separate `ALLCALL7.TXT.2022` is used for calibrated regression
tests; changing the database can change decoded results.

## Graphical live monitor

1. Open **Settings → Audio**, select the receiver's audio input and channel.
2. Choose a profile in **Settings → Decode** or the **Profile** menu.
3. Enter your station context in **Settings → Station** if needed. DX also
   requires the target **Callsign** in the main window.
4. Press the play button to monitor; press stop to finish. Capture is aligned
   to UTC 15-second FT8 slots, and decoded messages appear as they arrive.

The GUI monitors live audio; use the CLI for WAV files. Settings changes can be
applied while listening. Audio diagnostics show the actual opened format,
input/channel peaks and capture timing. **Compare** can merge an external
WSJT-X UDP feed into the display; **Output** sends ft8rs decode reports.

[GUI, audio setup and comparison guide →](docs/USAGE.md)

## Command-line examples

```sh
# List audio input devices
target/release/ft8rs monitor

# Monitor by index or exact device name
target/release/ft8rs monitor --device 0
target/release/ft8rs monitor --device "VB-Cable A" --channel mono --profile hybrid

# Decode a timestamped recording from this repository
target/release/ft8rs file crates/ft8rs-core/tests/ft8/210703_133430.wav

# Supply a start time when the filename has no YYMMDD_HHMMSS timestamp
target/release/ft8rs file recording.wav --start-time 230208_140300

# Listen for one target, using your call as additional context
target/release/ft8rs monitor --device 0 \
  --profile dx --my-call F1MLZ --his-call UA3QNA

# Send live decode reports to the default destination, 127.0.0.1:2238
target/release/ft8rs monitor --device 0 --udp
```

`--channel mono` requests a native mono stream and uses channel 1 if only a
multichannel stream is available. `left` and `right` select a channel explicitly;
`mix` averages all channels. Choose the channel carrying the receiver signal:
averaging can attenuate single-sided audio or cancel opposite-phase channels.

[Complete CLI options, audio diagnostics and troubleshooting →](docs/USAGE.md)

## Decode profiles

| Profile | Behavior | Main controls |
|---|---|---|
| `wsjtx` | Default WSJT-X-aligned FT8 path, with staged early/final decoding | Optional call/grid and receive-frequency context |
| `jtdx` | Independent JTDX FT8 path | Optional SWL and focused `nagain` search |
| `hybrid` | Independent WSJT-X, JTDX/noAGC and JTDX/AGC sessions, plus bounded cross cancellation | SWL controls both JTDX paths; cancellation can be disabled |
| `dx` | Target-related output from three-way observations and isolated target recovery | Target call required; a valid full call pair enables additional context recovery |

Hybrid and DX combine existing native capabilities in the orchestration layer.
Their sessions keep independent hash books, AP memory and residuals. Unknown
native provenance stays unknown; recovered messages do not become independent
confirmation. GUI markers `?`, `sic` and `ctx` expose these distinctions.

JTDX defaults to **one logical frequency partition**, including its two Hybrid
paths. CLI `--jtdx-threads 0` explicitly enables CPU-dependent automatic selection.
This changes search/state evolution; it is separate from FFT threads and is not
an OS worker count. SWL-on is not guaranteed to be a superset of SWL-off results.

[WSJT-X](crates/ft8rs-core/WSJTX.md) · [JTDX](crates/ft8rs-core/JTDX.md) ·
[Hybrid](crates/ft8rs-core/HYBRID.md) · [DX](crates/ft8rs-core/DX.md)

## Local tests and reference data

Decode tests require optimized builds and run locally:

```sh
# Workspace unit tests
cargo test --profile fast --workspace --lib --bins -- --test-threads=1

# Serial decoder suite with an isolated 2022 CallDB
scripts/test-decoders.sh fast

# Final release validation, including slow/manual gates
scripts/test-decoders.sh release --include-ignored
```

Daily tests reuse versioned native WSJT-X/JTDX vectors and WAVs; they do not run
a simulator or require Fortran. Coverage includes protocol coding, `test28`,
known-message audio, Watterson channels, state/false-output checks and fixed noisy
versions of historical recordings. Generation and candidate-review scripts are
preserved for explicit upstream updates.

The WSJT-X reference targets are **21/21** for the short recording and **424/424**
for the long recording. JTDX's calibrated targets are **20/20** and **430/431**;
the long case fixes four partitions and the 2022 CallDB. Every required old
message must remain; added or unlabelled rows cannot offset losses.

[Test framework](crates/ft8rs-core/tests/decoder/README.md) ·
[Reference updates](crates/ft8rs-core/tests/decoder/UPDATING.md) ·
[Validation conditions and latest run](docs/VALIDATION.md)

## Documentation and license

The [documentation index](docs/README.md) separates usage, development,
source-alignment notes and dated experiment reports. Listening latency is
measured separately from all-worker completion; a +2/+3-second display target
is not a hard deadline for native calls already running.

Licensed under GPL-3.0-only. Run `ft8rs license` or open the GUI's **About**
window for copyright, attribution and the WSJT-X notice.
