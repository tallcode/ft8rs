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
short fixture: 210703_133430.wav -> 20/20 target, 20 decoded total
long fixture:  230208_140300.wav -> 429/431 target
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
230208_140715,-23,0.5,1502,OH5NBJ SV1MRW KM17,
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
- This recovered the short-fixture `CQ DX DL8YHR JO41` row in some band/thread
  schedules. The row is marked `W`, so it is outside the JTDX target.
- A narrower `1133..1600 Hz` run can find `140715 OH5NBJ SV1MRW KM17`, but the
  current full-band source-shaped run does not keep it. Treat this as the
  active full-slot scheduling/residual interaction gap.
- JTDX `ft8b.f90` scans all odd/even SD memory entries and lets the last
  matching entry win. Mirroring that behavior recovered
  `140700 4S6NCH KK1F FN31`; first-match selection can use an older nearby
  message template and miss the correct decode.
- `140545 IU2QDB RA3ABG 73` can be recovered if `RA3ABG` is present in the
  runtime `ALLCALL7.TXT`, but this is a CallDB coverage difference rather than
  a decoder-path alignment issue. The CSV marks this row as `Extra=E`, and the
  root call database is not modified for this milestone.
- `F1MLZ UA3QNA -04` enters `ft8b` with strong Costas sync, but regular BP/OSD
  subpasses do not produce a valid codeword. Do not recover it by relaxing
  false-decode filters; the open gap is in bit metrics, candidate/refinement,
  OSD, or another regular-path numerical detail.

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
- `decoder.f90` active-band narrowing for `filter` / `nagainfil` now lives at
  the outer JTDX decode-band split, not inside `sync8`, matching the source
  layering while preserving the original wide band for AGC normalization.
- `chkfalse8` now returns immediately for the source AP QSO/grid iaptypes
  `3/11/21/41` after their dedicated checks, instead of applying an extra
  generic grid/report validation that JTDX does not run there.
- `ft8b` results now carry the source `lhashmsg` state through `delbraces`.
  This keeps originally hash-braced messages out of odd/even AP memory even
  when the displayed message no longer contains angle brackets.
- `nft8rxfsens` is represented as profile state instead of a hard-coded
  FT8S argument. The high-sensitivity profile still sets it to `3`, matching
  the current target, and virtual-QSO attempt pruning now reads the same value.
- The outer call-DT memory now mirrors `extract_call.f90` instead of taking the
  second whitespace token directly. This matters for directed CQ forms such as
  `CQ DX CALL GRID`, where JTDX records `CALL` rather than `DX`.
- The FT8SD regular-failure fallback is no longer a global post-regular
  fallback. It is gated by the source `isubp2 == 3` / `srr < 7.0` condition and
  is attempted on the matching regular/AP failure paths.
- `msgparser` now follows the fixed-field source checks more closely, including
  the fifth-space position guard, source-style report extraction, and the
  1-based `ib.gt.3` brace-removal threshold translated to 0-based `ib >= 3`.
- `delbraces` now uses a fixed 37-byte buffer and source-style character
  shifts instead of tokenizing and joining whitespace.
- `packjt77sd` now mirrors the source-supported SD unpack surface: free text
  `0.0`, type `1/2`, and `i3=4` CQ-only messages. Unsupported packed types
  fall back to SD free text on pack, matching `pack77sd.f90`.
- `ft8sd` / `ft8sd1` now use `genft8sd` for symbols only and keep the source
  candidate text (`msgd` / `msg4(imax)`) as the decoded message, matching the
  Fortran callers that ignore `msgsent37` on success.
- `ft8mf1` / `ft8mfcq` rank the best and second-best tones with the source
  two-pass `s1/s2` search, so the second-best tone explicitly excludes the
  first-best tone. `ft8mfcq` also keeps `msgd` as the success text, matching
  `msg37=msgd`.
- `ft8s` now keeps the source candidate text when the fallback table has to
  regenerate symbols with `genft8sd`, and its four best-tone rows use the
  source `s1/s2/s3/s4` scan order instead of a generic top-N helper.
