# DX Profile — Development Plan

Implementation plan for `profile=dx`, the single-target DXpedition-chase decoder.
**`DX.md` is the product/design doc (what & why); this `PLAN.md` is the
engineering doc (how, step by step, with tests and acceptance).** Read `DX.md`
first for the model; this file assumes it.

Methodology: measure-first, incremental, each step independently
buildable/committable/testable, kill-with-evidence where a step proves
unviable (same discipline as `HYBRID.md`).

## Non-Negotiable Constraints (carried into every step)

- **C1 — Kernels untouched.** No change to `lib_wsjtx` / `lib_jtdx` decode
  semantics. The `wsjtx`/`jtdx` profiles stay aligned and independent. `dx`
  lives only in orchestration (`src/decode/dx/`) and feeds workers through
  existing config entry points.
- **C2 — Aligned ceiling, no fork.** Max sensitivity is `swl` + `nagain`
  (OSD `ndeep=5`, the cap) — both existing upstream flags. No private kernel
  fork, no new threshold tuning.
- **C3 — hiscall mandatory; mycall optional.** No `hiscall` → `dx` refuses to
  start. No `mycall` → no a8d and no MyCall-AP (the rest still runs).
- **C4 — Output only `hiscall`-related rows.** Everything else is harvest-only.
- **C5 — No callsign-list / `ALLCALL7.TXT` dependency** in `dx` logic; the single
  `hiscall` is the acceptance anchor. The `dx` orchestration neither adds,
  modifies, nor depends on `ALLCALL7.TXT`, and never routes dynamic calls into
  `searchcalls`/`chkfalse8`. **Clarification:** the JTDX worker may still use its
  static `ALLCALL7.TXT`/`searchcalls` internally as part of its own profile
  alignment — that is fine and must NOT be disabled to "satisfy" C5; what is
  emitted is decided solely by the `dx` `hiscall` filter.
- **C6 — Cross-slot feedback only.** Slot N uses only knowledge committed before
  slot N. No same-slot re-decode feedback. File mode must be reproducible.
- **C7 — Baselines frozen.** `wsjtx` 21/21 & 424/424, `jtdx` 20/20 & 430/431,
  `hybrid` 465 — row-for-row unchanged. `nagain` defaults false everywhere except
  the `dx` coordinator's own focused passes.
- **C8 — Deep search is always focused; there is no blind full-band deep pass.**
  `ndeep=5` runs only on a focused `nfqso ± 25 Hz` window (a harvested/given
  frequency). When no frequency is known, the slot does only the cheap full-band
  listen (`ndeep=3`) — never a blind full-band `ndeep=5`. The 0–1000 Hz
  DXpedition convention survives only as a **soft harvest prior** (prefer
  low-band candidate frequencies when guessing where the target is), not as a
  hard band cap on any pass.
- **C9 — `ndeep=5` only affects the AP branch.** It needs `mycall` (AP context)
  to do anything, and is only run focused (per C8). The regular non-AP OSD is
  hardcoded `ndeep=3`. Measured per-slot cost: cheap listen ≈6 s; focused deep is
  seconds; a *blind* full-band `ndeep=5` would be ≈23 s/slot (0–1000) — which is
  exactly why C8 forbids it.

## Architecture

```text
profile=dx -> DxStreamDecodeSession (src/decode/dx/)
                ├── JTDX worker  (SWL listen, nagain deep AP, MyCall-AP, lhound)
                ├── WSJT-X disposable worker per focus (runs a8d as part of it — Step 5, gated on hisgrid+mycall)
                ├── TargetContextStore (cross-slot: freq set, grid, TX parity, dt)
                └── per-slot: parity gate -> cost ladder -> harvest -> hiscall filter
```

Mirrors the `HybridStreamDecodeSession` shape (`src/decode/hybrid/mod.rs`):
own workers, orchestration-only, no kernel reach-in.

### Verified implementation facts (ground truth for the steps)

- Profile plumbing: `DecodeProfile` enum + `parse`/`as_str`
  (`src/stream/session.rs:18`), `ProfileStreamDecodeSession` dispatch
  (`src/stream/profile.rs`), `config.profile = DecodeProfile::parse(...)`
  (`src/main.rs:217`).
- Config carries everything `dx` needs: `nfa`/`nfb`, `nfqso`, `nftx`, `swl`,
  `nagain`, `lhound`, `mycall`/`hiscall`/`hisgrid` (`src/stream/session.rs:140`).
- JTDX worker (`JtdxStreamDecodeSession::new`, `src/decode/lib_jtdx/mod.rs:73`)
  always applies `clone_for_profile_jtdx_high_sensitivity`. It builds
  `ft8apset(config)` and `tone8(config)` **once** in `new()`. **apset depends on
  `mycall`/`hiscall`/`ncontest`/`lhound`; tone8 on `mycall`/`hiscall`/`lhound`**
  (`tone8.rs:45-49` — derived from the calls, not just lhound). **In the v1
  design these are rebuilt anyway** because each focused pass is a *fresh*
  disposable worker (Step 2), and the context-free listen and the with-`mycall`
  focused workers deliberately have **different** `mycall`/`hiscall` configs (so
  apset differs by role). The "stable fields → no rebuild" property only matters
  for the **v2** `set_slot_context` optimization on a shared session; it is not
  relied on by v1. Band split (`jtdx_decode_bands`) uses `nfa`/`nfb` and
  `ensure_ft8b_workspaces` re-sizes each decode, so per-slot band changes are
  already handled.
- Band narrowing already keys off config: `filter`→`±60` (`±290` hound),
  `nagain`→`±25` (`src/decode/lib_jtdx/mod.rs:390`).
- `ndeep`: `ap_ndeep` (`src/decode/lib_jtdx/ft8b/decode_helpers.rs:352`) returns 5
  when `nagain`; used only at the AP OSD call (`ft8b/regular.rs:168`); regular OSD
  is hardcoded `ndeep=3` (`ft8b/regular.rs:97`).
