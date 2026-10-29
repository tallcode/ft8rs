# Second Hybrid specialist campaign

This campaign validates the new Hybrid supplementary DSP branch. The specialist has no standalone product profile.

- `frozen/` preserves the exact frozen sources, executable, Cargo inputs and private CallDB.
- `runs/` contains all three development and eleven confirmation/file-run directories, including raw rows, diagnostics, metadata and the 192 generated confirmation WAVs.
- `analysis/` contains development and interim summaries, complete synthetic and recording comparisons, check summaries, the completion audit, the second-campaign aggregate JSON/TSV, and the result-directory migration record.
- `freeze-hybrid-specialist-v1.json` identifies the frozen implementation and declared matrix.
- `logs/`, `checks/` when present, and `status.json` preserve checks and execution status.
- `sources/` holds the campaign Rust harness; `tools/` contains current tool locations. `scripts/` and `tool-history/` retain immutable historical tool snapshots.
- `inputs/` contains the requested architecture and local input material.

Timing runs were serial. Completed raw run evidence is never overwritten. The first campaign remains independently preserved next door. Generated audio, frozen binaries and archives remain on disk and are ignored by Git; reproducible generators, seeds, parameters, hashes and compact analyses provide the source-controlled research record.

## Tools and development layout

`tools/` is this campaign's tool location and `sources/` contains its Rust
harness variants. Current module documentation is in
`crates/ft8rs-core/src/decode/experiment/` at the repository root. Frozen snapshots
retain the names and source layout used for the measurements. The new layout
is foundation cleanup, not a new sensitivity confirmation.
