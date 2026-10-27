# DX Target Decode

`--profile dx` listens for one known target over unchanged JTDX and WSJT-X
sessions. `hiscall` is required. `mycall` is optional and unlocks context recovery;
without it DX retains target-filtered JTDX SWL listening.

Related notes: [JTDX](JTDX.md), [WSJT-X](WSJTX.md), [Hybrid](HYBRID.md),
[shared architecture](COOPERATIVE_DECODER_DESIGN.md).

## Execution

At the beginning of each slot, age the target context and freeze a snapshot.
The persistent JTDX SWL listener and disposable recovery tasks use that snapshot
concurrently. Context feedback is committed only after the slot's tasks finish,
in stable task order. No worker reads another worker's residual or AP memory.

Each selected focus receives a narrow JTDX `swl+nagain` task. When a grid is
available it can also receive a WSJT-X/a8d task. Up to five foci are selected and
at most four recovery executor threads run at once. The original JTDX parameter
`jtdx_threads` selects logical frequency partitions; the current Rust session
processes those bands serially and does not spawn one OS thread per partition.

With a user call pair but no focus, DX can run one standard full-band WSJT-X
context task with `nagain=false`. After two misses it rests for one slot. A known
focus skipped by parity policy is not mistaken for an unknown-frequency target.
Recovery sessions are private and seeded only with the user call pair.

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
task order. `ctx` marks isolated context recovery and `?` preserves unknown
native provenance instead of claiming Regular.

The monitor watchdog controls whether later tasks start. An original decoder
already running cannot be forcibly cancelled by this gate. Message arrival and
all-worker completion are separate measurements; evaluate the listening display
budget at the next slot +2/+3 seconds and check that tails leave room for the next
slot's early stage.

## Validation

Run local optimized tests, including the existing a8d, long-target and synthetic
reproducibility gates. `./scripts/acceptance-gates.sh` places the 2022 calibration
CallDB beside a private test executable and leaves the live database unchanged.
Pure JTDX long-fixture calibration uses four partitions with the unchanged
430/431 threshold; see [JTDX.md](JTDX.md).

A focus that succeeds after external seeding does not establish cold-start
sensitivity. Recovery rows must not promote their own hypotheses into strong
observations. Unannotated outputs require separate verification, and a finite
fixture set does not establish an absolute false-positive rate.
