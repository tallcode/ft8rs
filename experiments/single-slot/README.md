# Single slot receiver experiments

Both research campaigns preserve their original measurements, failures, frozen
code/binaries, configuration and data. The current development foundation and its
technical documentation live in [decode/experiment](../../crates/ft8rs-core/src/decode/experiment/README.md).
The [dated measurement report](../../crates/ft8rs-core/src/decode/experiment/EXPERIMENTS.md)
describes the frozen version before the module cleanup.

- [First campaign](campaigns/01-single-slot-research/README.md): original research,
  v8/v9/v10 source variants, tools, self-checks, complete archive and analyses.
- [Second campaign](campaigns/02-hybrid-specialist/README.md): Hybrid-only frozen
  prototype, known-truth confirmation, recording comparisons and completion audit.

Each campaign owns `tools/` and `sources/`. There are no Rust harnesses or Python
experiment runners mixed into this top-level directory. Historical `scripts/`
and `tool-history/` are snapshots, not the current tool location. The complete
research timeline is retained in the second campaign's `NOTES.md` and first
campaign archive.

The product exposes only the optional Hybrid branch, currently named
`--profile hybrid --hybrid-experiment`, with an `exp` presentation suffix.
Historical freezes preserve the earlier specialist names. Frozen replay tools
must use their recorded source/binary; their results are not new measurements
of the refactored working tree.

Run decoders serially, with a private CallDB, and choose fresh output names.
Never rebuild a measured executable while a timed run is active. Generated WAVs,
archives and frozen binaries stay locally preserved and ignored by Git;
generators, seeds, parameters, hashes and compact analyses form the versioned
research record. Existing historical recordings/reference fixtures retain their
existing management. No raw run or first-campaign archive was deleted by cleanup.
