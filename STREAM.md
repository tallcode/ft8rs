# STREAM — FT8 streaming decode WSJT-X alignment notes

## Scope

This document tracks the decoder architecture required for `ft8rs` to behave like
WSJT-X FT8 receive code. It is not a performance wish list. When in doubt, align
with WSJT-X first, then optimize without changing observable decode flow.

Primary WSJT-X files reviewed:

- `wsjtx/lib/jt9a.f90`
- `wsjtx/lib/decoder.f90`
- `wsjtx/lib/ft8_decode.f90`
- `wsjtx/lib/ft8/ft8b.f90`
- `wsjtx/lib/ft8/sync8.f90`
- `wsjtx/lib/ft8/ft8_a7.f90`
- `wsjtx/lib/ft8/ft8_downsample.f90`
- `wsjtx/lib/ft8/get_spectrum_baseline.f90`
- `wsjtx/lib/ft8/subtractft8.f90`
- `wsjtx/lib/77bit/packjt77.f90`

## WSJT-X Audio Model

| Item | WSJT-X value | Notes |
|---|---:|---|
| Internal sample rate | 12000 Hz | Soundcard input is converted before decoder use. |
| Sample type at FT8 decoder boundary | `integer*2 iwave(15*12000)` | Decoder works on 15 s windows after shared-memory/audio handling. |
| FT8 frame window | 15 s | `NPTS = NMAX = 15*12000 = 180000`. |
| FT8 symbol samples | `NSPS=1920` | 6.25 baud, 0.160 s per symbol. |
| Sync step | `NSTEP=NSPS/4=480` | 0.040 s lag grid. |
| Downsample | `NDOWN=60` | 12000 Hz -> 200 Hz, 32 samples/symbol. |
| Sync FFT | `NFFT1=3840` | 3.125 Hz bins, exactly 2 bins per FT8 tone. |
| Downsample FFT | `NFFT1_LONG=192000`, `NFFT2=3200` | 16 s zero-padded long FFT, then 200 Hz baseband. |
| Default decode band | 200-3000 Hz | WSJT-X clamps spectrum baseline to 100-4910 Hz. |

`ft8rs` currently accepts `f32` samples and normalizes WAV input. The independent
decoder should expose a stable 12 kHz stream API and keep file/soundcard I/O
outside the decode module. For WSJT-X parity, the decoder core should treat each
FT8 analysis window as a 180000-sample 12 kHz buffer, with explicit zero padding
when simulating partial `nzhsym` passes.

## FFT Engine Policy

Current decision: keep both FFT engines, with different responsibilities.

- `FFTW @ 3840` is the WSJT-X alignment and acceptance path.
- `RustFFT @ 4096` is retained as a portable fallback and comparison path.

Rationale:

- WSJT-X `sync8` uses `NFFT1=3840`, so `12000/3840 = 3.125 Hz/bin`.
- FT8 tone spacing is 6.25 Hz, exactly 2 FFT bins at 3840.
- With `RustFFT @ 4096`, `12000/4096 = 2.9296875 Hz/bin`, so one FT8
  tone spacing is about 2.1333 bins. This changes the sync grid, candidate
  ordering, `sbase`, subtraction residuals, and low-SNR edge behavior.
- Therefore `RustFFT @ 4096` may be useful, and may sometimes decode similar
  counts, but it is not a strict WSJT-X parity path.

Operational policy:

- Final WSJT-X parity claims and release acceptance tests must use FFTW at
  3840.
- Stream acceptance tests now assert release mode at runtime, so accidental
  debug `cargo test` runs fail immediately instead of producing meaningless
  timing data.
- Candidate/sbase/subtraction/AP numerical comparisons against WSJT-X should
  use FFTW at 3840 only.
- RustFFT at 4096 remains useful for no-FFTW builds, smoke tests, and diagnosing
  whether a difference is caused by FFT sizing or by decoder logic.
- RustFFT should not be removed, but its results should not be used to relax or
  reinterpret WSJT-X alignment requirements.

Possible future direction:

- Add a `RustFFT @ 3840` mode after the WSJT-X-aligned FFTW path stabilizes.
- If RustFFT at 3840 matches candidate ordering, `sbase`, and decode results
  closely enough, it can become a no-FFTW aligned engine.
- Until that evidence exists, only FFTW at 3840 is the alignment baseline.

## WSJT-X Streaming / Disk Control Flow

### Source entry points

- GUI live receive sets `params.nzhsym` from half-symbol progress in
  `widgets/mainwindow.cpp`.
- Disk-file FT8 decode in `jt9a.f90` explicitly runs partial passes before the
  final pass:
  - non-multithread disk mode: `nzhsym=41`, then `47`, then `50`
  - multithread disk mode can use `41/46/49` or `41/46/50`, depending on
    `ndecoderstart`; this project should first align the ordinary `41/47/50`
    path because it matches the requested stream decoder behavior.

