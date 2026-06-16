# DX Chase Decode Notes

This document records `--profile chase`: a single-target DXpedition-pursuit
decoder built as an orchestration layer on top of the existing aligned WSJT-X and
JTDX kernels. It does not introduce a new decode algorithm and does not modify
`lib_wsjtx`/`lib_jtdx`.

Related documents:

- WSJT-X alignment: `WSJTX.md`
- JTDX profile: `JTDX.md`
- Hybrid result-union + shared knowledge: `HYBRID.md`

`chase` reuses the same shared-knowledge philosophy as hybrid (provenance,
confidence, cross-slot context harvest), but narrows it to **one operator-given
target callsign**.

## Scope

```text
profile=chase -> JTDX SWL worker (sensitivity + harvest)
              -> WSJT-X a8d / MyCall-AP worker (focused recovery)
              -> single-target context harvest (cross-slot, deterministic)
              -> hiscall output filter
```

Goal: given one target station (`hiscall`), produce the most reliable possible
reception of **that station's** transmissions, by automatically turning a
no/low-context situation into a full-context one. Everything not related to
`hiscall` is suppressed from output (but may be used internally for harvest).

This is the single-target specialization of the hybrid shared-knowledge idea. It
is feasible where hybrid's general `QsoContextHint` was killed-with-evidence
(`HYBRID.md`, 72-hint replay → 643s, 0 gain) precisely because the target is
**one known callsign**, not the whole band — there is no per-partner replay
explosion.

## Non-Negotiable Constraints

Inherits all of `HYBRID.md`'s constraints, plus the chase-specific rules the
owner set:

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
   begins), so file mode stays reproducible. No same-slot re-decode feedback.

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
  the user-provided `nfqso`/`nftx` (if any) **and** grown by harvest: any slot
  where `hiscall` **or** `mycall` appears in a decoded message contributes its
  frequency. Both sources are kept and used together — a user-supplied frequency
  is a trusted anchor, while harvested frequencies cover the cases where the user
  value is approximate, stale, drifting, or one of several (FH). The focused
  worker decodes at the union of {user frequency} ∪ {harvested frequencies}.
  (Harvesting on `mycall` matters: the target's QSO frequency is often first
  observable from *another* station working us at that frequency — see
  Validation Target.)
- **Grid.** A `hisgrid` is harvested when the target sends a `CQ <call> GRID` or
  `<mine> <target> GRID` row. Once known, it promotes recovery from MyCall-AP to
  full a8d.
- **Drift.** The frequency set ages out with a sliding window so the focus
  tracks a moving target; stale frequencies are dropped.
- **TX parity (sequence).** FT8 QSOs alternate every 15 s by parity
  (`jseq = (nutc/5) % 2`; `{:00,:30}=0`, `{:15,:45}=1`). The target transmits on
  **one** parity; the other parity carries *us and other hunters calling the
  target*. Harvest the parity on which `hiscall` is decoded as the target's TX
  parity. This gates the expensive work (see Per-Slot Decode Strategy): only the
  target's TX parity needs the deep search; the opposite parity needs only the
  cheap SWL listen (which is still the main *frequency* harvest source — hunters
  calling the target reveal the QSO frequency, exactly how 1152 Hz is harvested
  from `140630 F1MLZ RA3ABG`). Until the parity is observed, treat both as the
  target's (no premature skipping).
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
  transmissions. This reuses the hybrid `SharedHashCallBook` collision-safety
  machinery but with a one- or two-entry book, not a database.

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

Per slot, cheapest first, stop when the target is found (`hiscall` filter applied
to all output throughout):

1. **Listen / harvest — plain `--swl`, full passband, `ndeep=3`.** Cheap; also
   harvests the target's frequency/grid/dt/parity for later slots. (`nagain` here
   is pointless — no focus, and full-band `ndeep=5` adds nothing without context.)
2. **Focused recovery — when a frequency is known/harvested.** `--swl --nagain`
   at `nfqso` = harvested freq → `ndeep=5` inside `nfqso ± 25 Hz` (fast). For FH,
   ≤5 foci over the harvested Fox frequency set (`lhound`). Needs `mycall` for
   MyCall-AP.
3. **a8d — when `hisgrid` is also harvested** (and `mycall` set). Second engine,
   list decode at `nfqso`; reaches the weakest "DX-calling-me" replies.
4. **Blind deep fallback — only when no frequency has been harvested yet.**
   `--swl --nagain` with **no** `nfqso`, but **restricted to the 0–1000 Hz
   sub-band** (set `nfb=1000`). This is the most expensive pass; the 0–1000 Hz
   cap follows the DXpedition convention (Fox/rare DX transmits low — FH Fox sits
   ~300–900 Hz) and roughly cuts the candidate band 200–3000→200–1000 (~70%
   fewer), turning the ~26 min full-band run into ~8 min.

5. **Harvest update.** Update `TargetContextStore` from all confirmed rows for
   the next slot.

