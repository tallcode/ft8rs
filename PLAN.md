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
- item 10 has an initial regular-output filter layer;
- item 9 has AP type tables, a subpass planner, and BP/OSD AP execution for
  template-safe mask families, but not every AP/deep/special mask family;
- complete item 9 and item 10 coverage are still required before JTDX can be
  called aligned.

Remaining implementation order:

1. complete JTDX regular `nsubpasses` metric variants beyond the currently
   wired `isubp1=1..2` forward/reverse metric loop. The remaining source
   pieces are `csold` and the combined `cscs/csr` metric paths used by higher
   `nsubpasses`;
2. complete JTDX-owned FT8v2 source-level refinement, especially any remaining
   differences in OSD/BP acceptance thresholds and packed-message handling;
3. source-audit and tighten JTDX subtract/downsample residual interaction,
   including `freqsub`, `npos`, `lsubtracted`, and focused-QSO retry products;
4. source-audit the newly connected AGC forced-DT / `avexdt` behavior and
   focused-QSO odd/even memory behavior against JTDX with real profile output;
5. complete remaining AP/deep mask-family gating. The known deferred pieces are
   signal-classified `lqsosig`, `lmycsignal`, `ldxcsig`, `lcqsignal`,
   `lqsocandave`, and their associated `ndeep=4` raises;
6. complete remaining AP/deep-specific false-positive filter coverage,
   including filters that depend on JTDX signal classifiers;
7. decide how to handle JTDX `searchcalls` / `ALLCALL7.TXT` backed filters.
   They are not currently modeled and should not be silently approximated;
8. keep FT8S / superdeep-specific branches documented as not implemented until
   their source dependency closure is intentionally ported.

Do not promote `jtdx` as aligned until the relevant slices are complete.

Current closure status:

- `profile=wsjtx` short release test still passes on `210703_133430.wav`.
- `profile=jtdx` now has a CLI decode closure: the native JTDX path runs first,
  and if it currently emits no rows, the profile falls back to the protected
  WSJT-X-aligned stream decoder so file/monitor/output plumbing can be tested
  end to end. Treat fallback rows as closure scaffolding, not as a JTDX
  sensitivity result.
- The native JTDX `ft8b` path still needs repair before JTDX reporting can be
  promoted. The current native blocker is after sync candidate generation:
  candidates are present, but regular BP/OSD has not yet been verified to
  produce accepted native JTDX messages after the latest `twkfreq1` and `csr`
  metric alignment work.
- `profile=hybrid` currently runs both workers. Until native JTDX emits rows,
  the JTDX side may provide only fallback-equivalent rows that dedupe away
  against the WSJT-X worker.

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
- Do not use hybrid to hide JTDX implementation shortcuts.
- Do not let JTDX/hybrid technical notes drift into `WSJTX.md`.

## CLI Direction

Use built-in profile names:

```bash
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile wsjtx
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile jtdx
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile hybrid
```

Optional SWL remains explicit:

```bash
ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 --profile jtdx --swl
```

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
