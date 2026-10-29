# Documentation

Start with the [project README](../README.md) for building, launching and choosing
a decoder profile. Commands in these documents run from the repository root
unless a section says otherwise.

## Use and maintain ft8rs

| Document | Contents |
|---|---|
| [Usage](USAGE.md) | GUI controls, soundcard/channel setup, CLI reference, UDP and troubleshooting |
| [Development](DEVELOPMENT.md) | Workspace, optimized builds, FFTW, packaging and local checks |
| [Validation](VALIDATION.md) | Reproducible recording baselines, CallDB/configuration, evidence limits and latest local run |
| [Decoder tests](../crates/ft8rs-core/tests/decoder/README.md) | Shared adapters, fixed corpus, test coverage and result comparison |
| [Reference-data updates](../crates/ft8rs-core/tests/decoder/UPDATING.md) | Native generators, candidate capture, review and adoption |
| [Project rules](../AGENTS.md) | Native-code boundary, message preservation, local tests and repository workflow |
| [v0.0.22 release notes](releases/v0.0.22.md) | Changes since v0.0.21, validation, packages and known limits |

## Decoder implementation

| Document | Contents |
|---|---|
| [WSJT-X](../crates/ft8rs-core/WSJTX.md) | Source mapping, sample/numerical conventions, stages, AP and alignment risks |
| [JTDX](../crates/ft8rs-core/JTDX.md) | Independent state, CallDB, partition settings, source differences and partial `chkgrid` scope |
| [Hybrid](../crates/ft8rs-core/HYBRID.md) | Three baseline sessions, payload-aware deduplication, bounded cancellation and scheduling |
| [Hybrid specialist](../crates/ft8rs-core/src/decode/experiment/README.md) | Opt-in private receive DSP, native reuse, conditional admission, bounded residual search and evidence limits |
| [DX](../crates/ft8rs-core/DX.md) | Target context, protected tasks, three-way observations and controlled recovery |
| [Shared cooperative architecture](../crates/ft8rs-core/COOPERATIVE_DECODER_DESIGN.md) | State ownership, observation/evidence boundaries and output budgets |

## Dated research and measurements

These reports preserve the conditions and evidence behind implementation
choices. Their test counts and timings describe the measured revision, database,
configuration and machine. Use the current guides above for active behavior.

- [2026-10-28 Hybrid experiments](reports/2026-10-28-hybrid.md): logical partition
  comparison, three-path implementation, SWL settings and hash-display identity.
- [2026-10-28 DX experiments](reports/2026-10-28-dx.md): prototype versus implemented
  target recovery, known-truth cases, callback latency and worker cost.
- [2026-10-29 Hybrid specialist experiments](../crates/ft8rs-core/src/decode/experiment/EXPERIMENTS.md):
  private opt-in receive DSP, per-specialty native comparisons, errors and
  baseline retention, callback/completion timing and both preserved campaigns.
- [Earlier acceleration research](../crates/ft8rs-core/ACCEL.md): integer/SIMD and
  workspace optimizations, rejected approaches and archived profiling results.

Historical `/tmp` artifact paths are local provenance records, not distributable
fixtures. Durable tests and native source/artifact hashes live in the repository;
new local run logs can be kept under the ignored `target/decoder-validation/`.

## Changes reflected in this reorganization — 2026-10-27/28

| Revision | Durable behavior documented here |
|---|---|
| `1c5c0ab`, `8c9346b` | Native scaling/AP/protocol alignment, common anti-alias input conversion, source audits and numerical lint rules |
| `a99b7c1` | Isolated cooperative sessions, evidence boundaries and preserved native output |
| `9721d51` | Three-path Hybrid, payload-aware hash-display matching and one-partition default |
| `b647fc5` | DX three-way observation, protected target tasks and controlled single-round recovery |
| `f4d002c`, `21561b2` | Correct Mono capture, active audio diagnostics, responsive/stable settings and SVG monitor controls |
| Reference-framework extension | Fixed native vectors/WAVs, strict 21-row WSJT-X short gate, test28/channel/noise coverage and candidate update scripts |

The reference-framework extension is included in release `v0.0.22` together
with these documentation changes. Its completed local validation is recorded
separately in [validation](VALIDATION.md).
