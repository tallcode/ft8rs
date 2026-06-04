# JTDX Profile Notes

This document records only the JTDX-oriented FT8 decoder profile. It is the
technical checklist for `--profile jtdx`; WSJT-X alignment belongs in
`WSJTX.md`, and result-union orchestration belongs in `HYBRID.md`.

## Scope

`profile=jtdx` is a separate decoder path based on the JTDX FT8 source tree.
It is not a parameter overlay on the WSJT-X-aligned decoder.

Primary implementation boundary:

```text
src/decode/lib_jtdx/
```

The protected WSJT-X path must remain untouched while this profile is being
aligned. JTDX code may reuse a utility only when the behavior is completely
identical; otherwise the JTDX-specific logic stays under `lib_jtdx`.

## Goals

- Mirror the JTDX FT8 source dependency closure file by file where practical.
- Keep large source mirrors split into directory modules when a direct file
  would exceed the 1000-line maintenance limit.
- Optimize for high-sensitivity normal FT8 first.
- Keep SWL and other optional JTDX-only modes off by default until the normal
  FT8 path is stable.
- Allow runtime selection through `--profile jtdx`.

## High-Sensitivity Defaults

ft8rs intentionally uses high-sensitivity JTDX-style defaults, not necessarily
JTDX GUI defaults:

```text
nft8cycles = 3
nft8swlcycles = 3
lft8lowth = true
lft8subpass = true
ncandthin = 100
ndtcenter = 0
filter = false
hide_dupes = false
hide_hash = false
swl = false
hound = false
lenabledxcsearch = true
lwidedxcsearch = true
```

Frequency-dependent `napwid` follows the JTDX source behavior:

```text
freq < 30 MHz  -> napwid = 5
freq < 100 MHz -> napwid = 15
else           -> napwid = 50
```

`--hound` explicitly selects the JTDX Hound AP table.

## Source Mapping

The active mirror includes these major JTDX source areas:

```text
ft8_decode.f90       -> ft8_decode.rs / mod.rs orchestration
ft8b.f90             -> ft8b/
ft8s.f90             -> ft8s.rs
ft8sd.f90            -> ft8sd.rs
ft8sd1.f90           -> ft8sd1.rs
ft8apset.f90         -> ft8apset.rs
sync8.f90            -> sync8.rs
sync8d.f90           -> sync8d.rs
syncdist.f90         -> syncdist.rs
ft8_downsample.f90   -> ft8_downsample.rs
ft8_mod1.f90         -> ft8_mod1.rs
ft8_params.f90       -> ft8_params.rs
ft8mf1.f90           -> ft8mf1.rs
ft8mfcq.f90          -> ft8mfcq.rs
tone8.f90            -> tone8.rs
tonesd.f90           -> tonesd.rs
agccft8.f90          -> agccft8.rs
partintft8.f90       -> partintft8.rs
twkfreq1.f90         -> twkfreq1.rs
four2a.f90 wrapper   -> four2a.rs
chkfalse8.f90        -> chkfalse8.rs
chkspecial8.f90      -> chkspecial8.rs
call_q.f90           -> call_q.rs
callsign_q.f90       -> callsign_q.rs
chkgrid.f90          -> chkgrid.rs
chkflscall.f90       -> chkflscall.rs
searchcalls.f90      -> searchcalls.rs
chklong8.f90         -> chklong8.rs
filtersfree.f90      -> filtersfree.rs
packjt77 / ft8v2     -> ft8v2/
```

`ALLCALL7.TXT` is a runtime data file used by JTDX-style false-decode
filtering. ft8rs keeps the normal packaged lookup behavior: executable
directory first, then development locations.

## State Model

JTDX `ft8_mod1.f90` acts as state, not just constants. The Rust profile keeps
that state private to the JTDX session:

- slot audio buffer and AGC flags
- decoded-message arrays and duplicate state
- odd/even interval memory
- `lastrxmsg`, `lasthcall`, `incall`, and call-to-DT rings
- CQ/MyCall/QSO signal memory
- AP tables and AP mask plans
- Costas/tone/superdeep tables
- JTDX hash callbook
- `mycall`, `hiscall`, `hisgrid`, `nfqso`, `avexdt`, and forced-sync state

Hybrid must not share or mutate this state. It may only consume decoded result
rows.

## Implemented Shape

The current JTDX profile has a native decode path. It no longer falls back to
the WSJT-X decoder when no JTDX rows are found.

Implemented areas include:

- profile dispatch for `jtdx`;
- JTDX-owned state and constants;
- outer pass/cycle policy, including 3/6/9-pass cycle behavior;
- sync candidate generation with JTDX pass metrics, AGC support, forced-DT
  windows, candidate thinning, active/wide band split, and QSO virtual
  candidate accounting;
