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
| WSJT-X (`W`) | 21/21 target rows, every target required | 429/429 target rows |
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

## Runtime and input fixes — 2026-11-01

Local verification used macOS ARM, Rust/Cargo 1.97.1, default RustFFT and the
optimized `fast` profile, based on `1576198` plus the runtime/input fixes.
Workspace library/binary tests passed 265 cases (four routine ignored gates).
The default decoder runner passed 175 core unit tests and 27 integration tests;
those 175 unit tests overlap the workspace total. Its ten ignored integration
gates were not run. Separately, the production-worker long-recording gate passed,
retaining all 478 reference rows with exact decode fields and provenance.

Before/after CLI controls used identical recordings, configuration and a private
copy of the 2022 CallDB above. Complete displayed output was byte-identical for
WSJT-X, JTDX and Hybrid on both recordings: respectively 21/434, 20/455 and
21/478 observed rows. These are preserved output counts, including unlabelled
rows, not new known-truth gains or replacement target thresholds.
All-target Clippy with `-D warnings`, formatting, Python (11 cases), Node (5 cases)
and diff checks passed. WSJT-X delivery batching and native decoder sources were
unchanged. This run does not establish FFTW results or Windows/DAX hardware parity.

## GUI input and documentation hygiene — 2026-11-03

The GUI batches message insertion while preserving the sequential history limit,
slot ordering, local provenance and hash enrichment. One comparison worker owns
socket binding, receiving and closure; channel disconnection controls its
lifetime. UI configuration changes neither join that worker nor wait for a
receive timeout. Stale bind results and messages queued under an old endpoint
are isolated. Decoder algorithms, search settings and the process watchdog are
unchanged.

On macOS ARM, the optimized `fast` workspace library/binary tests passed 286
cases with zero failures; four existing manual long gates remained ignored.
This includes 57 GUI tests covering full row/display-index equivalence across
different batch boundaries and capacity limits, late input, hash enrichment,
loopback delivery/rebinding, stale failures, mailbox replacement and shutdown
while the socket owner is blocked. Tests ran serially beside a private 2022
CallDB copy; the runtime database was not replaced.

Workspace all-target type checking, all-target Clippy with `-D warnings`, format
and diff checks passed. Python site/packaging checks passed 11 cases with warnings
treated as errors; the Node language-routing checks passed five cases. Both
localized pages were generated using test release metadata for local validation,
without deploying or changing the published download version. Documentation now
states the existing 0.3-second DT matching requirement and the asynchronous
comparison settings lifecycle. No Windows interactive/GPU validation was run.

Strict API documentation checking exposed a CLI/core documentation filename
collision, an unescaped CPAL selector and four broken links in three mathematical
comments inside the JTDX parity module. All are fixed, and workspace API
documentation builds with `RUSTDOCFLAGS="-D warnings"`. The only native-mirror
change is Markdown code formatting on those three comment lines; executable
code and parity data are unchanged. No lint suppression is added.

Local logs and generated site fixtures are under `target/hygiene-checks/`;
earlier data-path timing evidence is in the ignored
`experiments/performance/2026-11-03-ui-implementation/` archive. Those timing
probes exclude native-window and GPU latency and are not i5-9600T measurements.

## WSJT-X upstream alignment and cleanup — 2026-11-04

The active classic-receiver pin is `v3.3.0-beta5`,
`d89fe30ca1dfd3c38def8f17228f09f042829f95`. Official GitHub release refs were
queried again: release/3.3 and beta5 still point to this commit. The independent
source checkout is clean; sibling ../wsjtx remains unchanged at 967c85a6.
The separate ft8var engine was not ported.

Current independent native coverage includes 68 source messages, 56 receive
payloads, 2,466 A8 and 1,236 A7 candidates. The source audit now covers 36 files,
including the current native waveform generation used for RR73. The original
JTDX and Hybrid production sources match HEAD. DX production orchestration
also matches HEAD; only its tests adapt frozen current input, exact identities
and malformed-text fixtures to the current encoder. Cleanup adds no decoder
algorithm or performance change.

All real WAVs are byte-identical. Five exact long-recording CSV observations
now omit W: old RR73 at slots 3,8,13,13 and CQ OH8CMS KP24 at slot11. The CQ
has a separately diagnosed current-codec sentinel effect, not an RR73 grid
cause. Other fields and J/H/He markers are unchanged. The current raw W target
requirement is 429 rather than 434. This label edit does not filter outputs.
The frozen 1,458-row degraded-retention corpus and its exact ten-observation
upstream policy remain unchanged; 1,450 non-exempt calibrated requirements and
all other supplemental requirements remain mandatory.

Active synthetic RR73 sensitivity inputs all use 32403. DX uses eleven native
frozen payload/tone rows. The general audio test substitutes the current native
RR73 AWGN waveform for both historical RR73 source entries, runs it once over
every adapter, and verifies no wrong content. Historical base protocol vectors
and audio provenance remain as compatibility data. Temporary legacy-only DX
fixtures, diagnostic print/export hooks and duplicate reports were removed.

| Current-native DX scenario | Exact retained target slots |
|---|---|
| Seed 2026102801, complete pair, pumping off/on | 0,2,4,6,8 in each case |
| Seed 2026102809, complete pair, pumping off/on | 0,2,4,6 in each case |
| Seed 2026102809, no operator | 0,2,4 |
| Seed 2026102879, target absent, known grid | None |

