# DX Sensitivity Plan — Deep-Integration Engine (T0 / T1 / T2)

Engineering design for raising `profile=dx` sensitivity **beyond the single-slot
kernel ceiling**, without touching the aligned WSJT-X/JTDX decoders. Product
context lives in `DX.md`; this file is the staged build plan.

The work adds a **dx-private deep-integration engine** as a third recovery engine
(alongside MyCall-AP and a8d). It does what the per-slot kernels structurally
cannot: exploit a *known, repeating* target across slots and the *tightly
constrained* dx message space. It is **not** a reimplementation of the FT8
decoder — single-slot decode is already at the OSD `ndeep=5` ceiling.

---

## 0. Why single-slot is already maxed

- The aligned ceiling is `--swl --nagain` = OSD `ndeep=5` (AP at max depth) +
  focused window + a8d (206-message list). Nothing in the kernels goes deeper for
  one 15 s slot.
- Sensitivity cannot come from lowering gates: looser sync/accept thresholds →
  more decode attempts → more CRC false-passes → **fabricated `hiscall` rows**,
  the worst possible error for a chaser. (Hard rule, carried from `DX.md`.)
- Therefore the remaining headroom is **outside one slot**: multiple observations
  of the same target (cross-slot integration) and the **collapsed hypothesis
  space** of a single known station (constrained deep search).

## 1. Non-negotiable constraints (carried forward)

- **C1 — Kernels stay aligned + independent.** No change to decode math,
  candidate order, AP scheduling, or thresholds in `lib_wsjtx`/`lib_jtdx`.
  Exposing an existing internal computation as `pub(crate)` (visibility only, no
  logic change) is permitted — precedent: `ihashcall`, `genft8`, `chkcrc14a`.
- **C2 — No threshold lowering.** Sensitivity comes from *more information*
  (averaging + priors), never from loosened acceptance gates.
- **C3 — Fabricated `hiscall` is the worst error.** Every new decode path needs
  an explicit false-alarm budget and guard; see §3.
- **C4 — File mode reproducible (precise form).** A slot's **configuration, focus
  set, and parity gate** use only knowledge **committed before** that slot —
  cross-slot feedback only. **Within the deep engine, the current slot's own
  observation may be extracted once and consumed once:** the seed-acquisition
  `sync8`/field-extraction (§4.2) reads the *current* slot's audio to get coarse
  `(freq, dt)` + the channel LLR, and T2's verdict combines `prior_stack +
  current field`. This is **not** same-slot replay and not a config/feedback loop:
  it (i) never re-runs the kernel *decode* on the slot, (ii) never writes the
  current observation back into this slot's config/focus/parity, and (iii) only
  *commits* the current field into cross-slot state **after** the verdict, for
  future slots. Acquisition is deterministic (same audio ⇒ same candidates), so
  file-mode output stays reproducible.
- **C5 — Output only `hiscall` rows.** The new engine feeds the same target
  filter / grid-contradiction / temporal-consistency gates already in
  `TargetContextStore`.
- **C6 — The deep engine is explicitly JTDX-derived, not profile-neutral.** It
  reuses the JTDX extraction/codec primitives (`ft8_downsample`/`extract_symbol_metrics`,
  `genft8`, `encode174_91`, `osd174_91`, …) as its numerical basis, without
  modifying `lib_jtdx`. This is a deliberate scoping choice, not an accident: a
  future WSJT-X-derived variant would be a separate adapter layer, not a tweak to
  this one. Document it so the dependency is intentional.

## 2. The three tiers and how they compose

```text
T0  AP tuning      drive the kernel's own AP with the live QSO state
                   (dynamic nQSOProgress + last-message tracking)
                          │  small gain, near-free, no new decode math
                          ▼
T1  Deep search    dx matched-filter over the collapsed hypothesis set
                   {mycall,hiscall,grid,QSO-state} -> ~dozens of messages
                          │  DETECTS below the blind-OSD threshold; is T2's decoder.
                          │  Single slot does NOT emit (needs >=2 slots / CRC, §3-5).
                          ▼
T2  Cross-slot     per (parity,freq) stack: align + sum per-slot LLRs,
    integration    discriminate the message at decode via T1 (+ CRC-OSD on llr_sum)
                          │  THE sensitivity lever (~1.5-2.5 dB per doubling); the
                          │  emit gain over `--swl --nagain` comes from here.
                          ▼
            same hiscall filter + grid/temporal gates -> emit
