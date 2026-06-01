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
src/decode/lib_jtdx/ft8b.rs
src/decode/lib_jtdx/ft8apset.rs
src/decode/lib_jtdx/sync8.rs
src/decode/lib_jtdx/sync8d.rs
src/decode/lib_jtdx/ft8_downsample.rs
src/decode/lib_jtdx/ft8_mod1.rs
src/decode/lib_jtdx/ft8_params.rs
src/decode/lib_jtdx/agccft8.rs
src/decode/lib_jtdx/partintft8.rs

src/decode/lib_jtdx/ft8v2/bpdecode174_91.rs
src/decode/lib_jtdx/ft8v2/chkcrc14a.rs
src/decode/lib_jtdx/ft8v2/encode174_91.rs
src/decode/lib_jtdx/ft8v2/ldpc_174_91_c_generator.rs
src/decode/lib_jtdx/ft8v2/ldpc_174_91_c_reordered_parity.rs
src/decode/lib_jtdx/ft8v2/osd174_91.rs
src/decode/lib_jtdx/ft8v2/packjt77.rs
src/decode/lib_jtdx/ft8v2/packjt77sd.rs
src/decode/lib_jtdx/ft8v2/subtractft8.rs

src/decode/lib_jtdx/syncdist.rs
src/decode/lib_jtdx/callsign_q.rs
src/decode/lib_jtdx/chkfalse8.rs
src/decode/lib_jtdx/chkspecial8.rs
src/decode/lib_jtdx/chkgrid.rs
src/decode/lib_jtdx/chkflscall.rs
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
- AP mask magnitude follows `ft8b.f90`: `apmag` is derived from
  `maxval(abs(llra))*1.01`, where `llra` already includes the source `2.83`
  LLR scale factor;
- decoded-row SNR now follows the JTDX `ft8b.f90` source formula: it uses the
  accepted `itone` sequence to accumulate signal/noise ratios over all 79
  symbols, applies the high-SNR and low-SNR nonlinear corrections, then clamps
  regular decodes at `-23 dB` and AP/deep decodes at `-24 dB`;
- AP/deep SNR estimation also keeps the source `iaptype>4` post-correction:
  very weak AP/deep rows fall back to `xsnrs-1.0` and clamp at `-26 dB`;
- false-positive filter quality now keeps the OSD `dmin` contribution and uses
  the source expression `qual=1.0-(nharderrors+dmin)/60.0`;
- `--hound` selects the JTDX `nhaptypes` AP table; hound AP types `21`, `22`,
  `23`, `24`, and fixed-bit type `111` are wired. Types `22` and `24` use the
  JTDX type-0.1 special-message template that feeds `apsymsp` in the Fortran
  source;
- `twkfreq1.rs` mirrors JTDX `lib/twkfreq1.f90`. The native JTDX `ft8b`
  branch now applies the `-delfbest` frequency tweak to the downsampled complex
  buffer before symbol extraction, matching the source order after the fine
  frequency search;
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
  `ft8b` has an initial conservative FT8S fallback for configured
  `mycall/hiscall` QSO messages near the QSO frequency after regular/AP
  BP+OSD attempts fail;
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
  rejection, type-4 `/R` plus grid rejection, and ARRL Field Day region/DX
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
  missing source-complete nonstandard-DX search classifiers, the JTDX `s256`
  CQ classifier branch, Hound fox-report/RR73 classifiers, and several
  source-specific CPU-pruning gates;
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

For high sensitivity, use `lft8lowth = true`.

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
- regular LLR source selection now follows the source `isubp1=1..2` and
  `isubp2=1..4` control flow for the forward `cs` and reverse `csr` metric
  arrays. `cscs` is available as saved forward data from `lreverse`, `csold`
  is available from the private signal-history cache, and AP/deep execution now
  runs inside the source-shaped nested `isubp1/isubp2` loop. The remaining gap
  is completing the source classifiers and pruning gates that are not yet
  represented in Rust;
- AP mask strength uses `max(abs(bmeta))*2.83*1.01`, matching the source
  expression `maxval(abs(llra))*1.01` after `llra=2.83*bmeta`;
- AP OSD fallback depth follows the source default branch: `ndeep=3` unless
  `nagain` filtering requests the source's `ndeep=5` path. Signal-classified
  raises to `ndeep=4` for QSO/MyCall/DXCall signal groups still require the
  corresponding JTDX signal classifiers before they can be enabled safely;
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
- AP/deep-specific rejection coverage has started with the JTDX `iaptype=35/36`
  DXCall-search weak/out-of-window first-callsign gate; remaining AP/deep
  rejection rules still need source audit.

Remaining filter caveats:

- JTDX `searchcalls` / `ALLCALL7.TXT` backed filters are not fully modeled yet;
- FT8S / superdeep-specific bypass and rejection branches are part of the
  current normal-FT8 high-sensitivity milestone. The first FT8S candidate
  matcher exists, but source-complete `ft8s`, `ft8sd`, `ft8sd1`, `ft8mf1`,
  `ft8mfcq`, and `tonesd` behavior is still incomplete;
- ARRL RTTY contest rewrite handling is not promoted as a target behavior in
  ft8rs yet, because this project remains focused on FT8 decode behavior.

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

- `profile=jtdx` currently runs only the native JTDX path. If the native path
  emits zero rows, the profile emits zero rows.
- The native JTDX blocker is in `ft8b` after sync candidate generation:
  candidates are present, and accepted native rows still need to be verified
  after the latest symbol-index and metric-source repairs.

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
