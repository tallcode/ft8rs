# JTDX Profile Notes

This document records the JTDX-oriented FT8 decoder profile only. WSJT-X
alignment is documented in `WSJTX.md`; result-union behavior is documented in
`HYBRID.md`.

## Scope

`profile=jtdx` is a separate decoder path under:

```text
src/decode/lib_jtdx/
```

It is not a parameter overlay on the WSJT-X decoder. The protected WSJT-X path
must remain independent. JTDX code may reuse a utility only when the behavior is
identical; otherwise the JTDX-specific implementation stays under `lib_jtdx`.

The profile targets normal high-sensitivity FT8 first. SWL, Hound, and other
optional modes exist as explicit options but are not the current alignment
baseline.

## Runtime Data

`ALLCALL7.TXT` is part of the JTDX runtime behavior. It feeds `searchcalls` and
therefore affects false-decode filtering and slot-local subtraction.

For source alignment, the repository copy is pinned to:

```text
jtdx/contrib/CallDB/ALLCALL7.TXT
```

Release packages include `ALLCALL7.TXT` next to the binary. Local builds copy
the root file into the target binary directory. The runtime lookup order is:

1. executable directory
2. current working directory
3. Cargo manifest directory

## Source Shape

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
four2a wrapper       -> four2a.rs
chkfalse8.f90        -> chkfalse8.rs   (name map only; see provenance note)
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

Large source mirrors may be split into directory modules to keep Rust compile
units manageable. The split should preserve a clear one-source-file ownership
boundary.

## State Model

JTDX `ft8_mod1.f90` is mutable decoder state, not just constants. The Rust
profile keeps this state private to a JTDX session:

- slot audio and AGC state;
- decoded-message arrays and duplicate state;
- odd/even interval memory;
- `lastrxmsg`, `lasthcall`, `incall`, and call-to-DT rings;
- CQ/MyCall/QSO signal memory;
- AP, tone, Costas, FT8S, and superdeep tables;
- JTDX hash callbook;
- `mycall`, `hiscall`, `hisgrid`, `nfqso`, `avexdt`, and forced-sync state.

Hybrid must not share this state with WSJT-X or another JTDX worker. It should
consume decoded rows only.

## Defaults

The profile uses high-sensitivity JTDX-style defaults:

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

Frequency-dependent `napwid` follows JTDX:

```text
freq < 30 MHz  -> napwid = 5
freq < 100 MHz -> napwid = 15
else           -> napwid = 50
```

`--jtdx-threads 0` keeps JTDX-style automatic band splitting. Explicit
`1..=24` values are useful for diagnostics, but `1` is not equivalent to JTDX's
usual GUI/source execution shape.

## Current Baseline

JTDX fixture rows use `Extra` markers as follows:

```text
blank -> target row
J     -> JTDX target row
W     -> WSJT-X-only reference row, not a JTDX target
E     -> excluded row
```

Current release-mode observations:

```text
210703_133430.wav -> 20/20 JTDX target rows
230208_140300.wav -> 430/431 JTDX target rows with --jtdx-threads 0
```

The remaining no-context JTDX miss is:

```text
230208_140700,-16,1.7,1153,F1MLZ UA3QNA -04,J
```

Supplying QSO context recovers it:

```bash
ft8rs file tests/ft8/230208_140300.wav \
  --start-time 230208_140300 \
  --profile jtdx \
  --my-call F1MLZ \
  --rx-frequency 1153
```

Focused diagnostics showed the `140700` recovery as `source=Regular iaptype=2`.
That means it is a MyCall/AP-assisted regular-path decode, not a pure
`iaptype=0` no-context regular decode. Do not recover it by relaxing
false-decode filters.

`140715 OH5NBJ SV1MRW KM17` is recovered by source-style auto threads and by
explicit `--jtdx-threads 4` or `8`. It is missed only by the single-thread
diagnostic run.

## Important Alignment Notes

- `ft8b` keeps JTDX AP/deep work inside the source-shaped `isubp1` metric loop.
- `sync8` candidate ordering uses the local JTDX `indexx` mirror rather than a
  direct Rust sort.
- `ft8_downsample` keeps JTDX residual and `freqsub/npos/lsubtracted` behavior
  workspace-local.
- `ft8b` preserves `lhashmsg` through `delbraces`, so hash-braced messages do
  not enter odd/even AP memory by accident.
- FT8S/FT8SD accepted messages follow their source-specific false-decode
  boundary instead of the broad regular/AP `chkfalse8` path.
- `chkgrid` remains intentionally partial. Full JTDX callsign-to-grid geography
  validation is mostly false-positive reduction and is deferred unless it
  blocks a real target.
- `chkfalse8.rs` **does not match the pinned `chkfalse8.f90`** and cannot be
  byte-verified against it (verified 2026-07-12). The Rust filter adds
  `msg37_2`/`lcall2hash` args and a `FilterContext { quality, xsnr, rxdt }` with
  a `primary_false_check` gate (`quality<0.39 || xsnr<-20.5 || rxdt<-0.5 ||
  rxdt>1.9 || iaptype∈{1,2,3,11,21,40,41}`); the pinned Fortran is the 6-arg
  `chkfalse8(msg37,i3,n3,nbadcrc,iaptype,lcall1hash)` with no quality/SNR/DT
  gating. That newer form is absent from **every** ref of the checked-out JTDX
  tree (HEAD 2022-03-01 is the newest; `git log --all -S quality`/`-S msg37_2`
  on `lib/chkfalse8.f90` finds nothing), so this file targets either a
  post-2022 JTDX release not in the checkout or a local enhancement on the 2022
  logic — no longer remembered, so no target version can be cited. It is a
  false-decode filter (only rejects, never adds decodes); the 20/20 & 430/431
  baselines are green with it as-is. **Do not** revert it to the 6-arg version
  to "restore alignment": that drops the gating the baselines are calibrated
  against and can shift decodes. If the upstream source is ever identified, pin
  that `chkfalse8.f90` to make this file diff-verifiable again. Full detail is in
  the `chkfalse8.rs` header.
- `ALLCALL7.TXT` version changes can change output. The pinned source-tree
  copy recovered RA3ABG-related extra rows but did not change the remaining
  no-context miss.

## Remaining Work

- Continue AP/deep per-`iaptype` gate audits as concrete misses or false
  positives identify relevant branches.
- Keep OSD/BP numerical details source-shaped; avoid mathematically equivalent
  rewrites when the source order can be mirrored.
- Validate Hound and SWL only when they become explicit targets.
- Keep temporary traces and experiment outputs out of commits. Durable findings
  belong in this document or in targeted tests.