```

T1 is the **decoder**, T2 is the **integrator**, T0 is **AP plumbing**. T1 is
independently testable on single slots and becomes the natural decoder for T2's
averaged symbols.

## 2b. Rejected alternatives (and why not)

Kept here on purpose so the choices are not re-litigated.

- **A — Write a full FT8 decoder from scratch.** *Rejected.* Single-slot decode is
  already at the OSD `ndeep=5` near-ML ceiling, so a reimplementation buys **zero**
  single-slot sensitivity; it would also fork away from the aligned kernels
  (violates C1) and is a large effort. The real headroom is cross-slot + priors,
  which is an *added engine*, not a new decoder.
- **B — Lower sync/accept thresholds for more sensitivity.** *Rejected.* More
  attempts ⇒ more CRC false-passes ⇒ fabricated `hiscall` (C2/C3). Sensitivity must
  come from information, not loosened gates.
- **C — Coherent cross-slot combining (sum complex `cs`).** *Rejected.* No shared
  phase reference exists across independent 15 s transmissions (§5), so coherent
  summation just adds noise. T2 combines **non-coherently in LLR space** (sum of
  real per-slot LLRs), which is valid and is *not* this rejected coherent sum.
- **D — Replicate the symbol front-end inside `dx/` (D1b).** *Not recommended.*
  Zero kernel edits, but it would drift from the kernel's exact extraction; prefer
  the single additive wrapper D1a (which changes no existing visibility).

## 3. Shared false-alarm framework (most important section)

A constrained matched filter over a *known* callsign is exactly where a fabricated
`MYCALL HISCALL RR73` is easiest to invent. Guards, in order:

1. **Hypothesis set is small and fixed** — generated only from
   `{mycall, hiscall, hisgrid, QSO-state}`; never a free callsign search.
2. **Detection threshold with multiple-comparison correction.** The matched-filter
   statistic under noise has a known distribution; pick a per-hypothesis false-
   alarm probability `Pfa_1`, then require the set-wide budget
   `Pfa_set ≈ M · Pfa_1` (Bonferroni over `M` hypotheses) to stay below a fixed
   target. More hypotheses ⇒ stricter per-hypothesis threshold. **Threshold
   calibration is per detector configuration:** the v1 thresholds are tied to the
   single-source (`Cs`, ×2.83) channel LLR; a future multi-source ensemble is a
   *different* detector and must be re-calibrated — the Cs-only thresholds do not
   carry over.
3. **No CRC backstop for the matched-filter path** — hypothesis codewords are
   CRC-valid *by construction* (`genft8`), so CRC cannot catch a false alarm here.
   The threshold + temporal consistency are the *only* guards. This is the key
   risk and the reason the threshold is conservative.
4. **CRC-guarded path retained where possible.** When a soft-LLR vector is
   available (single slot, or T2's stack), also run it through the existing
   CRC-checked decoder (`bpdecode174_91`/`osd174_91`). These are **blind** decoders,
   so a CRC-valid decode is high-confidence (no temporal corroboration) **only after**
   it `unpack77`s to a message that passes the dx target gates (hiscall filter, grid,
   freq/dt). A CRC-valid **non-target** message is discarded/diagnostic, never emitted.
5. **Temporal consistency (v1 admission — TWO legal matched-filter forms).** A
   matched-filter-only decode (no CRC) emits via **either**:
   (a) **`TwoSlotMatched`** — the same normalized message passes the *single-slot*
   matched-filter gate in **≥2 slots** (target detectable per-slot but sub-CRC); or
   (b) **`StackedLlrMatched`** — a T2 stack of **depth ≥2** passes the *stack*
   matched-filter gate on the summed LLR, **and every slot admitted into that stack
   passed PHYSICAL admission (freq/dt anchor + sync floor)** — *not* a per-slot
   hypothesis-margin gate (the margin is tracked only for confidence/branch/reset,
   never required for accumulation; §T2 two-level admission). Otherwise the weak
   slots T2 exists to stack would be excluded. (T2's headline case: no single slot is
   strong enough, the *sum* is.) Both are *matched-corroborated* provenance
   (`TwoSlotMatched` / `StackedLlrMatched`), distinct from `CrcConfirmed`.
   **This is essential:** reading temporal consistency as "two independent
   single-slot passes" only would forbid exactly the sub-single-slot targets T2
   exists to recover. (A CRC-valid decode, §3-4, emits on its own.)
   There is intentionally **no vague "kernel partial" path in v1** — a "partial"
   would have to be pinned to a concrete, measurable gate (e.g. a kernel sync
   candidate at the same focus/freq/dt with `nsync`/`syncavemax` above a set
   threshold) before it could be trusted, so it is deferred until such a gate is
   defined and measured. Until admitted, a sub-threshold hit is harvest/soft-annotate
   only. **Every emitted row carries its `DeepConfidence`** (`TwoSlotMatched` /
   `StackedLlrMatched` / `CrcConfirmed`) in provenance (UDP/debug), so FP analysis sees
   *which* path produced a row — the two are not collapsed into one "decoded".
6. **Calibration is a test deliverable with a sized budget**, not a guess. "0
   fabricated `hiscall` on the fixture" is necessary, not sufficient — a tiny
   fixture gives false confidence. **Concrete v1 budget (adjustable, but written
   now so thresholds are not set by gut feeling):** target **0 fabricated `hiscall`**
   over a corpus of **≥ 24 h of pure noise**, **≥ 1000 wrong-call slots**
   (`OTHER1 OTHER2 …`), **≥ 2 h of real on-band recordings** (the hardest case —
   real interferers, tone leakage, colored noise), plus **≥ 50 hash-collision /
   near-callsign cases** (a `<…>` that resolves to `hiscall`, and calls one
   edit-distance from `hiscall`). Thresholds are tightened until that 0-budget is
   met on this corpus (§7). **Statistical honesty:** this is *0 observed over a
   finite corpus*, **not** a true `Pfa = 0`. Record it as **`0 fabricated / corpus
   size / 95% upper confidence bound on Pfa`** (rule-of-three: 0 in N gives
   `Pfa ≲ 3/N`), so the guarantee is not overstated. The engineering gate stays
   "0 fabricated", but the reported quantity is the bound. **Two test tiers:** normal CI runs only small fast
   fixtures (detection + a few-slot trend); the heavy Pfa corpus is a
   **manual/`#[ignore]`/release gate** (24 h of noise can't live in CI) that **must
   be run and its numbers recorded before a tier is declared done** — same pattern as
   the existing ignored dx gates.

## 4. Source-code verification — reuse points, signatures, kernel effect

Every primitive the engine needs already exists. This section is the **verified
reuse contract**: exact signature, location, visibility, and purity, plus the
proof that wiring them up does **not** change the aligned decoders. The engine
lives in `src/decode/dx/` and depends only on these.

### 4.1 Already-usable primitives (no kernel edit)

| Primitive | Signature (verified) | Location | Vis | Effect |
|---|---|---|---|---|
| Message → 79 tones | `get_tones_from_77bits(&[u8;77]) -> [i32;79]` | [genft8.rs:20](src/decode/lib_jtdx/genft8.rs:20) | `pub(crate)` | pure |
| Text → bits+tones | `genft8(&str) -> Option<(String,[u8;77],[i32;79])>` | [genft8.rs:7](src/decode/lib_jtdx/genft8.rs:7) | `pub(crate)` | pure |
| Codeword | `encode174_91(&[u8]) -> [u8;174]` | [encode174_91.rs:10](src/decode/lib_jtdx/ft8v2/encode174_91.rs:10) | `pub(crate)` | pure |
| CRC build / check | `crc14(&[u8]) -> u16`, `chkcrc14a(&[u8]) -> bool` | [chkcrc14a.rs:11](src/decode/lib_jtdx/ft8v2/chkcrc14a.rs:11),[:5](src/decode/lib_jtdx/ft8v2/chkcrc14a.rs:5) | `pub(crate)` | pure |
| LLR → CRC decode | `bpdecode174_91(&[f32;N],&[i8;N],i32) -> Option<BpDecodeResult>` | [bpdecode174_91.rs:18](src/decode/lib_jtdx/ft8v2/bpdecode174_91.rs:18) | `pub(crate)` | pure |
| LLR → OSD decode | `osd174_91(&[f32;N],&[i8;N],usize) -> Option<BpDecodeResult>` | [osd174_91.rs:9](src/decode/lib_jtdx/ft8v2/osd174_91.rs:9) | `pub(crate)` | pure |
| Fixture waveform | `gen_ft8wave(&[i32;79],f64) -> (Vec<f64>,Vec<f64>)` | [gen_ft8wave.rs:11](src/decode/lib_jtdx/gen_ft8wave.rs:11) | `pub(crate)` | pure (tests) |

These are directly callable from `dx/` today. `BpDecodeResult` carries the decoded
91-bit word + hard-error count; CRC is already checked inside `bpdecode174_91`
([:69](src/decode/lib_jtdx/ft8v2/bpdecode174_91.rs:69) via `chkcrc14a`).

### 4.2 The symbol front-end (needs one additive wrapper = D1)

The soft-symbol field is produced inside `ft8b` by a three-step chain, all in
[ft8b/mod.rs](src/decode/lib_jtdx/ft8b/mod.rs); none is currently reachable from
`dx/`:

1. `ft8_downsample(ws, dd8, newdat1, freq, …)` → baseband `cd0` (`ComplexC`).
   [ft8_downsample.rs:95](src/decode/lib_jtdx/ft8_downsample.rs:95), `pub(crate)`.
   **Reads `dd8`, writes only its own workspace cache — does not mutate `dd8`.**
2. `refine_qso_sync(cd0, candidate, iqso, xdt0, ctx) -> QsoRefinementState{ cd0,
   ibest, refined_freq, refined_dt }` — [ft8b/mod.rs:224](src/decode/lib_jtdx/ft8b/mod.rs:224),
   **private, pure**. **Note (verified, critical):** this is a *narrow* refinement,
   **not** a full-window dt search — it scans only `idt ∈ (i0±8)` around
   `i0 = nint((xdt0+0.5)·FS2)`, i.e. **±8 samples = ±0.04 s** (`FS2 = 12000/60 = 200`
   Hz, [ft8b/mod.rs:234](src/decode/lib_jtdx/ft8b/mod.rs:234)). So **`xdt0` must
   already be a good coarse dt** (within ~±0.04 s); the kernel gets that coarse dt
   from `sync8`'s full-window search. The dx engine must supply it too (§D1 seed).
3. `extract_symbol_metrics(cd0, ibest, config, ctx) -> SymbolMetrics{ s8, cs, csr,
   cscs, s256, syncavemax, nsync, … }` — [ft8b/mod.rs:288](src/decode/lib_jtdx/ft8b/mod.rs:288),
   **private, pure**. **Note (verified):** `s8[tone][k]` is the tone **magnitude**
   `sqrt(re²+im²)` ([ft8b/mod.rs:417](src/decode/lib_jtdx/ft8b/mod.rs:417)), i.e.
   `|cs|`, *not* power `|cs|²`. `cs`/`csr`/`cscs` are the complex per-symbol
   amplitudes the bit-metric builder needs (§4.3).

**D1 — DECIDED (D1a), NOT YET IMPLEMENTED.** `DxSymbolField` / `dx_symbol_field`
do not exist in the tree yet — this is the selected approach, not landed code. The
plan is: add ONE additive `pub(crate)` wrapper in `ft8b/mod.rs` that composes the
three existing private steps and returns a small dx-facing struct. Nothing existing
is modified; the three steps keep their current bodies.

```rust
// ADDITIVE, in src/decode/lib_jtdx/ft8b/mod.rs — composes existing private fns,
// never subtracts, read-only on the audio it is given. Returns the single-slot
// channel LLR (computed in-module via the full SymbolMetrics) plus s8 for the
// cheap prefilter — NOT the raw cs (DxSymbolField{s8,cs} could not feed
// build_bit_metrics, which also needs csr/cscs/s256: see §4.3 F2).
// A field is extracted only from a COARSE (freq, dt) seed — never from freq alone.
pub(crate) struct DxSymbolSeed {
    pub freq: f32,
    pub xdt0: f32,   // coarse dt; refine_qso_sync only refines ±0.04 s around it (§4.2 note)
}

pub(crate) struct DxSymbolField {
    pub s8:  [[f32; 79]; 8],   // tone MAGNITUDE matrix — prefilter / sanity only
    pub llr: [f32; 174],       // single-slot CHANNEL LLR (no AP injection), the detector input
    pub ibest: isize,
    pub refined_freq: f64,
    pub refined_dt:   f64,
    pub syncavemax: f32,       // sync-quality terms the T1 detector needs (from SymbolMetrics)
    pub nsync: usize,
}

pub(crate) fn dx_symbol_field(
    dd8: &mut [f32],               // dx-OWNED copy of the slot residual
    ws:  &mut Ft8bWorkspace,       // dx-OWNED fresh workspace
    config: &StreamDecodeConfig,
    seed: DxSymbolSeed,            // COARSE (freq, dt) — see acquisition below
) -> Option<DxSymbolField> {       // None if sync quality is too low to trust
    // 1) ft8_downsample(ws, dd8, true, seed.freq, …)   -> cd0     (read-only on dd8)
    // 2) refine_qso_sync(cd0, SyncCandidate{freq:seed.freq,dt:seed.xdt0}, 1, seed.xdt0, regular_ctx) -> st
    // 3) extract_symbol_metrics(st.cd0, st.ibest, config, regular_ctx) -> m   (full SymbolMetrics)
    // 4) bmet = build_bit_metrics(&m, MetricSource::Cs)         // v1: ONE source (Cs)
    //    for i in 0..174 { llr[i] = 2.83 * bmet.bmeta[i] }      // scalefac=2.83, NO AP
    // 5) Some({ s8: m.s8, llr, m.syncavemax, m.nsync, st.* }).  NEVER call subtractft8.
}
```

**Seed acquisition (where the coarse `(freq, dt)` comes from — P1 fix).** `freq`
alone is not enough: `refine_qso_sync` only refines ±0.04 s, so a real signal at
dt = +0.5 s is missed if seeded at 0. The coarse dt is obtained, in **v1** priority:
1. **preferred (v1, zero new export)** — the **harvested target dt** already in
   `TargetContextStore` (`self.dt`, set from a target-as-sender row). This needs no
   new kernel/session API, so it keeps the net edit at the one D1 wrapper.
2. **acquisition** — when no harvested dt exists, run **`sync8` at the focus reusing
   the JTDX profile's existing sync path and config** (same `syncmin`/depth/SWL
   window/`imetric`/band/pruning — **not** a hand-rolled simplified search, or it
   produces non-aligned candidates) to get 0..N coarse `(freq, dt)`, one field each.
3. **later optimization** — expose the focused worker's `SyncCandidate(freq, dt)` as
   a **read-only candidate export/callback** (additive, no change to candidate
   generation or decode output) and reuse its `dt` directly. This is a *second*
   additive edit, so it is **deferred** unless 1+2 prove insufficient — v1 does not
   depend on it.
If no seed is available, **T1 does not run** (no candidate ⇒ no matched filter).
`xdt0 = 0` is valid **only** for synthetic fixtures generated centered.

Implementation notes: (a) the wrapper builds a minimal `SyncCandidate{freq,
dt:xdt0}` and a regular single-pass `Ft8bCandidateContext` (`ipass=1, npass=1,
lsubtract=false, lhighsens` per config, **no AP** — the channel LLR must be
AP-free so the prior is not double-counted in the matched filter). (b) **v1 pins
ONE LLR source: `MetricSource::Cs`, scaled `llr = 2.83 * bmeta`** — `2.83` is the
exact `scalefac` from `regular.rs:94` (`llrz = 2.83 * llr_source`). This is a
*baseline single-source channel LLR*, **not** the full JTDX regular decode, which
selects among several sources per subpass (`Cs`/`Csr`/`CscsCsrPower`/… via
`regular_llr_source`). A multi-source ensemble is a later extension once T1/T2 are
proven. (c) **`MetricSource`, `SymbolMetrics`, and the other ft8b internals stay
internal — they are NOT exposed to `dx/`.** The wrapper uses them in-module and
returns only the plain `DxSymbolField` (`s8` + `llr`). `dx_symbol_field` is the
**single exit** from the kernel for the deep engine, so no further internal types
leak out. Pure plumbing — no decode-math change.

