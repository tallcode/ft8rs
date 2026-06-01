# Hybrid Decode Notes

This document records only the hybrid decode profile. It should not contain
JTDX implementation details except where needed to define how hybrid launches
and merges independent decoder results.

## Scope

The `hybrid` profile is the high-sensitivity union mode:

```text
profile=hybrid -> WSJT-X decoder + JTDX decoder in parallel
```

Hybrid does not define a third decoder algorithm. It runs two independent
decoders and merges their decoded messages.

## Document Boundary

This file is the technical record for `profile=hybrid` only.

- Do not use this file to record JTDX decoder internals.
- Do not use this file to record WSJT-X decoder maintenance notes.
- Record only the launch, concurrency, state-sharing, merge, dedupe, source
  attribution, and output behavior of the hybrid runner.
- If a hybrid rule depends on a decoder detail, link the dependency conceptually
  and keep the decoder-specific explanation in that decoder's own document.
- Record all hybrid-specific risks, baseline rules, merge heuristics, and
  future cross-decoder research here. Do not put hybrid design notes into
  `WSJTX.md` or `JTDX.md`.
- Keep this document focused on orchestration. JTDX profile internals belong in
  `JTDX.md`; WSJT-X alignment internals belong in `WSJTX.md`.
- If a future issue involves both profiles, record only the state-sharing,
  dedupe, concurrency, and output consequences here.
- Do not copy JTDX parameter tables, AP mask details, false-positive filters, or
  source-audit notes into this file. Link the concept and keep the technical
  source record in `JTDX.md`.
- Do not copy WSJT-X alignment decisions into this file. Hybrid consumes the
  WSJT-X decoder as an independent worker.

## Core Rules

- Run `lib_wsjtx` and `lib_jtdx` in parallel.
- Share input samples only.
- Do not share decoder-private state in the first implementation.
- Do not share hashcallbook in the first implementation.
- Do not share AP memory in the first implementation.
- Do not share odd/even slot memory in the first implementation.
- Merge and deduplicate only decoded results.
- Preserve internal source attribution: `wsjtx`, `jtdx`, or `both`.
- Send only deduplicated results to CLI and UDP outputs.

This preserves explainability. A result can be traced back to one decoder or
both, instead of coming from a mixed state that is neither WSJT-X nor JTDX.

## Architecture

Target shape:

```text
input slot samples
        |
        v
hybrid runner
        |
        +-- lib_wsjtx decoder worker
        |
        +-- lib_jtdx decoder worker
        |
        v
result merger / deduper
        |
        v
CLI / UDP outputs
```

`hybrid` should live outside both decoder implementations:

```text
src/decode/hybrid/
```

## Current Implementation Status

Implemented or scaffolded:

- `profile=hybrid` is a selectable profile;
- the hybrid runner owns one WSJT-X session and one JTDX session;
- merge and dedupe scaffolding exists with internal source attribution;
- output remains normal decoded messages, not profile/debug metadata;
- hybrid can call the initial JTDX regular BP path when `profile=hybrid` is
  selected;
- the JTDX worker is native-only. It no longer returns protected WSJT-X
  fallback rows, so hybrid source attribution cannot silently collapse into
  WSJT-X-equivalent output;
- file/full-slot hybrid decode runs the WSJT-X and JTDX session workers in
  parallel for each slot;
- WSJT-X progressive `nzhsym=41/47/50` callbacks are forwarded immediately
  while the JTDX worker runs in parallel;
- after JTDX finishes, hybrid emits only JTDX-unique rows that were not already
  streamed by the WSJT-X side;
- dedupe tracks all rows for a normalized message within the slot, not just the
  most recent row, so repeated identical message text at different frequency/DT
  positions does not hide a true same-signal duplicate;
- monitor mode can use `profile=hybrid` at the full-slot boundary.

State boundary:

- the WSJT-X worker owns its WSJT-X session state;
- the JTDX worker owns its JTDX session state, including JTDX hash/AP memory;
- hybrid shares only the input slot samples and receives decoded result rows;
- hybrid does not inspect, merge, or mutate decoder-private AP, residual,
  hashcallbook, odd/even, or duplicate-suppression state.