**Critical scope of the 0–1000 Hz cap: it applies to the blind fallback (4)
ONLY.** Focused passes (2/3) decode at the actual harvested/given frequency
wherever it is — including above 1000 Hz. The validation target
`F1MLZ UA3QNA @1152 Hz` is recovered by the focused path (2), not the blind
fallback, so the 0–1000 Hz cap does not block it. Never apply the cap to a
focused pass, or known-high-frequency targets are lost.

If every pass misses, step 1's full-band listen result still stands. This is the
concrete form of "if other methods miss the target, the sensitivity pass still
tried."

## Output Policy

Emit only rows whose message involves `hiscall` (as a standard token, or as a
resolved/hash-matched compound call). Everything else is harvest-only. Normal
output shape stays `HHMMSS snr dt freq message`. Internal attribution
(`source`, provenance, confidence) is available to debug/test tooling only.

## False-Positive Control

A super-sensitive SWL pass raises the false-positive rate, so chase must control
FP — but **not** by mutating kernel false-decode filtering:

- **Do not** route the dynamic `hiscall` into JTDX `searchcalls`/`chkfalse8`.
  `searchcalls` is a process-global `OnceLock` loaded once from `ALLCALL7.TXT`
  (`searchcalls.rs`); dynamic injection would break reproducibility and change
  the kernel's accepted set (`HYBRID.md` marks this high-risk/killed).
- **Do** control FP at the orchestration boundary using the existing confidence
  model (`hybrid/evidence.rs`). A super-sensitive target row is trusted by:
  1. frequency proximity to a harvested target frequency (±`napwid`);
  2. grid consistency with a harvested `hisgrid` (a conflicting grid demotes it);
  3. corroboration across slots / workers (`ConfirmedMulti`);
  4. the worker's own strong internal gates (a8d already requires
     `nhard<=54`, `plog>=-159.0`, `sigobig>=0.71`; the 206-message list is
     constrained to mycall/hiscall/hisgrid, so its FP surface is naturally
     narrow).

Because chase only outputs one target call, the single `hiscall` is itself the
strongest FP filter. The callsign list / `ALLCALL7.TXT` is not needed here.

## Frequency Prioritization (orchestration-only)

The target frequency can be prioritized without touching kernels:

- Setting `nfqso` to the harvested frequency makes `sync8` order the
  `nfqso ± 10 Hz` candidates **first** (`WSJTX.md`; JTDX similar) — this is the
  decoder's own documented context behavior (allowed by hybrid constraint 10:
  "feed context, let the decoder decide").
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
| **B. Chase-private kernel fork** | copy a JTDX decode path into `src/decode/chase/` and push past `ndeep=5`/`syncmin` *in the copy only* | none to `lib_wsjtx`/`lib_jtdx` (isolated); high maintenance | **Not needed** — `ndeep=5` is already the OSD maximum; only revisit if a measured target proves the aligned ceiling insufficient |
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
run:    --profile chase --my-call F1MLZ --his-call UA3QNA
```

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

### Measured results (long fixture, `jtdx --swl --nagain`)

Phase-0 confirmation, all on `230208_140300.wav`:

| Config | Target 140700 | Rows | Time |
|---|---|---|---|
| `--swl` (no context) | missed | 454 | ~2 min |
| `--swl --nagain --his-call UA3QNA` | missed | 454 (=plain) | ~12 min |
| `--swl --nagain --my-call F1MLZ --his-call UA3QNA` (no freq) | **recovered** | 458 (+4) | **~26 min** |
| `--swl --nagain --my-call F1MLZ --rx-frequency 1152` | **recovered** | — | **seconds** |

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

Secondary checks:

- with `--his-call UA3QNA` only (no `mycall`): target stays missed — documents the
  `mycall`-gated behavior (measured above);
- compound-`hiscall` variant: pick a hashed call in the fixture and confirm
  hash-match + resolution path.

## Phased Plan (measure-first)

Follow the `HYBRID.md` methodology: measure before building, kill-with-evidence.

- **Phase 0 (measure, no new decoder).** On the long fixture, quantify: (a) can
  the harvest see 1152 Hz by 140630 and does MyCall-AP recover 140700 with the
  harvested frequency; (b) for a real FH recording, the harvestable rate of
  `hisgrid` and the a8d vs MyCall-AP recovery delta vs no-context; (c) the extra
  FP count from the SWL fallback. Decide scope from the numbers.
- **Phase 1.** `chase` profile skeleton: SWL worker + `TargetContextStore`
  (single target, cross-slot frequency/grid harvest) + focused worker; support
  "frequency known/unknown, single target". Validate the 140700 target.
- **Phase 2.** FH multi-frequency: `lhound` + bounded multi-focus + Fox tones +
  compound-call hash matching.
- **Phase 3.** Confidence/grid-consistency output gate; record an FP budget.

Hard gates each phase: pure-profile baselines row-for-row unchanged
(`wsjtx` 21/21 & 424/424, `jtdx` 20/20 & 430/431); file-mode reproducible; FP
recorded as a number.

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