### `ft8_decode.f90` state machine

Important saved state:

- `dd`, `dd1`: current and early-subtracted audio buffers.
- `ndec_early`: number of decodes from partial passes.
- `itone_save`, `f1_save`, `xdt_save`, `allmessages`: early/full decode memory.
- `ft8_a7` module state: `dt0`, `f0`, `msg0`, `ndec(jseq,k)`.

WSJT-X behavior by `nzhsym`:

| `nzhsym` | Input in `jt9a.f90` | `ft8_decode.f90` behavior |
|---:|---|---|
| 41 | first `41*3456` samples copied, rest zero | reset per-slot tables, decode partial buffer, save `ndec_early` |
| 47 | first `47*3456` samples copied, rest zero | subtract early decodes with `xdt_save(i)-0.5 < 0.396`, save `dd1` |
| 50 | full 15 s window | copy cleaned `dd1(1:47*3456)` over early part, append original remainder, subtract remaining early decodes, run full decode, then AP |

The important source-level boundary is `nzhsym * 3456` samples in the WSJT-X
shared decode buffer. For example, `41*3456 = 141696` samples and
`47*3456 = 162432` samples. The Rust stream decoder should therefore keep a
180000-sample decode window and zero everything after `nzhsym*3456` for
`nzhsym<50`.

## `sync8` Alignment Details

WSJT-X `sync8.f90`:

- Computes `s(i,j)` using power spectrum, `real(cx(i))**2 + aimag(cx(i))**2`.
- Uses `NFFT1=3840`, `df=3.125 Hz`, `JZ=62`, `jstrt=0.5/tstep`.
- Uses Costas correlation for full `abc` and late `bc` sync blocks.
- Uses `mlag=13` for `red/jpeak` and `mlag2=JZ` for `red2/jpeak2`.
- Normalizes `red` and `red2` by the 40th percentile over frequency bins.
- Builds pre-candidates by descending `red`, with optional `red2` second peak
  if the peak lag differs.
- Near-dupe pruning is in candidate order: if `df<4 Hz` and `dt<0.04 s`, keep
  only the stronger sync candidate.
- Prioritizes candidates within 10 Hz of `nfqso`, then appends remaining
  candidates in descending sync order.
- Returns `sbase` from `get_spectrum_baseline(dd,nfa,nfb,sbase)`, not from the
  symbol spectra used for sync.

Current `ft8rs` status after Iteration 12:

- `mlag=13` is now aligned.
- `DecodeOptions` now includes `nfqso`, so the "QSO frequency first" ordering
  can be represented.
- Stream test defaults no longer override `max_candidates` below the WSJT-X
  `MAXCAND=1000` alignment value.
- Candidate construction now follows the WSJT-X structure more closely:
  descending `red`, optional `red2`, near-dupe zeroing, `nfqso +/- 10 Hz`
  priority, then remaining candidates by sync.
- The final candidate pruning/ordering is now isolated in a small helper with a
  unit fixture for the WSJT-X order rule: near-dupe pruning, `nfqso +/- 10 Hz`
  priority, then descending sync.
- `sbase` now comes from `get_spectrum_baseline(dd,nfa,nfb)`.
- Remaining risk: the Rust `baseline` polynomial helper still needs a numerical
  parity check against WSJT-X `baseline.f90`/`polyfit` on the reference files.
- `sbase` indexing now follows the WSJT-X 1-based convention: FFT bin 0/DC is
  omitted and Vec index 0 is unused, so `sbase[nint(f/3.125)]` maps to the same
  bin as Fortran `sbase(nint(f/3.125))`.
- The Rust `pctile` equivalent now uses Fortran `nint(npts*0.01*npct)` ranking
  instead of a ceil percentile, and the lower-envelope sample set follows
  WSJT-X's fixed 1000-point cap.
- `SyncMode::Amplitude` and `SyncMode::AbsSum` remain non-WSJT-X comparison
  modes. Default alignment mode is `Power`.

## `ft8_decode` / `ft8b` Alignment Details

WSJT-X outer decode:

```text
npass = 3                # or 2 at depth 1
for ipass in 1..npass:
  newdat = true
  syncmin = 1.3          # depth 3
  imetric = 1 on pass 1
  imetric = 2 on pass 2 and pass 3
  if ipass == 3 and ndecodes == 0: cycle
  sync8(dd,...)
  for candidates in order:
    ft8b(dd, newdat, ..., imetric, ...)
    if valid and unique:
      save result
      ft8b subtracts from dd when lsubtract=true
      ft8_a7_save(jseq, xdt, f1, msg37)
```

