# Hybrid Decode Notes

This document records `--profile hybrid`: the result-union mode that runs the
WSJT-X-aligned decoder and the JTDX-oriented decoder together, plus the
hybrid-only shared-knowledge layer built on top of them. This is the single
source of truth for hybrid design notes.

Decoder internals belong in their own documents:

- WSJT-X alignment: `WSJTX.md`
- JTDX profile: `JTDX.md`

## Scope

Hybrid is not a third decoder algorithm. It is orchestration:

```text
profile=hybrid -> lib_wsjtx worker + lib_jtdx worker -> shared knowledge + result merge
```

It shares input samples and carefully gated hybrid-layer knowledge. Decoder
private state still belongs to each profile.

Current intent: hybrid's main sensitivity gain is the union of independent
WSJT-X/JTDX outputs. Shared knowledge is kept narrow: safe hash-call sharing,
passive evidence/confidence, and diagnostics. Active message-to-AP feedback is
not shipped in this milestone because the measured replay/import experiments did
not produce supported gains.

Goal: add hybrid-only shared knowledge without changing standalone `wsjtx` or
`jtdx` behavior, and without patching cross-decoder policy into mirrored source
files. The primary target is the **SWL / monitoring** use case (see Expected
Gain Ceiling).

## Non-Negotiable Constraints

1. Do not change `lib_wsjtx` source-alignment semantics.
2. Do not change `lib_jtdx` source-alignment semantics.
3. Do not add direct references from `lib_wsjtx` to `lib_jtdx`, or from
   `lib_jtdx` to `lib_wsjtx`.
4. Keep `profile=wsjtx` and `profile=jtdx` explainable as pure single-upstream
   profiles.
5. Put all cross-decoder behavior in hybrid orchestration layers.
6. Design for any number of decoders, not just WSJT-X plus JTDX.
7. Make future WSJT-X/JTDX source upgrades easy: mirrored libraries should not
   need shared-memory merge logic patched into them.
8. Treat false positives as a first-class risk. Higher-risk shared channels need
   stronger confidence gates.
9. Capture provenance and confidence at the profile orchestration/session
   boundary, never inside aligned decode kernels.
10. Hybrid sharing may only add information through adapter/session boundaries
    and upstream-shaped context entry points. It must not change the internal
    scoring, thresholds, candidate search order, or arithmetic of
    `lib_wsjtx`/`lib_jtdx`. Clarification: feeding legitimate context through a
    real entry point (e.g. `hiscall`/`nfqso`) is allowed even though the decoder
    *itself* then schedules different AP passes — that is the decoder's own
    documented context-driven behavior. What is forbidden is hybrid reaching in
    to reorder candidates, alter gates/metrics, or change the kernel's internal
    decisions. "Feed context, let the decoder decide" is in scope; "reach in and
    change how it decides" is not.

Operational restatement (must always hold):

- Run WSJT-X and JTDX sessions independently; share immutable slot samples.
- Share only documented hybrid-layer knowledge through adapter/session
  boundaries.
- Do not share AP memory, odd/even memory, residual buffers, or duplicate state.
- Deduplicate decoded rows before CLI/UDP output.
- Preserve internal source attribution: `wsjtx`, `jtdx`, or `both`; keep normal
  user output clean (no attribution by default).

A row should be traceable to one decoder, both decoders, or a documented hybrid
hint path — never to mixed state that no upstream decoder owns.

## Gain Types

Do not combine different benefits into one "decoded count" number:

| Gain | Meaning | Primary metric |
|---|---|---|
| `completeness_gain` | An already decoded row becomes more useful, e.g. `<...>` resolves to a full call | resolved rows, hash conflicts |
| `sensitivity_gain` | A row that no single profile decoded becomes decoded in hybrid | new rows vs best single profile |
| `stability_gain` | Rows normalize/dedupe more consistently across profiles | fewer representation-only diffs |
| `false_positive_cost` | A new suspicious row appears | extra rows not supported by the baseline |
| `latency_cost` | Sharing adds extra passes/replays | per-slot decode time, monitor delay |

Every channel report must label its gain type. `HashCallHint` mainly targets
`completeness_gain`; `QsoContextHint` and `SameParityA7` target
`sensitivity_gain`.