### 4.3 The engine works in LLR space (revised after the source audit)

Both T1 and T2 operate on the **per-slot channel LLR**, computed once per slot by
the D1 wrapper. This is cleaner than working on `s8` and resolves three audit
findings (F1 magnitude≠power, F2 `s8+cs` is insufficient, F4 redundancy):

- The kernel's LLR builder `build_bit_metrics(&SymbolMetrics, source)`
  ([decode_helpers.rs:16](src/decode/lib_jtdx/ft8b/decode_helpers.rs:16)) calls
  `cabs1/cabs2/cabs3` over the **complex** `cs`/`csr`/`cscs` (verified
  [decode_helpers.rs:28+](src/decode/lib_jtdx/ft8b/decode_helpers.rs:28)) — it needs
  the full `SymbolMetrics`, which is exactly why the D1 wrapper runs it *in-module*
  (where the full metrics exist) and returns the resulting `llr[174]`.
  `build_bit_metrics` is already `pub(super)` in `ft8b::decode_helpers`, so the
  wrapper — which lives in `ft8b/mod.rs` (the parent) — can call it **with no
  visibility change at all**.
- **T1 detector = LLR-domain matched filter, NOT a re-decode.** For each hypothesis,
  `encode174_91(msg) -> codeword[174]`; statistic `= Σ_i (2·codeword_i − 1)·llr_i`
  (the log-likelihood of that known codeword given the LLRs). The sign follows the
  kernel convention **`LLR > 0 ⟹ bit = 1`** (`bpdecode174_91.rs:53`/`:72`,
  `osd174_91.rs:63`/`:237`) — it is exactly the kernel's own per-bit consistency
  term. This is ML *over the known set*. Running `osd174_91` on a single slot's LLR
  would be **redundant** — the kernel already did that at `ndeep=5`; T1's value is
  evaluating the *known* codewords the blind decoder may have missed.
- **T2 combiner = sum the per-slot LLRs**, not the symbol field. For a repeated
  identical message the codeword bits are the same, so summing per-slot channel
  LLRs is optimal repetition soft-combining: `llr_sum = Σ_slots llr`. Decode the
  sum with the same matched filter **and** `osd174_91(llr_sum)` (CRC-guarded). This
  reuses `build_bit_metrics` per slot — **no `dx_llr_from_s8` and no summing of
  `s8`/power is needed.** Summing *real* LLRs is a valid non-coherent combiner and
  does **not** contradict rejecting the coherent sum of complex `cs` (§2b-C).

### 4.4 Does this touch the original decoders? — No (the C1 proof)

1. **Existing functions are unmodified, and no existing visibility changes.** D1
   adds *only* a new `pub(crate)` wrapper + struct in `ft8b/mod.rs`. It calls
   existing private/`pub(super)` items (`ft8_downsample`, `refine_qso_sync`,
   `extract_symbol_metrics`, `build_bit_metrics`) that are *already* reachable from
   `ft8b/mod.rs` — none needs to be re-exported. No existing function body, call
   order, AP schedule, or threshold changes. Precedent: `ihashcall`, `genft8`,
   `chkcrc14a` were exposed additively with unchanged decode output.
2. **The extraction chain is read-only on the audio.** `ft8_downsample` reads `dd8`
   and writes only its workspace cache; `refine_qso_sync`/`extract_symbol_metrics`/
   `build_bit_metrics` are pure. `subtractft8` — the *only* function that mutates
   `dd8` — is called in `ft8b` *after* a decode ([ft8b/mod.rs:102](src/decode/lib_jtdx/ft8b/mod.rs:102))
   and is **never** called by `dx_symbol_field`.
3. **The engine runs on dx-owned buffers.** `dx_symbol_field` takes a dx-owned
   `dd8` copy and a dx-owned `Ft8bWorkspace`, fully separate from the listen/focused
   kernel sessions, so even the workspace-cache mutation cannot reach a kernel
   session.
4. **Re-validation is part of D1.** After the additive exposure, the baseline
   **outputs** must stay byte-for-byte green (`wsjtx` 21/21 & 424/424, `jtdx`
   20/20 & 430/431, `hybrid`, dx gates, `wsjtx_source_audit`) — the existing paths
   compile and run with unchanged decode output.

**Net kernel edit for the whole plan: exactly one additive item** — a `pub(crate)`
`DxSymbolField` struct + `dx_symbol_field` wrapper in `ft8b/mod.rs` that returns the
per-slot channel LLR + `s8`. **Zero existing-visibility changes** (everything it
calls is already reachable from `ft8b/mod.rs`). The matched filter and the LLR
summation are entirely **dx-local** — no further kernel edit (the audit removed the
earlier `dx_llr_from_s8` idea by working in LLR space).

## 5. Cross-slot physics note (scopes T2 honestly)

Within one transmission the per-symbol amplitudes `cs` are phase-coherent; **across
slots there is no shared phase reference** (independent 15 s transmissions, clock
drift). So cross-slot combining must be **non-coherent**, and it is done in **LLR
space**: align freq/dt per slot, compute each slot's channel LLR (D1 wrapper, which
already does the within-slot coherent multi-symbol processing), then **sum the
per-slot LLRs** for a repeated identical message (optimal repetition soft-combining).
We do **not** sum `cs` (no cross-slot phase, §2b-C) and we do **not** sum `s8` — note
`s8` is a *magnitude* (`|cs|`, [ft8b/mod.rs:417](src/decode/lib_jtdx/ft8b/mod.rs:417)),
not power, and is used only for the cheap prefilter. Because FT8's per-slot symbol
detection is itself non-coherent, the gain is bounded by the non-coherent
integration loss — expect **~1.5–2.5 dB per doubling** at the SNRs of interest, not
the 3 dB/doubling of true coherent combining.

---

## T0 — QSO-state AP tuning (config-only, no kernel edit)

**Verified mechanism.** Source check of how the JTDX kernel uses QSO state:

- The AP-type *set* is chosen by `ap_types_for_config`
  ([ft8apset.rs](src/decode/lib_jtdx/ft8apset.rs)) keyed on **call-standardness**
  (`lhound` / mycall-std / hiscall-std → `NHAPTYPES` / `NAPTYPES` / `NDXNSAPTYPES`
  / `NMYCNSAPTYPES`), **not** on `nQSOProgress`. So `nQSOProgress` does *not* change
  which AP messages get built.
