# DX Chase Decode Notes

This document records `--profile dx`: a single-target DXpedition-pursuit
decoder built as an orchestration layer on top of the existing aligned WSJT-X and
JTDX kernels. It does not introduce a new decode algorithm and does not modify
`lib_wsjtx`/`lib_jtdx`.

Related documents:

- WSJT-X alignment: `WSJTX.md`
- JTDX profile: `JTDX.md`
- Hybrid result-union + shared knowledge: `HYBRID.md` (separate profile; `dx`
  does not depend on it and is not validated through it)

`dx` is intentionally independent: its context store, target filter, confidence
rules, and merge behavior live under the DX orchestration layer. If any code is
ever shared with `hybrid`, that should be a later explicit refactor after both
profiles are stable, not part of the DX design.

## Scope

```text
profile=dx -> JTDX SWL worker (sensitivity + harvest)
              -> focused JTDX swl+nagain worker (target-frequency recovery)
              -> focused WSJT-X a8d / MyCall-AP worker (target-frequency recovery)
              -> single-target context harvest (cross-slot, deterministic)
              -> hiscall output filter
```

Goal: given one target station (`hiscall`), produce the most reliable possible
reception of **that station's** transmissions, by automatically turning a
no/low-context situation into a full-context one. Everything not related to
`hiscall` is suppressed from output (but may be used internally for harvest).

This is a single-target chase profile, not a general result-union or shared-memory
mode. Its cost stays bounded because the target is **one known callsign**, not
the whole band — there is no per-partner replay explosion.

## Non-Negotiable Constraints

Carries forward the same top-level safety discipline used elsewhere in the
project, plus the chase-specific rules the owner set:

1. **Do not change `lib_wsjtx`/`lib_jtdx` alignment semantics.** The original
   decoders stay aligned and independent. This is the top principle. Any
   aggressive-strategy work that would touch kernel thresholds/candidate
   order/AP scheduling is out of scope for the shipped path (see
   "Aggressive-Strategy Evaluation").
2. **`hiscall` is mandatory** to start chase. Without it the mode does not run.
3. **`mycall` is optional.** Without `mycall` there is **no a8d and no
   MyCall-AP** (both need `mycall`); the rest (SWL harvest, focused regular/AP
   on the target frequency, hiscall filtering) still runs.
4. **Output only `hiscall`-related rows.** All other decoded rows are used
   internally (frequency/grid/dt harvest) but not emitted.
5. **No callsign list / no `ALLCALL7.TXT` dependency for chase logic.** The
   single `hiscall` is the only acceptance anchor. (A JTDX SWL worker still
   mirrors `searchcalls` internally as part of its alignment, but its result
   does not affect what chase emits, because the hiscall filter supersedes it.)
6. **All cross-decoder/feedback behavior lives in the chase orchestration
   layer**, fed back to workers only through documented config entry points
   (`mycall`/`hiscall`/`hisgrid`/`nfqso`/`nftx`/`napwid`/`nQSOProgress`).
7. **Feedback is cross-slot only** (slot N uses knowledge committed before slot N
   begins), so file mode stays reproducible. No same-slot re-decode feedback. The
   implementation snapshots foci, `hisgrid`, target dt, QSO progress, and deep
   hypotheses at slot start; listen/focused harvest from the current slot can only
   affect later slots.

## Use Case

Chasing one DXpedition/rare station:

- the target (`hiscall`) is known;
- the target's frequency may be **known**, **unknown**, or **multiple**
  (Fox/Hound "FH" mode, where the Fox transmits several streams per slot);
- `hiscall` may be a **compound/nonstandard callsign** (e.g. `EA5/DH0YAH`,
  `DH0YAH/P`), which FT8 transmits as a 10/12/22-bit hash and displays as
  `<...>` until resolved;
- the target may be **working other stations** (we listen in to harvest its
  grid/frequency/dt) or **calling/replying to us** (then a8d/MyCall-AP recovers
  it at very low SNR).

## Required vs Optional Context

| Input | Required? | Unlocks |
|---|---|---|
| `hiscall` | **required** | output filter, hash match if compound, frequency association |
| `mycall` | optional | **a8d** (needs mycall+hiscall+hisgrid+nfqso) and **MyCall-AP** (`iaptype=2`, needs mycall + frequency focus) |
| `hisgrid` | optional, **harvested** | promotes MyCall-AP recovery to full **a8d** list decode |
| frequency (`nfqso`) | optional **seed**, also **harvested** | focuses MyCall-AP/regular near the target QSO |

The defining trick: **`hisgrid` and `nfqso` are normally unavailable to a chaser,
but are harvestable by listening to the target QSO.** Harvesting them converts a
no-context decode into the documented full-context recovery path.

## Architecture

```text
                ┌──────────────── ChaseCoordinator ────────────────┐
 slot samples → │                                                   │
                │  Worker A: JTDX SWL high-sensitivity (lhound opt.) │ listen in,
                │     │ confirmed decodes                            │ discover target
                │     v                                              │
                │  TargetContextStore (single target)                │
                │     · hiscall = operator-given (anchor)            │
                │     · hisgrid <- harvested from target CQ/QSO       │
                │     · freq set <- frequencies where the target QSO  │
                │                   appears (drift-aware, FH = many)  │
                │     · recent dt / SNR trend                         │
                │     │ committed before next slot (deterministic)    │
                │     v                                              │
                │  Worker B: focused recovery (next slot)            │
                │     · MyCall-AP: mycall + nfqso = harvested freq    │
                │     · a8d: mycall+hiscall+hisgrid (harvested)        │
                │     │                                              │
                │     v                                              │
                │  hiscall output filter + confidence/freq/grid gate  │
                └──────────────────────→ deduped output ─────────────┘
```

Both workers are existing aligned kernels, unchanged. The coordinator only
selects config and post-filters rows.

## Target Context Harvest

Cross-slot, deterministic. From every confirmed decode in slot N, update the
single-target store (used for slot N+1 and same-parity slots):