### Expected Gain Ceiling

Judge the project against a realistic ceiling:

- The plain hybrid **union** (already implemented) is where the big cross-engine
  sensitivity benefit lives — each engine's genuinely unique decodes (WSJT-X
  `a8` list, JTDX FT8S/deep, divergent AP) already reach output through the
  union. Shared knowledge does **not** add these.
- Shared knowledge chases the **second-order tail**: "one decoder's output feeds
  another decoder's AP so it finds even more." That tail is inherently marginal.
- The strongest sensitivity lever (shared residual/subtraction, shared soft
  candidates) is **barred** by the alignment constraints. So message-level
  sharing is structurally capped at a small sensitivity contribution.

The size of the `sensitivity_gain` tail depends on the use case:

- **SWL / monitoring (primary target):** the listener cannot configure QSO
  context, so cross-decoder-discovered context (active calls, resolved hashes)
  is genuinely unconfigurable value. Here the tail is the *main justification*
  and can be meaningful.
- **Participating operator:** much of the discoverable context is already
  available via `--my-call`/`--his-call`, so the incremental tail is thinner.

Realistic expectation: the durable deliverable is `completeness_gain` (resolved
`<...>`) plus `stability_gain`; the `sensitivity_gain` tail is small for
operators but can matter for SWL. Phase 0 sizes that tail **in the no-context
SWL condition** and decides whether it justifies the complexity.

## Channel Priority

Shared knowledge is split into channels, implemented in safety/benefit order:

| Priority | Channel | Purpose | Lifetime | Default Risk | Current Decision |
|---:|---|---|---|---|---|
| 1 | `HashCallHint` | Share full-call mappings so another decoder can resolve `<...>` | session-long | low | shipped: safe completeness infra |
| 2 | passive evidence/reporting | Explain which decoder found each row and why rows differ | per slot + diagnostics | low | shipped: required guardrail |
| 3 | `QsoContextHint` | Share confirmed call pairs / frequency context via `mycall`/`hiscall`/`nfqso` inputs | sliding/session | medium-low | passive only; per-hint replay killed |
| 4 | `ApCallbookHint` | Share confirmed active calls into a decoder's AP call set (apset-style) | session-long/sliding | medium-low | future-only; needs new evidence |
| 5 | `SameParityA7` | Feed confirmed previous same-parity rows into WSJT-X-style `a7` memory | previous same-parity slot | medium-high | killed for current fixture |
| 6 | `FalseFilterCallbookHint` | Route confirmed calls into JTDX `searchcalls`/`chkfalse8` filtering | session-long | high (off by default) | killed/disabled |
| 7 | AP/deep imports | Feed AP/deep/list-assisted rows into another decoder's AP/deep paths | channel-specific | high | killed/disabled |

The old single `CallbookHint` is split into `ApCallbookHint` (feeds an AP call
set; medium-low risk; does not change the false-decode gate) and
`FalseFilterCallbookHint` (routes calls into JTDX `searchcalls`/`chkfalse8`,
which changes the accepted set — high risk, off by default). JTDX `searchcalls`
is fed by a static `ALLCALL7.TXT`; routing dynamic calls into it is a new mutable
injection point and must be FP-measured before it is ever enabled.

### Purpose-Led Decisions

The purpose is not to make the two mirrored decoders less independent; it is to
let a coordinator use **accepted messages** from one decoder to improve the next
hybrid decision, while preserving each decoder as an auditable upstream-aligned
implementation.

| Decision | Reason |
|---|---|
| Start with `HashCallHint`, not `SameParityA7` | Hash-call sharing is the safest, most deterministic channel; improves completeness without inventing a message. |
| Measure `QsoContextHint` before a7 import | Context can unlock real sensitivity in SWL/no-context operation; a7 import may have little marginal gain if JTDX already found the follow-up. |
| Keep a7 as a later, evidence-driven channel | a7 needs parity, confidence gates, AP-on-AP prevention, monitor scheduling. Do not let it be the project gate; kill with evidence if gain ≈ 0. |
| Separate AP call hints from false-filter callbook hints | Feeding an AP candidate set vs. a false-decode rejection filter are different risk classes. |
| Prefer bounded deterministic hints over unbounded replay | Monitor mode has a hard latency budget; file mode must be reproducible. |