- Parity: `jseq_from_nutc` = `(nutc/5)%2` (`src/stream/session.rs:789`); JTDX
  `IntervalKind::from_timestamp`. `{:00,:30}=0`, `{:15,:45}=1`.
- Hash for compound calls: `pub(crate) ihashcall` (`packjt77`), already reused by
  `hybrid/shared.rs`; `HashCallBook::save` resolves `<...>` natively.
- a8d lives in the WSJT-X `StreamDecodeSession` (`ft8_a8d.rs`, gated by
  `wsjtx_a8_allowed`, `src/stream/session.rs:667`).
- Reusable orchestration: `hybrid/evidence.rs` (confidence), dedupe/merge,
  `SharedHashCallBook` collision safety — `dx` may reuse these.

### Measured cost (long fixture `230208_140300`, 19 slots, this 8-core machine)

Clean `/usr/bin/time` totals; per-slot = total ÷ 19. (These are aggregate totals —
**not** per-slot — a point that was unclear in an earlier review.) **These are
reference measurements on this fixture/machine, not acceptance numbers** — e.g.
`~6 s` is plain no-context `--swl`; the actual `dx` primary listen also carries a
2-call `HashCallBook` seed (no AP context, so close, but not identical).
**Re-measure after the `dx` plumbing exists**; treat the latency design (focused =
seconds, watchdog-bounded) as the contract, not the specific seconds.

| Config | Total | ≈ per-slot | Notes |
|---|---:|---:|---|
| `--swl` (regular, `ndeep=3`) | ~2 min | **~6 s** | the cheap listen pass |
| `--swl --nagain` full-band, no `mycall` | 764 s | ~40 s | **0 extra rows** vs plain `--swl` (ndeep=5 needs AP/`mycall`) |
| `--swl --nagain --my-call` full-band | 1557 s | ~82 s | recovers target; +4 rows; far too slow |
| `--swl --nagain --my-call`, **0–1000 Hz** | 437 s | **~23 s** | 124 rows; the would-be blind deep — **dropped (Option A)** |
| `--swl --nagain --my-call --rx-frequency` (focused ±25 Hz) | — | **seconds** | the main lever; 27 AP candidates vs ~2000 full-band |

Design consequences (all already in the ladder): deep is always **focused**
(seconds, not the ~23–82 s blind/full-band cost); the monitor watchdog bounds the
≤5-foci worst case; `nagain` is gated on `mycall` (no AP context → 0 gain).
`ndeep=4` would roughly halve the deep cost (the 4↔5 difference in `osd174_91` is
only the `npre2` stage), but it is not reachable without touching `ap_ndeep`, and
Option A removes the slow pass anyway — so `ndeep=4` is **not** pursued.

## Steps

Each step lists **Goal / Changes / Dev notes / Tests / Acceptance**. Steps are
ordered by dependency; each is independently committable.

### Step 1 — Profile scaffolding (`dx` selectable, hiscall-gated, minimal filter)

- **Goal.** `--profile dx` exists, requires `--his-call`, delegates to one JTDX
  worker, and **already applies the minimal `hiscall` output filter** so C4 holds
  from the first step (no harvest/parity/deep yet — those are Steps 3/4).
- **Changes.**
  - `DecodeProfile::Dx` + `parse("dx")` + `as_str` (`session.rs`).
  - `ProfileStreamDecodeSession::Dx(DxStreamDecodeSession)` (`profile.rs`).
  - New `src/decode/dx/mod.rs`: `DxStreamDecodeSession` holding one JTDX worker
    built from the user config; `decode_slot_streaming_at` delegates, then keeps a
    row iff one of its **whitespace-split tokens equals the normalized `hiscall`**
    (exact token match, **not** substring — `K1ABC` must not match a longer token,
    and a short call must not match inside another). The full
    canonical/compound/hash rules land in Step 3.
  - `main.rs`: validate `hiscall` present when `profile==Dx`, else hard error.
- **Dev notes.** Keep `DxStreamDecodeSession::new(config)` mirroring
  `HybridStreamDecodeSession::new`. **`dx` owns the (single, for now) listen
  worker's config from Step 1: `swl=true`, `nagain=false`, full configured
  passband.** A user `--nagain` is a focused-pass concern and is **ignored /
  deferred until Step 4** — it must not make the listen worker run `nagain`. Do
  not "pass the raw config through". Update `README.md` profiles list + `--help`
  to include `dx`. The minimal filter is deliberately tiny; C4 is non-negotiable
  so it cannot wait.
- **Tests.**
  - `--profile dx --his-call UA3QNA` decodes the long fixture; **every emitted row
    contains `UA3QNA`** (C4 holds).
  - `--profile dx` (no hiscall) exits with a clear error.
  - Pure-profile baseline tests unchanged (C7).
- **Acceptance.** `dx` selectable; hiscall-required; **output already hiscall-only
  (C4)**; pure baselines byte-identical. **Scope:** the Step-1 minimal filter
  supports a **standard/plain `hiscall` only** — compound/hash target support
  begins at Step 3 (so a compound `hiscall` may produce empty output until then;
  that is staged behavior, not a bug).

### Step 2 — Two-session model: committing primary listen + isolated focused passes

`JtdxStreamDecodeSession` is **not** a pure function. Per `decode_slot` it mutates
several cross-slot fields: `_state` (`Ft8Mod1`: odd/even interval memory,
`avexdt`/force-sync, AP/QSO memory, duplicate state), **`book` (`HashCallBook`)**,
and **`regular_hash_calls`**. Running multiple decodes on one slot/session would
double-rotate parity and double-commit memory, desyncing the next slot from a real
JTDX run. So the design uses **two clearly-separated session roles**:

- **Primary listen session — long-lived, committing, no user-context AP.** Runs
  exactly **one** pass per slot (the cheap full-band SWL listen). **`dx` owns this
  config and pins it: `swl=true`, `nagain=false`, `nfa/nfb` = the full configured
  passband, and `mycall`/`hiscall` cleared so they do not enter the listen's
  apset.** **Precise meaning:** the listen still does JTDX's *own* no-context AP
  (the standalone `jtdx --swl` 430/431 baseline already includes AP-recovered
  rows) — what is off is only the **user-context (MyCall/hiscall) AP**; do **not**
  disable JTDX's internal AP. A user CLI `--nagain` is a focused-pass concern only
  and must **never** reach the listen; `dx` does not narrow the listen band unless
  the user set `--low/--high`. **Why no user-context AP:** if `mycall` drove the
  listen's apset it would run full-band MyCall-AP every slot — expensive and no
  longer comparable to a standalone `jtdx --swl` run. The listen is pure
  observation; all MyCall-AP / a8d recovery happens in the focused passes. The
  listen still **seeds its `HashCallBook` with `{hiscall, mycall}`** (resolution
  only, not AP) so a compound target shows as `<hiscall>` for harvest/filter — so
  it is "≈ standalone `jtdx --swl` plus a 2-call book seed", the broad harvest
  source.
- **Focused deep passes — state-isolated, non-committing.**
  - **v1 (default, foolproof): one fresh disposable JTDX worker PER FOCUS.** Build
    it from the base `dx` config + that focus's context, **seed its `HashCallBook`
    with `{hiscall, mycall}` via `import_hash_calls` before decoding** (a fresh
    worker's book is empty — without the seed a compound `hiscall` would stay
    `<...>` and be filtered out), decode the one focused window, return rows, and
    **drop** the worker. **Do NOT reuse one worker across multiple foci in the same
    slot:** even though it is discarded after the slot, decoding focus 1 then focus
    2 on the same worker double-rotates odd/even for the same timestamp and lets
    focus 1's duplicate/AP/hash state corrupt focus 2. One worker per focus avoids
    all intra-slot crosstalk. Cost: ≤5 worker builds (`ft8apset`/`tone8`/workspace
    alloc) + ≤5 seeds per target-parity slot — acceptable for v1; the focused
    decode itself is seconds.
  - **v2 (optional optimization, only if v1 build cost is profiled as a real
    bottleneck): `_no_commit` on a shared session** — snapshot **every** cross-slot
    mutable field (`_state` **and** `book` **and** `regular_hash_calls`), decode,
    then restore all of them. **Risk: missing one field is a silent state-leak
    bug** — do not attempt v2 until v1 is proven too slow.
- **Changes.**
  - **v1 needs no new session API:** each focused worker is a fresh
    `JtdxStreamDecodeSession::new(focus_config)` where the focus frequency/band/
    `nagain` are already in `focus_config`; the coordinator computes the window
    (`nfa/nfb = nfqso ± 25`). `mycall`/`hiscall`/`lhound` come from the base `dx`
    config (so apset/tone are correct).
  - **v2 only:** a `set_slot_context(&mut self, ctx)` retarget helper (update the
    stable-safe `_config` fields `nfqso`/`nfa`/`nfb`/`nagain`/`swl` without an apset
    rebuild) for the shared-session `_no_commit` path. Not built unless v2 is.
- **Dev notes.** `mycall`/`hiscall`/`lhound` are fixed for a chase. In v1 each
  focused worker is built once and used for exactly one focus (no per-slot mutation
  of a shared session).
- **Tests (config/call-chain, not fragile audio assertions).**
  - Unit: the coordinator builds a focus config with `nfa/nfb = nfqso ± 25`,
    `nagain=true`, and the base `mycall`/`hiscall`; the primary listen config is
    pinned (`swl=true`, `nagain=false`, full band) regardless of a user `--nagain`.
  - Unit: a focused pass (fresh disposable worker) does **not** alter the primary
    session's cross-slot state (parity/avexdt/`book`/`regular_hash_calls`); a fresh
    worker is seeded with `{hiscall, mycall}` before decode.
  - Integration: the existing manual reference recovers `F1MLZ UA3QNA`
    (`jtdx --swl --nagain --my-call F1MLZ --rx-frequency 1152`).
  - Standalone `jtdx` long fixture still 430/431 (these APIs unused there).
- **Acceptance.** Per-slot focus works; focused passes leave the primary session's
  cross-slot state untouched; `jtdx` baseline byte-identical.

### Step 3 — TargetContextStore + harvest + hiscall output filter

- **Goal.** Cross-slot harvest of the target's frequency **set**, grid, **TX
  parity**, and dt; output filtered to `hiscall` only (incl. compound/hash).