- **Frequency association.** The target's candidate frequency set is seeded by
  the user-provided `nfqso`/`nftx` (if any, and only when inside the configured
  passband) **and** grown by harvest. Both sources are kept and used together — a
  user-supplied frequency is a trusted, pinned anchor (it does not age out),
  while harvested frequencies cover the cases where the user value is
  approximate, stale, drifting, or one of several (FH). The focused worker
  decodes at the union of {user frequency} ∪ {harvested frequencies}, with every
  focus clamped to the passband before worker config is built.
  **Reliability differs by role:** a row where the target is the *sender* gives
  the target's own TX frequency — exact, and the only reliable source under FH
  (where the Fox transmits low and hunters call high, so a hunter's frequency is
  *not* the Fox's). A row where the target is the *addressee*, or a `mycall`-as-
  addressee row (another station working us), gives the *QSO* frequency, which
  equals the target's only in **simplex** — keep these as weaker hints outside
  Hound mode. In Hound mode, recipient/`mycall` rows inform parity only; they do
  **not** seed focus frequencies, because hunter TX frequency is not Fox TX
  frequency. (This is why `1152 Hz` is harvestable from `140630 F1MLZ RA3ABG` in
  the Validation Target: that exchange is simplex.)
- **FH multi-stream grid (Hound mode).** A Fox does not sit on one frequency — it
  transmits up to five simultaneous streams and **changes that count dynamically**
  mid-pileup. Upstream `foxgen.f90` places stream *n* at `nfreq + 60·(n-1)` (fixed
  `fstep = 60 Hz`). So once **≥2** distinct Fox streams have been observed in Hound
  mode (where every harvested frequency is a target-as-sender stream), `dx` stops
  chasing only the seen frequencies and instead pre-places the **full 5-focus grid
  at 60 Hz** from the lowest observed stream — `base, base+60, …, base+240` (each
  clipped to the passband). This way a Fox that suddenly grows from 2 to 3/4/5
  streams is already monitored, not discovered a slot late. **Dynamic lowest
  (fallback):** the anchor is the running *minimum* of the live observed streams,
  so if the always-on full-band listen later decodes a lower stream we had missed
  (or the Fox moves down), the minimum drops and the whole grid shifts down next
  slot — no focus slot is spent on a separate downward probe, so upward coverage is
  preserved. The `60 Hz` spacing is hardcoded to match upstream WSJT-X DXpedition
  mode (MSHV/non-standard Foxes are out of scope).
- **Grid.** A `hisgrid` is harvested when the target sends a `CQ <call> GRID` or
  `<mine> <target> GRID` row. Once known, it promotes recovery from MyCall-AP to
  full a8d.
- **Drift.** The frequency set ages out with a sliding window so the focus
  tracks a moving target; stale frequencies are dropped. The aging calculation
  handles a UTC-midnight wrap (`23:59:xx -> 00:00:xx`) so monitor sessions do not
  keep stale foci forever.
- **dt (time offset) — confidence/window hint, NOT a recovery lever.** Unlike
  frequency, `dt` cannot be "fed" to focus the decode: `sync8` already searches
  all time lags in the window and `ft8b` refines `dt` to sub-symbol before LLR
  extraction, so the decoder finds and refines `dt` on its own. `dt` matters for
  both the sync gate (the metric is evaluated at the best lag) and soft-symbol
  quality (sub-symbol misalignment smears LLRs) — but the decoder self-optimizes
  both. So harvested `dt` is used only for: (a) **confidence/FP** — a target row
  near the expected/consistent `dt` is more trustworthy, a wildly off-`dt`
  "match" is suspect; and (b) a **window hint** — if the target consistently
  shows a large `dt` (clock error / long path), keep `swl` (±3.5 s window)
  and/or `--force-sync` on so the wide window keeps covering it (`avexdt`
  already auto-recenters the window from recent decodes).
- **TX parity (sequence) — inferred by sender/recipient role, not by mere
  presence.** FT8 QSOs alternate every 15 s by parity (`jseq = (nutc/5) % 2`;
  `{:00,:30}=0`, `{:15,:45}=1`); the target transmits on **one** parity, the other
  carries *us and other hunters calling the target*. **Crucially, in an FT8
  message `CALL1 CALL2 …` the sender is `CALL2`; `CALL1` is the addressee** (CQ is
  the exception: `CQ CALL …` → `CALL` is the sender). So `hiscall` merely
  *appearing* does not mean the target is on air — its role decides:
  - target **IS transmitting** this parity when it is the sender: `CQ HISCALL …`,
    or `OTHER HISCALL …` (e.g. `MYCALL HISCALL -04` = the DX answering us);
  - target is being **called** (not transmitting) when it is the addressee:
    `HISCALL HUNTER …` (e.g. `HISCALL MYCALL R-05` = us calling the DX).
  Mark sender-role observations as **strong** TX-parity evidence; if only
  recipient-role rows are seen, infer the opposite parity as the target's but mark
  it **weaker**. Never collapse this to "hiscall appears → target TX parity" —
  that mislabels `MYCALL HISCALL report` (the very message we chase) as hunter
  parity, and the gate would then skip the real target slot. Until parity is
  observed, treat both as the target's (no premature skipping). The opposite
  (hunter) parity still runs the cheap listen and is a main frequency-harvest
  source. **Observed parity ages out after the same 16-slot freshness window used
  for target frequency context**: while fresh, it can skip focused/deep work on
  the opposite slot; once stale, `dx` probes both parities again until a new
  sender-role target row refreshes the observation. This avoids permanently
  missing a DX that changes sequence or whose old parity evidence no longer
  applies.
- **FH / multiple frequencies.** The harvested frequency is a **set**, not a
  single value — a Fox/Hound DX emits several streams per slot. The store keeps a
  **bounded** set (Fox uses ≤5), and the focused worker does a bounded
  multi-focus pass (≤5 foci at the harvested frequencies, `nfqso ± 25 Hz` each),
  never an unbounded per-candidate replay. Enable JTDX `lhound` for the Fox tone
  tables and wide candidate window (`half_width` 60→290, `ft8apset.rs` hound
  templates). The single-`nfqso` config field is therefore only a *seed*; the
  chase coordinator iterates the whole set per slot.
- **Compound `hiscall`.** Chase only ever needs to hash the calls it actually
  cares about — `hiscall` (always) and `mycall` (if compound). `ihashcall` takes
  one call at a time, so **`{mycall, hiscall}` is the complete input**; no
  callsign list / `ALLCALL7.TXT` is involved. Feed those (at most two) calls into
  the worker `HashCallBook` so the kernel resolves their `<...>` forms natively,
  and additionally hash `hiscall` with the kernel `ihashcall` (the same protocol
  hash both decoders use; reused, not forked, for all of 10/12/22 bits) to
  match `<...>` rows at a harvested target frequency as candidate target
  transmissions. This should be implemented as DX-local hash/collision handling
  with a one- or two-entry book, not by depending on the hybrid shared-book
  machinery.
- **Harvest hygiene (stability invariant).** Cross-slot focus is a feedback loop,
  so a bad harvest can lock later slots onto the wrong frequency. Only harvest a
  frequency from rows that actually contain `hiscall`/`mycall` (not low-confidence
  noise); age out stale frequencies; keep the user `nfqso` seed plus multiple
  candidates rather than over-committing to one value. This keeps the loop from
  drifting onto QRM.

## Sensitivity Ceiling and the `--swl --nagain` Lever

The aligned decoder's maximum sensitivity is reached with two existing upstream
flags, no kernel change:

- **`--swl`** (JTDX `swl`): wide time search (jzb/jzt ±3.5 s), `fdif0=3.0`,
  plus the high-sensitivity base the JTDX session always applies
  (`nft8cycles=3` → `npass=9`, `lft8lowth` → lowest `syncmin`).
- **`--nagain`** (JTDX `nagainfil`): **OSD `ndeep=5`** — the maximum
  (`osd174_91` caps at 5) — and, when `nfqso` is in band, a focused
  `nfqso ± 25 Hz` decode window (`lib_jtdx/mod.rs` band-narrow). `ndeep=5` is
  exactly the deep search JTDX ships *commented out* for SWL (`ft8b.f90:1458`,
  noted as "+4 decodes at -23 dB, >15 s, many false decodes").

Both objections JTDX had to `ndeep=5` are removed by chase: the `nfqso ± 25 Hz`
focus (from a harvested frequency) makes it fast, and the `hiscall` output filter
discards the extra false decodes. So **DX-chase max sensitivity = `--swl
--nagain` at the harvested/target frequency.** This is config wiring only;
`lib_jtdx` is untouched, both flags mirror upstream, and the default (neither
flag, `nagain=false`) leaves the aligned baseline byte-for-byte unchanged
(jtdx long stays 430/431).

Empirically, `--profile jtdx --swl --nagain --my-call F1MLZ --rx-frequency 1152`
recovers the otherwise-missed `140700 F1MLZ UA3QNA -04` (see Validation Target).

**`--nagain` only helps the AP branch, and only with context + focus.** Measured
on the long fixture: `--swl --nagain` with **no** `mycall`/`hiscall`/frequency
produced **identical** output to plain `--swl` (454 vs 454 rows, 0 added) but took
**~6× longer** (765 s vs ~2 min). Root cause: `ndeep=5` from `ap_ndeep` is wired
only into the AP OSD call (`ft8b/regular.rs:168`); the regular non-AP OSD is
hardcoded `ndeep=3` (`ft8b/regular.rs:97`). With no context there is no AP, so
the deep search has nothing to work on. Consequence for chase:

- **Harvest pass must be plain `--swl`** (full-band, no `nagain`) — adding
  `nagain` there only burns time for zero gain.
- **`--swl --nagain` is only worth running focused**: with `mycall` (→ AP) and an
  `nfqso` in band (→ the `±25 Hz` window that also makes it fast). Never run
  `--swl --nagain` full-band with no context.

## Per-Slot Decode Strategy

Two recovery engines only: **AP** (MyCall/context AP, OSD depth 3→5; `nagain`
forces 5) and **a8d** (a separate 206-message list decoder, needs `hisgrid`).
`ndeep=5` is *AP at maximum depth*, not a distinct method. Two hard facts from
measurement drive the ordering:

- `ndeep=5` only does anything via the **AP branch**, so it needs `mycall`
  (no AP context → zero gain).
- `ndeep=5` is cheap only when **focused** (`nfqso ± 25 Hz`); full-band it is
  ~26 min/long-fixture. So a harvested frequency is a *speed* lever, not a
  correctness one.

**Parity gate (first decision each slot).** Compare the slot's parity to the
harvested target TX parity:

- **Opposite parity** (us / other hunters transmitting — the target is not on
  air): run **only step 1** (cheap SWL). No focused/deep/a8d work — the target
  cannot be there, so spending `ndeep=5` is pure waste. This pass still harvests
  the QSO frequency from hunters calling the target.
- **Target's TX parity** (or parity not yet known): run the full ladder below.

Per slot, cheapest first, but **do not treat one target hit as proof that this
focus is exhausted**. Fox/Hound and multi-stream DX traffic can carry several
target-related messages near the same frequency, so a8d still runs when its
context is available. `hiscall` filter applies to all output throughout.

1. **Listen / harvest — plain `--swl`, full passband, `ndeep=3`, *pure
   observation*.** The listen carries **no MyCall/hiscall AP context** (it still
   does JTDX's own no-context AP, like a standalone `jtdx --swl`); all
   target-context recovery happens in the focused step. Cheap (~6 s); harvests the
   target's frequency/grid/dt/parity for later slots, and is the only pass when no
   frequency is known yet. (`nagain` here is pointless — no focus, and full-band
   `ndeep=5` adds nothing without context.) **Bootstrapping:** the *frequency* is
   harvested here (from stronger activity naming `mycall`/`hiscall`); the target's
   own *grid/parity* are harvested later from a focused-step target-as-sender hit.
2. **Focused recovery — requires a known frequency; `mycall` is optional.** `--swl
   --nagain` at `nfqso` = harvested freq → `ndeep=5` inside `nfqso ± 25 Hz`
   (fast). With `mycall`, this enables the MyCall-AP branch; without `mycall`, the
   same focused JTDX worker still runs as a narrow regular/SWL recovery pass and
   `hiscall` filter. For FH, ≤5 foci over the harvested Fox frequency set
   (`lhound`), low-band first per the 0–1000 prior. Without `mycall`, a8d/MyCall-AP
   are disabled and deep hypotheses shrink to CQ/target-only forms, but focused
   recovery is not skipped.
3. **a8d — when `hisgrid` is also harvested** (and `mycall` set). Second engine,
   focused at `nfqso`; reaches the weakest "DX-calling-me" replies. It is not
   skipped merely because step 2 found another target-related row near the same
   focus: under FH/multi-stream operation that row may not be the same message.
   Cost is bounded by the same focused-pass watchdog in monitor mode.
4. **Harvest update.** Fold every pass that ran into `TargetContextStore`, but
   **layered by trust**: the cheap listen (1) is the broad harvest source; the
   focused/deep passes (2/3) have a higher false-positive rate, so only their
   *target-filter hits* update target evidence, and `hisgrid`/parity update only
   from a target-as-**sender** row near an existing focus — never from a deep
   pass's non-target side-decode. This keeps the feedback loop from amplifying a
   false decode.

**There is no blind full-band deep pass (Option A).** Deep search (`ndeep=5`) is
*always* focused on a harvested/given frequency — never run blind across the
band. When no frequency is known, the slot does only the cheap listen (1); the
target is picked up once any frequency emerges (its own stronger slot, or a
hunter calling it). **Accepted limitation:** if a target is too weak for the
context-free listen **and** no one works it loudly enough to reveal the
frequency, no focus ever starts and that target is missed — recovery resumes as
soon as any decodable activity exposes the frequency. Rationale: a blind
full-band `ndeep=5` measured ~0 gain and
≈23 s/slot — not worth it. The **0–1000 Hz DXpedition convention** (Fox/rare DX
sits low, ~300–900 Hz in FH) survives only as a **soft prior**: when guessing or
ranking candidate target frequencies, prefer the low sub-band. It is *not* a hard
band cap on any pass — focused passes decode at the real frequency wherever it is
(the validation target `F1MLZ UA3QNA @1152 Hz` is recovered by the focused path,
which is never capped).

**Cross-slot state isolation (alignment + reproducibility).** Running several
passes in one slot must not disturb the JTDX worker's cross-slot decode state
(odd/even interval memory, AP/QSO memory, average-dt, duplicate/hash state). So
exactly **one** pass per slot — the cheap listen — commits to the worker, making
its per-slot evolution identical to a real `jtdx --swl` run; the focused/deep
passes are **state-isolated** (they decode and return rows without advancing that
committed state). This keeps the JTDX path faithfully aligned and keeps file-mode
output reproducible.

**Concurrent foci (orchestration-only parallelism).** Because each focus runs a
*fresh, fully isolated* disposable worker (its own kernel state, AP/hash book, and
residual — nothing shared with the listen worker or with other foci), the ≤5 foci
are decoded **concurrently, one thread per focus**, purely for wall-clock. This is
**not** kernel parallelism: every worker stays single-threaded internally and its
rows are byte-identical to a serial run, so sensitivity is unchanged. Determinism
is preserved by merging results in the fixed, sorted **foci order** (never
thread-completion order) before emit/harvest — so file-mode output stays
reproducible. (The cross-slot listen worker still commits serially, before the
focused fan-out.)

**Monitor latency.** Cheap listen ≈6 s; concurrent focused passes are seconds; the
≤5-foci FH worst case is now ≈ the slowest single focus rather than their sum. A
monitor-only 12 s budget still guards escalation: a focus whose JTDX pass has
already overrun the budget skips its a8d sub-pass. The watchdog does **not**
force-kill a worker already in flight. File mode runs every pass to completion (no
budget) so output stays reproducible.

## Output Policy

Emit only rows whose message involves `hiscall` — as a plain/compound token or
the kernel-resolved `<hiscall>`. We seed `{hiscall, mycall}` into the worker
`HashCallBook`, so a `<...>` whose hash matches `hiscall` is resolved to
`<hiscall>` natively and kept; **any still-unresolved `<...>` did not match our
two calls and is discarded** (not our target). Everything else is harvest-only.
Hash resolution can mislabel a colliding station as `<hiscall>` (negligible at
22-bit, ~1/1024 at 10-bit — same as stock WSJT-X/JTDX); cross-check frequency/dt
before trusting (False-Positive Control). Normal output shape stays
`HHMMSS snr dt freq message`. Internal attribution (`source`, provenance,
confidence) is kept inside diagnostics and tests only.

**Expect mostly-empty slots.** Because `dx` shows only the one target, most slots
emit **nothing** — that is normal monitoring, not a fault. A slot summary, if
shown, counts emitted (`hiscall`) rows, not the many filtered-out decodes; `dx`
may show a quiet "listening" state rather than a "0 decodes" that reads like a
failure.

## False-Positive Control

A super-sensitive SWL pass raises the false-positive rate, so chase must control
FP — but **not** by mutating kernel false-decode filtering:

- **Do not** route the dynamic `hiscall` into JTDX `searchcalls`/`chkfalse8`.
  `searchcalls` is a process-global `OnceLock` loaded once from `ALLCALL7.TXT`
  (`searchcalls.rs`); dynamic injection would break reproducibility and change
  the kernel's accepted set. This stays forbidden in DX regardless of any other
  profile's shared-knowledge experiments.
- **Do** control FP at the orchestration boundary using a DX-local confidence
  model. A super-sensitive target row is trusted by:
  1. frequency proximity to a harvested target frequency (±`napwid`);
  2. grid consistency with a harvested `hisgrid` (a conflicting grid demotes it);
  3. corroboration across slots / workers (`ConfirmedMulti`);
  4. the worker's own strong internal gates (a8d already requires
     `nhard<=54`, `plog>=-159.0`, `sigobig>=0.71`; the 206-message list is
     constrained to mycall/hiscall/hisgrid, so its FP surface is naturally
     narrow).

Because chase only outputs one target call, the single `hiscall` is itself the
strongest FP filter. The callsign list / `ALLCALL7.TXT` is not needed here.

**Emission policy:** a chaser must not wait a same-parity cycle (~30 s) to see a
target row, so a `hiscall` match is **emitted in the slot it is found**.
Confidence is computed but is a *soft annotation* (kept for internal tooling, not
the clean CLI line). The **only** confidence-based suppression is a **hard grid
conflict** — a row whose grid contradicts a confidently-harvested `hisgrid` is
almost certainly a hash collision / fabrication and is dropped. Off-frequency,
off-`dt`, or single-sighting rows are flagged low-confidence but still emitted
(suppressing them would lose real first sightings and target drift).

**The hiscall filter is NOT a license to lower decode thresholds.** It removes
decodes about *other* callsigns, but it cannot remove a fabricated codeword that
happens to spell `hiscall` — and to a chaser that is the worst error (a false
"the DX answered me"). The real guard against fabricated decodes is the 14-bit
CRC (~1/16384 false-pass per attempt); lowering `syncmin` or the
decode-acceptance gates increases the number of attempts and thus the count of
CRC-false-passes, including fabricated-`hiscall` ones the filter can't catch.
Sensitivity must come from **context** (MyCall-AP + frequency focus), which adds
information and raises the *true*-decode probability, not from loosened gates,
which raise the noise floor too. The aligned sensitivity levers (`swl`+`lft8lowth`
→ `syncmin` 1.1; `nagain` → `ndeep=5`) are already at their ceiling; going below
them would also touch the kernel (violates C1). So: do not tune thresholds.

**DX deep rows are experimental-only until the real G2 corpus is green.** The
T1/T2 deep-integration engine can internally recover rows below the per-slot
kernel ceiling, but those rows are suppressed from normal CLI/UDP output unless
`--dx-deep-experimental-output` is set. This is covered by an end-to-end runtime
test: the same weak repeated synthetic audio is recoverable internally by the
stack, hidden by default output, and visible only with the explicit experimental
flag. The full external G2 corpus has been explicitly deferred and should be
accumulated gradually from real operation; it remains the future default-output
release gate. When enough field data exists, record `0 fabricated / exposure /
95% Pfa upper bound`, with exposure broken down into `slots`, `focus_trials`,
`field_trials`, `hypothesis_trials`, `stack_osd_candidates`,
`stack_osd_attempts`, `stack_osd_skipped_budget`, `deep_rows_emitted`, and
`emitted_fabrications`. The report prints separate rule-of-three upper bounds for
slot, focus, hypothesis, and stack-OSD-attempt denominators.

## Frequency Prioritization (orchestration-only)

The target frequency can be prioritized without touching kernels:

- Setting `nfqso` to the harvested frequency makes `sync8` order the
  `nfqso ± 10 Hz` candidates **first** (`WSJTX.md`; JTDX similar) — this is the
  decoder's own documented context behavior: feed context through public config,
  let the decoder decide.
- Narrowing `napwid` focuses AP near the target.
- The session layer already controls *which* early decodes get subtracted and in
  what order; a chase strategy may subtract strong non-target signals first to
  clean the residual around a weak target, then decode the target window — this
  composes the existing kernel `subtractft8` at the orchestration layer without
  changing the subtraction math.

Changing the subtraction **algorithm itself** (not just call order/selection) is
not orchestration; it belongs to the Aggressive-Strategy Evaluation below.

## Aggressive Strategy: Resolved via `--swl --nagain`

Because chase filters hard on one `hiscall` at output, it can tolerate far more
internal false positives than a general decoder — which invites a more
aggressive internal search. The resolved answer is that the aggressive lever
**already exists as two aligned upstream flags** and needs no kernel change:

| Option | What | Alignment impact | Verdict |
|---|---|---|---|
| **A. `--swl --nagain` (config-only)** | enable upstream `swl` + `nagainfil` (OSD `ndeep=5`, `nfqso ± 25 Hz` focus) at the harvested frequency; `hiscall` filters the extra false decodes | none — both are existing upstream flags; `lib_jtdx` untouched; `nagain=false` default keeps baselines identical | **Recommended / shippable** — this *is* the enhanced SWL |
| **B. Private kernel fork** | copy a JTDX decode path into `src/decode/dx/` and push past `ndeep=5`/`syncmin` *in the copy only* | none to `lib_wsjtx`/`lib_jtdx` (isolated); high maintenance | **Not needed** — `ndeep=5` is already the OSD maximum; only revisit if a measured target proves the aligned ceiling insufficient |
| **C. Tune shared kernel** | relax thresholds inside `lib_wsjtx`/`lib_jtdx` | **violates principle 1** | **Forbidden** |

Option A is the implementation. `ndeep=5` is the OSD cap (`osd174_91` clamps to
5) and `npass=9`/lowest-`syncmin` are already the high-sensitivity base, so
`--swl --nagain` is the genuine aligned sensitivity ceiling. There is no need for
the fork (Option B) unless a future measured target proves even the ceiling
insufficient — and that would be an alignment-isolated private copy, never an
edit to the shared kernels.

## Validation Target

Use the existing long fixture `tests/ft8/230208_140300.wav`. Treat the row JTDX
misses with no context as the DXpedition reply to recover **with only `mycall`
and `hiscall`** (no `--rx-frequency`):

```text
target: 230208_140700, -16 dB, ~1153 Hz, "F1MLZ UA3QNA -04"  (CSV Extra=J)
run:    --profile dx --my-call F1MLZ --his-call UA3QNA
```

**This is a *mechanism* fixture, not a real DXpedition.** `F1MLZ/UA3QNA` is an
ordinary simplex FT8 QSO at 1152 Hz, not Fox/Hound. It validates the recovery
*mechanism* — harvest the QSO frequency, then focused MyCall-AP pulls back a
missed weak reply — not real-world DXpedition/FH performance, which stays
unmeasured until an FH recording exists.

Why it is recoverable from harvest alone (grounded in the fixture):

```text
140630  -12 dB  1152 Hz  F1MLZ RA3ABG KO95   <- mycall F1MLZ appears, easy decode
140700  -16 dB  1153 Hz  F1MLZ UA3QNA -04    <- TARGET (JTDX misses no-context)
140730  -19 dB  1152 Hz  F1MLZ UA3QNA -04    <- later (not usable: future slot)
```

Mechanism:

1. At 140630, `F1MLZ RA3ABG KO95` decodes easily. `mycall=F1MLZ` appears at
   ~1152 Hz → harvest 1152 Hz as the target-QSO frequency. (RA3ABG is **not**
   `hiscall`, so it is harvest-only and not emitted.)
2. At 140700, with `mycall=F1MLZ` + harvested `nfqso≈1152`, MyCall-AP
   (`iaptype=2`) recovers `F1MLZ UA3QNA -04` at -16 dB. `JTDX.md` already
   confirmed this row recovers as `source=Regular iaptype=2` with
   `--my-call F1MLZ --rx-frequency 1153`; chase supplies the frequency by
   harvest instead of by flag.
3. The `hiscall=UA3QNA` filter emits the recovered row.

Note the harvest signal here is `mycall` activity (others calling F1MLZ at the
QSO frequency), not prior `hiscall` activity (UA3QNA does not appear before the
target slot). The store must therefore harvest frequency from **either** `mycall`
**or** `hiscall` appearances.

**Caveat — the recovery evidence is from a *long-lived* `jtdx` session.** The
measured recovery used one session decoding all 19 slots (so by `140700` it had
accumulated `avexdt` and AP memory). The shipped `dx` uses a **fresh focused
worker per pass** (`avexdt=0`, empty memory, slightly different sync-window
center). **This has now been verified**: a fresh single-slot focused
worker `{swl, nagain, mycall=F1MLZ, hiscall=UA3QNA, nfqso=1152}` recovers exactly
`F1MLZ UA3QNA -04` — the `avexdt=0` concern did not materialize, and no mitigation
is needed.

**Synthetic validation fixture (`tests/ft8/dx_synth_ua3qna.wav`).** A 5-slot
fixture prepends a *synthesized strong* `F1MLZ UA3QNA -04 @1152` (a clean harvest
source) before four real contiguous slots (140645/140700/140715/140730), so the
end-to-end harvest→focus→recover flow has a controlled, deterministic target.
As a reference comparison only, a no-context WSJT-X+JTDX union decodes the synth
slot and the real 140730 but **misses the real 140700** — which is precisely the
row `dx` recovers from harvested context, so the fixture is a genuine DX-context
discriminator. This comparison does not make `dx` depend on `profile=hybrid`.
Generation recipe: `genft8("F1MLZ UA3QNA -04")` → `itone`;
`gen_ft8wave(itone, 1152.0)` → waveform; slot 0 places `waveform*0.5` at the
0.5 s FT8 start with deterministic Gaussian noise `*0.02`, then appends real
slots 140645/140700/140715/140730 from `230208_140300.wav`.

### Measured results (long fixture, `jtdx --swl --nagain`)

Phase-0 confirmation, all on `230208_140300.wav`. **`Time` is the whole-fixture
total over all 19 slots, NOT per-slot** — divide by 19 for per-slot (e.g. the
0–1000 blind case is 437 s total ≈ 23 s/slot, the focused case is seconds/slot).

| Config | Target 140700 | Rows | Total (19 slots) | ≈ per-slot |
|---|---|---|---:|---:|
| `--swl` (no context, `ndeep=3`) | missed | 454 | ~2 min | ~6 s |
| `--swl --nagain --his-call UA3QNA` (no mycall) | missed | 454 (=plain) | 765 s | ~40 s |
| `--swl --nagain --my-call F1MLZ --his-call UA3QNA` (no freq) | **recovered** | 458 (+4) | 1557 s | ~82 s |
| `--swl --nagain --my-call`, **0–1000 Hz** (would-be blind) | — | 124 | 437 s | **~23 s** |
| `--swl --nagain --my-call F1MLZ --rx-frequency 1152` (focused) | **recovered** | — | — | **seconds** |

Lessons, now empirically grounded:

- **`mycall` is the recovery key, not `hiscall`.** The target is `UA3QNA → F1MLZ`,
  recovered by MyCall-AP (`iaptype=2`) which needs `mycall=F1MLZ`. `hiscall=UA3QNA`
  alone adds **zero** rows (it cannot build the `F1MLZ UA3QNA` template); its job is
  the output filter and (with grid) a8d.
- **The harvested frequency is a *speed* optimization, not a correctness one.**
  `mycall + nagain` recovers the target full-band **without** any frequency, but
  takes ~26 min (every candidate becomes a MyCall-AP `ndeep=5` decode). Supplying
  the harvested `nfqso` narrows to `±25 Hz` (~27 AP candidates) and recovers the
  *same* row in seconds. This is exactly why chase must harvest the frequency: to
  fit the monitor 15 s budget, not to make the decode possible.
- The 4 rows `mycall` adds are **3 F1MLZ-QSO rows** (140700 UA3QNA target, 140730
  UA3QNA, 140730 RA3ABG) **plus 1 unrelated** `140730 CQ IW1PUR JN44` (a real but
  unrelated weak -22 dB CQ at 1146 Hz — IW1PUR CQs there all recording; the
  full-band `ndeep=5` search merely surfaced one more of its slots). This is the
  expected behavior of an aggressive deep search: it also pulls in unrelated weak
  signals. The `hiscall=UA3QNA` output filter discards all 3 non-UA3QNA rows
  (RA3ABG and IW1PUR), leaving only the 2 target rows — which is exactly why the
  hiscall filter, not internal FP gating, is chase's FP control.

### Implemented DX result (current code)

Command:

```bash
target/release/ft8rs file tests/ft8/230208_140300.wav \
  --start-time 230208_140300 \
  --profile dx \
  --my-call F1MLZ \
  --his-call UA3QNA
```

Current behavior:

- recovers `140700 F1MLZ UA3QNA -04` without `--rx-frequency`;
- also emits the later target repeat `140730 F1MLZ UA3QNA -04`;
- emits no RA3ABG/IW1PUR side-decodes;
- FP budget on this target chase is **0 same-slot-unsupported rows** against the
  CSV;
- release ignored gate below passes, currently ~220 s total for the 19-slot
  fixture.

```bash
cargo test --release test_dx_profile_long_ua3qna -- --ignored
```

The synthetic discriminator fixture also passes:

```bash
cargo test --release test_dx_profile_synthetic_ua3qna -- --ignored
```

File-mode reproducibility is covered by running the same synthetic fixture twice
with experimental deep output enabled over the first 3 target slots and comparing
the emitted rows:

```bash
cargo test --release test_dx_profile_synthetic_reproducible -- --ignored
```

The a8d path is reachable through `profile=dx` when `mycall`, `hiscall`,
`hisgrid`, and a focus frequency are available:

```bash
cargo test --release test_dx_profile_a8d_fixture
```

Secondary checks:

- with `--his-call UA3QNA` only (no `mycall`): target stays missed — documents the
  `mycall`-gated behavior (measured above);
- compound-`hiscall` variant: pick a hashed call in the fixture and confirm
  hash-match + resolution path.

## Implementation Status

Follow the project methodology used throughout the decoder work: measure before
building, kill-with-evidence.

- **Shipped.** `profile=dx`, mandatory `--his-call`, DX-local target filter,
  long-lived JTDX SWL listen worker, cross-slot `TargetContextStore`, disposable
  focused JTDX `swl+nagain` workers, disposable focused WSJT-X/a8d workers when
  `hisgrid+mycall+focus` exist (the ≤5 foci decoded **concurrently, one isolated
  worker per thread**, merged in deterministic foci order), bounded ≤5 focus
  selection, target-only
  emit/merge, hard target-sender grid contradiction suppression, and monitor-only
  focused-pass watchdog.
- **Validated.** Real long fixture recovers `140700 F1MLZ UA3QNA -04` without
  `--rx-frequency`; the synthetic fixture recovers the same weak row via
  harvest→focus; the a8d fixture is reachable through `profile=dx`; synthetic
  file-mode output is reproducible. Focus selection is also deterministic:
  harvested foci are sorted by confidence / low-band priority / frequency rather
  than listen row order, and concurrent focused workers are merged in that fixed
  order. Pure `wsjtx`/`jtdx` profile gates remain unchanged.
- **Experimental.** Deep cross-slot T1/T2 rows are collected internally but are
  not normal output unless `--dx-deep-experimental-output` is set. T1 matched
  filtering is wired but threshold-disabled by default until calibration; the
  current positive evidence is T2 stacked LLR + CRC/OSD recovery. Deep rows now
  carry a JTDX-formula SNR estimate when the recovered message can be re-packed
  into tones from the current `DxSymbolField.s8`; CLI/UDP use that dB value. If a
  message cannot be re-packed, CLI renders `DX` and UDP uses `-99` as an explicit
  unavailable-SNR sentinel. The estimator still needs broader real-signal
  calibration before deep rows can be default output; the current synthetic guard
  verifies monotonicity when signal amplitude increases on the same message/noise
  seed, and the short real-audio scaffold checks already-decoded rows against the
  JTDX decoder's own SNR path (`pairs=20`, `mean_abs_delta=0.02`,
  `max_abs_delta=0.21` on `210703_133430.wav`). Even CRC-valid deep stack rows are
  typed and treated as
  `CrcConfirmedExperimental` before the sized false-alarm corpus is green; this is
  not a default-output trust label for the active weak-stack path.
  The current T1 calibration scaffold is only a measurement hook: last local
  synthetic release run reported `target_min_stat=67.530`,
  `target_min_margin=32.861`, `false_max_stat=41.916`,
  `false_max_margin=18.150` over 444 false fields (wrong-call, near-call,
  hash-like/braced-call, and pure-noise). A real-audio T1 scaffold also reuses
  already-decoded rows from `210703_133430.wav`; last local release run ranked the
  exact decoded v1-compatible message first for 16/18 rows, with the two misses on
  adjacent report/terminator hypotheses. These are front-end/scoring datapoints;
  they do not enable matched-only output.
  A real-audio false-alarm ceiling scaffold
  (`dx_t1_real_audio_false_alarm_ceiling_scaffold`, ignored/release) now measures
  the raw matched-filter score that an **absent** target's v1 hypotheses reach on a
  real on-band recording — the empirical floor any `min_stat` must clear. Last
  local release run on `230208_140300.wav` (4 absent targets × 4 foci × 6 slots,
  244 scored fields) reported `false_max_stat=144.461` (worst case a bare
  `CQ ZZ1ZZZ`), `false_max_finite_margin=131.554`, with `61` degenerate
  single-runner fields whose `margin` was `+inf`. **Two hard conclusions, both
  arguing against a single-slot matched-only gate:** (1) `margin` is unsafe as a
  sole gate — a bare-`CQ` hypothesis set (and any non-standard call like the
  observed `QQ9QQQ`, which collapses to 1 hypothesis) has no runner-up, so
  `margin = stat − (−∞) = +inf` passes trivially. (2) The `false_max_stat≈144`
  ceiling **overlaps the weak-true-signal range**: on `210703_133430.wav` the
  weakest correctly-ranked true decode was `stat≈158` and a genuine decoded row
  (`WA2FZW DL5AXX`) scored only `stat≈77` — below the false-alarm ceiling. A global
  `min_stat` high enough to clear false alarms would therefore suppress real weak
  targets, which is precisely the regime deep is meant to recover. The matched
  filter does **not** cleanly separate true from false on real audio with one
  global single-slot threshold; the safe sensitivity path is cross-slot
  corroboration + CRC (T2), not lowering the T1 single-slot gate. The runtime gate
  stays `INFINITY` and these remain measurement-only datapoints; the scaffold's
  `< 180.0` assertion is a regression guard against ceiling inflation on this
  fixture, not a calibrated `Pfa` threshold (absolute `stat` is not normalized
  across SNR/recording, so a real threshold needs the deferred field corpus to
  characterize the ceiling distribution across many recordings).