Practical target order:

```text
shared hash-call resolution
  -> passive evidence/confidence/reporting
  -> passive QSO-context opportunity measurement
  -> stop unless a new fixture shows real sensitivity headroom
  -> future: bounded AP call-set import (not per-hint full-slot replay)
  -> future: same-parity a7 import only if measured worthwhile
  -> future: higher-risk AP/deep/false-filter imports only behind flags
```

The current fixture results made the durable milestone ship only the low-risk
hash/evidence/reporting layer and **not** active message-to-AP feedback.

## Architecture

```text
input slot samples
        |
        v
HybridCoordinator
        |
        +-- DecoderAdapter(wsjt-x)
        +-- DecoderAdapter(jtdx)
        +-- DecoderAdapter(future decoder...)
        |
        v
accepted rows + provenance + confidence
        |
        v
SharedKnowledgeStore
        +-- SharedHashCallBook   (session-long, collision-safe)
        +-- ActiveCallContext    (sliding/session)
        +-- SameParityMemory     (previous/current by parity)
        +-- DecodeEvidenceStore  (dedupe, confidence, attribution)
        |
        v
merge / dedupe -> CLI / UDP outputs
```

Layer boundary:

```text
lib_wsjtx / lib_jtdx    = upstream-shaped decode kernels (untouched)
profile session layer   = knows which path produced each row
DecoderAdapter          = profile-specific import/export bridge + provenance tag
HybridCoordinator       = policy, scheduling, dedupe, confidence
SharedKnowledgeStore    = neutral shared facts, with per-channel lifetimes
```

Implementation boundary: `src/decode/hybrid/`. Shared evidence uses an open
decoder source id with `wsjtx`/`jtdx` constants (not a closed two-value enum), so
future decoders can join the same store without redesigning confidence/dedupe.

### Decoder Adapter Interface

```rust
trait HybridDecoderAdapter {
    fn id(&self) -> DecoderId;
    fn import_capabilities(&self) -> &[ImportCapability];
    fn begin_slot(&mut self, timestamp: SlotTimestamp, imports: &[SharedKnowledgeImport]);
    fn decode_slot(&mut self, samples: &[f32]) -> Vec<DecoderOutput>;
}

enum ImportCapability {
    HashCallHint,
    QsoContextHint,
    ApCallbookHint,
    SameParityA7,
    FalseFilterCallbookHint, // high risk, off by default
}
```

There is no separate export hook. The coordinator builds shared knowledge from
`decode_slot` outputs by applying admission, dedupe, confidence, and channel
policy.

## Shared Knowledge Views

Do not force every shared fact into an `a7`-style parity memory model.

### SharedHashCallBook

Session-long mapping from FT8 hash values to full calls. It is the *lowest-risk*
first channel because it never invents a message or changes LDPC/OSD math: it
only lets another decoder resolve a hash call when a full-call mapping is known.

**It is not output-neutral — resolution can change the accepted/AP set, so it
must be measured, not assumed display-only:**

- WSJT-X `a7` skips memory entries containing `<`. Resolving a `<...>` row to a
  full call makes it `a7`-eligible, which can change later AP decodes.
- JTDX gates `lhashmsg`/`delbraces` on resolution state to keep hash-braced
  messages out of odd/even AP memory; changing resolution can change what enters
  JTDX AP memory.

This is the mutable hash book threaded into `ft8b`, not the static `ALLCALL7.TXT`
that feeds JTDX `searchcalls`/false-decode filtering; the static DB is untouched.

Rules:

- export to the shared book only full calls from `Regular` / `ConfirmedRegular`
  / `ConfirmedMulti` evidence. `Assisted` (a7/a8/deep/list) full calls may update
  a decoder's *private* book but are not exported — an assisted false call in the
  shared book would poison every decoder's resolution and AP. (Phase 1, before
  the evidence store exists, gates on the cheap adapter-local `Regular`
  provenance; the `ConfirmedMulti` tier arrives with Phase 2.)
- never overwrite a conflicting mapping silently;
- if one hash has multiple full-call candidates, mark it ambiguous; do not force
  resolution;