- **Changes.**
  - `src/decode/dx/context.rs`: `TargetContextStore` with a bounded frequency set
    (sliding-window aging), harvested grid, observed TX parity, recent dt.
  - **TX-parity inference — must use sender/recipient role, not mere presence.**
    In an FT8 standard message `CALL1 CALL2 …`, **`CALL2` is the sender (the
    transmitting station); `CALL1` is the addressee.** CQ is the exception
    (`CQ CALL GRID` → `CALL` is the sender). So `hiscall` *appearing* does not
    mean the target is transmitting — its **role** decides:
    - **Strong target-TX-parity evidence (target IS transmitting this parity):**
      `CQ HISCALL GRID`; or `HUNTER HISCALL -10` (HISCALL is `CALL2` = sender).
      (With our own call this is `MYCALL HISCALL -04` = the DX answering us.)
    - **Target-is-recipient this parity (someone is *calling* the target; the
      target is NOT transmitting):** `HISCALL HUNTER -10` (HISCALL is `CALL1` =
      addressee; `HUNTER` is the sender). (With our own call this is
      `HISCALL MYCALL R-05` = us calling the DX.)
    - **Test-writer note:** the two forms differ only by callsign order —
      `HUNTER HISCALL …` is target **TX**, `HISCALL HUNTER …` is target **RX**.
      Do not swap them.
    - If only recipient-role rows are seen, infer the target's TX parity as the
      **opposite** parity, but mark it **weaker** (inferred, not observed). Never
      collapse this to "hiscall appears → target TX parity" — that mislabels
      `HUNTER HISCALL report` (the real target-TX message) as hunter parity and
      the gate would skip the true target slot.
  - **Frequency harvest is layered by pass and role, and gated by FH mode.** A
    row’s frequency joins the set; but **prefer target-as-sender rows** (their
    frequency is the target’s own TX frequency). Pin the default policy:
    - **simplex / non-FH (`lhound` off):** target-as-recipient and
      `mycall`-as-recipient frequencies *may* become focused candidates, at **weak
      confidence** (QSO frequency ≈ target frequency in simplex).
    - **Fox/Hound (`lhound` on):** a hunter’s / `mycall`’s TX frequency is **not**
      the Fox’s TX frequency. Use recipient/`mycall` frequencies **only as a weak
      clue, never as a direct Fox focus** unless corroborated by a
      target-as-**sender** row. Otherwise we waste foci decoding the wrong band.
    The **cheap listen pass is the trusted broad harvest source**; focused/deep
    passes (higher FP) may update target evidence **only from target-filter hits**,
    and `hisgrid`/parity updates require the target as **sender** plus proximity to
    an existing focus — never from a deep pass’s non-target side-decode.
  - `dt` is recorded per row. **`dt` is a confidence/window signal, not a recovery
    lever** — the decoder finds and sub-symbol-refines `dt` itself (`sync8` lag
    search + `ft8b` time refine), so it cannot be "fed" like `nfqso`. Use it only
    for (a) Step-7 confidence and (b) a hint to keep `swl`/`--force-sync` on if the
    target runs an unusual large `dt` (`avexdt` already auto-recenters the window).
  - `src/decode/dx/filter.rs` (or inline): the target output filter. **Canonical
    matching rules (decided):**
    1. **Seed the book via the existing session API.** Feed `{hiscall, mycall}`
       (≤2 entries) into the worker `HashCallBook` at session start through the
       **existing session-level `import_hash_calls`** — both workers already have
       it (`StreamDecodeSession::import_hash_calls`, `session.rs:393`;
       `JtdxStreamDecodeSession::import_hash_calls`, `lib_jtdx/mod.rs:108`, added
       in the hybrid milestone). Seeding happens at the **session/profile layer,
       never by kernel-level callbook mutation** (C1). They are known up front, so
       the **kernel resolves any matching-hash `<...>` to `<hiscall>`/`<mycall>`
       natively** during decode (calls10/12/22 lookup) — no separate hash step in
       the filter.
    2. **Normalize** each message word: trim surrounding `;`/`,`, uppercase,
       strip resolving braces so `<UA3QNA>` ≡ `UA3QNA` (reuse
       `hybrid::normalize_message`). A genuinely unresolved `<...>` stays distinct.
    3. **Keep iff `hiscall` appears** — as a plain/compound token or the
       kernel-resolved `<hiscall>`. Standard calls (e.g. `UA3QNA`) normally arrive
       in full; the hash path matters mainly for a **compound `hiscall`**
       (`EA5/DH0YAH`), which the seeded book resolves to `<EA5/DH0YAH>`.
    4. **Everything else is discarded — including any remaining `<...>`.** By
       definition a still-unresolved `<...>` did **not** match our two calls, so
       it is not our target. (Owner decision: if the hash *is* hiscall's, the
       kernel already shows `<hiscall>` and rule 3 keeps it; otherwise drop it.)
- **Hash-collision caveat (hand to Step 7).** Kernel hash resolution can mislabel
  a *different* station as `<hiscall>` when its hash collides — negligible at
  22-bit (~1/4M), ~1/4096 at 12-bit, **~1/1024 at 10-bit**. This is identical to
  stock WSJT-X/JTDX with a callbook, not a chase-specific flaw. Mitigate by
  cross-checking a resolved-`<hiscall>` row against the harvested **frequency**
  and **dt** before treating it as a confirmed target (Step 7 confidence); trust
  22-bit matches most. A rare 10-bit collision cannot be fully eliminated.
- **Harvest hygiene (required — stability safeguard, not optional).** The
  cross-slot focus is a feedback loop: a bad harvest can lock subsequent slots
  onto a wrong frequency. So: only harvest a frequency from rows that **actually
  contain `hiscall` or `mycall`** and are not low-confidence/assisted noise;
  age out stale frequencies (sliding window); keep the set bounded; never
  over-commit to a single harvested value (retain the user-given `nfqso` seed and
  multiple candidates). This prevents the feedback loop from drifting onto QRM.
- **Dev notes.** Reuse `hybrid::shared` collision logic with a ≤2-entry book.
  Harvest is committed *after* a slot, used by the *next* slot (C6).
- **Bootstrapping (how the cold-start chicken-and-egg resolves).** The listen is
  context-free (no MyCall-AP), so a *weak* target is usually **not** decoded in
  the listen. The two harvested quantities bootstrap from different sources:
  - **frequency** comes from the listen decoding *stronger nearby activity that
    names our calls* — others calling the target, or others calling `mycall` at
    the QSO frequency (this is how `1152` is harvested from `140630 F1MLZ RA3ABG`);
  - **grid and TX-parity** come later from a **focused-pass target-as-sender hit**
    (once a frequency exists, the focused deep decodes the target itself, and
    that hit yields its grid/parity).
  So frequency unlocks the focus, and the focus unlocks grid/parity.
- **Tests.**
  - Synthetic rows → assert harvested freq set / grid / parity.
  - hiscall filter keeps only target rows; compound-call variant resolves
    `<...>` and matches; non-target rows (RA3ABG, IW1PUR) dropped.
  - Harvest hygiene: a non-target row at the target's expected frequency does
    **not** get harvested as the focus (must mention hiscall/mycall).
