# ft8rs

`ft8rs` is a Rust FT8 streaming decoder with a command-line interface. It
provides:

- live soundcard monitoring aligned to system time;
- WAV file input decoded by 15-second FT8 slots;
- a UI-independent FT8 decode core;
- CLI output by default, with optional UDP decode reports;
- selectable decode profiles: `wsjtx`, `jtdx`, `hybrid`, and `dx`.

The default build uses RustFFT at the WSJT-X/JTDX-aligned 3840-point size and
needs no FFTW at runtime. FFTW can be selected at compile time for alignment
checks.

## Build

```bash
cargo build --release                  # default (RustFFT)
cargo build --release --features fftw  # FFTW backend, for alignment checks
```

The FFTW backend needs system libraries: `brew install fftw` on macOS, or
`sudo apt-get install -y libasound2-dev libfftw3-dev pkg-config` on Linux. The
FFT backend is fixed at compile time; a built binary cannot switch at runtime.

## Runtime Files

Keep `ALLCALL7.TXT` beside the `ft8rs` binary when using the JTDX profile.
Release packages include it; a local `cargo build` copies the repository copy
(pinned to the JTDX CallDB used for alignment) into the target binary directory.

## CLI

```bash
target/release/ft8rs --help
target/release/ft8rs monitor --help
target/release/ft8rs file --help
target/release/ft8rs --version
```

Tagged releases print the tag (e.g. `ft8rs 0.0.2`); development builds append git
metadata (e.g. `ft8rs 0.0.0-dev+3ed8eaa`).

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

`monitor` aligns capture to the next UTC 15-second FT8 slot and streams decodes
as they are produced:

```text
HHMMSS SNR DT FREQ MESSAGE
------ slot done: NN decodes ------
```

For experimental DX deep-stack rows, the CLI prints a JTDX-formula SNR estimate
when the recovered message can be re-packed into tones. If that estimate is not
available, the CLI prints `DX` in the SNR column and UDP reports use `-99` as the
same explicit unavailable-SNR sentinel.

```text
133430  16 +0.3  2571 W1FC F5BZB -08
133430  14 -0.1  2157 WM3PEN EA6VQ -09
133430  -2 -0.8  1197 CQ F5RXL IN94
------ slot done: 21 decodes ------
```

Limit the number of slots (otherwise it runs until stopped):

```bash
target/release/ft8rs monitor --device "VB-Cable A" --slots 2
```

## Decode A WAV File

Offline decoding takes the same decode options as `monitor`, reading a recording
instead of live audio. A WSJT-X-style filename sets the start time; otherwise
pass `--start-time` (`YYMMDD_HHMMSS` or `HHMMSS`):

```bash
target/release/ft8rs file tests/ft8/210703_133430.wav
target/release/ft8rs file recording.wav --start-time 230208_140300
```

## Profiles

```text
wsjtx  - WSJT-X-aligned decoder, default
jtdx   - JTDX-oriented high-sensitivity decoder path
hybrid - WSJT-X and JTDX result union
dx     - single-target DX chase profile, requires --his-call
```

`dx` is not a `hybrid` alias. It is a separate single-target chase mode with its
own orchestration and target filter, so it can be tuned independently.

```bash
target/release/ft8rs monitor --device "VB-Cable A" --profile jtdx --swl
target/release/ft8rs monitor --device "VB-Cable A" --profile hybrid

# dx requires --his-call; --my-call unlocks AP/a8d recovery
target/release/ft8rs monitor --device "VB-Cable A" \
  --profile dx --my-call F1MLZ --his-call UA3QNA
```

JTDX band-decode threads default to source-style auto selection (`--jtdx-threads
0`); pass an explicit `1..=24` only for diagnostics.

## Decode Context

AP and focused-retry logic can use callsign/grid context:

```bash
target/release/ft8rs monitor --device "VB-Cable A" \
  --profile jtdx \
  --my-call K1ABC --my-grid FN20 \
  --his-call W9XYZ --his-grid EN60 \
  --qso-progress 0 --rx-frequency 1153
```

## UDP Reports

UDP output is off by default and can run alongside CLI output. The default
destination is `127.0.0.1:2238`.

```bash
target/release/ft8rs monitor --device "VB-Cable A" --udp
target/release/ft8rs monitor --device "VB-Cable A" --udp --udp-host 127.0.0.1 --udp-port 2238
```

## Options