- unresolved `<...>` rows may still be output, but must not become AP seeds;
- pure `wsjtx`/`jtdx` sessions keep private hash books only.

**Injection timing is pinned**: the shipped version injects the shared book at
the **slot boundary, before decode/unpack** (cross-slot, deterministic) — this
yields the a7/lhashmsg behavioral benefit and therefore must be FP-measured.
Same-slot sharing is deferred or done as deterministic post-processing only,
never as a scheduling-dependent real-time feedback path.

Record **who resolved a displayed call** separately from decode provenance
(`display_resolution: Native | SharedBook`). A `SharedBook` resolution does not
change the row's decode `Provenance` (it stays `Regular`).

### ActiveCallContext

Sliding/session view of confirmed calls, call pairs, approximate frequencies,
grids, and recent reports, fed through existing config concepts (`hiscall`,
`nfqso`).

**This is the channel that can actually raise the decode count** (context
unlocks deep/MyCall AP — e.g. a -16 dB reply recoverable only with `hiscall`
context), so it carries the highest payoff *and* the hardest open design
problem: `hiscall`/`nfqso` are **single-QSO-shaped** (one target applied against
every candidate), but hybrid decodes the whole band. The measured per-hint
full-slot replay architecture is rejected (see Phase 3). Candidate architectures
are now limited to: a bounded call list seeded into the AP call set; a very
small most-likely-partner set per slot with a hard latency cap; or keeping
context passive for reports only.

Initial rule: build hints only from `Regular`/`ConfirmedRegular` evidence; never
from `ImportedMemory`, all-assisted agreement, unresolved hash calls, or
suspicious rows.

### SameParityMemory

Previous/current rows by neutral FT8 parity, only for channels that need
previous same-parity semantics (WSJT-X-style `a7`). Bounded; never a general
session history.

### DecodeEvidenceStore

The coordinator's dedupe and attribution view: normalized row identity, decoder
sources, provenance, confidence, and import eligibility.

## Provenance Capture

The confidence model needs to know how a row was decoded; that information
already exists at the profile session boundary, so no kernel change is required:

- WSJT-X rows are assembled from separate regular, `a7`, and `a8` paths in the
  stream/session layer.
- JTDX rows expose `source` and `iaptype` in the JTDX session/result layer.

Adapters emit an additive `DecoderOutput { row, provenance }`; the public decoded
row type does not change.

```rust
enum Provenance {
    Regular,         // plain regular decode, no AP priors
    ApMask,          // current-slot bit-mask AP only
    A7Memory,        // cross-slot / QSO-memory-assisted decode
    A8List,          // WSJT-X ft8_a8d list decode
    JtdxDeep,        // JTDX deep / superdeep / FT8S / FT8SD
    ImportedMemory,  // decoded only because of a hybrid import/hint
}
```

JTDX `iaptype` is classified by range/context, not by `>0` alone. When the
adapter cannot prove a row is current-slot-only `ApMask`, it downgrades to
`A7Memory`/assisted (keeps trust symmetric with WSJT-X `a7`).

`ImportedMemory` is terminal: kept for attribution/dedupe/debug, but never a
future import seed. A hash-call resolution does **not** make a row
`ImportedMemory` — its decode did not depend on the hint, only its display did,
so it keeps its own provenance.

## Confidence Model

Confidence is the false-positive control used by higher-risk channels.

```rust
enum DecodeConfidence {
    ConfirmedMulti,   // >=2 non-imported sources, with >=1 Regular/ApMask source
    ConfirmedRegular, // single decoder, Provenance::Regular
    ConfirmedAp,      // single decoder, Provenance::ApMask
    Assisted,         // A7Memory / A8List / JtdxDeep / ImportedMemory / all-assisted quorum
    Speculative,      // diagnostic only; never materialized as shared knowledge
}
```

Rules:

- `ImportedMemory` never contributes to quorum or tier; the row is rated on the
  remaining independent evidence, and is `Assisted` if nothing independent
  remains.
- A quorum made only of assisted sources is capped at `Assisted`, not promoted
  to `ConfirmedMulti`.
- Import eligibility keys off the tier alone (`ConfirmedMulti`/`ConfirmedRegular`
  eligible); pure-`ImportedMemory` rows classify as `Assisted` and are therefore
  ineligible.
