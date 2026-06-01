# Decode Profile Plan

This file is the stage plan only. Keep profile-specific technical notes in the
profile documents:

- WSJT-X alignment details: `WSJTX.md`
- JTDX profile details: `JTDX.md`
- Hybrid union details: `HYBRID.md`

Do not duplicate the technical reports here. This keeps manual review simple:
one profile, one technical document.

## Goal

Second-stage development adds profile-aware decoding while preserving the
existing WSJT-X-aligned baseline.

Profiles:

```text
profile=wsjtx  -> existing WSJT-X-aligned decoder
profile=jtdx   -> separate JTDX-oriented high-sensitivity decoder
profile=hybrid -> WSJT-X and JTDX in parallel, result-level union
```

Rules:

- `wsjtx` remains the default.
- WSJT-X behavior must not change while adding JTDX or hybrid.
- Current scope is normal FT8 decoding, with `jtdx` optimized for
  high-sensitivity behavior first.
- SWL-specific expansion is deferred. Existing SWL switches may remain, but do
  not spend this phase implementing or tuning SWL-only behavior.
- JTDX code lives in its own implementation path.
- Hybrid lives outside both decoder implementations.
- Do not extract shared decoder internals in this phase.
- Do not run sensitivity/baseline tests until the planned implementation slices
  are complete enough to produce meaningful output.

## Documentation Boundaries

`WSJTX.md` records only WSJT-X alignment and maintenance notes.

`JTDX.md` records only the JTDX profile:

- JTDX source dependency closure
- JTDX high-sensitivity defaults
- JTDX state model
- JTDX pass/sync/downsample/ft8b differences
- JTDX baseline selection
- JTDX implementation order

`HYBRID.md` records only hybrid behavior:

- parallel decoder launch
- state sharing boundaries
- merge and dedupe rules
- source attribution
- hybrid baseline selection
- future cross-decoder research

If a direct contrast is needed, keep it brief and put it in the document for
the profile being implemented.

Documentation contract for this stage:

- JTDX profile technical notes, source-audit findings, parameter choices,
  unfinished risks, and future TODOs belong in `JTDX.md`.
- Hybrid technical notes, parallel execution behavior, result merge policy,
  dedupe rules, source attribution, and hybrid TODOs belong in `HYBRID.md`.
- WSJT-X technical notes stay in `WSJTX.md`; do not add JTDX or hybrid design
  records there unless the note is strictly about protecting WSJT-X behavior.
- `PLAN.md` stays as the coordination/index document and should only summarize
  where detailed notes live.
- When a note could fit more than one document, split it by responsibility:
  decoder/source details go to the profile document, while cross-decoder launch
  and merge behavior goes to `HYBRID.md`.

Current documentation split:

- `JTDX.md` is the living technical report for `profile=jtdx`.
- `HYBRID.md` is the living technical report for `profile=hybrid`.
- Do not duplicate JTDX or hybrid details in `WSJTX.md`.
- When a future change touches both JTDX and hybrid, record the decoder-specific
  part in `JTDX.md` and the merge/output/concurrency part in `HYBRID.md`.

## Target Source Shape

```text
src/decode/
  lib_wsjtx/
    ...
  lib_jtdx/
    ...
  hybrid/
    ...
```

Boundary rules:

- `lib_wsjtx` is the protected WSJT-X-aligned implementation.
- `lib_jtdx` is a separate JTDX-oriented implementation.
- `hybrid` runs decoder instances and merges results; it is not a third decode
  algorithm.
- `lib_jtdx` must not call `lib_wsjtx` internals unless a file is explicitly
  audited and documented in `JTDX.md` as identical.

## Baseline Rules

The CSV `Extra` column means:

- empty: main verified baseline
- `W`: WSJT-X extra decode
- `J`: JTDX extra decode
- `E`: known questionable/other extra decode

Profile target rows:

- `wsjtx`: empty or `W`
- `jtdx`: empty or `J`
- `hybrid`: empty, `W`, or `J`

The existing WSJT-X baseline remains a hard gate. JTDX and hybrid are
informational until their implementations are complete enough to establish
stable baselines.

## Development Steps

### Step 1: Protect WSJT-X