- **T2 hardening.** Stack OSD candidates are batch-ranked before spending the
  per-slot CRC/OSD budget, and a conservative sustained-hypothesis-flip reset
  prevents clearly different repeated target messages from being accumulated
  forever into one LLR sum. Internal counters track stack CRC/OSD candidates,
  attempts, and budget skips; `--dx-deep-diagnostics` prints the per-slot
  foci/field/hit/CRC-budget report to stderr without changing decode output.
  T1 two-slot corroboration also requires the same normalized message; a
  same-parity, physically compatible hit with a different message is retained as a
  separate observation rather than promoted.
  A current-single-slot CRC contradiction guard suppresses stack output when the
  current slot already decodes as a different target message. The stack also keeps
  the capped per-slot LLR history and requires statistical member-slot support for
  the final decoded codeword (positive total support plus a 2/3 non-negative-slot
  majority) before a stack CRC row can be emitted experimentally. This covers both
  obvious and sub-CRC variants of the "DX keeps working different callers" hazard
  without rejecting a single noisy weak outlier.
  Synthetic tests now cover both `MYCALL HISCALL ...` and the more important
  `OTHER HISCALL ...` repeated-message shape; the latter proves the blind stack
  path rather than the enumerable matched-filter path. This is intentionally a
  repeated-identical-message guarantee: if the DX changes the caller/report every
  slot, the stack must not merge those slots and no T2 gain is claimed. Both shapes
  have matching ignored/release G1 amplitude-search gates; last local G1-B pass was
  `RA3ABG BG5ATV -10` at `amp=0.0030`, `noise=0.0800`.
