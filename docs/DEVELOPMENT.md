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
unsigned. The `Build packages` workflow in `.github/workflows/release.yml` builds
Windows CLI + GUI in one Cargo invocation; Linux always builds and packages only
the CLI. Both use the existing release optimization settings.

### Development packages and cache warming

In GitHub Actions, select **Build packages → Run workflow**, then choose `main`
or a branch in **Use workflow from**. The same operation is available from the
CLI:

```sh
gh workflow run release.yml --ref main
gh workflow run release.yml --ref codex/your-branch
```

The workflow must first exist on the default branch; the selected branch must
also contain the workflow and packaging helper. Manual runs create development
artifacts only, including when the selected ref is a tag. They do not create
GitHub Releases or tags.

Development version format:

```text
0.0.0-dev.<github.run_number>.g<first 12 characters of commit SHA>
0.0.0-dev.31.g97e82bd12345
```

The number is the workflow's visible build number, such as **#31**, rather than
GitHub's larger unique run ID. This is a SemVer prerelease version. It appears
in CLI `--version`, GUI About
and development archive filenames. A rerun keeps the same version and replaces
that run's same-named artifacts; there is no attempt number in the version.
Every archive includes `BUILD-INFO.json` and a README recording the full commit,
source ref, workflow build number, unique run ID and Actions run URL, plus a
companion SHA-256 checksum file.

Download the packages from the run's **Artifacts** section. Packages and build
timing reports are retained for **one day**. This artifact lifetime does not
shorten the Cargo build-cache lifetime, which follows GitHub's cache policy.

Pushes to `main` run the same builds automatically to warm Linux and Windows
release caches, and also provide one-day development artifacts. Cache keys
include OS/architecture, the installed Rust compiler, Cargo manifests/lockfile,
Cargo configuration and source commit. Compatible restore prefixes allow tag
and branch builds to reuse default-branch caches and retain unaffected build
data across dependency updates. Branch caches remain branch-scoped; they do not
populate `main` or other sibling branches/tags.

### Tagged releases

Only pushing a `v`-prefixed SemVer tag, such as `v0.0.23`, publishes a GitHub
Release. Tagged releases keep the existing download filenames
`ft8rs-linux-x86_64.tar.gz` and `ft8rs-windows-x86_64.zip`. The temporary Actions
artifacts expire after one day; the files published to Releases remain there.
This guide describes packaging; it does not publish a release when running
tests or editing documentation.