WSJT-X `ft8b.f90` inner decode:

- Downsample, refine time `+/-10`, refine frequency `+/-2.5 Hz`, downsample
  again, refine time `+/-4`.
- Hard sync gate:
  - `syncmin=6`, or `7` when `imetric=2`, or `8` when `ndepth<=2`
  - bail out when `nsync <= syncmin`
- Builds five regular LLR metric streams:
  - `llra`: `nsym=1`
  - `llrb`: `nsym=2`
  - `llrc`: `nsym=3`
  - `llrd`: bit-by-bit normalized `nsym=1`
  - `llre`: per-bit strongest of `bmeta/bmetb/bmetc`
- When `imetric=2`, the temporary `s2` metric is squared before bit metrics.
- Regular passes are 1..5.
- If AP is enabled, extra AP passes start at pass 6 and use `nappasses` and
  `naptypes` keyed by `nQSOProgress`.
- `nzhsym<50` disables AP passes inside `ft8b`.
- `decode174_91` uses `maxosd=2` for depth 2/3; depth 1 is BP only.
- SNR uses `xsnr2` when `nagain=false`; false positive filter is
  `nsync <= 10 && xsnr < -25.0`.
- `decode174_91` now distinguishes WSJT-X `maxosd=0` channel-LLR OSD from
  `maxosd>0` BP-posterior OSD, caps `maxosd` at 3, removes the non-WSJT-X raw
  fallback after BP-posterior OSD, computes BP-success `dmin`, and only accepts
  OSD results when `nharderrors > 0` like WSJT-X.
- The Rust OSD path now implements the WSJT-X `ndeep=2` first preprocessing
  search shape used by `ft8b` (`nord=1`, `npre1=1`, `nt=40`, `ntheta=10`) and
  uses `nextpat91`-style pattern iteration instead of the older ad-hoc last-64
  order-2 brute force.

Current `ft8rs` status after Iteration 12:

- Outer decode now uses WSJT-X pass count: 2 passes at depth 1, otherwise 3.
- Pass 1 uses `imetric=1`; passes 2/3 use `imetric=2`.
- `imetric=2` now squares `s2` before bit metric extraction.
- `bmete` has been added and the regular pass set is now
  `llra/llrb/llrc/llrd/llre`.
- The previous pass-1 depth override has been removed.
- The SNR false-positive gate/clamp now uses `-25 dB`.
- Ad-hoc AP masks have been removed from the default regular decode path.
- Iteration 16 added the first WSJT-X-style internal AP pass scheduler:
  - `nappasses=(2,2,2,4,4,3)`
  - `naptypes` table for QSO progress 0-5
  - AP pass metric alternation `llra/llrc`
  - AP pass count is controlled by `lft8apon`, `lapcqonly`, `ncontest`, and
    `nzhsym>=50`
  - Initial default non-contest iaptype 1-6 masks were generated from real
    `pack77` bit patterns instead of hand-coded partial masks.
- Iteration 17 exposed WSJT-X AP/QSO parameters through `StreamDecodeConfig`:
  `nfqso`, `nftx`, `nQSOProgress`, `ncontest`, `napwid`, `lft8apon`,
  `lapcqonly`, `nagain`, `mycall`, and `hiscall`.
- Iteration 18 replaced the provisional AP bit generation with a closer port of
  WSJT-X `ft8apset.f90` + `ft8b.f90` AP mask branches:
  - `apsym(1:58)` is now derived from `pack77(mycall hiscall RRR)` with the
    same dummy-hiscall and standard/nonstandard-call gates.
  - `aph10` is now generated from the WSJT-X 10-bit callsign hash for Hound AP.
  - `iaptype` 1-6 now apply the contest-specific CQ/MyCall/MyCall+DxCall/tail
    masks for `ncontest` 0-5, 7, and 8, while `ncontest=6` remains disabled.
  - `ndepth=2` now uses `maxosd=2`, matching the active WSJT-X code path.
- Remaining gaps:
  - `nagain` is now consumed for the WSJT-X `nfqso +/- 20 Hz` search window and
    adjacent-tone SNR selection.
  - OSD `ndeep=2` now follows the first WSJT-X preprocessing rule, but `npre2`
    and deeper `ndeep>=3` branches are not yet fully ported.
  - AP masks now follow the WSJT-X branch structure, but still need bit-level
    regression checks against WSJT-X for contest and Hound examples.
  - `nzhsym` is represented at the stream orchestration level and gates internal
    AP, but long-decode slot timing still needs reconciliation with WSJT-X.

## AP / Cross-slot Memory

WSJT-X `ft8_a7`:

- `ft8_a7_save` stores current decoded fragments by even/odd `jseq`.
- On a new UTC or `nzhsym=41`, previous current entries are moved from `k=1`
  to `k=0` for that parity.