- Keep the current decoder under `lib_wsjtx`.
- Preserve existing public decode behavior.
- Keep existing WSJT-X tests as hard gates.
- Do not add JTDX conditionals inside the WSJT-X path.

### Step 2: Add Profile Dispatch

- Add `--profile wsjtx|jtdx|hybrid`.
- Default to `wsjtx`.
- Dispatch at the stream/session boundary.
- Keep CLI/output behavior unchanged for `wsjtx`.

### Step 3: Build JTDX Skeleton

- Add `lib_jtdx` with the JTDX FT8 source dependency closure.
- Add JTDX state and constants.
- Add JTDX-owned outer decode policy.
- Keep incomplete behavior explicit until JTDX can be measured as a profile.
- Record technical decisions in `JTDX.md`.

### Step 4: Implement JTDX Decode Slices

Implement JTDX in closed slices:

1. outer pass/cycle policy;
2. sync candidate path;
3. downsample path;
4. `ft8b` regular sync/downsample/symbol extraction;
5. JTDX-owned regular BP/unpack path;
6. OSD fallback;
7. complete JTDX-owned FT8v2 support;
8. AGC sync path;
9. AP/deep decode;
10. false-positive filters.

Initial status:

- items 1-8 are scaffolded or partially implemented;
- JTDX subtract/state plumbing is present for regular decodes, including
  `itone`, `subtractft8`, and `freqsub/npos/lsubtracted`;
- JTDX focused-QSO downsample retries now use the generated `c2/c3` products
  for directly inferable `nqso=2/3` cases;
- JTDX focused-QSO `lastrxmsg` memory is now retained in the JTDX session and
  used by the `nqso` decision when a previous same-thread decode exists;
- JTDX focused-QSO memory also includes the source-level odd/even
  `calldteven` / `calldtodd` call-DT rings, previous-slot `even` / `odd`
  copies, and the `incall` ring used to restore `lastrxmsg` before a slot;
- JTDX pass 4/7 half-sample shift behavior is present;
- JTDX AGC now mutates the session `dd8` once before the decode passes and
  carries `lagccbail` through `ft8_mod1` state into `sync8`;
- JTDX duplicate suppression is now state-driven through
  `allmessages/allsnrs/allfreq` rather than a profile-agnostic `HashSet`;
- JTDX regular-output false-positive filtering includes the current
  source-level protocol-shape guards that do not depend on AP/deep execution;
- JTDX AP/deep-specific false-positive filtering has started with the
  `iaptype=35/36` weak/out-of-window DXCall-search first-callsign gate;
- JTDX AP execution is wired for template-buildable AP mask families
  `1/2/3/4/5/6/11/12/13/14/21/22/23/24/31/35/36/40/41/42/43/44/111`;
- JTDX regular and AP decode now use source-shaped `isubp2` LLR source
  selection for `llra` / `llrb` / `llrc` / `llrd`, including the JTDX
  non-SWL regular skip of `isubp2=4` and the regular `isubp1=1..2`
  pass-dependent `llrd` retries;
- JTDX AP mask magnitude now follows `maxval(abs(llra))*1.01`, including the
  source `2.83` LLR scale factor in `llra`;
- JTDX decoded-row SNR now follows the source `ft8b.f90` 79-symbol
  signal/noise accumulation and nonlinear correction path, with different
  regular/AP lower clamps;
- JTDX false-positive filter quality now carries the OSD `dmin` term and uses
  `1.0-(nharderrors+dmin)/60.0`;
- JTDX false-positive filtering now includes additional source-shaped regular
  FT8 guards for `/R` standard messages and weak/out-of-window
  `<...> CALL GRID` hash-call messages;
- JTDX AP/deep OSD depth now follows the source default branch more closely:
  AP fallback uses `ndeep=3` by default and `ndeep=5` for `nagain` filtering;
- JTDX Hound AP table selection is exposed through `--hound`; hound special
  message AP types `22/24` are built from the JTDX type-0.1 special-message
  template;
- JTDX `ft8b` now applies the source-shaped `twkfreq1` constant-frequency
  correction after `delfbest` refinement and before extracting symbols;
- JTDX symbol extraction now keeps both forward `cs` and reversed `csr`
  symbol matrices, applies the source weak-symbol edge scaling when
  `syncav < 2.5`, and uses `csr` for the regular `isubp1=2` bit-metric
  retry instead of only reusing the forward `cs` metrics;