- **Acceptance.** Harvest correct & deterministic; output contains only
  hiscall-related rows; compound-call path covered; harvest cannot be poisoned by
  a non-target row.

### Step 4 — Parity gate + cost ladder (core orchestration; the validation milestone)

- **Goal.** Implement the per-slot ladder from `DX.md` (Option A — deep is always
  focused, no blind full-band deep): parity gate → cheap listen → focused deep
  recovery (≤5 foci) → harvest update.
- **Changes.** `dx/mod.rs` orchestration using Step 2 API + Step 3 store:
  1. **Parity gate.** If slot parity ≠ harvested target TX parity (and parity is
     known): run only the cheap listen pass (2), nothing deeper.
  2. **Listen** — the **context-free pinned config** from Step 2 (`swl`, full
     passband, `nagain=false`, `ndeep=3`, `mycall`/`hiscall` off in apset, book
     seeded for resolution), on the **primary session (committing pass)**. Cheap
     (~6 s); handles the "no frequency yet" case (no separate blind deep pass).
     Primary harvest source.
  3. **Focused deep** — **requires `mycall` AND a known frequency** (C9: `ndeep=5`
     only helps the MyCall-AP branch — measured 0 gain without `mycall`). **Focus
     selection (must be deterministic for C6):** take the candidate frequency set,
     **collapse candidates within one `±25 Hz` window into a single focus** (two
     candidates 5 Hz apart are the *same* deep pass — never spend two foci on one
     window), **sort the resulting foci by (confidence desc, then low-band-first
     per the 0–1000 prior, then frequency asc)**, take the top **≤5**, and **`log`
     any foci dropped by the cap** (no silent truncation). For each focus:
     `swl + nagain`, `nfqso=freq`, `nfa/nfb=freq±25`, run on Step 2’s **fresh
     disposable focused worker (v1)** so it never advances the primary’s cross-slot
     state. **In file mode run all ≤5 foci** (no order-dependent early-stop) and
     emit deterministically-sorted output, so the run is reproducible (C6);
     early-stop ("found → skip remaining foci") is a **monitor-only** latency
     optimization. **Without `mycall`:** skip focused deep entirely — `dx` still
     listens + harvests + filters (a non-`nagain` focused diagnostic may exist
     behind a flag, but is not the default). (a8d added in Step 5.)
  4. Harvest update (layered, per Step 3): broad from the listen pass; from
     focused passes only target-filter hits. hiscall-filter output.
- **Dev notes.**
  - **Escalation, not early-discard — and early-stop is monitor-only.** "Stop
    early" means *stop climbing to more expensive passes* once the target is found;
    it does **not** mean discard harvest (always fold the decodes of every pass
    that ran into the store, **subject to the layered-harvest trust rules** —
    focused/deep non-target side-decodes do not update target frequency/grid/
    parity). **File mode does not early-stop** (it runs the full bounded ladder so
    output is reproducible, C6); early-stop is a monitor-mode latency optimization
    only.
  - **Cross-slot state safety (per Step 2).** Only the listen pass commits to the
    primary JTDX session. Focused/deep passes run on a **disposable worker (v1)**
    (the `_no_commit` shared-session path is a v2 optimization only), so multiple
    passes per slot cannot double-rotate parity or double-commit AP/avexdt/`book`
    memory.
  - **Where cross-slot AP intelligence lives.** A fresh focused worker has **empty
    a7/odd-even AP memory**, so the focused recovery relies on *current-slot*
    MyCall-AP (apset built from `mycall`/`hiscall`), **not** JTDX's worker-level
    cross-slot a7 memory. That is intentional: the cross-slot intelligence is
    carried by the **coordinator's harvest** (frequency/grid/parity/`hisgrid`),
    which is fed back as current-slot config — so isolation costs no real
    sensitivity, it just moves the memory from the worker to the coordinator.
  - **Monitor watchdog (monitor mode only).** Bound per-slot focused work (≤5 foci)
    with a ~12 s wall-clock guard. Because Rust cannot force-kill a thread, the
    guard runs each focused pass on a **disposable worker** (never borrowing the
    primary session): on timeout, abandon that worker’s result and continue; the
    abandoned worker finishes and is dropped. Cap **at most 1 outstanding
    abandoned worker** (if one is still running, skip the next focused pass rather
    than stack workers and snowball CPU). The primary session is never lent to a
    watchdog thread. **File mode does NOT use the guard** — it runs every pass to
    completion (reproducible, C6); the wall-clock guard is non-deterministic and
    hence monitor-only.
- **Tests (the headline validation).** (Depends on **Step 0** having proven the
  fresh-worker recovery — if Step 0 needed the avexdt/force-sync mitigation, apply
  it here too.)
  - `--profile dx --my-call F1MLZ --his-call UA3QNA` (NO `--rx-frequency`) on
    `230208_140300.wav` **recovers `140700 F1MLZ UA3QNA -04`**, frequency
    harvested from `140630 F1MLZ RA3ABG`. Output contains the UA3QNA rows and
    **not** RA3ABG/IW1PUR.
  - Parity gate: assert the non-target parity runs only the listen pass (e.g.
    via a decode-pass counter / timing).
  - File-mode reproducibility: identical output across repeated runs.
- **Acceptance.** Target recovered from harvest alone; output = hiscall-only;
  file-mode reproducible; per-slot latency recorded (cheap listen + focused
  passes; no blind deep pass exists).
- **Note — this is a mechanism fixture, not a real DXpedition.** `140700
  F1MLZ/UA3QNA` is an ordinary FT8 QSO at 1152 Hz, not Fox/Hound. It validates
  the *mechanism* (harvest frequency → focused MyCall-AP recovers a missed weak
  reply), not real-world DXpedition/FH performance. Real FH gain is unmeasured
  until an FH fixture exists (Step 6 blocker).

