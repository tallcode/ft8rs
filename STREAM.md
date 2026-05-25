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
- Candidate construction now follows the WSJT-X structure more closely:
  descending `red`, optional `red2`, near-dupe zeroing, `nfqso +/- 10 Hz`
  priority, then remaining candidates by sync.
- `sbase` now comes from `get_spectrum_baseline(dd,nfa,nfb)`.
- Remaining risk: the Rust `baseline` polynomial helper still needs a numerical
  parity check against WSJT-X `baseline.f90`/`polyfit` on the reference files.
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
- `decode174_91` uses `maxosd=2` for depth 3; depth 1 is BP only.
- SNR uses `xsnr2` when `nagain=false`; false positive filter is
  `nsync <= 10 && xsnr < -25.0`.

Current `ft8rs` status after Iteration 12:

- Outer decode now uses WSJT-X pass count: 2 passes at depth 1, otherwise 3.
- Pass 1 uses `imetric=1`; passes 2/3 use `imetric=2`.
- `imetric=2` now squares `s2` before bit metric extraction.
- `bmete` has been added and the regular pass set is now
  `llra/llrb/llrc/llrd/llre`.
- The previous pass-1 depth override has been removed.
- The SNR false-positive gate/clamp now uses `-25 dB`.
- Ad-hoc AP masks have been removed from the default regular decode path.
- Remaining gaps:
  - `ft8b` still does not implement the full WSJT-X internal AP
    `nappasses/naptypes/nQSOProgress` system.
  - `nagain` is now consumed for the WSJT-X `nfqso +/- 20 Hz` search window and
    adjacent-tone SNR selection.
  - `nftx`, `napwid`, `ncontest`, `lapcqonly`, and `lft8apon` are represented
    in options but not all are consumed by the decode core yet.
  - `nzhsym` is represented at the stream orchestration level but not yet used
    inside `ft8b` for every WSJT-X branch.

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
- `ft8b` internal AP pass system is not equivalent to WSJT-X. The separate
  `ft8_a7d` implementation is closer but depends on accurate previous-slot
  memory and `sbase`.
- `HashCallBook` is shared through `Rc<HashCallBook>`, which is the right
  architectural direction for cross-slot hash resolution.

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
- Long-file segmentation still uses `15 s +/- 1 s` overlap and calls
  `decode_slot` on a 17 s slice. The stream decoder currently takes the first
  15 s of that slice, so the leading 1 s changes slot timing. This must be
  reconciled with WSJT-X `jt9a.f90` timing before treating results as final.

## Near-term Alignment Priorities

1. Consume the remaining WSJT-X parameters in the decode core:
   `nftx`, `napwid`, `ncontest`, `lapcqonly`, `lft8apon`, `nQSOProgress`.
2. Implement or explicitly defer the full `ft8b` internal AP
   `nappasses/naptypes` path after regular decode parity is stable.
3. Reconcile long-file slot timing with WSJT-X instead of relying on the current
   17 s test slice.
4. Numerically compare `get_spectrum_baseline` against WSJT-X output.
5. Only after the above alignment work, run release tests with timeout and
   sensitivity assertions.

## Documentation Policy

The active project documents are:

- `STREAM.md`: technical alignment report and current status.
- `TRY.md`: iteration/attempt log.
- `README.md`: user-facing overview; do not change unless explicitly requested.

Other Markdown planning/report files have been removed or folded into the two
active project documents.
