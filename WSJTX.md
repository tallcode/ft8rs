# WSJT-X Alignment Notes

This document keeps developer-facing details for maintaining FT8 decode behavior
against WSJT-X. User build and CLI instructions belong in `README.md`.

## Scope

`ft8rs` targets FT8 receive/decode compatibility with WSJT-X in the default
`wsjtx` profile. This file records only source-alignment notes, test baseline
rules, and known maintenance constraints for that profile.

Other profiles have separate technical reports and must not add design notes to
this file.

Priority order:

1. source-level architecture differences;
2. miss-driven architecture differences;
3. source-level parameter differences;
4. miss-driven parameter differences.

Do not improve speed or sensitivity by changing WSJT-X decode semantics,
candidate ordering, AP scheduling, residual subtraction order, or gate values.

Main reference files:

- `wsjtx/lib/ft8_decode.f90`
- `wsjtx/lib/ft8/ft8b.f90`
- `wsjtx/lib/ft8/sync8.f90`
- `wsjtx/lib/ft8/sync8d.f90`
- `wsjtx/lib/ft8/ft8_a7.f90`
- `wsjtx/lib/ft8/ft8_a8d.f90`
- `wsjtx/lib/ft8/ft8_downsample.f90`
- `wsjtx/lib/ft8/subtractft8.f90`
- `wsjtx/lib/ft8/decode174_91.f90`
- `wsjtx/lib/ft8/osd174_91.f90`
- `wsjtx/lib/77bit/packjt77.f90`

## Baseline

Release tests protect the active alignment baseline.

| Fixture | Active target |
|---|---:|
| `tests/ft8/210703_133430.wav` | `21/21` WSJT-X target rows, minimum accepted `19` |
| `tests/ft8/230208_140300.wav` | `424/424` WSJT-X target rows |

CSV `Extra` semantics:

- blank `Extra`: verified target row;
- `W`: WSJT-X extra row, included in the WSJT-X target;
- `J`: JTDX extra row, ignored for WSJT-X miss/diff;
- `E`: other/problem row, ignored for WSJT-X miss/diff.

The short fixture now uses the same `Extra` column as the long fixture. Current
marker counts are:

- `210703_133430.csv`: blank `20`, `W` `1`;
- `230208_140300.csv`: blank `411`, `W` `13`, `J` `20`, `E` `14`.

The active long WAV is normalized to `12 kHz / mono / 16-bit`, and sample 0 is
aligned with the filename timestamp.

`tests/ft8/a8d_k1jt_bg5atv_pm00.wav` is a synthetic one-slot a8 fixture:
regular decode is expected to miss it, while WSJT-X a8 context
`mycall=K1JT`, `hiscall=BG5ATV`, `hisgrid=PM00`, `nfqso=1000 Hz` must recover
`K1JT BG5ATV PM00` at about `-17 dB`. The JTDX profile is expected to produce
no decode for this fixture because it does not use the WSJT-X a8 list decoder.

## Source Layout

The decode core lives under `src/decode`. Files under `src/decode/lib_wsjtx` mirror
WSJT-X `wsjtx/lib` names where practical.

Important mappings:

- `src/decode/lib_wsjtx/ft8_decode.rs` -> `wsjtx/lib/ft8_decode.f90`
- `src/decode/lib_wsjtx/ft8/ft8_params.rs` -> `wsjtx/lib/ft8/ft8_params.f90`
- `src/decode/lib_wsjtx/ft8/sync8.rs` -> `wsjtx/lib/ft8/sync8.f90`
- `src/decode/lib_wsjtx/ft8/sync8d.rs` -> `wsjtx/lib/ft8/sync8d.f90`
- `src/decode/lib_wsjtx/ft8/ft8b.rs` -> `wsjtx/lib/ft8/ft8b.f90`
- `src/decode/lib_wsjtx/ft8/ft8_downsample.rs` -> `wsjtx/lib/ft8/ft8_downsample.f90`
- `src/decode/lib_wsjtx/ft8/ft8_a7.rs` -> `wsjtx/lib/ft8/ft8_a7.f90`
- `src/decode/lib_wsjtx/ft8/ft8_a8d.rs` -> `wsjtx/lib/ft8/ft8_a8d.f90`
- `src/decode/lib_wsjtx/ft8/ft8apset.rs` -> `wsjtx/lib/ft8/ft8apset.f90`
- `src/decode/lib_wsjtx/ft8/twkfreq1.rs` -> `wsjtx/lib/ft8/twkfreq1.f90`
- `src/decode/lib_wsjtx/ft8/gen_ft8wave.rs` -> `wsjtx/lib/ft8/gen_ft8wave.f90`
- `src/decode/lib_wsjtx/ft8/encode174_91.rs` -> `wsjtx/lib/ft8/encode174_91.f90`
- `src/decode/lib_wsjtx/ft8/genft8.rs` -> `wsjtx/lib/ft8/genft8.f90`
- `src/decode/lib_wsjtx/ft8/decode174_91.rs` -> `wsjtx/lib/ft8/decode174_91.f90`
- `src/decode/lib_wsjtx/ft8/bpdecode174_91.rs` -> `wsjtx/lib/ft8/bpdecode174_91.f90`
- `src/decode/lib_wsjtx/ft8/osd174_91.rs` -> `wsjtx/lib/ft8/osd174_91.f90`
- `src/decode/lib_wsjtx/ft8/subtractft8.rs` -> `wsjtx/lib/ft8/subtractft8.f90`
- `src/decode/lib_wsjtx/77bit/packjt77.rs` -> `wsjtx/lib/77bit/packjt77.f90`
- `src/decode/lib_wsjtx/indexx.rs` -> `wsjtx/lib/indexx.f90`

