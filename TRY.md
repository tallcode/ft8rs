# TRY.md - FT8 Streaming Decode Attempt Log

本文只保留仍有复盘价值的尝试结论。长期技术状态、架构说明和 CLI 用法放在
`STREAM.md` / `README.md`。

## Current Baseline

- Default decoder build: `rustfft@3840`。
- WSJT-X parity build: `FFTW@3840` with `--features fftw`。
- `rustfft@4096` and runtime FFT switching have been removed。
- Short fixture `tests/ft8/210703_133430.wav`: currently `21` unique messages。
- Long fixture `tests/ft8/230208_140300.wav`: normalized `12 kHz` no-offset
  fixture, protected floor `424/449`。
- Tests must use release mode for sensitivity/performance work。
- Do not chase score by lowering `ncand/ndepth`, disabling AP, relaxing gates,
  or expanding non-WSJT-X search space。

## Milestone Summary

### Milestone 1: 361 -> 381

- Converted stream decoder from one-shot full decode to WSJT-X-style
  `nzhsym=41/47/50` progressive flow。
- Key fix: `sync8d` time indexing must not wrap. WSJT-X uses signed indices and
  contributes zero outside the buffer; old Rust used modulo wrap into the end
  of `cd0`。

### Milestone 2: 381 -> 401

- Aligned pass FFT lifetime: refresh long FFT per outer pass, not after every
  subtract inside the pass。
- Fixed outer `syncmin`: depth 1/2 uses `2.1`, depth 3 uses `1.3`。
- Fixed `sync8` Fortran 1-based time-bin access。
- AP `sync8d` frequency tweak uses `ctwk * Costas`; second time refine uses
  plain Costas sync。
- Fixed `pack_jt77::is_stdcall()` 0-based conversion, restoring weak CQ AP
  templates for calls such as `F1PPH`、`R6KEE`、`IW1PUR`。

### Milestone 3: 401 -> 422

- Fixed several 1-based/0-based and message gate issues:
  - `subtractft8` sample index。
  - `pack77_1` `R GRID` third-word check。
  - `split77/chkcall` standard callsign checks。
  - `unpack77` CQ invalid guards。
  - stream AP memory `chkcall` digit-position semantics。
- Long-file harness now decodes EOF tail slot instead of dropping it。
- Diff CSV output fixed to stable columns and more robust message matching。
- Recording-start diagnostic found stable `+0.785s` timing residual。

### Milestone 4: 422 -> 424

- The old `48 kHz` fixture's `+0.785s` start offset was folded into a new
  normalized `12 kHz` fixture, so tests no longer need an offset parameter。
- `gen_ft8wave` envelope and `subtractft8` refined-DT `sqf()` were aligned to
  WSJT-X, raising the protected long score to `424/449`。
- Many numeric-homology cleanups were made in FFT/downsample, `ft8b`,
  `ft8_a7d`, LDPC/OSD, `sync8` ordering, and `nuttal_window`。
- Type 3 false positive `CQ 001 IZ7MMG 549 2025` was fixed by validating the
  two RTTY callsign slots against WSJT-X `pack77_3/chkcall` structure, not by
  disabling `i3=3` or contest messages。

## Effective Changes Kept

### Stream / Windowing

- Progressive `nzhsym=41/47/50` state is shared inside a slot。
- Final `nzhsym=50` zero-pads after `50*3456` samples。
- `DecodeOptions.initial_messages` carries early decodes into full-stage
  duplicate/pass-control state without returning them twice。
- EOF non-empty tail slots are decoded。
- `nagain=true` full-stage behavior uses original full slot while searching
  `nfqso±20Hz`。

### Sync / Candidate Ordering

- `sync8` time bins follow Fortran 1-based semantics at array boundaries。
- `sync8` percentile normalization and final candidate ordering use a local
  WSJT-X `indexx` port。
- near-dupe boundary uses the source-shaped `tdiff < 0.04` behavior; exact one
  `NSTEP` separation is not merged by Rust roundoff。

### FFT / Downsample / Metrics

- Core FFT calls use WSJT-X-shaped `four2a_r2c` / `four2a_c2c` wrappers。
- `ft8_downsample` uses unnormalized inverse FFT and explicit
  `fac=1/sqrt(NFFT1*NFFT2)`。
