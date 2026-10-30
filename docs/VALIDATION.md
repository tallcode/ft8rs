# FT8 validation baseline

These are reproducible test operating points, not universal sensitivity or
latency claims. Commands run from the repository root; decode tests use `fast`
or `release` and run locally. See [development checks](DEVELOPMENT.md) and the
[shared decoder framework](../crates/ft8rs-core/tests/decoder/README.md).

## Historical recordings

Recordings and CSVs remain in `crates/ft8rs-core/tests/ft8/`. Short tests use
100–3000 Hz; long tests use 200–3000 Hz and all 19 slots through a persistent
session. Native baseline tests have SWL/AGC off and no user QSO context.

| Profile | Short recording `210703_133430` | Long recording `230208_140300` |
|---|---:|---:|
| WSJT-X (`W`) | 21/21 target rows, every target required | 434/434 target rows |
| JTDX (`J`) | 20/20 target rows | 453/453 target rows, fixed one partition |
| Hybrid (`H`) | 21/21 target rows | 476/476 target rows |
| Hybrid + experiment (`He`) | 21/21 target rows | 479/479 target rows |

The decoder runner uses `ALLCALL7.TXT.2022`, SHA-256:

```text
e647033c0ed4ef2d7c3c56839cfbd819f224805bcd7e0689ac656a0e8b22b298
```

Production defaults to one JTDX partition; the current recording CSV pins this
setting. The four-partition long-recording gate also checks the current JTDX
target rows. Explicit zero is machine-dependent automatic selection. The
shipped runtime CallDB is a different snapshot and can change results. Do not
replace either database to make a test pass.

CSV `Extra` lists the decoders that produced each reference row in the fixed
replay, separated by `|`:

| Marker | Interpretation |
|---|---|
| `W` | WSJT-X |
| `J` | JTDX |
| `H` | Hybrid |
| `He` | Hybrid with experimental decoding enabled |
| blank | Reviewed reference message, absent from this fixed replay |

The current fixed replay uses `ALLCALL7.TXT.2022`, 100–3000 Hz for the short
recording, 200–3000 Hz for the long recording, and one JTDX partition. Reviewed
additions are included; the two identified wrong decodes are not reference rows. DX evaluates only
target-related messages under its configured target/context. The degraded
retention corpus keeps its exact required rows in `retention.tsv`; its diagnostic
classification uses the current CSV. Raw output counts can include unlabelled
rows and must not be confused with target coverage.

The known pure JTDX long miss remains:

```text
230208_140700,-16,1.7,1153,F1MLZ UA3QNA -04,
```

It cannot be offset by a different new row. The WSJT-X short test's former floor
of 19 is retired; all 21 targets are now required.

## Native corpus and retention requirements

| Layer | Versioned coverage | What it establishes |
|---|---|---|
| FT8 message coding | 49 message vectors per original engine; hash vectors | Native pack/unpack, CRC, codeword and tone ordering |
| Original `test28` | 35 inputs + 14 integer boundaries per engine | Raw pack/unpack text, integer values, validity and explicit hash state |
| Native waveform scenarios | 13 per original engine, plus clean carriers | Known-content audio, offsets, weak/negative/context and subtraction regressions |
| Native Watterson | 8 cases per engine, 16 WAVs total | Source-specific multipath/fading output with fixed seeds |
| Degraded recordings | 6 fixed WAVs × 4 audio adapters | Individually retained existing CSV-target observations |

The accepted degradation baseline contains 24 cases and 1458 required message
rows across adapter/recording combinations; these are not 1458 distinct on-air
messages. Only existing profile-appropriate CSV targets become requirements.
Unlabelled and other-reference observations remain separate in capture reports.
These requirements are explicitly **Rust regression baselines**, not complete
native-decoder outputs or independent ground truth.

Native `degrade_snr` adds noise relative to total mixed-audio power. A nominal
+1 dB setting is not a measured 1 dB SNR loss for each station. Watterson models
and native C random/FFTW output differ by source/platform; their toolchain,
source/artifact hashes and seeds are recorded. Daily tests read stored WAVs.

## Latest completed local run — 2026-10-28

This run covers the source-alignment/three-path/audio/GUI commits through
`21561b2`, plus the native-reference and regression-framework extension included
in release `v0.0.22`. It used the default RustFFT backend on this macOS ARM host.
The totals below are a dated result, not fixed expected test counts.