- JTDX symbol metrics now also apply the source tone-spectrum normalization
  after symbol extraction, scaling `s8`, `cs`, and `csr` when a tone row is
  more than `1.5x` above the minimum row energy;
- JTDX regular bit-metric extraction now maps the Fortran data-symbol columns
  from 1-based `ks=8..36` / `44..72` to Rust 0-based `7..35` / `43..71`,
  avoiding a full-symbol off-by-one in BP/OSD input metrics;
- JTDX symbol extraction now also preserves `cscs`, the forward-symbol matrix
  saved during `lreverse` passes for later combined `cscs/csr` metric
  variants;
- JTDX-owned `encode174_91` now exists, so JTDX can generate source-shaped
  reference tone sequences from packed 77-bit messages without calling the
  protected WSJT-X encoder path;
- JTDX `ft8b` now builds initial `tone8myc` / `tone8`-style reference tone
  hints from configured calls and uses them for the first native
  `lmycsignal`, `lqsosig`, `lqsosigtype3`, standard-DX
  `ldxcsig/lcqdxcsig`, nonstandard-DX `ldxcsig/lcqdxcnssig`, QSO end-message,
  and `lqsocandave` classifiers;
- JTDX `ft8b` now computes the 256-point `s256` CQ classifier branch from
  `cd0(ibest+224:ibest+479)*ctwk256`, matching the normal FT8 high-sensitivity
  source path used to raise `lcqsignal` beyond the basic `rscq` check;
- JTDX-owned `packjt77sd` and `genft8sd` now exist for FT8S/superdeep message
  tone generation. The native `ft8b` path has an initial conservative FT8S
  fallback that tries configured `mycall/hiscall` message candidates after
  regular/AP BP+OSD paths fail near the QSO frequency;
- JTDX `ft8b` now has a private even/odd signal-history cache for CQ,
  MyCall, and QSO candidate symbol matrices. Slot-local temporary signal
  matrices are promoted at slot end, and later candidates can recover `csold`
  by matching frequency and DT;
- high-order JTDX bit-metric sources for `cs+csold` power and sum variants are
  wired into the metric builder. When matching prior slot symbol matrices are
  found, JTDX raises CQ/MyCall/QSO candidate `nsubpasses` to `5/8/11` and
  AP/deep attempts now run inside the same `isubp1` metric-source loop, so
  `csr`, `cscs/csr`, and `cs/csold` variants can feed AP/deep work. Ordinary
  regular decode still skips `isubp1>2`, matching the source rule that those
  extra subpasses are not ordinary regular decodes;
- item 10 has an initial regular-output filter layer;
- item 9 has AP type tables, a subpass planner, and BP/OSD AP execution for
  template-safe mask families, but not every AP/deep/special mask family;
- complete item 9 and item 10 coverage are still required before JTDX can be
  called aligned.

Remaining implementation order:

1. complete the remaining JTDX AP/deep source gating matrix. AP/deep now runs
   inside the source-shaped `isubp1` metric-source loop and can use high-order
   `nsubpasses`. The current gate set covers standard/nonstandard call shape,
   missing my/his-call, AP width, QSO-candidate priority, MyCall priority,
   CQ-only, `lqsomsgdcd`, `stophint`, `nmic`, standard/nonstandard DXCall
   signal classifiers, the `s256` CQ classifier, and QSO end-message
   classifiers (`RRR`/`73`/`RR73`);
2. complete JTDX-owned FT8v2 source-level refinement, especially any remaining
   differences in OSD/BP acceptance thresholds and packed-message handling;
3. source-audit and tighten JTDX subtract/downsample residual interaction,
   including `freqsub`, `npos`, `lsubtracted`, and focused-QSO retry products;
4. source-audit the newly connected AGC forced-DT / `avexdt` behavior and
   focused-QSO odd/even memory behavior against JTDX with real profile output;
5. complete remaining signal-classified AP/deep gates. The known deferred
   pieces are hound fox report/RR73 classifiers, source-specific CPU-pruning
   gates, and the associated `ndeep=4` raises;
6. complete remaining AP/deep-specific false-positive filter coverage,
   including filters that depend on JTDX signal classifiers;
