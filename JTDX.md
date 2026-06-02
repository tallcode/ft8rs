# JTDX Profile Notes

This document records only the JTDX-oriented FT8 decoder plan and source audit
notes. It should not contain WSJT-X implementation notes except where a direct
contrast is needed to explain a JTDX-specific decision.

## Scope

The `jtdx` profile is a separate JTDX-oriented decoder path with
high-sensitivity defaults.

It is not:

- a configuration overlay on the WSJT-X decoder
- a JTDX GUI-default clone
- a mixed WSJT-X/JTDX implementation
- the hybrid union decoder

The intended implementation boundary is:

```text
src/decode/lib_jtdx/
```

## Document Boundary

This file is the technical record for `profile=jtdx` only.

- Do not use this file to record WSJT-X maintenance notes.
- Do not use this file to record hybrid merge policy, except when stating what
  JTDX exposes to the hybrid runner.
- Keep JTDX source-audit details here so a reviewer can inspect JTDX behavior
  without reading `WSJTX.md` or `HYBRID.md`.
- Record all JTDX-specific parameter choices, implementation risks, source
  mapping notes, and unfinished alignment tasks here. Do not let these notes
  drift into the WSJT-X document.
- If a JTDX behavior later feeds hybrid, record the JTDX decode behavior here
  and leave only the hybrid launch/merge/output rule in `HYBRID.md`.
- Keep this document independent from `WSJTX.md`; WSJT-X should only appear as
  a brief contrast when it explains why the JTDX profile intentionally differs.
- Do not record hybrid launch, dedupe, output, threading, or source-attribution
  policy here. Those belong in `HYBRID.md`.
- When a JTDX implementation detail affects hybrid, keep the decoder detail in
  this file and record only the orchestration consequence in `HYBRID.md`.

## Core Rules

- Use JTDX source as the implementation reference.
- Keep JTDX code separate from the WSJT-X-aligned path.
- Do not call WSJT-X decoder internals from `lib_jtdx` unless a file has been
  explicitly audited and documented as identical.
- Do not extract common decoder internals yet.
- Treat JTDX `ft8_mod1.f90` as the state model for this path.
- Current implementation priority is normal FT8 high-sensitivity decoding.
- Keep SWL optional, not enabled by default, and do not expand SWL-only behavior
  in this milestone.

## High-Sensitivity Defaults

The first ft8rs `jtdx` profile should use high-sensitivity JTDX-style settings:

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
```

These are ft8rs high-sensitivity defaults. They should not be described as
JTDX GUI defaults.

SWL-specific tuning is deferred. Existing SWL switches may remain for
compatibility with the profile surface, but high-sensitivity normal FT8 is the
active target.

Hound AP table selection is explicit:

```bash
ft8rs file input.wav --start-time 230208_140300 --profile jtdx --hound
```

## Dynamic Behavior

The following JTDX behaviors should remain dynamic because the source implements
them that way:

- pass count derived from `nft8cycles` / `nft8swlcycles`
- pass-specific sync metric selection
- pass-specific `syncmin`
- DT/time-window behavior
- candidate thinning/ranking behavior
- residual-aware downsample cache invalidation
- frequency-dependent `napwid`

`napwid` should follow JTDX frequency-dependent behavior by default:

```text
freq < 30 MHz  -> napwid = 5
freq < 100 MHz -> napwid = 15
else           -> napwid = 50
```

Manual `napwid` override can be added later if needed.

## Source Dependency Closure

`lib_jtdx` should mirror the JTDX FT8 dependency closure, not only files whose
names begin with `ft8`.

Initial file mapping target:

```text
src/decode/lib_jtdx/ft8_decode.rs
src/decode/lib_jtdx/ft8b/
src/decode/lib_jtdx/ft8s.rs
src/decode/lib_jtdx/ft8sd.rs
src/decode/lib_jtdx/ft8sd1.rs
src/decode/lib_jtdx/ft8apset.rs
src/decode/lib_jtdx/sync8.rs
src/decode/lib_jtdx/sync8d.rs
src/decode/lib_jtdx/indexx.rs
src/decode/lib_jtdx/tone8.rs
src/decode/lib_jtdx/tonesd.rs
src/decode/lib_jtdx/ft8_downsample.rs
src/decode/lib_jtdx/ft8_mod1.rs
src/decode/lib_jtdx/ft8_params.rs
src/decode/lib_jtdx/ft8mf1.rs
src/decode/lib_jtdx/ft8mfcq.rs
src/decode/lib_jtdx/agccft8.rs
src/decode/lib_jtdx/partintft8.rs

src/decode/lib_jtdx/ft8v2/bpdecode174_91.rs
src/decode/lib_jtdx/ft8v2/chkcrc14a.rs
src/decode/lib_jtdx/ft8v2/encode174_91.rs
src/decode/lib_jtdx/ft8v2/ldpc_174_91_c_generator.rs
src/decode/lib_jtdx/ft8v2/ldpc_174_91_c_reordered_parity.rs
src/decode/lib_jtdx/ft8v2/osd174_91.rs
src/decode/lib_jtdx/ft8v2/packjt77/
src/decode/lib_jtdx/ft8v2/packjt77sd.rs
src/decode/lib_jtdx/ft8v2/subtractft8.rs

