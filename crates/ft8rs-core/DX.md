# DX Target Decode

`--profile dx` listens for one known target over unchanged JTDX and WSJT-X
sessions. `hiscall` is required. Three independent observations supplement the
original DX tasks: the original JTDX listener, ordinary WSJT-X and a complementary
JTDX AGC path. `mycall` is optional; a valid full call pair unlocks additional
context recovery. Target-only listening uses ordinary search without inventing
the missing call.

Related notes: [JTDX](JTDX.md), [WSJT-X](WSJTX.md), [Hybrid](HYBRID.md),
[shared architecture](COOPERATIVE_DECODER_DESIGN.md).

## Execution

At the beginning of each slot, age the target context and freeze a snapshot.
The persistent JTDX SWL listener and disposable recovery tasks use that snapshot
concurrently. Context feedback is committed only after the slot's tasks finish,
in stable task order. Only the original listener and original recovery results
update the protected target context. Supplemental observations and cancellation
never change that state. No worker reads another worker's residual or AP memory.

Each selected focus receives a narrow JTDX `swl+nagain` task. When a grid is
available it can also receive a WSJT-X/a8d task. Up to five foci are selected and
at most four recovery executor threads run at once. The original JTDX parameter
`jtdx_threads` selects logical frequency partitions; the current Rust session
processes those bands serially and does not spawn one OS thread per partition.

With a user call pair but no focus, DX can run one standard full-band WSJT-X
context task with `nagain=false`. After two misses it rests for one slot. A known
focus skipped by parity policy is not mistaken for an unknown-frequency target.
Recovery sessions are private and seeded only with the user call pair.

The original listener is reused, not duplicated. Its AGC selection is preserved;
the supplemental JTDX session uses the opposite AGC selection. Both use DX's
existing SWL policy and preserve explicit logical partition settings. Hybrid and
DX share the observation execution boundary and AGC configuration factory in
`decode/cooperative/observation.rs`. Native mutable books/workspaces remain
independent; only the immutable slot samples are borrowed by multiple workers.

After these observations, an unresolved target-sender focus with eligible nearby
interference may receive one controlled WSJT-X cancellation recovery. At most two
unresolved foci and three waveform-verified seeds are used. An addressed target
never makes the other station's frequency a resolved target-sender focus. The
existing 0.70 support check, original subtraction and decoder thresholds are
unchanged. No additional JTDX residual deep search is launched.

Additional WSJT-X context requires a normalized, legal, distinct call pair;
otherwise its private call pair/grid are cleared for ordinary search. Explicit
empty `<>` in supplemental output is rejected; legitimate `<...>` is preserved.
The original DX output is never re-filtered by this supplemental guard.

## Target context

- Parse sender/recipient roles through the shared message parser. Frequency
  belongs to the sender; an addressed target does not own the sender's frequency.
- Keep user grid and pinned frequency separate from observations. Reconfiguration
  migrates valid observations and then reapplies new user values. Replacing or
  clearing a pin removes the old pin.
- Treat RR73 as an acknowledgement. Hard user-grid conflicts are rejected before
  harvest, so rejected rows cannot alter the state that rejected them.
- Native Regular target transmissions can establish grid and strong parity.
  Assisted recovery only supplies low-confidence frequency hints; it cannot
  establish grid, strong parity or independent identity.
- Update same-confidence frequencies with current observations. Native frequency
  and parity information expire after 16 slots, assisted hints after 4. An
  observed grid expires after 64 slots without native target activity. User
  values are not aged out by these rules.
- A parity contradiction clears the old lock and restores both-slot search;
  confirmation requires another distinct-slot observation. Inferred parity does
  not suppress focus searches.

## Fox/Hound foci

Actual user and observed foci are selected before predictions. Multistream
extrapolation requires at least two native target-sender observations in the same
slot, fitting the existing 60 Hz grid within tolerance. Pins, assisted hints and
observations from different slots cannot manufacture a multistream grid.

Foci are capped and bounded by the configured band. The strategy reuses existing
native inputs; it does not change native candidate ordering or thresholds.

## Output and budget

Only target-related rows passing the user-grid rule are emitted. A shared output
ledger deduplicates callbacks and final rows, preserving unresolved identities.
Monitor mode streams recovery rows immediately; file mode submits them in stable
task order. `ctx` marks the original isolated context recovery, `sic` marks
terminal cancellation recovery, and `?` preserves unknown native provenance
instead of claiming Regular. Supplemental worker errors are recorded separately
in `last_diagnostics()`; they do not erase successful protected results.

The monitor watchdog controls whether later tasks start. An original decoder
already running cannot be forcibly cancelled by this gate. Message arrival and
all-worker completion are separate measurements; evaluate the listening display
budget at the next slot +2/+3 seconds and check that tails leave room for the next
slot's early stage.

Additional cancellation has its own three-second real-time start budget. A
running native call is drained and its results are still emitted; the start
budget does not cancel decoders or affect the original DX watchdog. Observation
workers run alongside original work only when enough CPUs remain for every
original worker. On smaller machines they are deferred, and late optional starts
are skipped in real time. File mode keeps canonical unbudgeted search unless an
explicit watchdog was requested. Diagnostics separately record original task
completion/skips, all three observers, resolved-focus skips, recovery starts,
waveform checks, optional budget skips and errors.

## Validation and measured scope

Run local optimized tests with the private calibration database:

```sh
scripts/test-decoders.sh fast
scripts/test-decoders.sh release --include-ignored
```

The complete runner includes the a8d, long-target, synthetic reproducibility,
three-way target-preservation, known-waveform recovery and incomplete-context
checks. To run only the additional known-waveform gates:

```sh
cargo test --profile fast -p ft8rs-core --lib decode::dx::waveform_tests:: -- --ignored --test-threads=1
```

These gates generate deterministic signals through the existing encoder. The
shared native-fixture suite independently checks native-generated waveforms,
Watterson channels and degraded historical recordings through the DX adapter.
DX's target filter is part of its contract; it does not need to return unrelated
messages from the general corpus.

The [2026-10-28 report](../../docs/reports/2026-10-28-dx.md) separates prototype
experiments from implemented behavior. The measured implementation preserved
all previous target messages and seven additional known-truth synthetic
recoveries across the four controlled comparisons. Unlabelled real-recording
candidates were not treated as confirmed gains. That finite set establishes
neither an absolute false-positive rate nor a fixed dB sensitivity improvement.

A focus that succeeds after external seeding does not establish cold-start
sensitivity. Recovery rows must not promote their own hypotheses into strong
observations. Compare callback arrival, all-worker completion and next-slot
queueing separately. See [shared architecture](COOPERATIVE_DECODER_DESIGN.md),
[current validation conditions](../../docs/VALIDATION.md) and
[development checks](../../docs/DEVELOPMENT.md).
