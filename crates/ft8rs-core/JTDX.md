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

The public decoder input is normalized 12 kHz floating-point audio. Convert it
to signed 16-bit PCM amplitude (`sample * 32768`) once when filling `dd8`,
matching the `integer*2 -> real` assignment in JTDX `jt9a.f90`. Keep the original
AGC, nonlinear soft-metric, and SNR noise-floor constants in their native scale;
do not compensate for normalized input by weakening decoder/filter thresholds.

AGC compensation remains an independent receiver option (`nagcc`), disabled by
default like JTDX's `AGCcompensation` setting. High-sensitivity profile setup
preserves the caller's choice instead of forcing it on. When explicitly enabled,
the original `agccft8` processing, including its tail handling, applies.

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
nagcc = false (preserve the caller's explicit setting)
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

`--jtdx-threads 0` selects automatic band splitting. The current Rust session
processes these logical bands serially; the value does not spawn that number of
OS threads. Automatic selection depends on visible cores: 12 cores select 9
bands. Band count changes search and state evolution, so it is part of the decode
configuration rather than a pure speed setting.

## Calibrated Baseline

JTDX fixture rows use `Extra` markers as follows:

```text
blank -> target row
J     -> JTDX target row
W     -> WSJT-X-only reference row, not a JTDX target
E     -> excluded row
```

The long-fixture acceptance point is fixed:

```text
fixture          = 230208_140300.wav, all 19 slots in one reused session
CallDB           = ALLCALL7.TXT.2022
frequency range  = 200..3000 Hz
jtdx_threads     = 4 logical bands
swl              = false
nagcc            = false
user QSO context = none
accepted floor   = 430/431 target rows
```

The constructor retains its existing high-sensitivity mapping. The four-band
setting belongs to calibration; production defaults remain automatic. Automatic
mode must record the actual core/band count when comparing results, because
`threads=0` is not a fixed operating point across machines. Do not lower the
430/431 threshold or alter aligned decoder code to compensate for a different
partition setting.

Run the calibrated local gate with the private-database helper:

```bash
./scripts/acceptance-gates.sh test_jtdx_profile_long_audio
```

The short fixture retains its existing 20/20 target floor. The remaining
long-fixture no-user-context miss is:

```text
230208_140700,-16,1.7,1153,F1MLZ UA3QNA -04,J
```

QSO-assisted recovery is a separate operating point and must not be counted as a
no-user-context baseline success. Preserve the native distinction between a
regular algorithm path and whether AP assistance was used.

## Important Alignment Notes

- JTDX's Field Day sections and RTTY multipliers follow the pinned reference
  `packjt77.f90`: 85 sections (`GTA`, `MAR`, `NT`) and 65 multipliers. Keep
  these separate from the newer WSJT-X tables (86/171), so the same payload
  bits retain the original JTDX meaning.
- `ft8b` keeps JTDX AP/deep work inside the source-shaped `isubp1` metric loop.
- `sync8` candidate ordering uses the local JTDX `indexx` mirror rather than a
  direct Rust sort.
- `ft8_downsample` keeps JTDX residual and `freqsub/npos/lsubtracted` behavior
  workspace-local.
- `ft8b` preserves `lhashmsg` through `delbraces`, so hash-braced messages do
  not enter odd/even AP memory by accident.
- FT8S/FT8SD accepted messages follow their source-specific false-decode
  boundary instead of the broad regular/AP `chkfalse8` path.
- `chkgrid` remains intentionally partial; the complete geography port is
  explicitly out of scope under the review decision below.
- `chkfalse8.rs` groups checks from both `chkfalse8.f90` and the caller
  `ft8b.f90`. The `quality<0.39`, `xsnr<-20.5`, DT, and AP-type gates are in
  the pinned **caller** before it invokes the six-argument `chkfalse8`.
  `msg37_2` and `lcall2hash` are caller-side state too. The earlier claim that
  these gates came from an unidentified post-2022 source was incorrect: it
  compared only the callee. Keep the existing gates and values; do not remove
  them merely to match the Fortran callee's argument list. This Rust grouping
  is not a byte-for-byte equivalence claim for every helper.
- `ALLCALL7.TXT` version changes can change output. The pinned source-tree
  copy recovered RA3ABG-related extra rows but did not change the remaining
  no-context miss.

## Deferred Full `chkgrid` Port — Review Decision 2026-10-27

The user explicitly accepts some false positives and requested that a large,
complex full port be abandoned and documented. The pinned JTDX source
`2a0e2bea8c66c9ca94d2ea8034cf83a68cfa40eb`, `lib/chkgrid.f90`, contains
3,392 physical lines, 997 `if`/`else if` conditions, two forward jumps, and
20 early returns in addition to the final return. Its country/prefix-specific
allowed grids, geographic exclusions, invalid-call rules, and special-station
exceptions require a substantial independent callsign/grid validation corpus.

The full geography port was therefore abandoned for this review. Keep the
existing grid syntax and coarse area checks. The current implementation does
not use the callsign to validate geography and leaves `lwrongcall=false`, so
some implausible callsign/grid pairs can survive; this is an accepted false-
positive tradeoff, not a claim of complete JTDX parity. Do not compensate by
adding stricter ad hoc filters, changing decoder constants, or sacrificing
weak-signal recovery. Revisit the full port only at the user's request.

For source checks with a local reference checkout, run
`FT8RS_REQUIRE_JTDX_SOURCE=1 cargo test --release -p ft8rs-core --test jtdx_source_audit_test`.
Use `FT8RS_JTDX_SOURCE_DIR` to explicitly select its repository root. The audit
checks both protocol tables and the caller-side filter-gate provenance.

## Remaining Work

- Continue AP/deep per-`iaptype` gate audits as concrete misses or false
  positives identify relevant branches.
- Keep OSD/BP numerical details source-shaped; avoid mathematically equivalent
  rewrites when the source order can be mirrored.
- Validate Hound and SWL only when they become explicit targets.
- Keep temporary traces and experiment outputs out of commits. Durable findings
  belong in this document or in targeted tests.