- `HashCallHint` may start with lighter rules but must stay collision-safe.
- `QsoContextHint` / `SameParityA7` start from `ConfirmedRegular` only.

### Neutral Shared Row Model

```rust
struct SharedDecode {
    message: String,
    normalized_message: String,
    freq_hz: f64,
    dt_sec: f64,
    snr_db: i32,
    timestamp: SlotTimestamp,
    parity: usize,
    sources: SmallVec<[DecoderSource; 4]>, // inline cap is not a decoder limit
    confidence: DecodeConfidence,
    evidence: Vec<DecodeEvidence>,
    import_eligible: bool,
}
```

Store-admission and import eligibility are separate decisions. `SharedDecode` is
specifically a *decoded row*; non-row shared facts (hash mappings, context hints)
use distinct types so "shared knowledge" is not assumed to be all decode rows.

## Parity Bridge

Only `SameParityA7` needs parity. The neutral parity must match both profiles:

```text
WSJT-X  jseq = (nutc / 5) % 2        -> {:00,:30} = 0, {:15,:45} = 1
JTDX    IntervalKind::from_timestamp -> {:00,:30} = Even, {:15,:45} = Odd
```

So `Even -> 0`, `Odd -> 1`. A unit test asserts the mapping for all FT8 slot
seconds; this is not left as an implicit assumption.

## Admission & Import Policy

A decoded row may enter `DecodeEvidenceStore` if it was accepted through the
decoder's normal final gate, is non-empty, is in-passband, has a sane FT8 `dt`,
is not a same-slot duplicate, and is useful for at least one channel.

Exclude from import eligibility (at first) if: contains unresolved `<...>`; free
text/telemetry; outside supported FT8 QSO families; provenance is `Speculative`
or `ImportedMemory`; or frequency/DT conflicts suspiciously with a stronger row.

Initial import-eligible message shapes:

```text
CALL1 CALL2 GRID4 | REPORT | RREPORT | RRR | RR73 | 73
CQ CALL GRID4
```

Per-channel import rule:

| Channel | Default source confidence | Default target |
|---|---|---|
| `HashCallHint` | `Regular`/`ConfirmedRegular`/`ConfirmedMulti`, collision-free | all other decoders |
| `QsoContextHint` | `ConfirmedRegular` only | passive only; per-hint replay killed |
| `ApCallbookHint` | `ConfirmedRegular` only | disabled this milestone |
| `SameParityA7` | `ConfirmedRegular` only | diagnostic helper only; shipped path killed |
| `FalseFilterCallbookHint` | disabled | killed/disabled |
| AP/deep import | disabled | killed/disabled |

General rules: never import a row into a decoder already in its `sources`; never
import `ImportedMemory`; never promote an imported result into a new seed;
unsupported/killed capabilities are ignored by the adapter; pure profiles never
see shared imports.

Barring `Assisted` rows from *import* does **not** drop them from output — they
still reach CLI/UDP via the union; the bar only stops them seeding another
decoder's AP (second-order propagation).

## Deduplication

Output dedupe policy:

```text
same slot, same normalized message, |Δfreq| <= 5 Hz, |Δdt| <= 0.3 s
```

Normalization handles repeated/surrounding whitespace, resolved hash-brace
forms (`<RK4FF>` ≡ `RK4FF`), and SNR/freq/DT differences for the same message.
Do not over-normalize: two real rows with the same text but clearly different
freq/DT are not collapsed unless they are the same signal. Unresolved `<...>`
stays distinct.

Keep the dedupe layers separate: **output dedupe** controls CLI/UDP; **evidence
dedupe** merges decoder sources and computes confidence; **hash-call dedupe**
handles collisions/ambiguity. A merge in the evidence store must not
retroactively change an already-emitted output row.

## Output Policy

Normal CLI rows stay clean: `HHMMSS snr dt freq message`. UDP receives
deduplicated decoded messages only. Debug/test tooling may expose
`source=wsjtx|jtdx|both`.

## Threading Model & File-Mode Reproducibility