- AP at `nzhsym=50` uses only previous entries for the same parity.
- Entries containing `/` or `<` are skipped.
- If a current decode has the same second call and near frequency as a previous
  AP candidate, the previous entry is flagged `f0=-98` and skipped.
- `ft8_a7d` brute-forces 206 message variants and accepts only when
  `dmin <= 100` and `dmin2/dmin >= 1.3`, with extra CQ/grid guards.

Current `ft8rs` status after Iteration 13:

- `prev_even/prev_odd` still carry the same-parity previous entries, matching
  the intent of `ndec(jseq,0)`.
- Current regular decodes are now extracted before AP and used to suppress
  previous AP entries whose frequency is within 3 Hz and whose saved fragment
  contains the same second token. This models the WSJT-X `f0=-98` "do not use"
  behavior for the AP pass.
- AP results now preserve the refined `freq` and `dt` returned by `ft8_a7d`
  instead of reporting `0.0/0.0`.
- Remaining gaps:
  - The internal data structure is still simplified compared with the exact
    WSJT-X `ndec(jseq,k)` arrays, but the observable same-parity previous/current
    suppression behavior is now represented.
  - CQ special fragment handling is approximated from decoded words; it should
    be checked against `split77` word classification if AP sensitivity remains
    short.
- `ft8b` internal AP pass scheduling and masks are now structurally close to
  WSJT-X. The separate `ft8_a7d` implementation still depends on accurate
  previous-slot memory and `sbase`.
- `HashCallBook` is shared through `Rc<HashCallBook>`, which is the right
  architectural direction for cross-slot hash resolution.
- Stream-level hash collection now filters decoded tokens before saving them to
  the shared `HashCallBook`, avoiding grid/report tokens such as `FN20` or
  `RR73`. This keeps the table closer to WSJT-X callsign-only hash semantics.
- Stream-level `ft8_a7_save` entry extraction now uses a `split77`-like word
  normalization before saving `call_1 call_2`, including the WSJT-X `CQ xxx
  CALL -> CQ_xxx CALL` rewrite and subsequent `CQ_` skip.

## Current Tests and Constraints

Requested acceptance:

- Short stream decode: `tests/ft8/210703_133430.wav`, at least 19/20 messages,
  each decode under 15 s.
- Long stream decode: `tests/ft8/230208_140300.wav`, at least 366/449 matches,
  every segment under 15 s.

Test rules:

- Always run decode tests in release mode.
- Add/keep per-slot timeout checks before running long decode tests.
- Add/keep sensitivity checks against the provided baseline CSV before doing
  long experimental runs.
- Do not relax the performance floor. A slot taking more than 15 s is not a
  streaming decoder.

Current local test harness status:

- `test_stream_decode_long_audio` now asserts `total_matched >= 366`.
- The long test now has a severe sensitivity early abort at `366-10`.
- Long-file segmentation now feeds exact 15 s windows to `decode_slot`, matching
  WSJT-X `jt9a.f90` use of the 180000-sample shared decode buffer. The previous
  `15 s +/- 1 s` overlap harness shifted the effective slot by 1 s because the
  decoder consumed the first 15 s of a 17 s slice.

## Near-term Alignment Priorities

1. Add bit-level AP-mask regression checks against WSJT-X for contest and Hound
   examples.
2. Audit live-stream window handoff against WSJT-X `nzhsym` progress; the file
   harness now uses exact 15 s slots, but soundcard buffering still needs the
   same 180000-sample window contract.
3. Numerically compare `get_spectrum_baseline` against WSJT-X output.
4. Only after the above alignment work, run release tests with timeout and
   sensitivity assertions.

## Current Development Plan

After the latest requirement restatement, the active plan is:

1. Keep FFTW@3840 and RustFFT@4096, but use only FFTW@3840 for WSJT-X parity
   claims and acceptance tests.
2. Finish source-level control-flow parity before release decode tests:
   `jt9a.f90` windowing, `ft8_decode.f90` `41/47/50` state, `ft8b.f90`
   regular/AP passes, and `ft8_a7` same-parity memory.
3. Add focused non-release parity checks where they do not decode whole files:
   AP mask bit fixtures, baseline numerical fixtures, and candidate ordering
   fixtures.
4. Keep the stream decoder independent from UI and soundcard/file I/O.
5. Only when the above parity checks are in place, run the required release
   stream tests with per-slot timeout and sensitivity aborts.

## Documentation Policy

The active project documents are:

- `STREAM.md`: technical alignment report and current status.
- `TRY.md`: iteration/attempt log.
- `README.md`: user-facing overview; do not change unless explicitly requested.

Other Markdown planning/report files have been removed or folded into the two
active project documents.
