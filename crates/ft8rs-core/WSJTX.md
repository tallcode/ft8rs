# WSJT-X Alignment Notes

This document keeps developer-facing details for maintaining FT8 decode behavior
against WSJT-X. User build and CLI instructions belong in `README.md`.

## Scope

`ft8rs` targets FT8 receive/decode compatibility with WSJT-X in the default
`wsjtx` profile. This file records only source-alignment notes, test baseline
rules, and known maintenance constraints for that profile.

Other profiles have separate technical reports and must not add design notes to
this file.

## Current upstream alignment

The classic receive-code alignment target is WSJT-X `v3.3.0-beta5`, commit
`d89fe30ca1dfd3c38def8f17228f09f042829f95`. The official `release/3.3` branch
and beta5 tag were rechecked on 2026-11-04 and still point to this commit;
`master` remains at `967c85a6`. The sibling checkout is unchanged. Verification
uses the independent checkout `target/wsjtx-upstream-20261104`.

This profile mirrors the classic FT8 receiver. Upstream's `ft8var` coarse-cache,
FFT32 pruning and multithreaded residual transactions remain a separate engine;
this update does not silently substitute that engine or claim its speedup.

The current update includes:

- `015b4894`: suffix-aware A8 hypotheses for standard `/R` and `/P` calls,
  retaining former nonstandard-call candidates in their original order.
- `25a626af`: A8 templates, private reusable scratch, bounded overlap loops,
  raw/smoothed spectra, reduced copies, deterministic ties and bounded waveform
  phase wrapping on the applicable classic path.
- `8a0bddd9`: direct construction of the shifted subtraction filter. Rust's
  existing heap/session ownership already covers the native stack-storage fixes.
- `2e272882`: A7 zero-distance protection.
- `2c1b22b6`: RR73 AP tail matches acknowledgment 32403. Standard source packing
  prioritizes that acknowledgment over grid value 32373; receive unpack can
  still render both wire forms as RR73.
- `50f6cc83`, `fad848ef`: grammar-first strict encoding, native genft8 free-text
  compatibility, staged hash effects, contest bounds and current Field Day /
  telemetry rendering. AP setup uses strict encoding. Type 3 wire acceptance
  follows the native receiver even where source grammar is stricter.
- A7 sentinel construction now follows the native loop: construct the base,
  then append its RRR/RR73/73/report tail. The former Rust early return omitted
  those tails. No special sentinel rejection or legacy RR73 recovery is added.

WSPR and transmit-only controls remain outside this receive-only scope.

### Approved historical-recording differences

The user superseded the earlier decision to defer codec/AP migration: keep the
original decoder synchronized with upstream, and handle compatibility in wsjtx+.
The independently diagnosed losses are accepted only for the exact observations
in `tests/decoder/data/ft8/upstream-retention/exceptions.tsv`:

- WSJT-X, +1 dB long recording: `YO3GOD OY1DZ RR73` (slot 3),
  `I3EVK VE7ON RR73` (slot 8), `PD4I RL6LS RR73` and
  `RJ3DC OE9GWV RR73` (slot 13).
- WSJT-X, Hybrid and Hybrid-exp, +1 / +3 dB long recordings:
  `CQ OH8CMS KP24` (slot 11).

That is five distinct messages and ten profile/recording observations: eight
frozen calibration rows and two supplemental Hybrid-exp checks. Exceptions are
bound to recording SHA-256, profile, slot, complete message, the 2022 private
CallDB, optimized build context and this pinned upstream revision. Every absence
is logged as `EXPECTED UPSTREAM LOSS`; all other identities remain mandatory.
Original-audio snapshots have no loss exemption. Five exact long-recording CSV
rows now omit W (four old RR73 and the diagnosed CQ); all other fields and
J/H/He markers are preserved, so the current W requirement is 429 rows.
The historical retention corpus
is unchanged; this policy is separate and is not a native truth oracle.

The original 3.0 protocol, channel and audio data are preserved. The independent
native `ft8/upstream/wsjtx-3.3/` layer supplies active current packing/rendering
expectations, all 411 A8 messages for six contexts, and all 206 A7 messages for
six contexts. Earlier native codewords, CRC, tones and waveforms remain checked
as historical wire inputs. A7 source construction is extracted verbatim, with
its source-section hash recorded alongside driver/generator/toolchain hashes.

Native Type 3 wire acceptance adds the unconfirmed
`CQ 001 IZ7MMG 549 2025` on original-audio slot 3. This is recorded separately
and is not claimed as a known-truth sensitivity gain or hidden by a custom filter.

The evidence and future compatibility work belong in
[src/decode/lib_wsjtx_plus/README.md](src/decode/lib_wsjtx_plus/README.md).

Active RR73 sensitivity fixtures use native acknowledgment 32403, including
the standalone AWGN case. Source checks cover 36 pinned codec, template,
receiver and waveform files. The consolidated wsjtx+ report records synthetic
coverage and compatibility diagnoses. Independent plus development now uses a
test-only harness and remains outside this original profile; see its separate
report linked above.

Reproduction and pin audit:

```sh
python3 scripts/decoder-references.py audit-upstream --source target/wsjtx-upstream-20261104
FT8RS_WSJTX_SOURCE_DIR="$PWD/target/wsjtx-upstream-20261104" \
  FT8RS_REQUIRE_WSJTX_SOURCE=1 scripts/test-decoders.sh release --include-ignored
```

See `tests/decoder/UPDATING.md` for native reference generation. The test runner
verifies fixture hashes and installs the private CallDB/build context before
using the explicit exception policy.