- JTDX downsample workspace, shifted products, residual/subtract state, and
  frequency tweak ordering;
- regular BP/OSD decode through JTDX-owned pack/unpack/hash/CRC/LDPC/OSD
  modules;
- `genft8.f90` tone recovery is mirrored as `get_tones_from_77bits`, so
  accepted 77-bit payloads produce the same 79-tone sequence used by SNR and
  subtract paths;
- forward/reverse symbol extraction and metric-source selection;
- per-slot duplicate suppression using JTDX-style decoded arrays;
- AP table selection, AP mask planning, AP LLR source selection, and a broad
  subset of standard/nonstandard/Hound AP mask families;
- FT8S, FT8SD, memory-filter, and superdeep scaffolding;
- JTDX false-positive filters for the regular path and part of AP/deep;
- JTDX-owned subtract, pass 4/7 half-sample shifts, and decoded-state updates;
- special-message secondary rendering at the outer decode layer;
- `delbraces.f90` is mirrored and applied after hash-call flags are captured,
  preserving `<...>` while removing braces around ordinary nonstandard calls
  before filtering/output/state updates;
- JTDX `searchcalls` database lookup via `ALLCALL7.TXT`;
- monitor and file mode can select `--profile jtdx`.

`ft8apset` intentionally keeps the JTDX AP-symbol surface, not only a packed
message-bit surface: packed templates become `+1/-1` AP symbols, and failed
template validation produces the source-style `99` sentinel positions. This
preserves the Fortran distinction between "this AP type is not present" and
"this AP type exists but its source array is poisoned by a failed template".

## Current Baseline

Baseline rows for `profile=jtdx` use:

```text
Extra is empty or J
```

Testing rules remain:

- release mode only for sensitivity/performance checks;
- keep timeout limits;
- run short tests only after meaningful formula/gate changes;
- run long tests only after a batch of source-aligned changes.

Current observed checkpoint:

```text
short fixture: 210703_133430.wav -> 20/20 target, 21 decoded total
long fixture:  230208_140300.wav -> 430/431 target
```

Current marker counts:

```text
210703_133430.csv: blank 20, W 1
230208_140300.csv: blank 411, J 20, W 13, E 14
```

The short fixture contains one `W` row, `CQ DX DL8YHR JO41`, which is excluded
from the JTDX target. After mirroring JTDX's outer frequency-band scheduling
more closely, this row is decoded by `profile=jtdx` too. It remains outside
the JTDX target count because the CSV marker says it is WSJT-X-only reference
data.

Current long-fixture misses are:

```text
230208_140700,-16,1.7,1153,F1MLZ UA3QNA -04,J
```

Frequency-band scheduling note:

- JTDX `decoder.f90` auto-selects a thread count from available cores, splits
  `nfa..nfb` into `nthr` bands, and invokes `ft8_decode` once per band.
- `--jtdx-threads` maps to JTDX `params%nmt`: `0` keeps source auto mode,
  `1..24` requests a user thread count capped at available logical cores.
- The Rust JTDX profile now mirrors that outer band split, keeps one
  `Ft8bWorkspace` per band so `lsubtracted/npos/freqsub` are thread-local like
  the source, and follows the source's center-out OpenMP section order for
  `numthreads >= 4`.
- This recovered the short-fixture `CQ DX DL8YHR JO41` row. On the current
  8-core development machine,
  `--jtdx-threads 0` and `--jtdx-threads 6` both produce the short extra row;
  `1/2/4/8` do not. A narrower `1133..1600 Hz` run can find
  `140715 OH5NBJ SV1MRW KM17`; later source-aligned SD candidate selection
  also keeps it in the full-band run.
- JTDX `ft8b.f90` scans all odd/even SD memory entries and lets the last
  matching entry win. Mirroring that behavior recovered
  `140700 4S6NCH KK1F FN31`; first-match selection can use an older nearby
  message template and miss the correct decode.
- `140545 IU2QDB RA3ABG 73` can be recovered if `RA3ABG` is present in the
  runtime `ALLCALL7.TXT`, but this is a CallDB coverage difference rather than
  a decoder-path alignment issue. The CSV marks this row as `Extra=E`, and the
  root call database is not modified for this milestone.
- The remaining `F1MLZ UA3QNA -04` row enters `ft8b` with strong Costas sync,
  but regular BP/OSD subpasses do not produce a valid codeword. Do not recover
  it by relaxing false-decode filters; the open gap is in bit metrics,
  candidate/refinement, OSD, or another regular-path numerical detail.

Recent source-level alignment:

- Added root-level Rust mirrors for JTDX `genft8.f90`, `genft8sd.f90`, and
  `tone8myc.f90`. `tone8.rs` now calls ordinary `genft8` for the branches where
  JTDX `tone8.f90` does so, while the SD decoders continue to call `genft8sd`.