- **T0 AP tuning.** Target QSO progress inferred from committed prior target rows
  is applied to the next focused JTDX worker and deep symbol extraction. If no
  committed progress exists, the user-supplied `--qso-progress` remains the
  fallback. Same-slot listen results are not used to configure a focused replay;
  foci, `hisgrid`, target dt, QSO progress, and deep hypotheses are snapped at
  slot start. The `dx_committed_qso_progress_feeds_next_focused_config` test covers
  the harvest → snapshot → focused-config path.
- **FP budget.** The UA3QNA long-fixture chase emits 0 same-slot-unsupported
  target rows. Non-target rows remain harvest-only and are not printed. A fast
  DX deep-engine smoke test also verifies that deterministic wrong-call and pure
  noise LLR stacks do not fabricate the target, but the full sized false-alarm
  corpus remains a future hard gate before deep output can be default-enabled.
  This field corpus is intentionally deferred for now and should be filled from
  future operating recordings rather than fabricated just to close the plan.
  A real-on-band absent-target scaffold also passes locally: running
  `tests/ft8/230208_140300.wav` with experimental deep output and absent target
  `ZZ1ZZZ` emitted 0 rows. A heavier matrix version also emitted 0 rows across
  4 absent targets × 4 focus frequencies × first 6 real slots (`261.82s`). This is
  useful evidence for the real-audio path, not a replacement for the required ≥2 h
  real on-band corpus.
  The ignored `dx_false_alarm_corpus_manual_gate` currently reports the scaffold
  format; last local release run was `emitted_fabrications=0/7264`,
  `pfa95_slots<=0.000413`, `pfa95_focus<=0.000413`,
  `pfa95_hypothesis<=0.000008`, `pfa95_stack_osd<=0.000472`, `slots=7264`,
  `focus_trials=7264`, `field_trials=7264`,
  `hypothesis_trials=384992`, `stack_osd_candidates=6356`,
  `stack_osd_attempts=6356`, `stack_osd_skipped_budget=0`,
  `deep_rows_emitted=0`, `wrong_slots=1024`, `near_call_slots=288`,
  `hash_collision_slots=192`, `noise_slots=5760`, which covers the synthetic
  wrong-call, synthetic hash-like / near-callsign, and 24h-equivalent synthetic
  pure-noise LLR sub-budgets but is still not the final G2 corpus. The ignored
  `test_dx_profile_external_g2_corpus_no_deep_false_alarm` gate is now the
  real-recording harness: point `FT8RS_DX_G2_CORPUS` at a directory containing
  `noise/*.wav`, `wrong_call/*.wav`, `on_band/*.wav`, and
  `hash_collision/*.wav`. With the env var set, all four categories are mandatory
  and must satisfy the PLAN budgets: `noise >= 5760` slots,
  `wrong_call >= 1000` slots, `on_band >= 480` slots, and
  `hash_collision >= 50` slots.
  The corpus can include `manifest.csv` with columns
  `label,wav,target,mycall,focus,nfa,nfb`; supported labels are `noise`,
  `wrong_call`, `on_band`/`real_on_band`, and `hash_collision`. Use the manifest
  to record the manually confirmed absent target and focus/frequency window for
  each category or individual wav. Blank `wav` applies a case to every wav in that
  category; each non-comment row must have exactly 7 columns, and a non-blank
  `wav` must be a `.wav` file name, not a path. The harness also rejects
  non-finite or reversed frequency windows, requires `focus` to lie inside
  `nfa..nfb` with `0 <= nfa < nfb <= 5000`, and rejects a non-blank `wav` that is
  not present in the matching category directory. A parse-checked starter template
  is tracked at `tests/ft8/g2_manifest.example.csv`; copy it to
  `$FT8RS_DX_G2_CORPUS/manifest.csv` and edit the manually confirmed absent
  targets/focus windows. With no manifest, the harness falls back to the legacy
  hard-coded absent-target cases. With the env var unset it skips cleanly; a
  skipped run is not G2 evidence.