- `tonesd` / `ft8mf1` now keep the source SD candidate text after `genft8sd`
  symbol generation, and `ft8mf1` applies the source 12-character guard to the
  first two message fields.
- `ft8sd` / `ft8sd1` now apply the same source 12-character `c1/c2` guard
  before building SD candidate variants.
- `agccft8` now uses the JTDX `indexx` mirror to select its median spectral
  level instead of sorting the vector directly, matching the source call shape.
- `ft8b` `iqso=4` non-deep SD/MF recovery now requires the preceding `iqso=1`
  refined state, matching the source jump to label `32` that reuses the
  existing symbol matrix instead of running a new sync search.
- `ft8b` symbol extraction now keeps `syncavpart(1:3)` as an explicit
  three-element vector before `maxval`, and the wide soft-sync `scoreratiow`
  division follows the source loop without an extra zero guard.
- `ft8b` CQ/MyCall tone SNR hints now use the source `signal / noise`
  expression directly instead of Rust's previous fallback value when the noise
  denominator was non-positive.
- `sync8` now uses the JTDX `indexx` mirror for red baseline selection and
  candidate ordering instead of direct Rust sorting, preserving the source's
  sorted-index workflow.
- AP `lqsocandave` pruning is branch-specific like `ft8b.f90`: standard
  focused QSOs only keep `iaptype=3..6` on the late averaged-signal subpasses,
  while nonstandard DX-call QSOs only keep `iaptype=11..14`.
- `syncdist` is written as the same repeated `maxloc` / zero / retry ladder as
  the include file, rather than a compact rank loop, so hard-sync rank tie
  order stays audit-visible.
- `decoder.f90` `avexdt` update is mirrored with the same `nFT8decd`
  branches. For `lforcesync` with zero decodes, ft8rs stores the source's final
  next-slot state: JTDX briefly assigns `forcedt` for reporting and then resets
  `avexdt` to zero before the next decode.
- Subtract/downsample residual state was rechecked against `ft8_decode.f90` and
  `ft8_downsample.f90`: `npos` resets per pass, `lsubtracted` persists across
  passes, and `freqsub` invalidates the cached long FFT when the next candidate
  is within 50 Hz of a subtracted signal.

## Known Gaps

Highest-value unfinished alignment items:

- finish file-by-file source audit of `lib_jtdx` against `jtdx/lib`;
- keep validating Hound/SWL-only AP branches against real JTDX output before
  calling those optional modes aligned;
- complete AP/deep-specific false-positive filter coverage;
- keep `chkgrid.f90` as a deliberately partial mirror. The full geographic
  callsign/grid rule table mostly reduces false positives and is not planned
  for this milestone. Maintain the current syntax/early-state behavior and
  only add small source rules when a real false positive points there;
- keep `filtersfree.f90` without the `datacor(datapwr)` gate until `datapwr`
  is represented at the Rust filter boundary. The current mirror intentionally
  preserves only deterministic text-shape filters;
- JTDX `four2a.f90` calls single-precision `sfftw_*`, while the Rust profile
  currently runs the local FFT abstraction with `f64` buffers. This is a known
  numerical-path difference and should be changed only as a deliberate
  full-chain FFT precision project, not as an isolated sensitivity tweak;
- decide how to represent the `filtersfree.f90` `datapwr` correlation gate;
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
- The source skips virtual `iqso=3` when `nft8rxfsens < 3`; keep that gate
  tied to the FT8S sensitivity setting even though the default target uses `3`.
- Special-message rows can produce both `msg37` and `msg37_2`; both need the
  normal duplicate/output/hash/memory path.
- Do not infer hash-message state from the final rendered text alone:
  JTDX sets `lhashmsg` before `delbraces`, and later memory gates use that
  original state.
- Keep call-DT extraction tied to the source `extract_call.f90` rules; simple
  token-2 extraction breaks directed CQ messages.
- Keep FT8SD fallback tied to the subpass where `ft8b.f90` calls it. Moving it
  after all regular/AP attempts broadens the decode path and can add false
  positives.