For user controls and builds, see [usage](../../docs/USAGE.md) and
[development](../../docs/DEVELOPMENT.md). Shared calibration conditions and the
latest dated local result are maintained in [validation](../../docs/VALIDATION.md).

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
| `tests/ft8/210703_133430.wav` | `21/21` WSJT-X target rows; every target row is required |
| `tests/ft8/230208_140300.wav` | `429/429` WSJT-X target rows |

CSV `Extra` is a pipe-separated list of decoder memberships: `W` (WSJT-X),
`J` (JTDX), `H` (Hybrid), `He` (Hybrid + experimental decoding). A blank value
means the reviewed reference message was absent from the fixed replay. The
long fixture has 429 `W` rows after the five explicitly approved annotation
changes described above. The historical degraded-recording requirements
are frozen separately in `tests/decoder/data/ft8/degraded/retention.tsv`.

The short test's old `19` acceptance floor was inherited from the early
streaming decoder and remained after the documented `21/21` milestone. The
2026-10-28 local replay again matched all 21 rows with no extras; the test now
asserts the complete required set instead of a permissive count floor.

The shared [FT8 decoder tests](tests/decoder/README.md) retain independent native
Fortran/C++ protocol and waveform oracles. Their initial comparisons identified
two porting differences: standard Type 1 packing accepted unsigned contest
serials, and zero bit metrics skipped upstream normalization's division. The
former prevented Type 3 packing of `KA1ABC G3AAA 529 0013`; the latter could make
a7 select a remembered message on a later digitally silent slot. Both corrections
restore the reference behavior without changing decoder thresholds or AP search
schedules. The JTDX oracle also protects its original Type 4 trailing-hash
rejection rule.

The original `test28` corpus also covers raw `pack28`/`unpack28`, including
empty-suffix packing (`A0`) and the reserved range `532444..2063591`. The latter
must remain unresolved rather than wrapping into directed CQ tokens. Native
Watterson waveforms and `degrade_snr` recording variants extend the shared audio
matrix; their generation scripts and provenance are retained with the tests.
Per-message retention expectations for degraded recordings are explicitly Rust
regression baselines, separate from native codec and waveform references.

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
WAV and live 48 kHz input share the unchanged WSJT-X `fil4` coefficients.
Other downsampling ratios use a centered, windowed-sinc input adapter to avoid
aliasing; already-12 kHz input and decoder constants remain unchanged.

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
- Each stage reads a fresh audio snapshot, including when a caller updates a
  fixed-size buffer in place. A pointer/length pair is not an audio version.

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
- Noise power follows `10**(0.1*(sbase-40))` for all valid dB baselines,
  including zero and negative values. A positivity gate or a fixed linear
  fallback breaks SNR and the existing `nsync<=10 / SNR<-25` rejection gate.

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

- AP template pack/unpack validation shares a hash-call table, as Fortran's
  `pack77`, `save_hash_call`, and `unpack77` do. Seed both context calls before
  checking the round trip so a compound MyCall resolves rather than becoming
  `<...>`. Keep the original message-type and round-trip rejection rules.
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

- message enumeration uses `getmsg(1..206,mycall,dxcall,dxgrid)` normally;
  a standard `/R` or `/P` DX call additionally retains candidate 207, or
  candidates 207–411 when MyCall is also suffix-aware standard;
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
- Type 3 source grammar and wire acceptance are distinct; current receive
  special c28 tokens follow the native reference, including CQ/DE forms;
  accepted wire syntax alone does not establish a true on-air message;
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
- Type 3 source grammar and wire acceptance are different boundaries. Preserve
  pinned native receive behavior; do not insert ad-hoc callsign filters or call
  unconfirmed output a sensitivity gain. Independent wire references cover it.
- Current WSJT-X FT8 uses `5+2*nappasses`, `lapcqonly=>7`, and AP `*1.1`.
  Older `*1.01` notes belong to other/commented paths.

## Remaining Risks

- Native-generated pack/CRC/codeword/tone vectors, raw-call boundaries and
  deterministic BP/OSD experiments now supplement source-shape and release audio
  checks. The finite corpus is not exhaustive numerical or sensitivity coverage.
- More independent fixtures would help for high drift, collisions, AP progression
  and band-edge signals. Contest and hash cases are covered, but their full
  combinations with those channel conditions are not.
- AP mask tests cover active bit positions and gates, but not every
  `ncontest/iaptype` byte-for-byte generated pattern.

## Validation Commands

Run these before accepting decode-core changes:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --profile fast -- -D warnings
scripts/test-decoders.sh fast
scripts/test-decoders.sh release --include-ignored
FT8RS_REQUIRE_WSJTX_SOURCE=1 cargo test --release -p ft8rs --test wsjtx_source_audit_test
```

The source audit discovers a sibling `wsjtx` checkout from the crate/workspace
ancestors. Set `FT8RS_WSJTX_SOURCE_DIR` to the WSJT-X repository root to choose
an explicit reference. An invalid explicit path always fails; setting
`FT8RS_REQUIRE_WSJTX_SOURCE` also fails if automatic discovery finds no source.
Without either variable, environments without a reference checkout may skip
the upstream comparisons.

Clippy cleanup must preserve the original floating-point literals, arithmetic
precision, tie ordering, and accumulation order. Ordinary Rust style warnings
are fixed in code. Intentional exceptions use local `#[expect(..., reason =
"...")]` attributes: verbatim reference coefficients, source-shaped decoder
argument lists, indexed BP/pivot updates, and existing public/inline event
ownership. Do not truncate coefficients, rearrange numerical reductions, box
public payloads, or relax filters solely to satisfy a style lint. Avoid broad
crate/module lint allowances; an obsolete local expectation should itself warn.

For FFTW alignment:

```bash
cargo test --release -p ft8rs --features fftw test_stream_decode_short_audio
cargo test --release -p ft8rs --features fftw test_stream_decode_long_audio
```