- **Deferred.** Real Fox/Hound gain remains unmeasured because no FH fixture is
  available. The bounded multi-focus and hash-seeding machinery is present, but
  real-signal FH acceptance stays deferred until a recording exists.

Hard gates: pure-profile baselines row-for-row unchanged (`wsjtx` 21/21 &
424/424, `jtdx` 20/20 & 430/431); file-mode reproducible; output only contains
`hiscall` rows; FP budget recorded. `hybrid` remains a separate profile and is
only checked when a change touches shared dispatcher/output code.

## Known Limitations / Future Hardening

These are edge-case robustness gaps, not correctness bugs — the validated path
(harvest → focus → recover `140700`, FP budget 0) is unaffected. Recorded here so
they are deliberate, not forgotten.

- **Watchdog is a "don't-start-next-pass" gate, not a true interrupt.** The
  monitor 12 s budget is checked before each focus and before each a8d sub-pass;
  a single focused pass already in flight is never force-killed (no thread
  cancellation). It bounds the *number* of passes started, not total wall-clock.
  Acceptable because focused passes are seconds; documented under *Monitor
  latency*.
- **Harvested `hisgrid` can be poisoned by a hash collision.** A ~1/1024 10-bit
  collision that mislabels another station as `<hiscall>` in a sender position
  with a different grid would lock the wrong `hisgrid`, after which
  `has_hard_grid_contradiction` could suppress the *real* target's rows. Only
  applies to harvested (not user-provided) grids. Future: trust only a
  user-supplied `hisgrid` for the contradiction gate, or require grid
  corroboration across ≥2 slots before locking.

## Do Not Do

- Do not modify `lib_wsjtx`/`lib_jtdx` thresholds, candidate order, AP
  scheduling, or subtraction math (principle 1).
- Do not route dynamic calls into `searchcalls`/`chkfalse8`, and do not require
  or extend `ALLCALL7.TXT` for chase logic.
- Do not do same-slot re-decode feedback (breaks reproducibility) or unbounded
  per-candidate replay (the killed `QsoContextHint` failure mode).
- Do not output non-`hiscall` rows.
- Do not claim FH gains before measuring on a real Fox/Hound recording.
- Do not ship the aggressive Option B without an explicit flag, an
  alignment-isolated private copy, and a measured FP budget.