This is intentional for the first hybrid phase. It keeps hybrid explainable as
a result union instead of creating a third mixed decoder state.

Not complete yet:

- complete JTDX AP/deep results flowing into the merger;
- stable hybrid baseline measurement.

Hybrid remains incomplete until JTDX regular, AP/deep, and false-positive
control are stable enough to measure. Do not promote hybrid to a hard baseline
gate yet.

## Threading Model

Hybrid should run both decoders concurrently per slot.

For file mode:

```text
read WAV
split into slots
send each slot to WSJT-X and JTDX workers
merge results
output deduplicated messages
```

For monitor mode:

```text
capture audio
close a slot on wall-clock boundary
start JTDX worker
run WSJT-X worker and stream its progressive events immediately
emit JTDX-unique events after the JTDX worker finishes
emit slot-complete summary after both workers finish
```

Use shared input ownership, such as `Arc<[f32]>`, to avoid duplicating large
slot buffers. Each decoder still owns its own workspace, residual data, FFT
state, memory, and AP/hash state.

## Result Deduplication

The deduper should work before CLI/UDP output so duplicated messages are not
printed or reported twice.

Initial dedupe policy:

- primary key: normalized message text within the same slot
- secondary guard: frequency and DT proximity
- source attribution: `wsjtx`, `jtdx`, or `both`

Suggested tolerances for initial investigation:

```text
same slot
same normalized message
frequency difference <= 5 Hz
DT difference <= 0.3 s
```

These tolerances should be treated as implementation defaults to validate, not
as final rules.

## Message Normalization

Deduplication should be more robust than exact row comparison. At minimum,
normalization should handle:

- repeated spaces
- surrounding whitespace
- equivalent bracket/hash presentation where safely identifiable
- same message with slightly different SNR/frequency/DT

Do not over-normalize in a way that merges genuinely different FT8 messages.

## Output Policy

Normal CLI output should remain clean:

```text
HHMMSS snr dt freq message
```

Hybrid source attribution should be available for tests/debugging, but not
shown by default.

Debug/test output may show:

```text
source=wsjtx
source=jtdx
source=both
```

UDP output must receive deduplicated messages only.

## Baseline Rule

For `profile=hybrid`, the CSV baseline should use:

```text
Extra is empty, W, or J
```

This reflects the expected union of:

- main verified baseline
- WSJT-X extra decodes
- JTDX extra decodes

Initial hybrid tests are informational until `lib_jtdx` exists. Once both
decoders are active, hybrid should be measured against the union baseline.

## Hash And AP Sharing

Do not share hashcallbook or AP memory in phase 1.

Reason:

- WSJT-X and JTDX hash/AP behavior may not be identical.
- Shared internal state can create a third mixed decoder.
- Mixed state makes missed-message diagnosis hard.

Phase 1 rule:

```text
independent decoder state
result-level merge only
```

Current implementation follows this boundary: the WSJT-X session and JTDX
session each own their own hash/call memory, and hybrid only receives decoded
message rows for result-level dedupe.

Future research may add controlled cross-decoder hints, but that must be:

- explicit
- testable
- disabled by default
- documented separately

## Implementation Order

1. Add `--profile hybrid`.
2. Add a hybrid runner skeleton.
3. Add result merger/deduper.
4. Run both decoder instances once JTDX can emit real decode candidates.
5. Keep hybrid marked incomplete until the JTDX side is stable enough to
   measure.
6. Preserve source attribution internally.
7. Enable hybrid informational baseline.
8. Later promote hybrid baseline to a hard gate after behavior stabilizes.

## Do Not Do Yet

- Do not share decoder-private state.
- Do not share hashcallbook.
- Do not share AP memory.
- Do not use hybrid as a place to hide JTDX implementation shortcuts.
- Do not emit duplicate UDP reports for the same decoded message.

## Future Research

- Controlled cross-decoder hints.
- Passing high-confidence decoded calls from one decoder to the other.
- Shared call hinting after both pure profiles are stable.
- A `--profile-file` experimental mode for non-baseline tuning.