- But `config.nQSOProgress` *does* gate **which `iaptype` is tried per candidate**,
  in `regular.rs` ([:565](src/decode/lib_jtdx/ft8b/regular.rs:565),
  [:582](src/decode/lib_jtdx/ft8b/regular.rs:582),
  [:603](src/decode/lib_jtdx/ft8b/regular.rs:603), incl. the FH-specific
  `nfoxspecrpt`/`nmic` pruning). So feeding the *right* progress unlocks/prunes the
  right AP hypotheses for the current QSO state.

**So T0 is real but config-only:** drive `nQSOProgress` from the live, harvested QSO
state instead of a static `--qso-progress`. It cannot add a hypothesis the kernel
AP lacks — that is T1's job. Expected gain is modest (the focused worker already
passes mycall/hiscall/hisgrid, so the AP *set* is already built); the win is trying
the correct progress-gated subset.

**Steps.**
1. **Diagnostic first (do not hardcode a guessed mapping).** A verbal
   `message → progress` table risks diverging from the actual `regular.rs`
   `iaptype` gating. Step 1 is a measurement: instrument a run to log, per decode,
   `(last-target-message, inferred-progress, the iaptypes regular.rs actually
   gated in/out at that progress)`; from that table derive the mapping that lines
   up with the kernel's real `nQSOProgress` cases ([:565](src/decode/lib_jtdx/ft8b/regular.rs:565),
   [:582](src/decode/lib_jtdx/ft8b/regular.rs:582), [:603](src/decode/lib_jtdx/ft8b/regular.rs:603)).
2. `TargetContextStore`: track the inferred `nQSOProgress` (and last harvested
   target message) using that measured mapping — cross-slot, deterministic.
3. `dx_focus_config`: set `nQSOProgress` from the tracked value (fall back to
   `--qso-progress` when nothing is inferred yet).
4. Keep it on the target TX parity only (D4).

**Acceptance.** Long fixture: no regression (≤ current baselines); any extra
recovery is bonus. Unit test: the progress inference is deterministic and matches
the `regular.rs` gating cases. **No kernel change** (config plumbing only — T0
touches neither §4.2 nor §4.3).

---

## T1 — dx constrained deep-search (matched filter)

**Idea.** For a focused target slot the plausible messages collapse to ~dozens:
`CQ HISCALL [GRID]`, and target-as-sender-to-us `MYCALL HISCALL {-NN | R-NN |
RR73 | 73 | GRID}` (CALL2 = sender = `HISCALL`). Evaluate each *known* codeword
against the slot's channel LLR (an ML test over the known set — not a blind
re-decode, which the kernel already did), pick the best, gate hard.

**New file:** `src/decode/dx/deepsearch.rs` (dx-local; reuses §4.1 + the D1 wrapper).

**Interface (dx-local).**
```rust
// Both derived once from genft8(msg) -> (bits, itone): itone[79] for the cheap
// prefilter, codeword[174] = encode174_91(bits) for the LLR matched filter.
struct Hypothesis { msg: String, itone: [i32; 79], codeword: [u8; 174] }

enum DeepConfidence {
    // The two forms below are the "matched-corroborated" class (both emit, NOT CrcConfirmed):
    TwoSlotMatched,    // same normalized msg passes the SINGLE-slot gate in >=2 slots
    StackedLlrMatched, // T2 stack depth>=2: summed-LLR passes the STACK gate; admitted
                       //   slots passed PHYSICAL admission (freq/dt anchor + sync floor),
                       //   NOT a margin gate (margin is tracked only — §T2 two-level)
    CrcConfirmed,      // CRC-valid (OSD) + passed target gates — highest confidence
}
struct DeepHit { msg: String, stat: f32, freq: f64, dt: f64, conf: DeepConfidence }

// LLR-domain matched filter: log-likelihood of a KNOWN codeword given the
// channel LLR. Sign matches the kernel convention LLR>0 => bit=1
// (bpdecode174_91.rs:53 `zn>0=>1`, and its own consistency metric is
// `(2*cw-1)*llr` at :72; osd174_91.rs:63/237 `llr>=0=>1`). So bit=1 rewards
// POSITIVE llr. (No CRC check — the codeword is valid by construction; the
// detection THRESHOLD is the guard, §3.)
fn matched_filter(llr: &[f32; 174], codeword: &[u8; 174]) -> f32 {
    (0..174).map(|i| (2.0 * codeword[i] as f32 - 1.0) * llr[i]).sum()
}

// Cheap prefilter to rank before the full matched filter: per-symbol magnitude
// ratio at the hypothesis tone over the 58 DATA symbols (Costas excluded — common
// to all hypotheses). s8 is a MAGNITUDE, so this is a coarse ranker only.
fn prefilter(field: &DxSymbolField, itone: &[i32; 79]) -> f32 { /* Σ s8[itone[k]][k]/Σ_t s8[t][k] */ }

fn dx_deep_search(field: &DxSymbolField, hyps: &[Hypothesis]) -> Option<DeepHit>;
```

**Detector.** The prefilter (magnitude ratio on `s8`) only *ranks* — on its own it
is inflated by strong QRM, tone leakage, and colored noise. The emit decision uses
`matched_filter` on `field.llr` (the kernel's own per-slot channel LLR from the D1
wrapper) plus **v1 sanity terms that come only from fields already in
`DxSymbolField`**: **winning margin** over the runner-up hypothesis, **Costas/sync**
consistency (`syncavemax`/`nsync`), and **freq/dt** consistency vs the harvested
target. **Noise-floor / LLR-norm normalization is deferred** — it has no defined
source field in v1; if measurements show it is needed, add `llr_norm` (or
`noise_floor`) to `DxSymbolField` sourced from the matching JTDX path, not an
ad-hoc invented metric. v1 gates do **not** depend on it.

**Hypothesis set — what the matched filter can and cannot cover (scope, important).**
The matched filter needs **enumerable** codewords, i.e. messages **fully determined
by `{mycall, hiscall, hisgrid, nQSOProgress}`**. That is exactly the **"DX talks to
me" / "DX CQs"** cases. The **"DX is working someone else"** case
(`OTHER HISCALL …`) has an **unknown** peer `OTHER` — an unbounded callsign space —
so it is **NOT enumerable and the matched filter cannot cover it**. This is a real
scope limit, not an oversight: a bounded constrained search by definition cannot
test an unknown callsign.

**How the broader goal is still served:** the **T2 OSD-on-summed-LLR path is a
*blind* decode + hiscall target filter**, so it recovers **any repeated
hiscall-containing message — including `OTHER HISCALL …`** (same `OTHER`, repeated
across slots) — even though the matched filter can't. So:
- *DX→me / DX-CQ* → matched filter (fast, single/few slots) **and** T2 OSD.
- *DX→others (repeated)* → **T2 OSD path only** (blind + target-filtered).
- *DX→others, non-repeating, below the kernel threshold* → **not recoverable** (no
  enumerable hypothesis, no repeat to stack; the kernel's blind single-slot OSD is
  already maxed). Harvested via the listen when decodable. This is the honest edge.