### Step 5 — a8d via a focused WSJT-X worker (optional, gated on harvested `hisgrid` + `mycall`)

- **Goal.** When `hisgrid` is known and `mycall` set, reach WSJT-X a8d at `nfqso`
  for the weakest "DX-calling-me" replies.
- **Important — a8d has no standalone entry point.** `ft8_a8d_result` runs
  *inside* `decode_slot_nzhsym50_and_finish` (`session.rs:588`, gated by
  `wsjtx_a8_allowed`) and consumes that flow's `full_residual`. So Step 5 runs a
  **focused WSJT-X `StreamDecodeSession`** (its regular/a7 path plus a8d) at the
  harvested `nfqso`; a8d fires as part of it. Running "a8d only" is not possible
  without new plumbing — *optionally* a later optimization could extract a
  standalone a8d helper, but that is out of scope here.
- **Changes.** The a8d pass is a **focused pass too, so it obeys the same
  state-isolation discipline as Step 2**: a **fresh disposable WSJT-X
  `StreamDecodeSession` per focus** (built focused at `nfqso`, narrow `nfa/nfb`,
  with `mycall`/`hiscall`/`hisgrid`), **seeded with `{hiscall, mycall}` before
  decode**, run, then dropped. `StreamDecodeSession` has its own cross-slot state
  (a7 AP memory, `jseq`, `book`, `regular_hash_calls`); do **not** keep a
  long-lived committing WSJT-X session for this — that would pollute cross-slot
  state exactly like the JTDX case. a8d output is one of the disposable worker's
  rows; merge/dedupe with the JTDX rows (reuse hybrid merge).
- **Dev notes.** a8d is gated by `wsjtx_a8_allowed` (needs hiscall≥3, hisgrid≥4,
  nfqso, **and no prior decode within 3 Hz of nfqso**). Consequence: a8d only
  fires when that disposable worker's own regular/AP decode **missed** the target
  at `nfqso` — i.e. a8d is the last-resort recovery when MyCall-AP didn't catch it.
  Feed harvested grid as `hisgrid`. The focused window keeps the extra regular/a7
  work cheap.
- **Tests.** `tests/ft8/a8d_k1jt_bg5atv_pm00.wav` recovered through `dx` when
  grid is harvested/provided; confirm no FP regression on the long fixture.
- **Acceptance.** a8d reachable from `dx`; a8d fixture recovered; no new FP.

### Step 6 — FH / multi-frequency (`lhound`, bounded multi-focus)

- **Goal.** Handle Fox/Hound: multiple simultaneous target frequencies.
- **Changes.** Iterate the harvested frequency set (≤5 foci) in Step 4’s focused
  stage; enable `lhound` via the existing **user `--hound` flag** (Fox tone tables
  + wide window). FH auto-detection is out of scope — the operator knows it is an
  FH DXpedition and sets `--hound`.
- **Dev notes.** Cost = (#foci) × focused-pass; keep ≤5 and bounded.
- **Blocker.** No Fox/Hound fixture exists. Until one is added, multi-focus is
  unit-tested with synthetic multi-frequency input only; real FH gain is
  **unmeasured**.
- **Tests.** Synthetic multi-freq harvest → ≤5 foci issued; with a real FH
  fixture (when available) measure recovery + latency.
- **Acceptance.** Bounded multi-focus implemented & unit-tested; FH real-signal
  gain either measured (fixture) or explicitly **killed/deferred-with-evidence**.

### Step 7 — False-positive control, confidence, finalize docs

- **Goal.** Lock the FP story and finalize documentation.
- **Changes.** Primary FP control is the hiscall filter (C4). Add a confidence
  model (reuse `hybrid/evidence.rs`) — but **define its action precisely**, since
  normal CLI output is clean (no attribution):
  - **Emit immediately, do not wait for confirmation.** A chaser must not wait a
    full same-parity cycle (~30 s) before seeing a target row, so dx emits a
    hiscall match in the slot it is found. Confidence is computed, not used as an
    emission delay.
  - **Suppress only on a hard contradiction:** a row whose grid conflicts with a
    confidently-harvested `hisgrid` is almost certainly a hash collision / false
    and is dropped. (Grid is a strong identity check.)
  - **Everything else emits; confidence is a soft annotation** carried to
    UDP/debug only (off-frequency, off-`dt`, single-sighting, or a first-time
    hash-resolved `<hiscall>` that could be a 10-bit collision) — the clean CLI
    line is unchanged. **Temporal-consistency** raises confidence for a target row
    that recurs across same-parity slots at the same frequency; it lowers it for a
    one-off, but never suppresses on that alone.
  Update `DX.md` "Measured results" and this plan’s status.
- **Do NOT lower decode thresholds (hard rule).** The hiscall filter removes
  decodes about *other* calls but cannot remove a fabricated codeword that spells
  `hiscall` — the worst error for a chaser. CRC (~1/16384 false-pass/attempt) is
  the real guard; lowering `syncmin` / decode-acceptance gates increases attempts
  → more CRC-false-passes → more fabricated-`hiscall` rows the filter can't catch.
  Sensitivity comes from context (AP + focus), which raises true-decode
  probability without raising the noise floor. The aligned levers
  (`swl`+`lft8lowth`→`syncmin` 1.1, `nagain`→`ndeep=5`) are already maxed; going
  lower would also touch the kernel (violates C1).
- **Tests.** Record an FP budget number on the long fixture (extra
  hiscall-matching rows not in the CSV baseline). Confirm aggressive-search
  side-decodes (e.g. IW1PUR) never reach output.
- **Acceptance.** FP budget recorded as a number; docs reflect shipped behavior.

## Future Enhancements (post-MVP, all kernel-free)

These extend Steps 1–7 once the core validation holds. Each is orchestration-only
(no `lib_*` change) and must keep all hard gates green; measure before trusting.

### E1 — QSO-state modeling → precise AP context (sensitivity)

- **Idea.** Track the target QSO's state from decoded messages (and our own TX if
  known), and steer AP via `nQSOProgress`/context to the **messages the target is
  about to send**. Crucially, predict **two branches, not one**:
  1. **Advance** — if the target received our reply, it sends the *next* message
     (e.g. after `F1MLZ UA3QNA -04` and our `R-report`, expect `... RR73`/`RRR`).
  2. **Repeat-current** — if the target did **not** receive us (the common case
     on a marginal DX path), it **re-sends its current message** (`F1MLZ UA3QNA
     -04` again). **This branch is often dominant** in weak-signal chasing — the
     DX keeps repeating because the hunter isn't getting through, so AP must keep
     expecting the *same* message, not only the next one.
