# Hybrid experiment product development — campaign 03

This campaign follows the Git checkpoint `e692a160e7d14fa1a27afa147be9b0aa7706c7d7`.
The two preceding campaigns remain immutable. Product design and the report live
in [decode/experiment](../../../../crates/ft8rs-core/src/decode/experiment/README.md).

- `sources/measure.rs`: isolated blind product-profile runner, truth generator,
  full precision callback rows, callback time and all-work time.
- `tools/prepare.py`: snapshot current core and protocol-only lab bridge, preserve
  source/build/database fingerprints and build with release fat LTO.
- `tools/audit.py`: verify frozen sources/binary/CallDB, raw rows, original baseline
  retention and every local generated waveform hash.
- `tools/analyse.py`: truthful aggregation and dated recording-annotation matching.
- `tools/run.py`: serial synthetic development/fresh confirmation or cold slot
  recording comparisons; refuses to overwrite output and checks every baseline
  row's message, f, DT, SNR, sync, tones and provenance separately.
- `frozen/`: source/build conditions, failure logs and successful snapshots.
- `runs/`: raw TSV, stderr/diagnostics, truth, process status, hashes and parsed
  results. Generated audio remains in each run's ignored `waveforms/` directory.
- `checks/`: local optimized tests and static checks.
- `analysis/`: derived reports and final preservation manifest.
- `annotations/`: supplemental user-reviewed labels bound to exact recording
  hashes, slots, messages, frequencies and DTs; original CSV files are unchanged.

Commands from the repository root (choose unused names):

```sh
python3 experiments/single-slot/campaigns/03-experiment-development/tools/prepare.py SNAPSHOT
python3 experiments/single-slot/campaigns/03-experiment-development/tools/run.py SNAPSHOT DEV --stage development
python3 experiments/single-slot/campaigns/03-experiment-development/tools/run.py SNAPSHOT FRESH --stage confirmation
python3 experiments/single-slot/campaigns/03-experiment-development/tools/run.py SNAPSHOT FILES --stage recordings
```

Do not build or run another CPU benchmark during timed measurements. The runner
uses the executable's private shipped CallDB and product stream defaults, with
`cooperative_realtime=false`; `exp` means Hybrid with its supplementary branch
enabled, not a standalone receiver. Native-only algorithms and Hybrid use their
original routines. All four methods see the identical generated float32 samples.
The generator's truth is used by grading only, never passed to a decoder.

The original release-harness build `development-v2` failed because the harness
mistook the profile API's callback count for a returned vector. Its source and
build log remain preserved; no measurements ran under that build. The corrected
harness is frozen under `development-v2-fixed-harness`.

Finite negative controls do not prove zero false output. Recording rows without
annotations are candidates. Counts at multiple SNRs and scenarios must not be
presented as a universal sensitivity shift. Synthetic input models share native
waveform synthesis; historical audio and other channel models have separate value.

For `file` jobs the raw harness `END` unmatched count is computed against an empty
truth list; it is **not** a false-decode count. Parsed `result.json` explicitly
sets `correct`/`wrong` to null. `tools/analyse.py` matches additions to the dated
CSV annotations using message, frequency and DT; unmatched additions remain
unlabelled candidates. File replay uses cold sessions and the harness's fixed
`261029_120000` decoder timestamp, consistently for both configurations, not a
warm live stream or a multi-slot AP-memory benchmark.

`tool-history/measurement-v1/` preserves the wrappers used during measurement.
The current tools correct file-summary aggregation to keep ungraded counts null;
the initial derived summaries are retained with an `initial-ungraded-aggregation`
suffix. No raw callback, truth, diagnostic, input or measured binary was changed.

On 2026-10-29 the user accepted the three current recording additions as annotations
after checking real callsigns and matching grids. `analysis/recordings.json` now
reports three `annotated` additions with origin `user-review`; its earlier
interpretation is retained in `analysis/recordings-before-user-review.json`.
The decoder never reads these labels; only offline analysis uses them.