**v1 matched-filter set — minimal closed loop (C3-1).** Keep `M` small (larger ⇒
worse `Pfa`, more `pack77` risk). **v1, target-as-sender to me / CQ only**, from
`{mycall, hiscall, hisgrid, nQSOProgress}`:
- `CQ HISCALL HISGRID` **only when `hisgrid` is known**; otherwise `CQ HISCALL`
  (a no-grid CQ also packs as a valid standard message).
- `MYCALL HISCALL <rpt>` for `<rpt>` ∈ `{ -NN, R-NN }` over a bounded SNR range
  (e.g. −24..+0 dB), plus `RR73` and `73`.

That is the full QSO ladder for one standard pair (`M` a few dozen; `nQSOProgress`
from T0 narrows it). **Deferred to phase 2 (not v1):** compound/hashed forms
`MYCALL <HISCALL> …`, grid-in-reply variants, and other special message types —
they enlarge `M` and need extra `pack77`/hash handling, so they land only after v1
is proven and the `Pfa` budget holds. **Every generated message is round-tripped:
`genft8(msg)` then `unpack77` of its bits, and the *normalized* unpacked message
must equal the intended one** — else the hypothesis is dropped (this catches a
string that packs into a *different* message type, not just a `None`/pack failure).
When phase 2 adds compound/hashed forms, this round-trip **must use the DX-local
`{mycall, hiscall}` `HashCallBook`/unpack context** (the same one the runtime
resolves with), **not** any hybrid/global shared state — otherwise test-time and
run-time hash resolution diverge. Codewords are cached, rebuilt only when inputs
change (deterministic).

**Steps.**
1. **Seed acquisition (required first).** Turn the focus frequency into 0..N coarse
   `DxSymbolSeed{freq, dt}` per the §4.2 priority (harvested dt → focused-worker
   `SyncCandidate` → `sync8` at the focus). **No seed ⇒ T1 does not run** for that
   focus. For each seed, `dx_symbol_field(seed)`; a `None` (sync too weak) is
   skipped.
2. Hypothesis generator: for each message, cache `itone` + `codeword` via
   `genft8` / `encode174_91` (rebuilt only when inputs change; deterministic).
3. `prefilter()` (uses `itone`) to rank → top-K hypotheses **per extracted field**.
4. For the top-K, `matched_filter(field.llr, codeword)` + sanity terms → the
   detection statistic; **threshold** calibrated to its noise distribution
   (Bonferroni over `M`, §3-2) + **temporal-consistency gate** (§3-5) before emit;
   below threshold → `None`.
5. Wire as a third engine inside `decode_one_focus` (concurrent per-focus path).
   **Run condition (corrected — do NOT reuse the "target within 3 Hz" skip):** in
   FH/multi-stream traffic a target row already present near the focus does **not**
   mean T1's hypothesis has been decoded — the same FH lesson that made `a8d` run
   unconditionally ([dx/mod.rs:107](src/decode/dx/mod.rs:107)). So **T1 runs
   whenever the focus exists and the engine is enabled**. **`dx_symbol_field` +
   scoring + T2 accumulation ALWAYS run** (the field/LLR/score feed T2's stack,
   threshold calibration, and stack confirmation). The "already emitted/confirmed"
   check **only suppresses a duplicate *emit attempt*** for that exact normalized
   message — it never skips extraction, scoring, or accumulation, so T2 is never
   starved of an observation. Sensitivity-first.

**Acceptance.** (Single-slot T1 is a *detector*, not an emitter — per §3-5/D2 a
CRC-less single-slot hit is never emitted.)
- Detection: on a synthesized slot ~1 dB *below* the kernel OSD threshold (where the
  kernel misses), the **correct hypothesis ranks #1** and clears the internal
  statistic/margin gate — reported as **soft-annotate only**, not emitted. Actual
  *emit* recovery is validated by T2 / a two-slot-corroboration fixture (§7), not
  here.
- False alarm: over a large pure-noise + wrong-call fixture set, the #1 hypothesis
  stays below the gate ⇒ **0 fabricated `hiscall`** (measured, §7).
- Determinism + no kernel change; clippy clean.

**Risk.** No CRC backstop (§3-3). Mitigation = conservative threshold + temporal
consistency; ship T1 initially as *corroboration/soft-annotate* and only promote
to standalone emit after the false-alarm fixture is green.

---

## T2 — cross-slot non-coherent integration

**Idea.** When the target repeats the same message (it didn't hear us — the
dominant weak-chase case), stack the slots **in LLR space**. **Do not key the stack
on the message hypothesis** — that is circular, since at low SNR the message is the
unknown. Each slot's channel `llr[174]` is hypothesis-agnostic (it is just the soft
bits at that freq), so a stack is keyed on **`(parity, freq_bin)` only** and
accumulates the aligned per-slot `llr`; the hypothesis is discriminated **at decode
time** (T1 matched filter / OSD) against the *summed* LLR.

**Two-level admission (critical — do NOT gate accumulation on hypothesis margin).**
T2's whole point is that *single slots are too weak to be confident*; if a slot had
to clear a hypothesis-margin gate to be added, the weak slots that most need stacking
would be rejected and **depth would never grow** (Risk a). So:
- **Physical admission = the accumulation gate.** A slot is added to the
  `(parity,freq_bin)` stack when its **`refined_freq`/`refined_dt` are within the
  stack anchor tolerance and its sync quality (`syncavemax`/`nsync`) clears a *low*
  floor** — i.e. "is there plausibly signal at this freq/dt", *not* "is the message
  already confident". This is what lets sub-threshold slots accumulate.
- **Hypothesis margin = soft, never a per-slot hard reject.** The per-slot best-hyp
  only *updates* the stack's running `best_hyp` confidence; a single low-margin or
  flipped slot does **not** evict it. Only a **sustained, confident** best-hyp flip
  (e.g. the new best wins by a margin over ≥2 consecutive slots) triggers a
  reset/branch — guarding against a genuine message change without starving depth.
A **top-2/3 branch** (parallel tentative stacks) is the recommended hardening if even
the sustained-flip rule proves too brittle (promoted from "later" toward v1).

**New file:** `src/decode/dx/stack.rs`; stacks owned by `TargetContextStore`.

**Interface (dx-local).**
```rust
struct StackKey { parity: usize, freq_bin: i32 }   // NOT keyed on the message (§Idea)
struct SlotStack {
    key: StackKey,
    sum_llr: [f32; 174],       // Σ of aligned per-slot channel LLRs (repetition combining, §5)
    n: u32,                    // depth, capped at D3 (=8)
    last_nutc: u32,            // for the 16-slot age window (D3)
    best_hyp: HypId,           // TRACKED only (not an accumulation gate); sustained
    flip_run: u8,              //   confident flip over >=2 slots => reset/branch (Risk a)
    // Accumulation is gated by PHYSICAL admission (freq/dt anchor + sync floor), NOT
    // by best-hyp margin — else weak slots never stack and depth never grows. A
    // top-2/3 parallel-stack branch is promoted toward v1 if sustained-flip is brittle.
}
impl SlotStack {
    fn accumulate(&mut self, field: &DxSymbolField);   // sum_llr += field.llr (after physical admission)
    fn decode(&self) -> Option<DeepHit>;               // see below
}
```