The stream, input, output, and CLI layers must not depend on private FT8 work
buffers. Use the public session/config/result interfaces.

## Audio Model

FT8 core constants:

| Item | Value |
|---|---:|
| internal sample rate | `12000 Hz` |
| decode buffer | `15 * 12000` samples |
| `NSPS` | `1920` |
| `NSTEP` | `480` |
| `NDOWN` | `60` |
| `sync8 NFFT1` | `3840` |
| long downsample FFT | `192000 -> 3200` |
| default passband | `200..3000 Hz` |

All file and soundcard input must become a stable 12 kHz mono stream before it
enters the decode core. EOF tail slots must be flushed rather than dropped.

## FFT Policy

Only WSJT-X-aligned `3840` sync FFT bins are supported.

- default: `RustFFT @ 3840`;
- optional: `FFTW @ 3840` with `--features fftw`;
- removed: runtime FFT backend switching and `rustfft@4096`.

`ft8_downsample` keeps the reference scale expression at the call site:

```fortran
fac=1.0/sqrt(float(NFFT1)*NFFT2)
c1=fac*c1
```

The Rust code intentionally calculates this as a local `f32` expression before
writing back to f64 buffers, matching WSJT-X default `real` behavior.

## Streaming Control Flow

The stream adapter follows WSJT-X partial decode stages:

| Stage | Boundary | Behavior |
|---:|---:|---|
| `nzhsym=41` | `41 * 3456` samples | early decode with zero padding |
| `nzhsym=47` | `47 * 3456` samples | subtract selected early decodes and save cleaned prefix |
| `nzhsym=50` | `50 * 3456` samples | final regular decode, AP decode, current-slot memory save |

Important rules:

- `ndepth=1` skips `nzhsym<50`;
- `nzhsym<50` disables `ft8b` internal AP passes;
- final decode zero-pads after `50 * 3456`, not after the full 15 s buffer;
- AP parity uses `jseq = mod(nutc/5, 2)` from the slot timestamp.

## `ft8_decode` and `ft8b`

Outer regular passes:

- `npass=2` for depth 1, otherwise `3`;
- pass 1 uses `imetric=1`;
- passes 2 and 3 use `imetric=2`;
- `syncmin=2.1` when `ndepth<=2`, otherwise `1.3`;
- each pass refreshes `sync8` and `sbase` from the current residual.

`ft8b` details:

- downsample -> time refine -> frequency refine -> second downsample -> final
  time refine;
- hard sync gate mirrors `syncmin=6`, `imetric=2 => 7`, `ndepth<=2 => 8`,
  with bailout when `nsync <= syncmin`;
- `imetric=2` squares temporary `s2` before bit extraction;
- AP pass scheduling follows current WSJT-X FT8:
  `npasses=5+2*nappasses(nQSOProgress)`, `lapcqonly=>7`, `nzhsym<50=>5`;
- AP magnitude follows current WSJT-X FT8:
  `apmag=maxval(abs(llrz))*1.1`.

`cs` and `s8` are deliberately written like the F90:

```fortran
cs(0:7,k)=csymb(1:8)/1e3
s8(0:7,k)=abs(csymb(1:8))
```

Do not divide `s8` by `1000`.

## `sync8`

Critical alignment points:

- `jstrt=0.5/tstep` is assigned to integer in F90, so it truncates to `12`;
- `m/m36/m72` are Fortran 1-based time-bin indices; convert to Rust 0-based
  only at array access;
- missing that conversion shifts sync by one `NSTEP` (`480` samples, `0.04 s`);
- `mlag=13`, `mlag2=JZ`;
- `red` and `red2` use 40th-percentile normalization;
- near-dupe pruning is candidate-order dependent;
- `nfqso +/- 10 Hz` candidates are ordered first;
- `s=fac*s` is kept after near-dupe suppression and before candidate sorting to
  match the F90 control-flow shape, even though the scaled `s` is not used later.

## AP and Cross-slot Memory

WSJT-X AP behavior:

- `ft8_a7_save` stores `msg0/dt0/f0` by parity;
- on a new slot, current memory moves to previous memory for that parity;
- AP at `nzhsym=50` uses previous entries of the same parity;
- entries containing `/` or `<` are skipped;
- current regular decodes suppress previous AP candidates already explained.

Current Rust shape:

- `a7[jseq][0]` is previous same-parity memory;
- `a7[jseq][1]` is current memory;
- `A7SaveEntry` stores fixed-width uppercase `character*37`-style `msg0`;
- `call_1/call_2/grid4` are derived from `msg0` at AP decode time;
- `xbase` is recomputed from the current slot `sbase`;
- regular and AP decode share `ft8_downsample_from_cx`, `sync8d/sync8d_twk`,
  and the 32-point symbol FFT helper.

The stream adapter has a small AP seed retry around saved-entry frequency:
`[0.0, +0.5, -0.5]`. This is outside the `ft8_a7d` core mirror. Removing it
drops the active long WSJT-X target by one row, so keep it documented as an
adapter-boundary quantization guard.

`ft8_a8d` is implemented as the WSJT-X list decoder at `nfqso`. It is only
entered when AP is enabled, contest modes 6/7 are inactive, `hiscall` has at
least 3 characters, `hisgrid` has at least 4 characters, no prior regular/AP
decode landed within 3 Hz of `nfqso`, and no a7 AP decode contains `hiscall`.

Source-shape details to preserve:

- message enumeration uses `getmsg(1..206,mycall,dxcall,dxgrid)`;
- generated waveform uses `gen_ft8wave(itone,NN,32,2.0,200.0,0.0,...)`;
- lag search is `-200..200` in steps of `4`, then `lagpk-8..lagpk+8` in
  steps of `1`;
- frequency tolerance is `abs(f1-fbest) <= 5.0`;
- SNR uses the `s1(-200:-100)` and `s1(100:200)` average and clamps to `-30`;
- accept gate remains `nhard<=54`, `plog>=-159.0`, and `sigobig>=0.71`;
- callback output uses `sync=10.0`;
- AP memory save follows `ft8_decode.f90`: save frequency is `nfqso` (`f1`),
  not the displayed `fbest`.

## Pack/Unpack and Hash

The project scope is FT8 receive/decode. WSPR-specific payloads are intentionally
excluded.

Receive-side families that should remain covered:

- `i3=0,n3=0`: free text;
- `i3=0,n3=1`: DXpedition special messages;
- `i3=0,n3=3/4`: ARRL Field Day exchange;
- `i3=0,n3=5`: telemetry;
- `i3=1/2`: standard messages and `/R`/`/P` forms;
- `i3=3`: ARRL RTTY contest exchange;
- `i3=4`: nonstandard/hash calls;
- `i3=5`: EU VHF contest exchange with hashed calls.

Important gates:

- `i3=0,n3=6` is out of scope and rejected;
- Type 3 callsign fields must validate through WSJT-X `chkcall`-style rules;
- hash unpack uses the shared stream `HashCallBook`, not a per-slot book;
- diff matching treats resolved hash display forms such as `<RK4FF>` and
  `RK4FF` as equivalent, while unresolved `<...>` remains distinct.

## Subtraction

`subtractft8` preserves WSJT-X 1-based sample variables:

```fortran
nstart=dt*12000+1 + idt
do i=1,nframe
   j=nstart-1+i
   if(j.ge.1.and.j.le.NMAX) camp(i)=dd(j)*conjg(cref(i))
enddo
```

Rust maps `i=0` to the same Fortran sample with:

```rust
let j = nstart_1based + rust_i as isize;
let sample = dd0[(j - 1) as usize];
```

Other aligned details:

- 180000-point circular FFT filter;
- cshifted cos² window and endpoint correction;
- refined-DT `sqf()` rebuilds local `dd` per trial and writes back once;
- `gen_ft8wave` first/last ramps match WSJT-X.

## Fixed Pitfalls

Keep these in mind during future audits:

- `sync8d` out-of-range indices contribute zero, never modulo-wrap.
- `sync8` time-bin access is 1-based until the Rust array boundary.
- `subtractft8` has two separate 1-based variables: `nstart` and `j`.
- AP parity must use `jseq=mod(nutc/5,2)`, not a blind toggle.
- `packjt77::is_stdcall()` and AP `chkcall` need WSJT-X call-area position
  semantics.
- Type 3 false positives should be fixed with WSJT-X-style callsign validation,
  not by disabling contest messages.
- Current WSJT-X FT8 uses `5+2*nappasses`, `lapcqonly=>7`, and AP `*1.1`.
  Older `*1.01` notes belong to other/commented paths.

## Remaining Risks

- LDPC/OSD has source-shape tests and release audio coverage, but still lacks a
  broad independent set of WSJT-X-generated golden vectors.
- More fixtures would help for contest messages, hash calls, high drift,
  collisions, AP progression, and band-edge signals.
- AP mask tests cover active bit positions and gates, but not every
  `ncontest/iaptype` byte-for-byte generated pattern.

## Validation Commands

Run these before accepting decode-core changes:

```bash
cargo fmt --check
cargo test --release test_stream_decode_short_audio -- --nocapture
cargo test --release test_stream_decode_long_audio -- --nocapture
cargo test --release --test wsjtx_source_audit_test -- --nocapture
```

For FFTW alignment:

```bash
cargo test --release --features fftw test_stream_decode_short_audio -- --nocapture
cargo test --release --features fftw test_stream_decode_long_audio -- --nocapture
```