- **Mechanism.** Cover both branches by cycling/seeding `nQSOProgress` across the
  current and next state; config-only, no kernel change.
- **Acceptance.** On a fixture with a stalled/repeating exchange, AP recovers the
  repeated target message at least as well as a fixed-progress run; no FP
  increase past budget. **solid.**

### E2 — Dual-worker focused union (sensitivity + cross-confirmation)

- **Idea.** On the target parity, run **both** WSJT-X and JTDX focused on the
  harvested frequency and union their target rows — WSJT-X `a7`/`a8` and JTDX
  deep/FT8S recover different things; agreement also raises confidence (stability).
- **Mechanism.** Reuse the `hybrid` two-worker merge, scoped to the target.
- **Cost.** ~2× per-slot on the target parity only (parity gate keeps the other
  parity cheap). **solid.**

### E3 — Orchestrated QRM subtraction (sensitivity + anti-interference)

- **Idea.** When a strong station sits near a weak target, decode+subtract the
  strong one first (existing `subtractft8`, orchestration-controlled order), then
  re-decode the cleaned residual focused on the target.
- **Mechanism.** Same capability the stream layer already uses for `nzhsym`
  staging; no kernel change. A focused sub-step of Step 4. **solid.**

## Future Research (harder; kernel-free path unproven)

- **Same-parity slot stacking/averaging.** The target repeats identical messages
  (e.g. `F1MLZ UA3QNA -04` at 140700 *and* 140730). Combining repeated slots
  could raise SNR, and CRC would reject mis-stacked content (no FP). **Hard:** the
  kernel consumes a waveform; FT8's non-coherent symbol averaging across slots is
  not cleanly reconstructable into a waveform without touching the kernel.
  Investigate, don't commit.
- **Interference-excision preprocessing.** Deterministic notch of birdies/carriers
  and impulse blanking on the audio *before* the kernel (feeding a filtered
  waveform is allowed; it does not change the decoder). Must avoid the target
  frequency and stay deterministic (file reproducibility). Anti-QRM; some
  distance to a safe, measured implementation.

## Global Definition of Done

- All steps in a terminal state (shipped-with-evidence or killed/deferred-with-
  evidence). Step 6 (FH) may be deferred for lack of a fixture, documented as such.