`decode()` runs **two independent decoders** on the **summed** LLR. They are
*not* chained — the OSD path must **not** be gated on the matched filter, or the
`OTHER HISCALL …` (DX→others) case could never reach the deepest decode (it has no
matched-filter hypothesis):

- **(A) Matched filter** (`matched_filter(sum_llr, codeword)` over the enumerable
  hypothesis set) — fast path for **DX→me / DX-CQ**; CRC-less, gated by the §3
  threshold + temporal consistency. Covers only known codewords.
- **(B) Blind stack OSD** — the general path that **recovers any repeated
  hiscall-containing message, including `OTHER HISCALL …`**. Its trigger is a
  **signal-presence / cost gate, NOT the matched filter**: run when **stack
  depth ≥2 AND physical admission held (freq/dt anchor + sync floor) AND the cost
  budget allows**. Then `bpdecode174_91(sum_llr, &[0i8; 174], 30)`; if it fails,
  **`osd174_91(sum_llr, &[0i8; 174], 5)`** (**v1 pins `ndeep = 5`** — max depth as a
  CRC *guard* on the already-stronger summed LLR, not a threshold lowering or kernel
  change; the depth≥2 + sync-floor gate, not the matched filter, is what keeps it
  off pure noise — record its per-call cost). **All-zero `apmask` — no AP** (the
  stack LLR is the AP-free channel LLR, §4.2). `osd174_91`/`bpdecode174_91` are
  **blind** — a CRC-valid result can be some *other* legal FT8 message, so it is
  high-confidence **only after** `unpack77` + the dx target gates (hiscall filter
  C5, grid-contradiction, freq/dt consistency). A CRC-valid **non-target** message
  is **discarded / diagnostic, never emitted**; a target-passing one is
  `CrcConfirmed` and skips the temporal gate (§3-4).

(A) and (B) are complementary: (A) is the cheap known-target detector; (B) is the
deep blind recovery that makes "DX→others repeated → T2" actually true.

No `dx_llr_from_s8` and no `s8` summation: the per-slot LLR already came from
`build_bit_metrics` (full coherent within-slot processing); summing real LLRs is the
right non-coherent cross-slot combiner (§5, §2b-C).

**Scope honesty (P1-233).** This CRC-OSD runs on the **DX engine's chosen single-source
channel LLR** (§4.2: `Cs`, `×2.83`). It is soft-combining + CRC on *that* LLR — it does
**not** claim to reproduce JTDX's full regular decode, which runs OSD over several
per-subpass LLR sources. It is a faithful CRC check of the dx-stacked LLR, not a
multi-subpass equivalent; a multi-source LLR stack is a later extension.

**Steps.**
1. Stack store in `TargetContextStore` (cross-slot, deterministic; depth ≤ 8 and
   16-slot age window per D3). **Accumulation is gated by PHYSICAL admission only**
   (freq/dt within anchor + low sync floor, step 2) so sub-threshold slots can stack;
   the best-hyp is tracked but **does not hard-reject** a slot — only a *sustained
   confident* flip resets/branches (§Idea two-level admission, Risk a).
2. Per slot on the target parity: `dx_symbol_field` at the harvested freq/dt. The
   per-slot sync (`refine_qso_sync` inside the wrapper) already centres freq/dt, so
   the returned `llr[174]` is over fixed bit positions and is summed **directly** —
   no post-hoc LLR alignment. **Physical admission gate (before accumulating):** the
   slot's `refined_freq`/`refined_dt` must be **within the stack anchor tolerance**
   (the anchor = the freq/dt the stack was opened at); a slot whose refinement drifted
   onto an adjacent signal or QRM is **skipped or opens a new stack — never added**,
   so a mis-synced LLR cannot pollute `sum_llr`. This freq/dt gate is more physical
   than the best-hyp margin (Risk a) and complements it (Risk b — measure the loss).
3. **Two-phase per slot (resolves the ordering ambiguity).** On slot N: (i) extract
   this slot's `field`; (ii) **decide** on `prior_stack.sum_llr + field.llr` — decode
   the *already-committed* stack LLR combined with the current observation, so the
   current slot contributes to its own recovery and depth is N+1; (iii) **commit**
   `field.llr` into `sum_llr` for future slots. This is **not** same-slot replay:
   nothing re-runs the kernel decode on slot N, and the committed stack used in step
   (ii) is built only from prior slots — so file mode stays reproducible (C4). Emit
   under the §3 gates.
4. Honest scoping per §5: non-coherent only; `log()` the achieved depth and the
   estimated gain (no silent capping).

**Acceptance.**
- Recovery trend: a message synthesized *below single-slot threshold* and repeated
  N slots is **not** recovered at N=1 but **is** by some N≤8, and the recovery
  SNR improves monotonically with depth (measured, matches the ~non-coherent
  trend within tolerance).
- False alarm: stacking pure-noise / wrong-call slots yields **0 fabricated
  `hiscall`** (measured).
- Determinism: file-mode output identical across repeated runs.

**Risks.** (a) **Message change mid-stack corrupts the sum** (summing LLRs of two
different messages pulls the differing bits toward 0) — **but the fix must NOT be a
per-slot hypothesis-margin gate**, because at the low SNR that motivates T2 the
per-slot best-hyp is *itself unstable*, and hard-rejecting low-margin slots would
reject exactly the weak slots that need stacking, so **depth would never grow** —
T2's gain would self-defeat. Correct mitigation (per the two-level admission above):
**physical admission (freq/dt anchor + low sync floor) decides accumulation**; the
best-hyp is *tracked, not gating*; only a **sustained confident flip** (new best wins
by a margin over ≥2 consecutive slots) resets/branches. If even that is too brittle,
promote the **top-2/3 parallel candidate stacks** to v1 (so a real message change
grows a sibling stack instead of stalling the right one). A full per-hypothesis
router stays a later fallback. (b) freq/dt mis-alignment loses gain → measure
alignment sensitivity. (c) noise-stacking false alarms → the §3 gates + measured
budget.

---

## 6. Sequencing & Definition of Done

1. **D1 (decided, not implemented)** — add `DxSymbolField` + `dx_symbol_field` in
   `ft8b/mod.rs` (§4.2, additive); re-validate baselines (§4.4). Unblocks T1/T2.
2. **T0** — diagnostic-first mapping, then config-only (`TargetContextStore`
   progress inference + `dx_focus_config`); long-fixture no-regression. No kernel edit.