src/decode/lib_jtdx/syncdist.rs
src/decode/lib_jtdx/callsign_q.rs
src/decode/lib_jtdx/chkfalse8.rs
src/decode/lib_jtdx/chkspecial8.rs
src/decode/lib_jtdx/chkgrid.rs
src/decode/lib_jtdx/chkflscall.rs
src/decode/lib_jtdx/searchcalls.rs
ALLCALL7.TXT
```

Additional hint/CQ/call support files should be added to `lib_jtdx` as soon as
the implemented JTDX slice reaches those calls.

## Current Implementation Status

Implemented or scaffolded:

- profile dispatch can select `jtdx`;
- JTDX-owned state skeleton exists in `ft8_mod1.rs`;
- JTDX-owned constants exist in `ft8_params.rs`;
- JTDX outer pass/cycle policy is represented in `ft8_decode.rs`;
- JTDX sync candidate path has an initial `sync8.rs` implementation;
- JTDX `sync8d.rs` exists because the JTDX FT8 path depends on it;
- JTDX downsample has its own workspace and band/edge-scaling path;
- JTDX `ft8b.rs` contains the early sync/downsample/symbol-extraction slice;
- JTDX-owned `ft8v2` now has CRC, BP, reordered parity data, and unpack
  support in its own module path;
- JTDX-owned `encode174_91` now adds CRC14 and LDPC parity bits from the JTDX
  generator matrix, giving this profile its own encode path for source-derived
  reference tone generation;
- JTDX-owned OSD fallback and generator data exist in `ft8v2`;
- JTDX `agccft8.rs` exists and `sync8.rs` can use the AGC sync branch for
  candidate generation;
- AGC now runs once at the JTDX session/slot boundary, mutating the JTDX `dd8`
  buffer and storing `lagcc` / `lagccbail` in `ft8_mod1` state before
  `ft8_decode` passes, matching the JTDX source control-flow shape more
  closely than per-`sync8` temporary AGC copies;
- the JTDX forced-sync time-window path is wired through `--force-sync`:
  `agccft8` computes `forcedt`, the session keeps `avexdt` across slots, and
  `sync8` derives `jzb/jzt` from the current `avexdt` value;
- after each JTDX slot, `avexdt` is updated with the JTDX weighted formulas and
  three-point DT median accumulation used by the higher-level JTDX decoder
  wrapper;
- JTDX-owned initial false-positive filter modules exist for regular decode
  output (`chkfalse8`, `chkspecial8`, `chkgrid`, `chkflscall`,
  `callsign_q`);
- JTDX-owned `searchcalls.rs` now mirrors `searchcalls.f90` over
  `ALLCALL7.TXT`, so `chkflscall` uses the same database-backed accept/reject
  shape instead of the earlier callsign-quality heuristic. At runtime ft8rs
  looks for `ALLCALL7.TXT` next to the executable first, then falls back to the
  working directory and repository root for development builds;
- JTDX AP type tables from `ft8_mod1.f90` are present and the regular `ft8b`
  path now plans AP subpasses from those tables by `nQSOProgress` and
  standard/nonstandard call shape;
- JTDX AP mask context construction now lives in `ft8apset.rs`, matching the
  source-level separation between `ft8b.f90` and `ft8apset.f90`;
- AP execution is now wired for the JTDX regular path for the mask families
  that can be built from explicit `mycall` / `hiscall` / optional `hisgrid`
  templates or fixed CQ/MyCall masks: `1`, `2`, `3`, `4`, `5`, `6`, `11`,
  `12`, `13`, `14`, `21`, `23`, `31`, `35`, `36`, `40`, `41`, `42`, `43`,
  `44`, and `111`;
- regular and AP decode now use a JTDX-shaped `isubp2` to LLR-source mapping:
  non-SWL regular decode skips the source `isubp2=4` path, the regular
  `isubp1=1..2` loop now selects `llra` / `llrb` / `llrc` / `llrd` with the
  same pass-dependent rules as `ft8b.f90`, SWL can use the regular `isubp2=4`
  `llrd` path, and AP subpasses select `llra` / `llrb` / `llrc` according to
  the explicit JTDX table rather than a simple modulo cycle;
- regular/AP subpass gating now uses the measured `syncavemax` from the
  Costas-symbol pre-scan instead of a constant placeholder, and the pre-scan is
  source-shaped as a Costas-only pass rather than mutating the later symbol
  metric matrices;
- `sync8` candidates now keep the source `candidate(4)` CQ marker separate
  from the `candidate0(5)` thinning sort metric, preventing candidate thinning
  or filtering from corrupting the CQ-only/AP path marker;
- QSO-candidate and MyCall-signal subpass pruning now follows the source
  control flow more closely: decoded QSO candidates skip the whole subpass
  loop, and the MyCall extra-subpass skip is only applied when `mycall` is a
  standard callsign;
- JTDX AP subpass gates now keep the hound `31` / `36` / `111` `lapcqonly`
  checks split the same way as the source, and the hound `31` / `36` Fox
  frequency limit is only applied when wide DX search is disabled;
- JTDX AP subpass gates for the both-nonstandard-call DX-call search branch no
  longer apply the standard-call wideband `loutapwid` restriction, matching the
  source branch where wideband search is used by default for that monitoring
  path;
- AP mask magnitude follows `ft8b.f90`: `apmag` is derived from
  `maxval(abs(llra))*1.01`, where `llra` already includes the source `2.83`
  LLR scale factor;
- decoded-row SNR now follows the JTDX `ft8b.f90` source formula: it uses the
  accepted `itone` sequence to accumulate signal/noise ratios over all 79
  symbols, applies the high-SNR and low-SNR nonlinear corrections, then clamps
  regular decodes at `-23 dB` and AP/deep decodes at `-24 dB`;
- FT8S/FT8SD SNR estimation keeps the source post-correction: very weak
  FT8S/FT8SD rows fall back to `xsnrs-1.0` and clamp at `-26 dB`; this is not
  applied to AP/deep rows because the source condition is `lft8s .or. lft8sd`;
- false-positive filter quality now keeps the OSD `dmin` contribution and uses
  the source expression `qual=1.0-(nharderrors+dmin)/60.0`;
- `--hound` selects the JTDX `nhaptypes` AP table; hound AP types `21`, `22`,
  `23`, `24`, and fixed-bit type `111` are wired. Types `22` and `24` use the
  JTDX type-0.1 special-message template that feeds `apsymsp` in the Fortran
  source;
- Hound fox-report/RR73 classification now has the source-shaped
  `idtonefox73` / `idtonespec` tone hints, derived base-call handling, the
  `lfoxspecrpt` / `lfoxstdr73` ratios, and the corresponding AP skip gates for
  progress states 1 and 3;
- `twkfreq1.rs` mirrors JTDX `lib/twkfreq1.f90`. The native JTDX `ft8b`
  branch now applies the `-delfbest` frequency tweak to the downsampled complex
  buffer before symbol extraction, matching the source order after the fine
  frequency search;
- `sync8d.rs` now builds the main `csync` Costas templates from the same
  `gen_ft8wave("CQ 2E0DLA IO92")` waveform seed used by JTDX `cwfilter.f90`,
  rather than a pure 32-point tone approximation. The sync correlation helpers
  also use the source `cd0(-800:4000)` bounds, so virtual and edge candidates
  are not clipped to the ordinary data-symbol window;
- JTDX symbol extraction now has the source-shaped second symbol pass: it
  computes the reverse-conjugate 32-sample vector, applies the weak-signal
  first/last-sample scaling when `syncav < 2.5`, supports the source
  `lreverse` pass selection, and stores both forward `cs` and reversed `csr`
  matrices for later metric retries;
- during `lreverse` passes, symbol extraction now also preserves the source
  `cscs` forward-symbol matrix before replacing `cs` with the reversed symbol
  matrix. This prepares the later `isubp1=3/6/9` combined `cscs/csr` metric
  variants without enabling those source paths prematurely;
- the symbol-metric path now applies the JTDX tone-spectrum normalization after
  extraction: `sp` is built from the Costas-prefix and data/tail ranges, the
  weakest tone row is used as reference, and rows above the `spr > 1.5`
  threshold scale `s8`, `cs`, and `csr` by the source factors;
- regular decode now recomputes bit metrics for `isubp1=1` from `cs` and
  `isubp1=2` from `csr`, so the second regular subpass is no longer only an
  LLR-source re-selection over the same forward-symbol data;
- regular bit-metric extraction now explicitly converts the Fortran 1-based
  symbol columns to Rust 0-based columns. The JTDX source data halves use
  `ks=8..36` and `ks=44..72`; Rust must consume columns `7..35` and `43..71`
  from the 79-symbol arrays;
- JTDX `ft8b.f90` symbol FFT bins need the same 1-based to 0-based treatment:
  source `csymb(1:8)` maps to Rust FFT buffer indexes `[0..7]`, not `[1..8]`.
  The same correction applies to the `csymb256` CQ classifier bins. Using
  `[1..8]` shifted every tone row and drove `nsync` too low for all native JTDX
  candidates;
- JTDX session setup must initialize `ft8_mod1.nfawide/nfbwide` from the active
  decode frequency range. Leaving the Fortran-module defaults as zero collapses
  `sync8` wide search to the first FFT bin and makes the native profile appear
  to have no viable candidates;
- `ft8b` now builds initial `tone8myc` / `tone8`-style reference tone hints
  with the JTDX-owned pack/encode path. Those hints drive first-pass
  `lmycsignal`, `lqsosig`, `lqsosigtype3`, and `lqsocandave` classifiers used
  by `nsubpasses` and AP/deep OSD depth selection;
- `ft8b` now owns a JTDX-private even/odd signal-history cache for CQ,
  MyCall, and QSO candidate symbol matrices. It clears temporary signal
  storage at slot start, promotes temporary matrices to the current parity at
  slot end, and can recover `csold` for a later candidate using the JTDX
  frequency/DT proximity rules;
- high-order metric sources for `cs+csold` power and `abs(cs)+abs(csold)` sum
  variants are now available to the bit-metric builder. When `csold` is found
  from the private signal-history cache, CQ/MyCall/QSO candidate subpass counts
  are raised to the JTDX source levels `5/8/11`. AP/deep attempts now execute
  inside the same `isubp1` metric-source loop as regular decode, so these
  high-order metric variants can feed AP/deep work. Ordinary regular decode
  continues to skip `isubp1>2`;
- AP/deep gating now has the first source-shaped branch layer for the JTDX AP
  type tables: standard/nonstandard call shape, missing configured calls,
  AP-width checks, `lapcqonly`, `lqsomsgdcd`, `stophint`, `nmic`, QSO-candidate
  priority, MyCall priority, standard-DX `ldxcsig/lcqdxcsig`, and the
  `RRR`/`73`/`RR73` QSO-end classifiers are represented from the state
  currently available in Rust;
- JTDX-owned `packjt77sd` now packs/unpacks the FT8S message subset and
  `genft8sd` produces source-shaped tone sequences for superdeep matching.
  `ft8s.rs` now mirrors the main JTDX `ft8s.f90` matcher for configured
  `mycall/hiscall` QSO messages near the QSO frequency after regular/AP
  BP+OSD attempts fail, including the source candidate windows, iterative
  demodulation passes, threshold ladder, and sync/parity ratio guards;
- `ft8sd1.rs` and `ft8sd.rs` now mirror the previous-slot superdeep recovery
  branches fed by JTDX odd/even copy state. The `ft8b` path now looks up
  `msgd/lcq` from the previous-slot copy by frequency and DT proximity and can
  run the source-shaped `ft8sd1` and `ft8sd` threshold ladders before giving up
  on that candidate;
- `ft8mf1.rs` and `ft8mfcq.rs` now mirror the JTDX memory-filter superdeep
  scoring branches. `ft8mf1` consumes the `tonesd` report/grid candidate tone
  tables, scores them against the ranked data-symbol powers, applies the
  source-shaped message-state rejections, and returns only messages that pass
  the `u1/u2/qual/thresh` gates;
- `ft8mfcq.rs` has a direct JTDX source counterpart (`lib/ft8mfcq.f90`).
  Earlier review notes that treated it as an ft8rs-only matcher were a false
  positive;
- `ft8mf1.rs` was re-audited for the 0-based/1-based report index mapping.
  The Rust `ipk` range checks intentionally map the JTDX Fortran `ipk=55..57`
  acknowledgement rows to Rust `54..56`; the `73` row is not being rejected by
  an off-by-one bug;
- `tonesd.rs` now mirrors the JTDX `tonesd.f90` superdeep sync-template
  construction. When a previous-slot `msgd/lcq` candidate is available, `ft8b`
  promotes the source-shaped `iqso=4` attempt and passes the generated
  `csyncsd` / `csyncsdcq` templates into `sync8d` so virtual-candidate
  superdeep sync contributes to the DT/frequency search. It also owns the
  76-entry report/grid candidate table consumed by `ft8mf1`, matching the
  `itone76` / `idtone76` / `msgsd76` role from the source;
- `tone8.rs` now mirrors the JTDX `tone8.f90` table-building role more
  closely. A slot session precomputes `csynce`, MyCall/CQ/nonstandard-DX/Hound
  tone hints, and the 56-report QSO table once, then `ft8b` / `ft8s` consume
  those tables rather than rebuilding isolated templates on demand. `sync8d`
  also builds the `csynccq` extension from the JTDX `cwfilter.f90` seed
  message. The 56-row table now follows the JTDX standard/nonstandard
  callsign split, including the `<...>` wrapping used when only one side is
  nonstandard, and `csynce` is generated from the first source-shaped row;
- `ft8s.rs` now consumes the `tone8` precomputed 56-report table before
  falling back to local construction. This keeps the FT8S matcher closer to
  the JTDX precompute-and-reuse model and reduces duplicated message
  generation in the decode loop;
- `ft8s.rs` pass 5/6 acceptance now keeps the source-shaped iterative
  history: pass 5 checks the earlier `nmatch` ladder, and pass 6 keeps the
  separate 61/62/63 branches with their distinct sensitivity, parity-growth,
  and `imax==ipk` guards. The `lr73` index guards were rechecked and left as
  the expected 0-based translation of the JTDX `ft8s.f90` rows;
- `syncdist.rs` now owns the JTDX `syncdist.f90` rank-distribution helper
  used by the hard-sync AP skip gate. The logic was already present in the
  `ft8b` path; moving it back into the mirror file closes the empty-stub
  source-layout gap;
- `partintft8.rs` now contains the delayed-buffer/noise-fill helper shape from
  `partintft8.f90`, including the source `ndelay * 1200` sample shift. It is
  intentionally not wired into the normal file/monitor decode path because
  ft8rs does not yet model JTDX's outer partial-data-loss decoder mode;
- `ft8apset.rs` now precomputes AP mask plans for the active AP table when a
  JTDX session is created, instead of constructing every mask directly inside
  `ft8b`. The current representation uses `Option<ApMaskPlan>` for
  buildable/not-buildable entries; it still does not preserve the exact JTDX
  sentinel-array surface (`99` values) for failed templates;
- `ft8apset.rs` type 31 now follows the source call-shape split: standard
  DX calls use `CQ DX GRID` only when a four-character grid is available, while
  nonstandard DX calls build the type-4 `CQ DXCall` full-message mask instead
  of incorrectly forcing the grid form;
- `ft8apset.rs` type 35/36 now also follows the source call-shape split:
  standard DX calls use the type-1 tail mask, while nonstandard DX calls use
  the type-4 `<MyCall> DxCall 73/RR73` mask with the Fortran `14:77` range
  translated to Rust `13..77`;
- `chkgrid.rs` now mirrors the first JTDX grid-area prefilter layer. In
  particular, `lchkcall` now means "grid requires callsign/prefix validation",
  not "grid format is invalid". Until the full callsign-prefix geography table
  is ported, `lgvalid` remains optimistic for syntactically valid four-character
  locators so the partial port cannot reject valid JTDX exceptions; the
  previous narrow `obviously_wrong_call_grid` shortcut was removed because it
  rejected valid remote locator areas;
- `osd174_91.rs` now uses a local JTDX `indexx` mirror for reliability
  ordering over `f32` values. This removes an avoidable Rust `sort_by` shape
  difference without widening the source `real` reliability vector to `f64`;
- The current short-test miss, `CQ DX DL8YHR JO41`, has been traced through
  sync and `ft8b`: the candidate is present and passes hard sync, but regular
  BP/OSD does not return a CRC-valid codeword. Its packed first 29 bits differ
  from JTDX `mcq` by six bits, so the normal `iaptype=1` CQ AP mask is not a
  valid shortcut for this `CQ DX` form;
- `ft8b` soft-sync gating was re-audited against JTDX `ft8b.f90`; the Rust
  path now preserves the source behavior where `scoreratio2` remains the
  accumulated middle-sync ratio while `scoreratio`, `scoreratio1`, and
  `scoreratio3` are normalized;
- focused-QSO virtual attempts now follow more of the source control flow:
  `nqso=3` visits `iqso=1/2/3`, `iqso=3` applies the source `ibest+1`
  adjustment, `iqso=4` uses the wider 0.5 Hz frequency retry step, and
  `iqso=2/3/4` FT8S/FT8SD attempts run before the ordinary hard-sync and
  BP/OSD gates. The `iqso=2/3` FT8S matcher receives the source `sqrt(s8)`
  symbol powers;
- FT8S and FT8SD decode results are now tagged separately from regular BP/OSD
  results. The JTDX session applies the source-specific false-decode guards
  from `ft8b.f90`: `lft8s` is rejected after `lrepliedother`, FT8S/FT8SD is
  rejected when configured `mycall` appears as the later callsign, and FT8SD
  is rejected when its two-call base duplicates a previously received regular
  base message. These deep results intentionally bypass the broad regular/AP
  `chkfalse8` path, as in the source;
- the JTDX session now carries the slot-local `lft8sdec` flag. Accepted FT8S
  rows set it before later candidates are processed, and `ft8b` uses it in the
  focused-QSO and AP-gate branches that the source guards with `lft8sdec`;
- regular decode unpacks 77-bit payloads through the JTDX-owned unpack context
  with `mycall` / `hiscall` available for hash-style call presentation;
- the JTDX session now owns its own JTDX `HashCallBook`; decoded calls are
  collected into that book for later hash lookups without sharing state with
  the WSJT-X decoder;
- successful JTDX regular decodes are saved back into the JTDX `ft8_mod1`
  `allmessages` / `allsnrs` / `allfreq` / `ndecodes` state fields so later
  AP/deep and filter slices have the expected state surface;
- focused-QSO `lastrxmsg` memory is retained in the JTDX session. A decoded
  same-thread `mycall hiscall ...` message near `nfqso` updates the stored last
  message and DT, and later `ft8b` candidates can use that DT in the JTDX
  `nqso` decision;
- focused-QSO recovery now also mirrors the surrounding JTDX state model:
  `calldteven` / `calldtodd` retain call-to-DT entries by interval parity,
  `even` / `odd` decoded-message arrays are copied into `evencopy` /
  `oddcopy` at the next same-parity slot, and the `incall` ring keeps recent
  decoded messages beginning with `mycall`. These state rings can restore
  `lastrxmsg` before a slot when the configured `hiscall` matches prior
  decoded traffic;
- per-slot JTDX decode arrays are reset before decoding, and duplicate
  suppression now uses the JTDX `allmessages` / `allsnrs` / `allfreq` rules
  instead of a generic Rust `HashSet`;
- JTDX-owned `gen_ft8wave.rs` and `ft8v2/subtractft8.rs` exist; successful
  regular decodes now carry `itone`, perform the JTDX-style subtract path, and
  update `freqsub` / `npos` / `lsubtracted` for later downsample decisions;
- outer pass 4 and pass 7 now apply the JTDX `dd8` half-sample shift behavior,
  including the `dd8m` save/restore used by 9-pass decoding;
- `hide_hash` is wired for regular output rejection where the decoded message
  still contains an unresolved `<...>` hash token beyond the first call field;
- regular-output false-positive filtering now includes additional
  source-level protocol-shape guards from JTDX `ft8b.f90`: type-2 `/P`
  enforcement, invalid `<...>` terminal acknowledgements, AP type-2 hash/grid
  rejection, type-4 `/R` plus grid rejection, `i3=1/2` ` R ` grid/callsign
  validation with second-call hash handling, and ARRL Field Day region/DX
  checks;
- AP false-positive filtering now receives the decoded row's estimated SNR and
  DT offset, allowing the JTDX `iaptype=35/36` weak/out-of-window DXCall-search
  first-callsign rejection to run in Rust;
- regular non-AP BP/OSD decode can attempt to emit messages without calling
  `lib_wsjtx` decoder internals;
- `profile=jtdx` no longer falls back to the protected WSJT-X stream decoder
  when the native path emits no rows. Empty output now means native JTDX did
  not decode that slot;
- monitor mode can use `profile=jtdx` at the full-slot boundary.

Not complete yet:

- the remaining JTDX AP/deep skip/gating matrix. AP/deep now runs inside the
  source-shaped nested `isubp1/isubp2` loop and can consume `csr`,
  `cscs/csr`, and `cs/csold` metric variants, but the Rust gate set is still
  missing source-complete nonstandard-DX search classifiers and several
  source-specific CPU-pruning gates. The JTDX `s256` CQ branch and the first
  Hound fox-report/RR73 classifiers are represented, but still need validation
  against real JTDX output;
- full JTDX FT8v2 source-level refinement;
- deeper AGC state integration with the JTDX-owned slot buffer;
- source-level validation of the newly wired `lforcesync` / forced-DT /
  `avexdt` behavior with real JTDX profile output;
- source-level validation of the newly wired odd/even `calldteven` /
  `calldtodd`, `incall`, and `lastrxmsg` restoration behavior with real JTDX
  profile output;
- remaining AP/deep source-level gating around the newly wired mask families;
- source-level validation of the newly added subtract and pass-shift paths
  against JTDX before baseline testing;
- AP/deep-specific source-level false-positive filter coverage after AP/deep
  decode execution exists;
- final JTDX baseline measurement.

`profile=jtdx` is still an incomplete JTDX profile. It can run the initial
regular BP path, but it must not be treated as a JTDX-aligned baseline until the
remaining source slices above are implemented and measured.

## State Model

JTDX `ft8_mod1.f90` is a state container, not just a constants module. It
contains data such as:

- `dd8`
- decoded message history
- odd/even slot memory
- last received message state
- call/DT memory
- CQ/mycall/QSO signal memory
- AP type tables
- Costas and gray-code tables
- AGC flags
- `mycall`, `hiscall`, `hisgrid`
- `nft8cycles`, `nft8swlcycles`
- `lhound`
- `avexdt`

Rust should model this as a JTDX-owned state object, not as global mutable
state and not as shared state with `lib_wsjtx`.

The Rust JTDX session now owns the main focused-QSO memory rings:

- `lastrxmsg` / `lasthcall`
- `calldteven`
- `calldtodd`
- `incall`
- `even` / `odd`
- `evencopy` / `oddcopy`

These are intentionally JTDX-private. Hybrid must not share or merge this
state with the WSJT-X session. The current Rust implementation follows the
source shape for restoring `lastrxmsg` from `incall` and previous-slot
odd/even copies before a slot, and for saving decoded call-DT entries after
accepted decodes.

The JTDX profile also keeps a JTDX-owned hash callbook. Hybrid must not share
that callbook with the WSJT-X session; any cross-decoder hinting belongs to a
future explicit research phase.

Per-slot decode arrays (`allmessages`, `allsnrs`, `allfreq`, `ndecodes`) are
slot-local and reset before decode. They still drive duplicate suppression and
regular decode state during a slot.

AGC and time-window state belongs to the JTDX session state. The slot buffer is
AGC-adjusted once before decode passes, `forcedt` is retained when forced sync
is enabled, `avexdt` persists across slots, and `sync8` only reads
`lagcc` / `lagccbail` plus the pass-local `jzb/jzt` window derived from
`avexdt`. Do not reintroduce per-candidate or per-`sync8` copied AGC buffers.

JTDX references a small amount of FT4 module state for hide-test/telemetry
flags. Because ft8rs does not target FT4, this should be represented as minimal
JTDX-compatible FT8 state rather than importing an FT4 decoder.

## Major JTDX Differences

### Outer Decode

JTDX `ft8_decode.f90` carries controls such as:

- `nft8rxfsens`
- `ncandthin`
- `ndtcenter`
- `swl`
- `filter`
- `lft8lowth`
- `lft8subpass`
- `lhideft8dupes`
- `lhidehash`
- `numthreads`

These belong to `lib_jtdx`, not the WSJT-X path.

### Pass And Cycle Policy

JTDX derives pass count from cycle settings:

- cycle value 1 -> 3 passes
- cycle value 2 -> 6 passes
- cycle value 3 -> 9 passes

For ft8rs high-sensitivity JTDX, use cycle value 3.

### Sync Thresholds

JTDX base behavior:

- base `syncmin = 1.5`
- low-threshold or SWL mode changes thresholds by pass group
- pass 1/4/7 can use `1.225`
- pass 2/5/8 can use `1.5`
- pass 3/6/9 can use `1.1`

For high sensitivity, use JTDX sensitivity level 2 semantics:
`lft8lowth = true` and `lft8subpass = true`.

### Sync Candidate Generation

JTDX `sync8.f90` includes:

- larger candidate pools
- `ncandthin` based thinning
- `ndtcenter` based ranking
- wider SWL time window
- special lower threshold near `nfqso`
- looser near-duplicate time comparison than the classic WSJT-X path

JTDX changes sync metric by pass group:

- pass 1/4/7: amplitude
- pass 2/5/8: power
- pass 3/6/9: absolute real plus absolute imaginary

### Downsample

JTDX `ft8_downsample.f90` differs in:

- narrower/asymmetric frequency band around the candidate
- high-sensitivity edge scaling
- residual-aware FFT/cache invalidation near subtracted signals
- optional `c0` / `c2` / `c3` shifted data products

The Rust JTDX path now maintains the `freqsub` / `npos` / `lsubtracted`
surface expected by `ft8_downsample.f90`. The full residual-aware behavior must
still be source-audited before the JTDX profile is measured.

The JTDX `c2` / `c3` shifted downsample products are now used by `ft8b.rs`
when the current profile context is a focused QSO thread. The available Rust
state can infer the direct `nqso=2` QSO-frequency case and the large positive
or negative DT virtual retry cases from `nfqso`, `hiscall`, candidate frequency,
candidate DT, the retained `lastrxmsg` DT, and the parity-selected
`calldteven` / `calldtodd` call-DT rings. The `nlasttx=5` extension is only
enabled when the retained last receive message matches the current
`mycall hiscall RRR` thread, mirroring the JTDX source guard. The virtual retry
path still needs source-level validation against real JTDX output before
profile measurement.

### `ft8b`

JTDX `ft8b.f90` should be implemented in closed slices:

- regular sync/downsample/symbol extraction
- regular BP/OSD decode
- JTDX-owned pack/unpack/hash handling
- AP subpass matrix
- deep/superdeep paths
- false-positive filters
- subtract/state update

Do not treat the whole file as one indivisible task.

Current AP boundary:

- `NAPTYPES`, `NMYCNSAPTYPES`, `NDXNSAPTYPES`, and `NHAPTYPES` are held in
  `ft8_mod1.rs`;
- `ft8b.rs` currently selects the standard, my-call-nonstandard, or
  dx-call-nonstandard AP table and expands the `isubp2/iaptype` sequence;
- AP masks for `iaptype` `1`, `2`, `3`, `4`, `5`, `6`, `11`, `12`, `13`,
  `14`, `21`, `22`, `23`, `24`, `31`, `35`, `36`, `40`, `41`, `42`, `43`,
  `44`, and `111` are built by packing explicit JTDX-owned templates or fixed
  JTDX bit-pattern masks and masking only the known bit ranges;
- AP template packing rejects mismatched `i3` values so an invalid template does
  not silently fall back to free text and inject an unrelated AP mask;
- AP LLR source selection follows the JTDX `isubp2` table, including the
  repeated `llrb` selections for subpasses `10`, `13`, and `16`;
- hard sync gating now mirrors the source CQ-specific branches more closely:
  `lcqcand` with `nsync=4/5/6` uses the JTDX `nsync+nsync2` plus CQ-shape
  thresholds and sets the dynamic `lapcqonly` control flag, while non-CQ
  candidates still require `nsync>=7`;
- the source `syncdist.f90` rank-distribution check is represented as
  `lskipnotap`, so suspicious Costas rank distributions skip regular decode
  while still allowing AP/deep work as in `ft8b.f90`;
- the extended soft-sync gate from `ft8b.f90` is represented and is only
  applied for the same out-of-QSO or in-QSO `stophint` cases as the source;
- the first AP CPU-pruning signal ratios are represented: `scqnr` is derived
  from the JTDX `cwfilter.f90` `msgcq25(2)` tone row, `smycnr` is derived from
  the configured `MyCall` tone row, and AP types `1/2/3` use the source
  subpass thresholds before constructing masks;
- regular LLR source selection now follows the source `isubp1=1..2` and
  `isubp2=1..4` control flow for the forward `cs` and reverse `csr` metric
  arrays. `cscs` is available as saved forward data from `lreverse`, `csold`
  is available from the private signal-history cache, and AP/deep execution now
  runs inside the source-shaped nested `isubp1/isubp2` loop. The remaining gap
  is completing the source classifiers and pruning gates that are not yet
  represented in Rust;
- AP mask strength uses `max(abs(bmeta))*2.83*1.01`, matching the source
  expression `maxval(abs(llra))*1.01` after `llra=2.83*bmeta`;
- AP OSD fallback depth follows the source branch: default `ndeep=3`, selected
  QSO/MyCall/DXCall signal groups can raise to `ndeep=4`, Hound keeps
  `ndeep=3`, and `nagain` requests the source's `ndeep=5` path;
- the AP executor then runs BP and OSD with the AP mask and passes `iaptype`
  into the JTDX false-positive filters;
- OSD returns `dmin` with the decoded codeword so downstream JTDX-style quality
  and filter checks can account for both hard errors and OSD distance;
- hound special-message mask families `22` and `24` use the JTDX special
  message template `mycall RR73; mycall <hiscall> -16` and apply the same
  masked ranges as the Fortran `apsymsp` consumers.

Current subtract boundary:

- `ft8b.rs` derives `itone` from the accepted codeword;
- subtract DT is refined with the JTDX `noff=10` three-point peak estimate;
- `ft8v2/subtractft8.rs` mutates the JTDX `dd8` slot buffer and supports both
  normal and SWL filter widths;
- `freqsub`, `npos`, and `lsubtracted` are updated after subtract so subsequent
  candidates and passes can see the residual state.

Current false-positive boundary:

- the regular BP/OSD path applies JTDX-owned `chkfalse8`, `chkspecial8`,
  `chkgrid`, `chkflscall`, and `callsign_q` checks;
- additional regular-output protocol guards from the latter half of
  `ft8b.f90` are mirrored in `chkfalse8.rs`;
- false-positive context now carries JTDX-style `qual`, `xsnr`, and `rxdt`
  inputs so the source-level weak/out-of-window and AP-specific gates can be
  added without changing the decoder boundary again;
- regular FT8 false-positive filtering includes the JTDX-style `/R` standard
  message double-callsign check and the weak/out-of-window
  `<...> CALL GRID` hash-call grid/callsign check;
- `i3=1/2` messages containing ` R ` now follow the JTDX source split: when
  the second callsign is hash-presented, validate the first callsign through
  `chkflscall`; otherwise validate the second callsign and grid through
  `chkgrid`;
- AP false-positive filtering now separates the JTDX `iaptype=2` and
  `iaptype=40` branches instead of sharing one simplified checker:
  `iaptype=2` rejects syntactically four-character but invalid grids, while
  `iaptype=40` only validates a grid when the source branch recognizes one;
- regular `i3=1..3` messages containing ` R `, `/R `, or `/P ` now include
  the JTDX `call_q.f90` first-character/two-digit guard before the
  `chkflscall` database-backed pair check, with the same `mycall` /
  `hiscall` exemptions used by the source branch;
- regular non-`R` `i3=1/3` and Field Day-style pair filtering now also runs
  the source `call_q.f90` guard before `chkflscall`, while skipping first-call
  hash messages the same way as the Fortran `go to 2` branch;
- ARRL Field Day false-positive filtering now matches the JTDX SNR/DT gate:
  geographic section validation always runs, but the `call_q`/`chkflscall`
  pair rejection only runs for `xsnr < -19` or `rxdt` outside `[-0.5, 1.0]`;
- the `lcall1hash && i3=1` branch from `chkfalse8.f90` is now represented for
  primary false-check cases: if the second callsign is not the configured
  `hiscall` and is followed by a syntactically valid grid, the JTDX
  `callsign_q` and `chkgrid` checks can reject the row;
- `i3=4` nonstandard/hash-call false-positive filtering now mirrors the JTDX
  source checks for `<...>` placement: hash-adjacent callsigns with embedded
  spaces, no-slash trailing digits, trailing slash, bad single-slash
  eleven-character shapes, `callsign_q`, or `chklong8` are rejected;
- `TU;` RTTY-shaped messages stay FT8 false-positive filters only, but their
  callsign precheck now uses the same lightweight `call_q.f90` rules as JTDX
  instead of the stricter `callsign_q` rules;
- `CQ` `i3=4` nonstandard-call filtering now uses the full `CQ ` suffix for
  no-slash messages, rejecting embedded spaces and trailing digits like the
  JTDX source instead of checking only the second whitespace token;
- `chkfalse8.f90`-local filters for ordinary pair checks, `i3=4`
  hash/nonstandard calls, `TU;`, free text, and `CQ_` / `^` now run only under
  the JTDX primary false-check condition (`qual`, `xsnr`, `rxdt`, or AP type),
  reducing cases where ft8rs was stricter than the source on high-quality rows;
- `CQ` filtering is now split by source control flow: the full
  `chkfalse8.f90` CQ branch runs only under the primary false-check condition,
  while high-quality rows keep only the later directed-CQ/grid validation from
  `ft8b.f90`;
- AP/deep-specific rejection coverage has started with the JTDX `iaptype=35/36`
  DXCall-search weak/out-of-window first-callsign gate; remaining AP/deep
  rejection rules still need source audit.

Remaining filter caveats:

- JTDX `searchcalls` / `ALLCALL7.TXT` backed lookup is wired into
  `chkflscall`. Remaining filter work is source-auditing every AP/deep
  `chkflscall` call site and its surrounding classifier gates, not replacing
  the database lookup itself;
- FT8S / superdeep-specific bypass and rejection branches are part of the
  current normal-FT8 high-sensitivity milestone. The main `ft8s.f90` matcher is
  now represented as `ft8s.rs`, and `ft8sd1` / `ft8sd` previous-slot recovery
  branches plus `ft8mf1` / `ft8mfcq` memory-filter branches are present.
  `tonesd` virtual-candidate sync waveform support is wired for `iqso=4`, and
  `sync8d` now has the `csynce`, `csynccq`, `csyncsd`, and `csyncsdcq`
  template families. The main `csync` Costas template now also comes from the
  JTDX GFSK waveform seed. FT8S/FT8SD-specific false-decode gates are
  represented. Remaining work is source audit of the combined superdeep
  control flow before profile measurement;
- ARRL RTTY contest rewrite handling is not promoted as a target behavior in
  ft8rs yet, because this project remains focused on FT8 decode behavior.

Current source-audit triage notes:

- Valid gaps: `ft8b.f90` still has source branches around free-text,
  special-message presentation, enabled-DX-call search, and wide-DX-call search
  controls that are not fully represented in the Rust JTDX path. These are
  decoder-surface gaps, not short-fixture candidate-search explanations yet.
- Closed or false-positive items: `ft8mf1` acknowledgement row indexing,
  `ft8sd` / `ft8sd1` selected-tone storage, and the existence of
  `ft8mfcq.f90` have been checked against the JTDX source and should not be
  treated as active bugs.
- Partially closed structural items: `tone8`, `ft8apset`, and `ft8s` now use
  session-level precomputed tables. A later pass can still refine exact JTDX
  global/sentinel behavior if an observed miss points there.

Current follow-up closure:

- JTDX high-sensitivity config now carries explicit `lenabledxcsearch` and
  `lwidedxcsearch` flags. They default on for `profile=jtdx`, and AP/deep
  gate branches now check them around `iaptype > 30` DXCall-search paths
  instead of relying only on frequency-window side effects.
- Type 0.1 special-message handling now has a local `msgparser.rs` mirror.
  Regular/AP decoded results record `l_free_text`, `l_special`, and
  `msg37_2`; `chkfalse8` / `chkspecial8` receives the parsed secondary message
  when filtering special messages. The CLI still emits the primary row only;
  matching JTDX's two-row callback behavior for special messages remains a
  separate output-layer task.
- `ft8b` workspace now carries slot-local decoded CQ/MyCall memory matching
  the role of JTDX `tmpcqdec` / `tmpmyc`: accepted CQ/MyCall decodes are saved
  with frequency and raw `xdt`, and later CQ/MyCall signal-memory candidates at
  the same position are not added again. This closes the main cache-separation
  gap without yet rewriting the whole JTDX late-pass signal-save control flow.
- JTDX false-decode filtering now includes local mirrors for `chklong8.f90`
  and the deterministic text-shape portion of `filtersfree.f90`. `chkfalse8`
  now applies long-callsign rejection, free-text shape rejection, `CQ_` / `^`
  rejection, `TU;` call-pair validation, and the source-shaped
  `iaptype=3/11/21/41` focused-QSO grid check.
- `filtersfree.rs` preserves the local JTDX source's unreachable
  `decoded(12:12)` mixed letter/digit branch instead of applying a
  typo-corrected interpretation, so ft8rs does not add a false-decode filter
  that the current JTDX source does not execute.
- `filtersfree.rs` intentionally does not yet apply the final
  `datacor(datapwr)` correlation gate because the current Rust filter boundary
  does not carry JTDX's `datapwr` state. That is a remaining source-fidelity
  item for the free-text path.
- `chkgrid.rs` remains the largest false-positive filter simplification:
  current Rust has basic grid format and a small set of obvious call/grid
  checks, while JTDX `chkgrid.f90` is a large geographic rule table. This
  should be migrated mechanically or table-driven rather than rewritten by
  hand.
- `tonesd.rs` now centralizes both the superdeep sync templates and the
  76-entry report/grid candidate table. That closes the earlier consumer-local
  table-generation gap; the remaining FT8S source-shape work is now mainly in
  the larger `chkgrid.f90` geography table and the free-text `datapwr`
  correlation gate.

## Baseline Rule

For `profile=jtdx`, the CSV baseline should use:

```text
Extra is empty or J
```

Initial JTDX tests are informational:

- release mode only
- timeout rules still apply
- no fixed decode-count target at first
- record observed count as the initial JTDX profile baseline

Current closure note:

- `profile=jtdx` currently runs only the native JTDX path. The short fixture
  now emits accepted native rows, so JTDX can be measured independently from
  the protected WSJT-X path. The current short-fixture checkpoint is 20/21 on
  `210703_133430.wav`; `K1JT HA5WA 73` was recovered by matching JTDX
  sensitivity level 2 semantics (`lft8lowth=true`, `lft8subpass=true`).
- The remaining short-fixture miss is `CQ DX DL8YHR JO41` near 2606 Hz.
  It reaches `ft8b` with a strong Costas gate (`nsync=14`) but regular
  BP/OSD does not recover a valid codeword. Temporary diagnostics showed that
  disabling JTDX subtract reduced the short result to 19, and raising regular
  OSD depth from 3 to 4 did not recover the message. Keep investigating regular
  metric/LDPC numerical equivalence rather than treating it as a candidate
  search problem.
- Large JTDX source mirrors are directory modules when a direct `.rs` mirror
  would exceed the 1000-line maintenance limit. `ft8b.f90` maps to
  `ft8b/`, and `packjt77.f90` maps to `ft8v2/packjt77/`; each child file must
  stay under the line limit while preserving source-shaped names and flow.
- `lib_jtdx` now owns its `four2a` FFT wrapper module and the JTDX decode
  files no longer call the shared top-level FFT wrappers. This keeps the
  decoder dependency boundary cleaner for profile-level review and later
  hybrid execution.
- JTDX `ft8b.f90` resets `syncavemax=3.` inside the outer regular/AP subpass
  loop. The Rust JTDX path now mirrors that behavior so the later
  `syncavemax < 1.8/1.9` guards do not use the earlier measured value.
- Remaining FFT follow-up: the local JTDX FFTW backend currently uses the
  default planning flag internally. If JTDX FFTW tuning becomes a target, wire
  profile-local thread/patience configuration into `lib_jtdx::four2a` without
  reintroducing shared decoder internals.

## Implementation Order

1. Add the JTDX skeleton files.
2. Add `ft8_mod1.rs` and `ft8_params.rs`.
3. Add JTDX profile dispatch.
4. Implement JTDX outer decode policy.
5. Implement JTDX `sync8`.
6. Implement JTDX downsample.
7. Implement JTDX `ft8b` regular decode.
8. Implement JTDX-owned `ft8v2` support.
9. Add JTDX AP/deep decode.
10. Add JTDX false-positive filters.

## Do Not Do Yet

- Do not reuse WSJT-X `ft8b`, `sync8`, `ft8_downsample`, `packjt77`,
  `subtractft8`, or AP internals.
- Do not reintroduce profile-level fallback to the protected WSJT-X decoder.
- Do not extract shared decoder internals.
- Do not enable SWL by default.
- Do not expand SWL-only behavior in this normal-FT8 milestone.
- Do not describe incomplete JTDX skeleton output as JTDX-aligned.