| Option | Alias | Meaning |
|---|---:|---|
| `--start-time` | `-s` | file start timestamp |
| `--device` | `-i` | soundcard input index or name |
| `--slots` | `-S` | number of slots to decode |
| `--profile` | | decode profile: `wsjtx`, `jtdx`, `hybrid`, or `dx` |
| `--low` | `-L` | low decode frequency |
| `--high` | `-H` | high decode frequency |
| `--rx-frequency` | `-f` | focused receive frequency |
| `--tx-frequency` | `-T` | transmit/focused AP frequency |
| `--ap-width` | `-A` | AP frequency window |
| `--depth` | `-d` | decode depth |
| `--max-candidates` | `-C` | maximum sync candidates |
| `--no-ap` | `-P` | disable AP decode |
| `--cq-only` | `-O` | CQ-only AP mode |
| `--my-call` | `-c` | local callsign context |
| `--my-grid` | `-G` | local grid context |
| `--his-call` | `-x` | DX callsign context (required by `dx`) |
| `--his-grid` | `-g` | DX grid context |
| `--qso-progress` | `-Q` | AP QSO progress, `0..=5` |
| `--swl` | | enable JTDX SWL mode for `jtdx`/`hybrid` |
| `--nagain` | | JTDX `nagainfil` deep mode (OSD `ndeep=5`, focused `nfqso±25 Hz`); combine with `--swl` for max sensitivity |
| `--force-sync` | | enable JTDX forced sync time-window tracking |
| `--hound` | | enable JTDX Hound AP table for `jtdx`/`hybrid`; used by focused DX passes |
| `--dx-deep-experimental-output` | | allow experimental DX T1/T2 deep-stack rows into normal output; validation-only until the false-alarm corpus is green |
| `--dx-deep-diagnostics` | | print per-slot DX deep-engine counters to stderr |

The DX T1/T2 deep-integration engine runs **only** when one of the two flags
above is set; a plain `--profile dx` run skips it entirely and stays at baseline
speed. Setting either flag adds the per-slot deep cost (extra downsample/sync8 +
matched-filter/stack work per focus).
| `--jtdx-threads` | | JTDX FT8 band-decode threads, `0=auto`, `1..=24` |
| `--udp` | `-u` | enable UDP output |
| `--udp-host` | `-o` | UDP destination host |
| `--udp-port` | `-p` | UDP destination port |
| `--fft-threads` | `-m` | FFTW plan threads (FFTW build only) |
| `--patience` | `-w` | FFTW planning patience (FFTW build only) |

The `--fft-threads`/`--patience` options require a binary built with
`--features fftw`; the default RustFFT build accepts only the default FFT
settings.

## Tests

Decode tests must run optimized (they assert non-debug mode and have per-slot time
budgets). For the day-to-day edit/test loop use the `fast` profile — it keeps the
same optimized, byte-identical results but compiles in a fraction of the time
(incremental rebuild ~16s vs ~208s for `--release` on an 8-core machine, because it
drops the shipped binary's LTO + single-codegen-unit last mile):

```bash
cargo test --profile fast test_stream_decode_short_audio
cargo test --profile fast test_stream_decode_long_audio
```

Use `--release` for the final acceptance run that must match the shipped binary's
exact optimization, and for the FFTW path:

```bash
cargo test --release test_stream_decode_short_audio
cargo test --release --features fftw test_stream_decode_short_audio  # FFTW path
```

Both profiles produce identical decode results (the byte-identical WSJT-X/JTDX
baselines pass under either); `fast` only changes build speed, not float semantics.

Current release expectations:

```text
default wsjtx short fixture -> 21/21 target rows
default wsjtx long fixture  -> 424/424 target rows
jtdx short fixture          -> 20/20 target rows
jtdx long fixture           -> 430/431 target rows with auto threads
dx a8d fixture              -> target row recovered
```

The fixture CSV files include an `Extra` marker column:

```text
blank -> multi-verified baseline
W     -> WSJT-X-only reference row
J     -> JTDX target row
E     -> excluded row
```

## DX G2 False-Alarm Corpus

DX T1/T2 deep-stack rows are experimental until the real false-alarm corpus is
green. The full corpus is currently a long-term field-collection item; collect it
gradually during real use, then run the gate before enabling deep rows by default.
The manual gate expects a local directory pointed to by
`FT8RS_DX_G2_CORPUS`:

```text
$FT8RS_DX_G2_CORPUS/
  manifest.csv
  noise/*.wav
  wrong_call/*.wav
  on_band/*.wav
  hash_collision/*.wav
```

Start from `tests/ft8/g2_manifest.example.csv`.
Each manifest row names a manually confirmed absent target and a focus window.
Blank `wav` applies to every wav in that category.
When `wav` is set, it must be a file name that exists in that category directory;
the harness rejects missing names so a typo cannot silently shrink the corpus.

Run the gate manually in release mode:

```bash
FT8RS_DX_G2_CORPUS=/path/to/g2 \
  cargo test --release test_dx_profile_external_g2_corpus_no_deep_false_alarm -- --ignored --nocapture
```

The required budgets are `noise >= 5760` slots, `wrong_call >= 1000` slots,
`on_band >= 480` slots, and `hash_collision >= 50` slots. A skipped run is only
harness evidence, not safety evidence. Until this field corpus is green,
experimental deep rows should remain behind `--dx-deep-experimental-output`.

## Developer Notes

- WSJT-X alignment notes are in `WSJTX.md`.
- JTDX alignment notes are in `JTDX.md`.
- Hybrid result-union notes are in `HYBRID.md`.
- DX chase profile notes are in `DX.md`.
- License: GPL-3.0.