| Check | Result |
|---|---|
| Complete core release suite, including every ignored gate | 205 passed, zero failures |
| Workspace fast library/binary tests | 217 passed, zero failures |
| Native Watterson replay across four adapters | 64/64 known-message recoveries, no wrong-content output |
| Required/diagnostic Watterson split | 48 mandatory-recovery trials, 16 disturbed-channel diagnostic trials |
| Degraded-recording retention | All 1458 required rows retained in 24 cases |
| Eight original-recording control reports | Byte-identical to the pre-extension controls |
| Existing base protocol/audio/waveform/provenance files | Byte-identical after extension adoption |
| Formatting and all-target Clippy with `-D warnings` | Passed; workspace checked again before v0.0.22 |
| Python/Shell syntax, fixture hashes and diff checks | Passed |

Workspace fast tests were rerun after syncing remote main for `v0.0.22`.
Core and workspace totals overlap; do not add them as unique test coverage.
The separate engine long actor gate passed in the preceding local validation;
the extension rerun above did not rerun that ignored engine gate. Runtime FFTW
feature tests and Windows DAX hardware comparison are not established by this
RustFFT run.

Local evidence is saved under the ignored directory
`target/decoder-validation/2026-10-28-extended/`: `summary.txt`, `validation.json`,
`core-release.log`, `workspace-fast.log`, `control-comparison.json`,
`release-controls/` and `calibration/{curves.tsv,observations.tsv}`. These logs are
not required in a fresh checkout. Regenerate local evidence with the documented
runners; the accepted fixtures/provenance remain versioned in the test tree.

## Dependency/toolchain refresh — 2026-10-28

The subsequent local refresh updated the default stable toolchain from Rust
1.98.1 to 1.99.1 and rustup from 1.29.1 to 1.29.2. All 13 direct registry
dependencies were checked against crates.io's highest non-yanked stable version;
eight changed: `num-bigint`, `cpal`, `clap`, `eframe`, `egui_extras`, `muda`,
`winresource` and `ico`. The remaining five were already current.

The final online `cargo update --dry-run --verbose` resolved zero further
updates. Older transitive `toml 0.8.2`, `toml_datetime 0.6.3` and
`toml_edit 0.20.2` remain constrained by upstream exact requirements, including
`proc-macro-crate 2.0.2`'s TOML pins. This does not claim every transitive package
can move to its latest incompatible branch.

Validation used optimized `fast` builds, the default RustFFT backend and this
macOS ARM host. Experimental processes ran serially; core recording checks used
the private 2022 CallDB, without replacing the runtime database.

| Check | Result |
|---|---|
| Workspace library/binary tests | 220 passed, zero failures |
| Complete core suite, including every ignored gate | 205 passed, zero failures |
| Separate engine long Hybrid delivery gate | Passed |
| Workspace documentation tests | Passed; no executable doctest cases |
| Two historical recordings across all four adapters | All eight before/after snapshots byte-identical |
| Degraded-recording retention | All 1458 required rows retained in 24 cases |
| Native Watterson replay | 64/64 known-message recoveries, no wrong-content output |
| Formatting and all-target Clippy with `-D warnings` | Passed |

Workspace and core totals overlap. The snapshot comparison retains the existing
pure JTDX long-recording miss; the refresh does not claim new sensitivity gains.
Windows hardware capture/window activation and the optional runtime FFTW backend
were not exercised by this run.

Local evidence is under the ignored directory
`target/dependency-upgrade-2026-10-28/`: dependency/version audits,
`workspace-fast.log`, `core-fast-all.log`, `engine-long.log`, `doctests.log`,
`before/`, `after/`, `compare.log` and `validation.json`.

## Limits and follow-up validation

The JTDX geography filter remains a deliberate partial port; implausible
callsign/grid pairs can survive. See the [recorded decision](../crates/ft8rs-core/JTDX.md#deferred-full-chkgrid-port--review-decision-2026-10-27).
Fixed seeds and a small recording set do not establish a universal dB gain or
absolute false-positive rate. SWL and different partition counts can change
candidate sets. Full AP-pattern, drift and collision coverage can still grow.

The listening display target is usually the next slot +2/+3 seconds. Callback
arrival, worker tail and next-slot queueing are separate measurements; running
native calls cannot be forcibly cancelled. Offline staged replay excludes actual
soundcard/GUI latency. Long live monitoring, dense-band stress and Windows DAX
hardware parity remain separate measurements.

Historical Hybrid/DX costs and known-truth experiments are retained in
[dated reports](README.md#dated-research-and-measurements), with their original
revisions, CallDBs and timing definitions. Do not mix those counts with the 2022
regression operating point above.
