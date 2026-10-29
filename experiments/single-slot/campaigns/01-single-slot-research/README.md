# First single slot receiver campaign

The canonical complete first-campaign snapshot is `2026-10-29-single-slot-preserved.tar.gz` in this directory. It includes all 58 then-existing runs, failures and partial outputs, research sources, build snapshots/binaries, private CallDB, input recordings/annotations and original analyses. The manifest gives every member hash. The redundant shared-directory archive link was removed after verification; this archive's content is unchanged.

`freeze-v8.json`, `freeze-v9.json`, `freeze-v10.json` and `self-check-v10.tsv` preserve version and self-check records. Checkpoints record the interrupted historical batch. `analysis/` contains the first campaign's 52 synthetic and six file-run aggregate records and its TSV matrix. Original raw records also live in `runs/`, and frozen source snapshots live in `frozen/`; their text/source files are versioned. Generated binaries/audio remain in the complete local archive.

Earlier synthetic inputs represented by generators/seeds remain represented that way; missing historical terminal transcripts are not invented. This campaign is immutable historical evidence. Archived files may contain original path names from the time of measurement; current working tools live in `tools/`.

## Tools and development layout

`tools/` is this campaign's tool location and `sources/` contains its Rust
harness variants. Current module documentation is in
`crates/ft8rs-core/src/decode/experiment/` at the repository root. Frozen snapshots
retain the names and source layout used for the measurements. The new layout
is foundation cleanup, not a new sensitivity confirmation.
