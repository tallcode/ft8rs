# Build and development

Run commands from the repository root. Project constraints and the direct-main
integration workflow are in [AGENTS.md](../AGENTS.md). Local checks are required;
CI does not replace local decoder validation.

## Workspace

| Package | Responsibility | Output |
|---|---|---|
| `ft8rs-core` | Native FT8 mirrors, stream sessions, WAV input and orchestration | Rust library named `ft8rs` |
| `ft8rs-engine` | Capture, UTC slot staging, decode actor, reconfiguration and UDP | Library shared by the frontends |
| `ft8rs-cli` | File/monitor commands and console output | `ft8rs` |
| `ft8rs-gui` | egui live monitor, settings and external UDP comparison | `ft8rs-gui` |
| `ft8rs-build` | Shared build-time Git/release version metadata | Build helper |

The production decoder is Rust. Fortran, Boost and a native upstream checkout are
needed only when explicitly regenerating reference data, not for normal builds
or tests.

## Build profiles

```sh
cargo build --release -p ft8rs-cli -p ft8rs-gui
cargo gui
```

`cargo gui` is the repository alias for `cargo run -p ft8rs-gui`. The development
profile optimizes the core and dependencies for usable live decoding while
keeping frontend iteration faster. It retains debug assertions, so use an
optimized test profile for decoder acceptance.

| Profile | Purpose |
|---|---|
| `dev` | Interactive frontend development; not the decoder test profile |
| `fast` | Optimized local tests; debug assertions off, no LTO, many codegen units |
| `release` | Final validation/shipping; fat LTO and one codegen unit |

`fast` is intended to preserve decode behavior while reducing build time. The
latest fixed-corpus controls matched release byte-for-byte; final acceptance
still uses release. Do not infer universal floating-point equivalence between
all platforms, compilers or FFT backends from that observation.

A stable Rust toolchain and the host's native compiler/linker are needed. Linux
CLI builds require ALSA development headers and `pkg-config`:

```sh
sudo apt-get install libasound2-dev pkg-config
```

The GUI additionally needs the host desktop/window-system dependencies used by
its eframe and menu backends. Release packaging currently builds Windows GUI +
CLI and Linux CLI; macOS app packaging runs locally. The configured Windows MSVC
and Linux GNU x86-64 targets use `target-cpu=x86-64-v3`; older CPUs need a separate
build configuration and validation.

## FFT backend

RustFFT at the native 3840-point sync size is the default. FFTW is compile-time
optional; the binary cannot switch at runtime:

```sh
cargo build --release -p ft8rs-cli --features fftw
cargo test --release -p ft8rs-core --features fftw test_stream_decode_short_audio
cargo test --release -p ft8rs-core --features fftw test_stream_decode_long_audio
```

The runtime FFTW feature links double-precision `fftw3_threads` and `fftw3`.
Install FFTW and make its library paths discoverable, for example `brew install
fftw pkg-config` on macOS or `libfftw3-dev pkg-config` on Debian/Ubuntu. The
**reference generator's** single-precision static FFTW with Fortran wrappers is a
separate requirement; see [reference updates](../crates/ft8rs-core/tests/decoder/UPDATING.md).
The old 4096-point synchronization backend is not supported.

## Runtime database and packaging

`crates/ft8rs-core/ALLCALL7.TXT` is the runtime database. The core build script
copies it into the Cargo profile's binary directory; release packages also keep
it beside the executable. JTDX looks in the executable directory, working
directory, then the core manifest directory. Hybrid and DX use JTDX too.
Database changes can alter filtering, subtraction and decoded rows.

`ALLCALL7.TXT.2022` is the separate historical calibration database. The decoder
test runners copy it beside private temporary executables. Never overwrite the
runtime or build-directory database to run calibration or concurrent experiments.

On macOS:

```sh
scripts/bundle-macos.sh
```

This builds the release GUI and assembles `dist/ft8.rs.app` with icon, microphone
usage metadata and CallDB. macOS can require input permission; local bundles are
unsigned. The release workflow in `.github/workflows/release.yml` packages tagged
Windows and Linux assets. This guide describes packaging; it does not publish a
release when running tests or editing documentation.

## Local checks

For normal Rust changes:

```sh
cargo fmt --all -- --check
cargo clippy --profile fast --workspace --all-targets -- -D warnings
cargo test --profile fast --workspace --lib --bins -- --test-threads=1
```

GUI UDP tests use a real local loopback socket. They need an environment that
permits local socket creation; a sandbox denial is not a decoder failure.
There is no TypeScript/ESLint toolchain in this workspace.

For decoder changes, also run:

```sh
scripts/test-decoders.sh fast
scripts/test-decoders.sh release --include-ignored
```

The complete runner verifies the fixed corpus, builds first, then runs all core
library/integration executables serially with the private 2022 CallDB. It includes
ignored historical profile tests, DX known-truth gates and the long degraded
recording matrix. `scripts/acceptance-gates.sh [filter]` remains a narrower helper
for only the ignored `stream_decode_test` gates; it is not the complete suite.

For changes to the engine's staged delivery, add the engine's separate long gate:

```sh
cargo test --release -p ft8rs-engine decode_actor_hybrid_long_emits_jtdx_unique -- --ignored --test-threads=1
```

This actor comparison uses the normal runtime database lookup, rather than the
core runner's private calibration setup. Hold that database fixed for comparisons.

Optional source audits require local original checkouts:

```sh
FT8RS_REQUIRE_WSJTX_SOURCE=1 cargo test --release -p ft8rs-core --test wsjtx_source_audit_test
FT8RS_REQUIRE_JTDX_SOURCE=1 cargo test --release -p ft8rs-core --test jtdx_source_audit_test
```

Set `FT8RS_WSJTX_SOURCE_DIR` / `FT8RS_JTDX_SOURCE_DIR` to select explicit roots.
Without an explicit path or require flag, an absent checkout may skip source
comparisons. Versioned native fixture tests still run without those checkouts.

## Decoder change review

Hybrid/DX orchestration must use existing native capabilities without changing
native thresholds, candidate order, AP scheduling or mathematical calculations.
Keep mutable native sessions, hash books and audio residuals isolated. Source
alignment corrections belong to their native profile and need independent
reference evidence, separate from an orchestration feature.

Compare old and new outputs per signal with the same recording, configuration,
CallDB, build profile and FFT backend. An added row cannot compensate for a lost
old row. Unlabelled observations stay unconfirmed. Use the
[snapshot comparison workflow](../crates/ft8rs-core/tests/decoder/README.md#beforeafter-message-preservation).

Measure callback arrival and all-worker completion separately. Run timed replay
processes serially, without concurrent compilation or CPU benchmarks. Record
visible cores and resolved partition counts for automatic JTDX mode. Real-time
task-start budgets cannot interrupt active native calls and do not guarantee a
hard display deadline.

Current operating points and recorded limitations live in
[validation](VALIDATION.md); detailed experimental timings live in
[dated reports](README.md#dated-research-and-measurements).