- `agccft8` `lforcesync` intentionally returns after `forcedt` calculation:
  this matches the source `if(lforcesync) ... else ... endif` structure.
- In `agccft8`, keep `spec`, `minval(s3(...))`, and `maxval(s3(...))` as
  explicit source-order loops. The values drive AGC bail/normalization and are
  better left audit-visible.
- In `filtersfree`, `?` intentionally increments both `nsign` and `nother`;
  this is source behavior, not a duplicate-counting bug.
- `encode174_91` may look structurally different from Fortran, but it still
  builds `message77 + CRC14 + generator-matrix parity`; treat it as aligned
  unless a bit-level fixture proves otherwise.
- In SD decoders, do not replace the successful message text with
  `unpack77sd` output. JTDX uses the original deep-search candidate text and
  only uses `genft8sd` / `packjt77sd` to produce tones.
- For `ft8mf1` and `ft8mfcq`, do not collapse the two source max searches into
  a one-pass top-two helper; tie behavior and second-best exclusion should
  follow the Fortran loops.
- In `ft8mf1` and `ft8mfcq`, keep `ref0` accumulation as an explicit
  `do i=1,58`-style loop, because it participates directly in the message
  confidence ratio.
- For `ft8s`, keep the four-rank search as the explicit `s1/s2/s3/s4` loops.
  The compact top-N form is tempting but creates avoidable tie-order ambiguity.
- In `ft8s`, keep false-deep-search power ratios (`ssync`, `spaty`,
  `spnoise`, `spother`) as explicit source-order sums rather than iterator
  reductions.
- In `ft8b`, keep bit-metric construction shaped like the source: fixed
  `s2(0:511)`, explicit `k=1,29,nsym`, separate `ks/ks1/ks2`, and paired
  `maxval(..., one)` / `maxval(..., .not.one)` searches. Generic helpers or
  vector top-N/max-by-bit code are mathematically close but make tiny numeric
  and tie-order differences harder to audit.
- In `ft8b` SNR estimation, avoid extra Rust-only safety clamps or `powi`
  rewrites around the source `log10` and correction formula unless a real
  input proves JTDX itself would guard that path.
- In `ft8b` sync scoring and tone normalization, prefer explicit source-order
  accumulation for `sum(s81)`, `sum(snrsync)`, `sum(s8(...))`, `minloc`, and
  `maxval`-style paths. Iterator reductions are concise but make sum order and
  first-tie behavior less obvious during Fortran audits.
- Keep `normalizebmet` and AP `apmag=maxval(abs(llra))*1.01` source-shaped:
  explicit square accumulation for `bmet2av` and explicit max scan for AP
  magnitude.
- In `sync8`, keep AGC `tall` group sums explicit (`sya/sycq/sybc`) to mirror
  the source branches.
- In `sync8` candidate ordering, fill the `indexx` input array explicitly from
  `candidate0(3,:)` or `candidate0(5,:)`; this mirrors the source call shape
  and makes the sync-vs-weighted-sync sort key obvious.
- In `bpdecode174_91`, keep the `sum(tov(1:ncw,i))` and parity syndrome
  accumulation as explicit loops. This preserves the LDPC source loop shape
  without changing the decoder math.
- In `osd174_91`, keep weighted-distance sums (`sum(xor*absrx)`) and bit-count
  tests as explicit loops where practical. The OSD structure is still Rust
  shaped, but the sensitive score accumulation should follow source order.
- Also keep OSD receive-vector reordering (`absrx`, `apmaskr`, `m0`) as
  explicit loops so it is visually traceable to the Fortran reordered arrays.
- In `sync8d`, keep the top-level `ipass` branch structure aligned with the
  source. In particular, `ipass=3/4/8` uses adjacent averaged sync vectors for
  both non-last and last-sync paths; only the metric changes from abs-sum to
  power.

## Do Not Do

- Do not modify `src/decode/lib_wsjtx` while working on this profile.
- Do not reintroduce fallback to the WSJT-X decoder.
- Do not describe incomplete JTDX output as JTDX-aligned.
- Do not enable SWL by default.
- Do not add profile-level shortcuts that cannot be traced to JTDX source.
