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

## Decoder implementation

| Document | Contents |
|---|---|
| [WSJT-X](../crates/ft8rs-core/WSJTX.md) | Source mapping, sample/numerical conventions, stages, AP and alignment risks |
| [JTDX](../crates/ft8rs-core/JTDX.md) | Independent state, CallDB, partition settings, source differences and partial `chkgrid` scope |
| [Hybrid](../crates/ft8rs-core/HYBRID.md) | Three baseline sessions, payload-aware deduplication, bounded cancellation and scheduling |
| [Hybrid experiment](../crates/ft8rs-core/src/decode/experiment/README.md) | Hybrid-only receive DSP, GUI/CLI opt-in, native waveform models, data admission and bounded residual search |
| [DX](../crates/ft8rs-core/DX.md) | Target context, protected tasks, three-way observations and controlled recovery |

Historical Hybrid partition measurements and the reviewed homepage recording
comparison are summarized in [Hybrid](../crates/ft8rs-core/HYBRID.md). The
known-truth recovery measurements are summarized in [DX](../crates/ft8rs-core/DX.md).
The specialist's development evidence remains in its
[experiment notes](../crates/ft8rs-core/src/decode/experiment/EXPERIMENTS.md).