- `osd174_91` now keeps the decoded bits on CRC failure and returns a negative
  `nharderror`, matching JTDX `osd174_91.f90`. The downstream false-decode gate
  rejects negative hard-error counts in the same place as the source.
- The OSD Gaussian-elimination loop no longer aborts if a pivot is not found in
  the `id..K+20` window; the source continues the outer loop.
- `ft8b` now checks `(i3,n3)` before `unpack77`, applies the JTDX `xnoi >= 0.01`
  SNR floor, and passes measured `srr` into non-virtual FT8S fallback calls.
- The `iqso=4` superdeep path now follows the source split more closely:
  deep-sync slots use the `tonesd` sync-template refinement and may try
  `ft8sd1`, while non-deep slots reuse the previous ordinary-symbol metrics and
  go directly to `ft8mf1` / `ft8mfcq`. The ordinary regular-failure SD fallback
  is limited to `ft8sd` with the source `srr < 7.0` guard.
- `ft8apset` now keeps the JTDX no-grid CQ-DX AP template as `CQ <hiscall>`
  instead of filling a dummy `AA00` grid. The mask still applies only the source
  `1:58` and `75:77` bit ranges for this case.
- The outer `ft8b` `isubp1` pruning now mirrors the source early
  `lmycsignal` skip and the later `lqsocandave` / `lmycsignal .and.
  lmycallstd` AP/regular subpass gates.
- The AP gate for standard-MyCall/nonstandard-DXCall now includes the source
  `lcqdxcnssig`, `lqso73`, and `lqsorr73` checks for `iaptype` 31/35/36.
- `call_q.f90` is now an independent mirror and is shared by the JTDX
  `chkfalse8` / `chkspecial8` paths, keeping it distinct from the stricter
  `callsign_q.f90` rules as in the source tree.

## Known Gaps

Highest-value unfinished alignment items:

- finish file-by-file source audit of `lib_jtdx` against `jtdx/lib`;
- complete the remaining AP/deep skip and CPU-pruning matrix from
  `ft8b.f90`; the highest-risk `iqso=4` SD entry split is now represented, but
  AP `iaptype` pruning still needs periodic source audit;
- validate newly wired forced-sync / `avexdt` / odd-even memory behavior
  against real JTDX output;
- complete AP/deep-specific false-positive filter coverage;
- keep `chkgrid.f90` as a deliberately partial mirror. The full geographic
  callsign/grid rule table mostly reduces false positives and is not planned
  for this milestone. Maintain the current syntax/early-state behavior and
  only add small source rules when a real false positive points there;
- JTDX `four2a.f90` calls single-precision `sfftw_*`, while the Rust profile
  currently runs the local FFT abstraction with `f64` buffers. This is a known
  numerical-path difference and should be changed only as a deliberate
  full-chain FFT precision project, not as an isolated sensitivity tweak;
- decide how to represent the `filtersfree.f90` `datapwr` correlation gate;
- audit residual-aware downsample invalidation after subtract;
- wire JTDX FFTW thread/patience settings into `lib_jtdx::four2a` only if
  JTDX FFT tuning becomes a profile target.

Explicitly deferred:

- SWL-only behavior as a default path;
- FT4, JT77, WSPR, or non-FT8 modes;
- sharing internals with the WSJT-X decoder;
- turning hybrid into a mixed-state decoder.

## Recent Source-Audit Notes

Keep these as active caution points while continuing alignment:

- `chkspecial8` uses a narrower source-local call rejection rule than the
  broader callsign-quality filters.
- `tone8` must generate available Hound/nonstandard DX hints before returning
  early for missing focused-QSO report templates.
- `sync8d` keeps source loop edge behavior where the final averaged sync item
  reuses the previous pair average in selected pass paths.
- `ft8apset` nonstandard masks must preserve JTDX dummy-call and base-call
  choices; do not simplify them into ordinary full-call masks.
  In particular, AP type 3 uses the `MyCall DxCall RRR` template for the
  first 58 bits, AP type 11 uses `MyCall <DxCall> -16`, AP type 41 uses
  `<MyCall> DxCall -15`, and AP type 40 uses `<MyCall> ZZ1ZZZ -15`.
- `ft8b` focused-QSO `iqso=3` should reuse refined state from `iqso=2`, not
  rerun an independent sync search.
- Special-message rows can produce both `msg37` and `msg37_2`; both need the
  normal duplicate/output/hash/memory path.

## Do Not Do

- Do not modify `src/decode/lib_wsjtx` while working on this profile.
- Do not reintroduce fallback to the WSJT-X decoder.
- Do not describe incomplete JTDX output as JTDX-aligned.
- Do not enable SWL by default.
- Do not add profile-level shortcuts that cannot be traced to JTDX source.