3. **T1** — `src/decode/dx/deepsearch.rs` (LLR matched filter over the hypothesis
   set; no kernel edit beyond D1's wrapper); wire into `decode_one_focus` with the
   corrected run condition; ship corroboration-first per D2; false-alarm fixture
   green before standalone emit.
4. **T2** — `src/decode/dx/stack.rs` (sum per-slot LLRs; matched filter + OSD on the
   sum). **No new kernel edit** — the audit removed `dx_llr_from_s8` by stacking in
   LLR space. Built on T1; synthetic depth-trend + false-alarm fixtures green.

### PLAN completion condition (the single gate)

This PLAN is **DONE** when **all** of the following are objectively true and
recorded. Each is checkable; none is "looks fine".

**G1 — Goal met (the headline capability).** On the T2 depth-trend fixture, `dx`
**emits** a *repeated* weak target that `--swl --nagain` (the current dx ceiling)
**misses**, and the recovery turns on with stack depth (the per-doubling trend is
measured and logged). This is the proof that the engine adds decode capability.

**G2 — Safety met (no fabricated `hiscall`).** Over the sized false-alarm corpus
(§3-6/§7: ≥24 h noise + ≥1000 wrong-call + ≥2 h real on-band + ≥50 collision/near-call),
the **measured** fabricated-`hiscall` count is **0**, recorded as
`0 / corpus-size / 95% Pfa upper bound` (not claimed as a true `Pfa = 0`, §3-6).

**G3 — Originals untouched (the principle).** Pure-profile baseline **outputs are
byte-for-byte unchanged** (`wsjtx` 21/21 & 424/424, `jtdx` 20/20 & 430/431,
`hybrid`, existing dx gates); `wsjtx_source_audit` unchanged **except for the one
additive wrapper** (§4.4); `lib_wsjtx`/`lib_jtdx` decode behaviour identical. (Not
"binary-identical" — the wrapper changes the binary; the guarantee is *unchanged
decode output + behaviour*.)

**G4 — Per-tier acceptance green.** D1, T0, T1, T2 each pass their own Acceptance
(above); the fast-CI fixtures and the manual/`#[ignore]` heavy gates have all been
run and recorded.

**G5 — Buildable + clean.** Everything builds; `clippy` is clean in `dx/` and the
one kernel wrapper; file-mode output is reproducible across repeated runs.

If any of G1–G5 fails, the PLAN is **not** done — partial tiers may ship behind the
flag (corroboration-first, D2) but the milestone is only complete at G1∧G2∧G3∧G4∧G5.

*Scope honesty (does the goal generalise?):* the capability G1 proves is for the
**repeating-target** case (Fox re-sending the same message — the dominant weak-chase
case). A target that sends a *different* message every slot cannot be LLR-stacked;
the engine then degrades to T1 detection + 2-slot corroboration, with no T2 gain.
This is the intended, honest scope — not a gap to be "fixed".

## 7. Validation fixtures (buildable now)

Reuse the `genft8` + `gen_ft8wave` recipe already used for `dx_synth_ua3qna.wav`.
**Two tiers:** the small fixtures below run in normal CI; the false-alarm corpus is a
**manual/`#[ignore]`/release gate** (like the existing ignored dx gates).

*Fast (CI):*
- **T1 detection** (not emit) — one slot with the target message at ~1 dB below the
  measured OSD threshold (calibrate amplitude vs noise); assert the **correct
  hypothesis ranks #1 and clears the internal gate** while the kernel misses. Emit
  is *not* asserted here (single-slot CRC-less hits don't emit, §3-5).
- **Two-slot emit corroboration** — the same weak message on **two** same-parity
  slots; assert T1 emits only after the second slot confirms the same normalized
  message (§3-5).
- **T2 depth trend** — N copies (N = 1,2,4,8) of the *same* weak message on the
  same parity/freq with small per-slot freq/dt jitter; assert recovery turns on
  with depth and improves monotonically.
*Manual / `#[ignore]` / release gate (not CI):*
- **False-alarm corpus (shared, critical) — the v1 budget from §3-6.** Components:
  **≥ 24 h pure noise; ≥ 1000 wrong-call slots** (`OTHER1 OTHER2 …`, never the
  target); **≥ 2 h real on-band recordings** (real interferers / tone leakage /
  colored noise); **≥ 50 hash-collision + near-callsign cases**. Run T1 and T2;
  **measure** the fabricated-`hiscall` rate and tighten thresholds until **0** over
  this corpus. "0 on a tiny fixture" is not acceptance — this sized corpus is. Must
  be run and its numbers recorded before T1/T2 is declared done.

## 8. Cost / effort (rough)

| Tier | New code | Validation | Risk |
|---|---|---|---|
| D1 | small (wrapper composing 4 existing steps + `build_bit_metrics` + scalefac) | re-validate baselines | none (no logic change) |
| T0 | small (context + config; diagnostic first) | long fixture no-regress | low |
| T1 | medium (hyp/codeword cache + LLR matched filter + sanity gates) | recovery + false-alarm fixtures | **false-hiscall** (no CRC backstop) |
| T2 | medium (LLR stack store + sum + matched filter / OSD) | depth-trend + false-alarm fixtures | mis-group / false-alarm |

## 9. Decisions (decided — design selected, not yet implemented)

- **D1 — additive wrapper (D1a).** One `pub(crate)` wrapper in `ft8b/mod.rs`, zero
  existing-visibility changes (§4.4). *Rejected:* replicate in `dx/` (D1b) — drift
  from the kernel's exact extraction.
- **D2 — corroboration-first, then promote.** T1/T2 ship gated so a CRC-less
  matched-filter decode is harvest/soft-annotate until it is matched-corroborated —
  either `TwoSlotMatched` or `StackedLlrMatched` (v1 admission, §3-5 — no vague
  "kernel partial"); promote to standalone emit only once the false-alarm fixture
  (§7) is green.
  *Rejected:* standalone emit from day one — a single-slot matched-filter false alarm
  has no CRC backstop (§3-3) and would risk a fabricated `hiscall` before the
  threshold is calibrated.
- **D3 — stack depth ≤ 8, age window = the existing 16-slot frequency window.**
  Caps memory and staleness and matches the depth where non-coherent gain has
  flattened (~5–7 dB). *Rejected:* unbounded stacking — stale slots add noise, not
  signal, and grow memory; depth is re-tuned from the T2 depth-trend fixture if the
  data warrants.
- **D4 — parity gate by confidence, dx-private.** T1/T2 are dx-only (they depend on
  the single-target context). Parity gating is **graded by confidence**, not a hard
  strict gate, so a wrong inference cannot silently lose the target's TX slot:
  **Observed** parity → run only the target TX parity; **Unknown** → run both;
  **Inferred** (from a recipient row, not yet seen as sender) → weak gate: prefer
  the inferred TX parity but keep an occasional loose probe on the other until the
  parity is *observed*. (The existing `should_run_focused` currently treats Inferred
  as strict as Observed — this grading refines that too.) *Rejected:* a hard strict
  gate the moment any parity is inferred — mis-inference would skip the real TX slot.