- **Hard gates (all must hold):**
  - C7 baselines row-for-row unchanged (`wsjtx`/`jtdx`/`hybrid`).
  - Validation target recovered: `--profile dx --my-call F1MLZ --his-call UA3QNA`
    (no frequency) → `140700 F1MLZ UA3QNA -04`, output hiscall-only.
  - File-mode reproducible (identical across runs; no scheduling-dependent
    feedback).
  - Output contains only hiscall-related rows.
  - `cargo fmt --check` clean; no new clippy warnings in the code `dx` touches
    (`src/decode/dx/` **and** Step 1's edits to `session.rs`/`profile.rs`/`main.rs`).
  - FP budget recorded.
- Constraints C1–C9 hold throughout.
- `DX.md` (product) and this `PLAN.md` (status) agree with the shipped code.

## Risks / Open Questions / Blockers

- **Fresh-worker recovery — RESOLVED (Step 0 verified).** A fresh single-slot
  focused worker recovers `140700 F1MLZ UA3QNA -04`; the `avexdt=0`/empty-memory
  concern did not materialize. No mitigation needed; the isolation design stands.
- **avexdt loss in fresh workers.** Fresh focused workers do not track the
  population `dt`, so large-`dt`/odd-`dt` targets may sync worse than a long-lived
  session would. Mitigation: seed the focused worker's `avexdt` from the harvested
  `dt`, or use `--force-sync`. Measure if real targets are missed.
- **FH fixture missing (blocker for Step 6).** No real Fox/Hound recording to
  measure multi-frequency gain. Need one with: Fox in low sub-band, a slot where
  the Fox calls "us", and ideally a slot where the Fox sends a grid.
- **Monitor latency.** Resolved by Option A: no blind full-band deep pass
  exists. Per-slot cost is the cheap listen (≈6 s) plus focused deep passes
  (seconds; ≤5 foci for FH). The ≤5-foci worst case is bounded by the
  monitor-only 12 s watchdog (abandon over-budget pass; file mode runs to
  completion). For reference, a *blind* full-band `ndeep=5` would be ≈23 s/slot
  (0–1000) — which is exactly why it is not in the ladder. (Earlier "~26 min /
  ~8 min" figures were whole-fixture totals over 19 slots plus harness overhead,
  not per-slot — corrected.)
- **Compound-call test fixture.** Need a fixture exercising a hashed `hiscall`
  (`<...>`) to validate Step 3’s hash-match/resolution end to end.
- **Cold-start parity.** At true cold start there is **no harvested frequency**, so
  the slot runs **listen only** (focused deep needs a frequency) — cheap. Only in
  the window where a frequency is already harvested but the target's TX parity is
  not yet observed do *both* parities run the focused ladder (≤ a few slots);
  bounded extra cost, confirm acceptable.
- **Residual miss (accepted under Option A).** A target that is too weak for the
  context-free listen **and** has no decodable QSO activity naming it / `mycall`
  (no one working it loudly enough) yields **no harvested frequency**, so the
  focused recovery never starts and the target is missed. Dropping the blind
  full-band deep (Option A) accepts this case; recovery resumes the moment any
  decodable activity reveals the frequency.
- **Two-worker cost (Step 5).** The disposable WSJT-X a8d worker adds another
  focused decode on the target parity (only when `hisgrid`+`mycall` hold); measure
  before always-on, or gate strictly on grid. It runs per-focus like the JTDX
  focused worker (state-isolated, dropped after).

## Test Fixtures & Commands

- Long fixture: `tests/ft8/230208_140300.wav` (`--start-time 230208_140300`).
  Validation target row `140700 F1MLZ UA3QNA -04` (CSV `Extra=J`).
- a8d fixture: `tests/ft8/a8d_k1jt_bg5atv_pm00.wav`.
- **DX synthetic fixture: `tests/ft8/dx_synth_ua3qna.wav`** (5 slots / 75 s,
  16-bit/12 kHz/mono). Purpose-built so cross-slot harvest can flow into the
  weak target. **Layout (slot → content):**
  - slot 0 = **synthesized strong** `F1MLZ UA3QNA -04 @1152` (the harvest source);
  - slots 1–4 = **real contiguous** `230208_140300.wav` segs 15–18
    (140645 / 140700 / 140715 / 140730).
  Slots 0, 2, 4 share one parity, 1 & 3 the other — so it exercises harvest +
  parity gate + recovery of **both** real targets in one fixture.
  **Generation recipe (deterministic; re-add the generator under `src/decode/dx/`
  when the module exists):** `genft8("F1MLZ UA3QNA -04")` → `itone`;
  `gen_ft8wave(itone, 1152.0)` → `re`; slot 0 = `re*0.5` placed at sample 6000
  (0.5 s TX start) + gaussian noise `*0.02`, length 180000; append
  `read_wav_mono_f32("230208_140300.wav").samples[15*180000 .. 19*180000]`; write
  16-bit. **Verified at generation:** slot 0 decodes `F1MLZ UA3QNA -04` in the
  context-free listen; slots 2 and 4 recover `F1MLZ UA3QNA -04` via a fresh
  focused worker.
  **Discriminating baseline** — `--profile hybrid` (WSJT-X+JTDX union, no context)
  on this fixture **decodes the synth slot and the real 140730, but MISSES the
  real 140700** (the `Extra=J` target). That is exactly the row `dx` adds via
  harvest→focus, so the fixture is a genuine discriminator, not a give-away.
- Baseline gates (must stay green):
  - `cargo fmt --check`
  - clippy **review gate, not `-D warnings`** (the mirrored kernels already emit
    ~164 expected warnings, so repo-wide `-D warnings` is infeasible). The gate is
    *no **new** warnings in code `dx` touches* — which includes Step 1's edits to
    `session.rs`/`profile.rs`/`main.rs`, not just `src/decode/dx/`. Check with:
    ```
    cargo clippy --release --all-targets 2>&1 | tee /tmp/ft8rs_clippy_dx.txt
    rg 'src/decode/dx|src/stream/profile.rs|src/stream/session.rs|src/main.rs' \
       /tmp/ft8rs_clippy_dx.txt
    ```
  - `cargo test --release test_stream_decode_short_audio` (wsjtx 21/21)
  - `cargo test --release test_stream_decode_long_audio` (wsjtx 424/424)
  - `cargo test --release test_jtdx_profile_short_audio -- --ignored` (20/20)
  - `cargo test --release test_jtdx_profile_long_audio -- --ignored` (430/431)
- New `dx` tests live in `tests/stream_decode_test.rs` (long-fixture ones
  `#[ignore]`, release-mode), plus unit tests under `src/decode/dx/`.
- Manual reference (proves the target is reachable today, pre-`dx`):
  `ft8rs file tests/ft8/230208_140300.wav --start-time 230208_140300 \
   --profile jtdx --swl --nagain --my-call F1MLZ --rx-frequency 1152`
  → recovers `140700 F1MLZ UA3QNA -04`.
  **CAVEAT — this is a *long-lived* session over all 19 slots; `dx` uses a *fresh*
  focused worker (avexdt=0, empty AP/odd-even memory). The recovery is NOT yet
  verified on the fresh-worker path** (see Step 0).

### Step 0 — Pre-flight: verify the fresh-worker recovery — **✓ VERIFIED**

The whole validation rests on a fresh per-focus worker recovering `140700`, but
the only earlier evidence was a long-lived session (which by `140700` has
accumulated `avexdt` and odd/even AP memory; a fresh worker starts at `avexdt=0`,
`Ft8Mod1::default`, and `avexdt` shifts the sync-window center
`jzb/jzt = ±86 + avexdt*25`).

**Result (verified, then probe removed for a clean slate):** a **fresh**
`JtdxStreamDecodeSession` with `{swl, nagain, mycall=F1MLZ, hiscall=UA3QNA,
nfqso=1152}` (book seeded `{F1MLZ, UA3QNA}`), decoding **only** the `140700`
slot, returns exactly **`1152Hz F1MLZ UA3QNA -04`** (1 row) in ~13 s. So the
fresh-worker architecture is **sound** — the `avexdt=0`/empty-memory concern does
**not** materialize for this target, and no `avexdt`-seed / `--force-sync`
mitigation is needed. A companion check confirmed a synthesized
`F1MLZ UA3QNA -04 @1152` frame decodes in the context-free listen (the harvest
source is buildable).

These were throwaway verification probes (they lived temporarily in
`lib_jtdx/genft8.rs` and `tests/stream_decode_test.rs`) and have been **removed**
now that the results are recorded; the proper regression test is re-created in
`src/decode/dx/` once the module exists (Step 4 end-to-end test, which subsumes
the fresh-worker recovery). The validation fixture they produced is kept — see
**Test Fixtures & Commands → DX synthetic fixture**.