File mode (one-shot): read WAV → split into slots → run both workers on the full
slot per slot → merge → output deduplicated rows. Monitor mode is **staged**: the
WSJT-X worker runs its real upstream `nzhsym=41/47` early decode and streams those
rows *before* the slot boundary; at the boundary (`nzhsym=50`, full audio) it
finishes the WSJT-X pass and runs JTDX, then emits JTDX-unique rows after JTDX
finishes and a slot summary after both finish. JTDX has no early sub-results, so
it only runs at `nzhsym=50`. The staged and one-shot paths emit the **identical
row set** — only the timing differs — asserted by `test_hybrid_staged_matches_oneshot`.

Use shared input ownership (`Arc<[f32]>`) to avoid duplicating buffers. Each
worker owns its own FFT, residual, AP, odd/even, and duplicate state. Hash-call
sharing happens only by importing safe full calls into each profile's existing
session hash book before a slot decode starts.

Reproducibility rules (must hold so file-mode tests are not flaky):

- imports for slot N use only knowledge committed before slot N begins;
- no decoder waits on another decoder's in-flight same-slot output;
- same-slot hash sharing, if added later, is deterministic post-processing, not
  scheduling-dependent real-time feedback;
- monitor mode may keep late knowledge for future slots/attribution/debug, but
  must not retroactively change already emitted rows.

## Current Status

Implemented / shipped this milestone:

- `profile=hybrid` selectable; one WSJT-X + one JTDX session owned by the runner;
  file mode runs both workers on the full slot per slot (one-shot). Monitor mode
  drives the WSJT-X worker's staged `nzhsym=41/47/50` decode so early WSJT-X rows
  stream *before* the slot boundary, with JTDX run once at `nzhsym=50`; JTDX-unique
  rows emit after JTDX finishes. Both paths emit the same row set
  (`test_hybrid_staged_matches_oneshot`);
- session-long `SharedHashCallBook`: only adapter-local regular full-call
  evidence is exported, collisions suppressed, safe calls imported into each
  decoder's private book at slot boundaries;
- passive evidence/confidence model (`SharedEvidenceStore`, `Provenance`,
  `DecodeConfidence`) built per slot after both workers finish; not used to
  change output or feed imports; open source-id type (not limited to two
  decoders);
- bounded `ActiveCallContext` / `QsoContextHint` scaffold derives candidate
  hints from confirmed regular evidence but injects nothing in normal operation;
- dedupe treats `<RK4FF>`/`RK4FF` as the same message and keeps internal
  attribution; JTDX no longer returns protected WSJT-X fallback rows.

Deliberately not shipped (killed/disabled with evidence below): `QsoContextHint`
injection, `SameParityA7` import, `ApCallbookHint`, `FalseFilterCallbookHint`,
AP/deep imports. The evidence store distinguishes WSJT-X regular/a7/a8 and JTDX
regular/deep; JTDX `iaptype>0` is conservatively treated as assisted memory.

## Phase 0 Measurement

The exploratory hybrid opportunity tests used during this phase have been removed
from the normal test tree. The measured results are kept here so the old
decisions remain auditable without carrying long-running diagnostic code.

All recovery numbers are reported **relative to the best single profile** (JTDX
alone), not only hybrid-off-vs-on, and **in the no-context SWL baseline** (the
listener cannot configure context for someone else's QSO, so discovered context
is the value hybrid uniquely adds). The divergence set is characterized, not just
counted, by message type / SNR / AP-provenance / decoder.

Observed on the long fixture with the default `rustfft` engine:

```text
HashCallHint:    unresolved_hash_rows=17  resolvable_by_other_decoder=0  hash_conflicts=40
QsoContextHint:  slots_with_hints=19  total_hints=76  max_hints_in_slot=4
Divergence:      rows=465  shared=423  wsjtx_unique=11  jtdx_unique=31  representation_only_diffs=2
  unique_by_provenance={Regular: 16, A7Memory: 7, JtdxDeep: 19}
  unique_by_message_class={Cq: 20, Grid: 10, Report: 9, RReport: 1, Rr73: 1, Hash: 1}
  unique_by_snr_bucket={VeryWeak: 12, Weak: 23, Mid: 7}
FalsePositiveCost: wsjtx_unique_supported=11 wsjtx_unique_unsupported=0
                   jtdx_unique_supported=29 jtdx_unique_unsupported=2
```

Interpretation:

- `HashCallHint` is collision-safe and shipped, but this fixture shows no
  same-signal `<...>` row another decoder can resolve. It is a low-risk
  completeness/stability feature, not a measured sensitivity lever here.
- `QsoContextHint` has enough candidate context to justify investigation, but the
  measured replay path was slow and produced unsupported rows; kept passive.
- `SameParityA7` is not a good current target: the divergence set is not
  dominated by a7-style opportunities (many unique rows are regular or JTDX deep)
  and the replay recovered no rows.
- The hybrid union has a small false-positive cost (2 unsupported JTDX-unique
  rows). Any active import must report whether it increases this number.

## Active-Channel Experiments (killed)

These experiments were diagnostic-only and are no longer kept as runnable tests.
The results below are historical measurements from the long fixture.

### QSO Context Replay

Replays the slot through a temporary JTDX session using ≤4 context hints
committed before the slot starts (via `hiscall`/`nfqso`), comparing replay rows
against the ordinary union and splitting added rows into CSV-supported vs.
unsupported.

```text
attempted_hints=72  added_rows=2  supported=0  unsupported=2  elapsed=643.85s
```

**Killed-with-evidence:** per-hint full-slot replay through `hiscall`/`nfqso` is
not viable — no supported gain, two unsupported rows, unacceptable streaming
latency. This kills this *injection architecture*, not all future QSO-context
work. Any future attempt must use a lower-latency bounded AP call-set import and
start from the same false-positive gate.

### Same-Parity A7 Replay

Imports JTDX-only regular rows from the previous same-parity slot into a
temporary WSJT-X session as a7 seeds, counting only `A7Memory` replay rows not
already in the union.

```text
attempted_slots=8  imported_seeds=8  added_rows=0  supported=0  unsupported=0  elapsed=203.32s
```

**Killed-with-evidence:** no new rows on this fixture. The session-boundary import
helper stays as a diagnostic/audit tool; normal hybrid does not enable it.

### Higher-Risk Channels

| Channel | Decision | Reason |
|---|---|---|
| `ApCallbookHint` | disabled | No low-latency import design has positive evidence; nearest measured path (per-hint replay) added 0 supported / 2 unsupported. |
| `FalseFilterCallbookHint` | killed/disabled | Dynamic calls in JTDX false-decode filtering change the accepted set; union already has 2 unsupported JTDX-unique rows. Needs a dedicated FP fixture + explicit flag. |
| AP/deep imports | killed/disabled | Highest AP-on-AP risk. Assisted rows stay output-visible but non-importable; `ImportedMemory` is terminal. |

These are terminal outcomes for the current fixture/milestone, reopened only with
a new fixture, explicit flag, and measured false-positive budget.

## Results Summary

| Channel | Gain Type | Measured Gain | False-Positive Cost | Latency Cost | Decision |
|---|---|---:|---:|---:|---|
| `HashCallHint` | completeness/stability | unresolved=17, resolvable=0, conflicts=40 | no new decode rows measured | slot-boundary import only | shipped |
| `QsoContextHint` per-hint replay | sensitivity | added=2, supported=0 | unsupported=2 | 643.85s | killed |
| `SameParityA7` | sensitivity | added=0 | unsupported=0 | 203.32s | killed |
| `ApCallbookHint` | sensitivity | not activated | blocked by replay FP result | not incurred | killed/disabled |
| `FalseFilterCallbookHint` | false-positive filtering | not activated | high; JTDX unique unsupported=2 | not incurred | killed/disabled |
| AP/deep imports | sensitivity | not activated | high AP-on-AP risk | not incurred | killed/disabled |

## Success Criteria & Definition of Done

This is a measure-first effort: fixed numeric decode targets are deliberately
**not** set up front (Phase 0 measures them). "Done" is terminal state + evidence.

Hard gates (non-negotiable, pass/fail; no channel merges unless all hold):

| Gate | Pass condition |
|---|---|
| Pure-profile alignment | `wsjtx` 21/21 short, 424/424 long; `jtdx` 20/20 short, 430/431 long — row-for-row unchanged |
| Kernel/constraint integrity | constraints 1–3, 9, 10 hold |
| File-mode reproducibility | identical output across repeated runs, independent of thread scheduling |
| False-positive budget | new-row FP count within a recorded budget (a number, not "feels low") |