Under the requested old-decoder/old-input versus new-decoder/new-input comparison,
these cases retain 22 versus 21 message observations: one lost RR73 and zero
additions. Its direct failure is ordinary BP/OSD CRC failure in the fixed noise,
not an active old RR73 template. Old A8 waveform compatibility, real RR73 and
CQ causes, full FEC evidence and the development stages are consolidated into
[the single wsjtx+ plan/report](../crates/ft8rs-core/src/decode/lib_wsjtx_plus/README.md).
At that alignment checkpoint the plus directory contained documentation only.
CPU SIMD belongs to stage 2; MKL/GPU remain the final stage. Subsequent plus
development is recorded separately below.

Before cleanup, the full serial release suite passed 232 tests, including all
12 normally ignored gates; the other workspace passed 106 tests with two existing engine
long gates ignored. All 1,934 pre-migration raw observations were retained;
W/H/He each added the same unconfirmed Type3 CQ, with three tiny SNR deltas each
(max 0.000219345 dB), no frequency/DT changes and seven byte-identical snapshots.
Unconfirmed additions were not counted as true gain.

Cleanup does not change original receiver code, real audio, configuration,
CallDB, degraded requirements or the precise allowance logic. The earlier full
long-noise matrix remains applicable; only its CSV-based diagnostic classification
labels change. Final targeted release checks rerun the core units, both DX
synthetic gates, all active native audio scenarios, real-recording snapshots,
the CSV-sensitive WSJT-X long gate and source audits. Original-audio before/after
snapshots are compared byte-for-byte; the intentional CSV hash change is recorded
separately from unchanged audio and decoder configuration.

Final targeted release checks: 198 passed, zero failures. All ten raw snapshots
are byte-identical before/after cleanup, retaining all 1,937 current observations;
the only input-condition hash change is the intentional CSV annotation edit.
The other workspace passed 106 optimized tests (two existing engine long gates
remained ignored). All-target workspace Clippy with `-D warnings`, formatting,
fixture/source-pin verification, maintenance-script syntax and local links pass.
The pre-commit check after synchronizing remote main passed 292 optimized
workspace library/binary tests; four existing manual gates remained ignored.
Evidence and current logs are retained
under `target/decoder-validation/2026-11-04-cleanup/`. The compact
`compatibility-evidence.json` preserves the diagnostic summaries, original source
identities, native truth, offline FEC inputs/results and prior validation history.
Temporary instrumented crates, probes, raw buffers, executables, WAV exports and
tool downloads are removed; the formal native reference source checkout remains.

## 2026-11-04: first wsjtx+ implementation stage

The independent, test-only plus receiver now has a paired original/reference/
optimized replay. The production WSJT-X/JTDX, Hybrid/DX, shared math and real
fixtures remain unchanged. The single [plus plan/report](../crates/ft8rs-core/src/decode/lib_wsjtx_plus/README.md)
contains the implementation, CPU measurements and compatibility backlog; it
remains the authoritative detailed report.

Three initial release rounds cover 26 cases / 80 slots / 941 observations per
round with bit-identical messages, metadata, tones and callback order. Median
corpus work drops from 86.593 to 53.760 seconds. A final build after adding the
optional FFTW R2C delegation repeats all three modes: 86.255 versus 54.799
seconds, again retaining all 941 observations, including cross-build comparison.
The two binaries' timings are reported separately. Both are serial staged
compute measurements; callback boundary offsets are an offline queueing model.
They do not establish live GUI latency, x86 performance or a sensitivity gain.

The final complete local release suite passes 274 tests, including all 13 manual
gates. The other workspace passes 106 optimized unit tests with two existing
engine gates ignored. Default all-target workspace Clippy passes with warnings
denied. Optional FFTW all-target type checking passes; strict feature Clippy
reports the unchanged shared FFTW file's pre-existing documentation lint.
Allowing only that lint on the command line leaves all other warnings denied
and passes. No original/shared file is changed to fix that baseline lint, and
FFTW runtime/performance is not claimed.

Formal evidence: `target/decoder-validation/wsjtx-plus-phase1-release-final/`
and `target/decoder-validation/wsjtx-plus-phase1-final-recheck/`. Temporary probes
are removed after retaining compact exploration summaries. Source and fixture
protection plus final check identities are recorded in `final-audit.json`.

## Limits and follow-up validation

The JTDX geography filter remains a deliberate partial port; implausible
callsign/grid pairs can survive. See the [recorded decision](../crates/ft8rs-core/JTDX.md#deferred-full-chkgrid-port--review-decision-2026-10-27).
Fixed seeds and a small recording set do not establish a universal dB gain or
absolute false-positive rate. SWL and different partition counts can change
candidate sets. Full AP-pattern, drift and collision coverage can still grow.

The listening display target is usually the next slot +2/+3 seconds. Callback
arrival, worker tail and next-slot queueing are separate measurements. Native
calls do not support cooperative thread cancellation. The GUI can enforce
its deadline by terminating the supervised child process; the standalone CLI
waits for running native calls. Offline staged replay excludes actual
soundcard/GUI latency. Long live monitoring, dense-band stress and Windows DAX
hardware parity remain separate measurements.

Historical Hybrid/DX costs and known-truth experiments are retained in
[decoder implementation and research notes](README.md#decoder-implementation),
with their original revisions, CallDBs and timing definitions. Do not mix those counts with the 2022
regression operating point above.