Release packages copy `crates/ft8rs-core/ALLCALL7.TXT`, the current runtime
database. It is already byte-identical to the official JTDX **20251217** database
(99,741 callsigns), with SHA-256
`17140773605b0d2a7a5a31458c11384d9efc413bf6288b262b7dd1f391ec32ea`.
Source: [JTDX files](https://sourceforge.net/projects/jtdx/files/), archive
`ALLCALL7_20251217_rc160.zip` (SHA-256
`5bf0661bf59fe6e015d4562c31a14f643bdeaae0b3c0b5a6587feccf4888dd5d`).
Check upstream before a release and update the runtime copy when a newer database
is available; builds package the checked-in copy for reproducibility.
Keep `ALLCALL7.TXT.2022` unchanged for calibrated regressions:
`scripts/test-decoders.sh` places it beside private test executables without
replacing the runtime database. Packaging tests use distinct runtime/historical
fixtures and require archives to contain the runtime bytes only.

SemVer prerelease tags (for example `v0.0.25-rc.1`) are marked as GitHub
prereleases. They do not replace the latest stable release or update the homepage.

The packaging contract and version validation can be checked locally with:

```sh
python3 -B -m unittest discover -s scripts -p test_package_release.py -v
cargo test --profile fast -p ft8rs-build
```

### Static homepage (GitHub Pages)

The user-facing Chinese/English homepage lives in `site/`. It uses plain HTML/CSS
with no frontend dependencies or browser-side GitHub API requests. The only
JavaScript selects `/zh/` or `/en/` at the root entry using `navigator.languages`
(the browser preferences behind `Accept-Language`). Pages cannot read HTTP
request headers in a static page. The first supported language wins; other
languages fall back to English. Each localized page has an explicit language
switch and works without JavaScript. The root has a full English fallback.
`scripts/build_site.py` fills in the version, publication time and exact download
URLs from the GitHub REST release response.

`pages.yml` runs only when manually dispatched on `main`. It automatically reads
GitHub's latest published stable release, then deploys both languages using that
one version. It has no version input, push trigger or automatic release trigger.
The renderer rejects drafts, SemVer prereleases, missing or incomplete assets,
and download URLs for a different version/repository. Any build or deployment
failure leaves the previously deployed homepage unchanged.
`SHA256SUMS.txt` lists archive basenames, so users can verify downloads together
in one directory. No extra personal access token is needed for deployment.

One-time repository setup:

1. In **Settings → Pages → Build and deployment**, select **GitHub Actions**.
2. Set the custom domain to `ft8.rs`, configure its DNS and enable **Enforce HTTPS**.
   A `CNAME` file does not configure a domain for an Actions deployment.
3. Allow `main` in the `github-pages` environment's deployment policy. Page
   deployments run from `main`; release tags do not need deployment permission.

To deploy the first page or update it after a new stable release, open
**Actions → Publish homepage → Run workflow**, choose `main`, and run it. No
version needs to be entered. The equivalent CLI command is:

```sh
gh workflow run pages.yml --ref main
```

The page source is always checked out from `main`. The release response is read
once, and both languages use its exact versioned asset URLs. Visitors do not
query GitHub to resolve the latest release; publishing a new software version
does not change homepage links until this workflow is run again.

Local checks and preview using a real, published release:

```sh
python3 -B -m unittest discover -s scripts -p 'test_*release.py' -v
python3 -B -m unittest discover -s scripts -p test_build_site.py -v
node --test scripts/test_site_language.cjs
gh api repos/tallcode/ft8rs/releases/tags/v0.0.24 > /tmp/ft8rs-release.json
python3 -B scripts/build_site.py --release /tmp/ft8rs-release.json --tag v0.0.24
python3 -m http.server 8000 --directory dist/site
```

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

The native Hybrid paths and DX orchestration must use existing native capabilities without changing
native thresholds, candidate order, AP scheduling or mathematical calculations.
Keep mutable native sessions, hash books and audio residuals isolated. Source
alignment corrections belong to their native profile and need independent
reference evidence, separate from an orchestration feature.

The authorized [Hybrid experiment](../crates/ft8rs-core/src/decode/experiment/README.md) is separate
ft8rs DSP in a private module. It reuses unchanged protocol/FEC functions and is
available only through Hybrid (`--profile hybrid --hybrid-experiment`); do not add
a standalone profile or expose its decoder as a public API. It is opt-in through
CLI/config or the Hybrid-only GUI setting **Experimental decoder**. Runtime
changes rebuild the decode session at the next slot boundary. Keep baseline
sessions and original audio isolated. Supplementary rows carry their own provenance and cannot create
independent confirmation or update native AP/hash memory. Evaluate each declared
specialty with known truth; partial standalone coverage is acceptable, but added
correct rows never erase errors or lost baseline rows in the report.

The single-slot research campaigns live under `experiments/single-slot/campaigns/`.
Preserve raw inputs/outputs, failures, interrupted runs, frozen code/binaries,
configuration and hashes. Use a new immutable run directory for every replay.
The first campaign archive remains unchanged; the second copies completed
batches into the repository between timed processes. The third campaign freezes
current product source before development/confirmation and records every callback
with exact displayed field retention. Its raw text and source are versioned.
Large binary/audio material
is ignored by Git and must also be backed up as filesystem content.
New synthetic input WAVs are reproducible local resources: keep their generator,
seeds, configuration, hashes and compact results in source control, while leaving
the generated audio and frozen binaries ignored. Retain the local copies for
the research archive. Existing historical recordings and accepted native fixture
management remain unchanged.

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