Per-channel terminal state (each channel must reach one, *with evidence*):
**Shipped-with-evidence** (gates green; gain/FP/latency recorded in the no-context
SWL baseline; `gain > cost`) or **Killed-with-evidence** (gain ≈ 0 or cost
unacceptable; negative result recorded here). Killed-with-evidence is a valid
completion; a channel left unmeasured is **not** done.

Project Definition of Done: every Channel Priority entry is in a terminal state;
all hard gates green for whatever shipped; the Results Summary records per-channel
{gain type, measured gain, FP cost, latency cost, decision}; and HYBRID/JTDX/
WSJTX docs reflect what shipped. "Done" does not mean every channel ships —
shipping only `HashCallHint` with the rest killed-with-evidence is a legitimate
completed outcome.

## Verified Gates

Latest release-mode checks with the default `rustfft` engine:

```text
profile=wsjtx short: 21/21
profile=wsjtx long:  WSJT-X baseline 424/424, total 434/458
profile=jtdx short:  20/20, elapsed 5.9s
profile=jtdx long:   JTDX baseline 430/431, total 446/458, each segment <15s
hybrid file mode:    repeated long-file output identical; total 465
hybrid staged==file: test_hybrid_staged_matches_oneshot (monitor staging emits
                     the identical per-slot row set as the one-shot file path)
```

Baseline row selection: WSJT-X profile uses `Extra=blank/W` (ignores `J/E`);
JTDX profile uses `Extra=blank/J` (ignores `W/E`); hybrid comparison diagnostics
compare against all CSV rows unless a diagnostic says otherwise.

Required checks before accepting hybrid changes:

```bash
cargo fmt --check
cargo check
cargo test --release test_stream_decode_short_audio
cargo test --release test_stream_decode_long_audio
```

Hybrid-specific: pure-profile baselines unchanged; file-mode hybrid reproducible
across runs; import-off/on reports by channel; added/lost/false-positive rows by
confidence tier.

## Current Comparison

Hybrid is compared against every row in the fixture CSV, ignoring the `Extra`
marker (unlike pure-profile tests). Current long-fixture observation:

```text
230208_140300.wav, profile=hybrid:
  CSV rows: 458   decoded rows: 465   matched rows: 457   missing rows: 1   extra decoded rows: 8
```

Hybrid should not become a hard gate until WSJT-X stays stable, JTDX has a
measured baseline, and JTDX AP/deep + false-positive filtering are stable enough
to trust the union.

## Recommended Current Implementation

Keep the milestone small and boring:

1. `SharedHashCallBook` is the only active shared-knowledge channel
   (session-long, collision-safe, exported only from regular evidence).
2. Keep `SharedEvidenceStore`, confidence, provenance, divergence reporting — the
   safety foundation for future work.
3. Keep `ActiveCallContext` passive (diagnostics only; no re-decodes/imports).
4. Keep `SameParityA7`, `ApCallbookHint`, `FalseFilterCallbookHint`, AP/deep
   imports disabled/diagnostic-only until a new fixture gives a positive,
   reproducible result with an acceptable false-positive budget.
5. Treat the hybrid union as the sensitivity deliverable; treat shared knowledge
   as completeness/stability infrastructure unless future evidence proves a
   sensitivity channel.

Prefer missing a possible second-order gain over importing a questionable
message. Sensitivity widens only after false-positive behavior is measured.

## Future Research

Disabled by default, each a new explicit/testable/documented hybrid phase:

- a lower-latency bounded AP call-set import (new fixture + explicit FP budget);
  do not repeat per-hint full-slot replay as a shipped architecture;
- dynamic false-filter callbook experiments behind an explicit flag;
- AP/deep imports with a non-recursive `ImportedMemory` gate;
- experimental profile files for non-baseline tuning.

## Do Not Do

- Do not share decoder-private state.
- Do not mutate mirrored decoder scoring, thresholds, candidate order, or
  arithmetic for hybrid.
- Do not use hybrid to hide JTDX shortcuts.
- Do not emit duplicate UDP reports for the same decoded row.
- Do not promote hybrid count as the primary sensitivity metric until the two
  pure profiles are understood.
