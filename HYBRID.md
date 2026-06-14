# Hybrid Decode Notes

This document records only `--profile hybrid`: the result-union mode that runs
the WSJT-X-aligned decoder and the JTDX-oriented decoder together.

Decoder internals belong in their own documents:

- WSJT-X alignment: `WSJTX.md`
- JTDX profile: `JTDX.md`

## Scope

Hybrid is not a third decoder algorithm. It is orchestration:

```text
profile=hybrid -> lib_wsjtx worker + lib_jtdx worker -> result merge
```

It shares input samples, not decoder-private state.

## Core Rules

- Run WSJT-X and JTDX sessions independently.
- Share only immutable slot samples.
- Do not share hashcallbook, AP memory, odd/even memory, residual buffers, or
  duplicate state.
- Deduplicate decoded rows before CLI/UDP output.
- Preserve internal source attribution: `wsjtx`, `jtdx`, or `both`.
- Keep normal user output clean; do not print attribution by default.

This keeps hybrid explainable. A row should be traceable to one decoder or to
both, instead of coming from mixed state that no upstream decoder owns.

## Architecture

```text
input slot samples
        |
        v
hybrid runner
        |
        +-- WSJT-X session worker
        |
        +-- JTDX session worker
        |
        v
merge / dedupe
        |
        v
CLI / UDP outputs
```

Implementation boundary:

```text
src/decode/hybrid/
```

## Current Status

Implemented or scaffolded:

- `profile=hybrid` is selectable;
- one WSJT-X session and one JTDX session are owned by the hybrid runner;
- file/full-slot hybrid decode runs both workers in parallel per slot;
- monitor mode can select `profile=hybrid` at the full-slot boundary;
- WSJT-X progressive `nzhsym=41/47/50` events are forwarded immediately while
  JTDX runs;
- after JTDX finishes, only JTDX-unique rows are emitted;
- dedupe tracks normalized messages within the slot, treats resolved hash-brace
  display variants such as `<RK4FF>` and `RK4FF` as the same message, and keeps
  source attribution internally;
- JTDX no longer returns protected WSJT-X fallback rows, so source attribution
  stays meaningful.

Not complete:

- JTDX AP/deep behavior is still being aligned, so hybrid sensitivity is not
  final;
- hybrid baseline is informational until JTDX stabilizes;
- source attribution is for tests/debugging, not normal CLI output.

## Threading Model

For file mode:

```text
read WAV
split into slots
run WSJT-X and JTDX workers for each slot
merge results
output deduplicated rows
```

For monitor mode:

```text
capture audio
close slot on wall-clock boundary
start both workers
stream WSJT-X progressive rows immediately
emit JTDX-unique rows after JTDX finishes
emit slot summary after both workers finish
```

Use shared input ownership such as `Arc<[f32]>` to avoid duplicating large
buffers. Each worker still owns its own FFT, residual, AP, hash, and memory
state.

## Deduplication

Initial dedupe policy:

```text
same slot
same normalized message
frequency difference <= 5 Hz
DT difference <= 0.3 s
```

Normalization should handle:

- repeated spaces;
- surrounding whitespace;
- equivalent resolved hash-brace presentation, for example `<RK4FF>` and
  `RK4FF`;
- SNR/frequency/DT differences for the same decoded message.

Do not over-normalize: two real FT8 rows with the same text but clearly
different frequency/DT positions should not be collapsed unless they are the
same signal. The unresolved hash marker `<...>` must remain distinct.

## Output Policy

Normal CLI rows stay clean:

```text
HHMMSS snr dt freq message
```

UDP receives deduplicated decoded messages only. Debug/test tooling may expose:

```text
source=wsjtx
source=jtdx
source=both
```

## Current Comparison

Hybrid is currently compared against every row in the fixture CSV, ignoring the
`Extra` marker. This is different from the pure WSJT-X/JTDX profile tests, where
`Extra` selects the relevant profile baseline.

Current release-mode observation for the long fixture:

```text
230208_140300.wav, profile=hybrid:
  CSV rows: 458
  decoded rows: 465
  matched rows: 457
  missing rows: 1
  extra decoded rows: 8
```

Hybrid should not become a hard gate until:

- WSJT-X profile remains stable;
- JTDX profile has a measured baseline;
- JTDX AP/deep and false-positive filtering are stable enough to trust the
  union result.

## Future Research

Possible later work, disabled by default:

- controlled cross-decoder call hints;
- feeding high-confidence calls from one decoder into the other;
- shared hash/call hints after both pure profiles are stable;
- experimental profile files for non-baseline tuning.

Any such work must be explicit, testable, and documented as a new hybrid phase.

## Do Not Do

- Do not share decoder-private state.
- Do not use hybrid to hide JTDX shortcuts.
- Do not emit duplicate UDP reports for the same decoded row.
- Do not promote hybrid count as the primary sensitivity metric until the two
  pure profiles are understood.