7. decide how to handle JTDX `searchcalls` / `ALLCALL7.TXT` backed filters.
   They are not currently modeled and should not be silently approximated;
8. complete the remaining FT8S / superdeep branches that are part of JTDX's
   normal FT8 high-sensitivity path. The initial FT8S candidate matcher exists,
   but source-complete `ft8s`, `ft8sd`, `ft8sd1`, `ft8mf1`, `ft8mfcq`, and
   `tonesd` behavior is not fully ported yet. SWL-specific behavior remains
   deferred.

Next checkpoint:

- source-audit and tighten the remaining AP/deep per-`iaptype` gates against
  JTDX `ft8b.f90`, especially Hound fox-report/RR73 branches and other
  source-specific pruning gates that still need source-complete classifiers;
- after that, run only compile checks first, then re-enable profile-level short
  decode smoke tests once native JTDX emits stable rows;
- keep any new temporary instrumentation out of committed code. If a diagnostic
  is useful enough to keep, document the finding here or in `JTDX.md` rather
  than leaving ad hoc output paths in the decoder.

Do not promote `jtdx` as aligned until the relevant slices are complete.

Current closure status:

- `profile=wsjtx` short release test still passes on `210703_133430.wav`.
- `profile=jtdx` now runs only the native JTDX path. The earlier WSJT-X
  fallback closure has been removed so JTDX and hybrid results cannot be
  mistaken for protected WSJT-X rows.
- The native JTDX `ft8b` path still needs repair before JTDX reporting can be
  promoted. The current native blocker is after sync candidate generation:
  candidates are present, but regular BP/OSD has not yet been verified to
  produce accepted native JTDX messages after the latest `twkfreq1` and `csr`
  metric alignment work.
- `profile=hybrid` currently runs both workers and can only gain JTDX rows once
  native JTDX emits accepted messages.

### Step 5: Build Hybrid Skeleton

- Add a hybrid runner outside both decoder paths.
- Run independent decoder instances.
- Share input samples only.
- Merge decoded results after each decoder finishes.
- Keep hybrid marked incomplete until JTDX can provide stable measured results.
- Record technical decisions in `HYBRID.md`.

Initial status:

- the runner and result merger exist;
- full-slot hybrid decoding runs both decoder sessions in parallel;
- monitor can use hybrid at the full-slot boundary;
- WSJT-X-side progressive `nzhsym=41/47` output now streams immediately in
  hybrid while the JTDX worker runs in parallel; JTDX-only results are emitted
  after the JTDX worker finishes and dedupe is applied.

### Step 6: Enable JTDX Reporting

- Add JTDX profile report mode after JTDX emits messages.
- Use rows where `Extra` is empty or `J`.
- Keep release mode and timeout rules.
- Record the first stable observed count as the initial JTDX baseline.

### Step 7: Enable Hybrid Reporting

- Run WSJT-X and JTDX decoders in parallel per slot.
- Deduplicate at the result level.
- Preserve source attribution internally.
- Use rows where `Extra` is empty, `W`, or `J`.
- Promote hybrid to a hard gate only after behavior stabilizes.

## Do Not Do Yet

- Do not extract a shared common decoder layer.
- Do not share hashcallbook or AP memory between WSJT-X and JTDX.
- Do not share residual, odd/even memory, or JTDX state in hybrid phase 1.
- Do not enable SWL by default.
- Do not expand SWL-only decode behavior in this milestone.
- Do not use hybrid to hide JTDX implementation shortcuts.
- Do not let JTDX/hybrid technical notes drift into `WSJTX.md`.

## CLI Direction

Use built-in profile names:

```bash
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile wsjtx
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile jtdx
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile hybrid
```

SWL-specific behavior is deferred. Keep the current option surface stable if it
already exists, but do not use SWL as the development target for this milestone.

If experimental external tuning is needed later, add a separate option such as
`--profile-file`. Do not overload `--profile` with both names and file paths in
this implementation stage.

## Future Research

- Controlled cross-decoder hints.
- Passing high-confidence decoded calls from one decoder to the other.
- Shared hash/call hinting after both pure profiles are stable.
- External experimental profile files.

These remain out of scope for the first complete `jtdx` and `hybrid`
implementations.