- Downsample setup expressions, taper generation, bin `nint`, `xbase`, and key
  `ft8b`/AP metric arithmetic were narrowed to WSJT-X default `real` where it
  matters for weak-signal parity。
- `regular s8` is stored at unscaled `abs(csymb)` while `cs` keeps `csymb/1e3`。
- `ft8_a7d` `nsym=1` metric uses `abs(cs(...))` rather than an unscaled `s8`
  shortcut。

### LDPC / OSD

- `platanh` uses WSJT-X piecewise approximation and `±7.0` saturation。
- OSD reliability ordering uses WSJT-X-style `indexx` before reversing to MRB。
- CRC-good but post-gate invalid codewords continue later passes, matching
  WSJT-X `cycle` semantics inside `ft8b`。
- Message-type guard matches WSJT-X:
  `i3>5 || (i3==0 && n3>6)` plus explicit project rejection of out-of-scope
  WSPR-style Type 0.6。

### Pack / Unpack / Hash

- FT8 receive supports active WSJT-X message families needed by current tests:
  Type 0.1, 0.3/0.4, 0.5, 1/2, 3, 4, 5。
- WSPR-style Type 0.6 remains out of project scope。
- Receive unpack uses `mycall/hiscall` hash context。
- Type 3 RTTY keeps legal contest exchanges but rejects `CQ`/`QRZ`/`DE` special
  tokens in the two callsign slots。

### Performance

- LDPC generator matrix cached with `OnceLock`。
- OSD/codeword work buffers are reused where safe。
- Candidate workspace is reused per pass。
- Unused pass-loop FFT removed。
- Tone generation, hard sync, and SNR work avoid repeated computation where
  semantics are unchanged。

## Rejected or Deferred Attempts

- Runtime `rustfft@4096` comparison path: removed; WSJT-X uses `3840`。
- Candidate parallelism: deferred because it can change duplicate/subtract and
  residual ordering。
- Pass-level coarse downsample cache: slowed long test due to large buffer clone。
- Broad `sync8` f32 rewrite: reduced protected long score and was rejected。
- Restricting all `i3=3` RTTY messages to `ncontest=4`: rejected as stricter
  than WSJT-X `ft8b.f90` post-decode gate。
- Duplicate-gated regular subtract: rejected for now; WSJT-X effective regular
  path subtracts inside `ft8b` before outer duplicate filtering。
- Forcing local `ibest` offsets for remaining misses: useful as diagnosis but
  not a committed heuristic。

## Remaining Miss Notes

### `230208_140430 F4JAR UX7UU -19`

- User confirmed this is a JTDX decode, so it is no longer a WSJT-X priority。
- Candidate reaches `ft8b` with strong hard sync, but selected soft-symbol
  alignment gives hard errors above the `<=36` acceptance line。
- Nearby time-sweep points can recover it, so the diagnostic remains useful for
  second time-refine / soft-symbol boundary comparison。

### `230208_140415 FO0L F4GYE JN07`

- Window-sensitive lost decode。
- Decoded in some old offset windows and not in the aligned formal window even
  with relaxed local search probes。
- More likely window/neighbor phase interaction than pack/unpack or AP memory。

### `230208_140445 VE7ON S56KFG JN76`

- Near-dupe boundary case around one `NSTEP` (`0.04s`)。
- Fixing the strict boundary behavior helped this class of issue。

## Current Next Steps

1. Continue WSJT-X source comparison before changing parameters。
2. Cluster remaining diff by slot, frequency, drift, message family, and tag。
3. Keep recording-start offset diagnostic while comparing file windowing,
   padding, continuous-buffer behavior, and AP memory。
4. Add focused fixtures only when they are cheap and protect a known boundary:
   AP masks, candidate ordering, EOF tail slot, hash display, Type 3 structure。

## Recent Validation

- `cargo test --release test_stream_decode_short_audio -- --nocapture`
  - `21` unique messages。
- `FT8RS_WRITE_DIFF=1 cargo test --release test_stream_decode_long_audio -- --nocapture`
  - `424/449`。
  - timing residual median near `+0.000s`。
  - every slot under `15s`。
- `cargo test --release rejects_type_3_rtty_cq_token_in_callsign_slot -- --nocapture`
  - Type 3 `CQ_001` false-positive structure rejected。
- `cargo test --release non_contest_quirk_gate_matches_wsjtx -- --nocapture`
  - non-contest post-gate matches current WSJT-X quirk handling。
