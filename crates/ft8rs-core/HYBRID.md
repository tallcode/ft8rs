# Hybrid Decode

`--profile hybrid` combines independent WSJT-X, JTDX/noAGC and JTDX/AGC sessions
and one bounded round of cross cancellation. These paths are orchestration over the
existing decoders. Hybrid also offers an opt-in private ft8rs specialist branch.
Hybrid does not alter the native algorithms under
`lib_wsjtx/` or `lib_jtdx/`; source-alignment corrections are reviewed separately.

Related notes: [WSJT-X](WSJTX.md), [JTDX](JTDX.md), [DX](DX.md).

## Optional specialist branch

`--profile hybrid --hybrid-experiment` enables the ft8rs single-slot receive DSP.
It is disabled by default and has no standalone profile or public decoder API.
The GUI does not yet expose an enable switch. See [specialist implementation](src/decode/experiment/README.md)
for the independent front end, soft metrics, conditional waveform admission,
single residual search and known limitations.

The specialist starts after all existing native/recovery callbacks, using the
remaining ten-second slot-work budget. Its output is appended through the
payload-aware ledger, never replacing native representatives. Tagged rows use
`Experiment` / CLI/GUI marker `exp`; passive specialist evidence cannot create
independent confirmation or seed native AP/hash memory. `--no-hybrid-sic`
continues to control the original cross-decoder cancellation, not the private
specialist's internal residual search. The specialist diagnostic is available in
`HybridSlotDiagnostics.experiment`. Neither its work budget nor an in-flight
native call guarantees a hard wall-clock completion limit.

## Baseline and ownership

Each of the three baseline sessions owns its hash book, AP/odd-even memory,
workspaces and residuals. All receive the same immutable input audio. Persistent baseline
sessions receive no cross-decoder hash imports: the original import API cannot
revoke an old entry after a later collision is found.

The baseline union is retained regardless of recovery success. Recovery uses a
private copy of the original audio and a disposable native session. It cannot
mutate the baseline decoder or feed results back into another recovery round.

## Shared components

`src/decode/cooperative/` supplies conservative message roles and normalization,
signal matching, the output ledger and cancellation-seed validation. Hybrid and
DX keep their own task selection and state policies.

The ledger separates source agreement from first emission. A repeated JTDX-only
row remains JTDX-only, while agreement with WSJT-X can add that source without
emitting a duplicate. Message text, frequency and DT identify matching signals;
unresolved hashes are not equated with resolved identities from text or a short
callsign hash alone. The ledger may deduplicate a hash-display expansion only
when both original 79-tone sequences are complete and identical, the remaining
message fields agree and frequency/DT match. Unresolved placeholders with
different complete native payloads remain distinct even if their display strings
match. Fully resolved messages retain the existing normalized-text equivalence,
including equivalent native message formats and hash-brace display variants. Conflicting resolved calls
also remain distinct despite a hash collision. Native evidence and confidence
are retained separately; payload identity does not confirm a resolved callsign
or share a hash table. If original payload is unavailable, no hash expansion is
inferred. The first emitted native presentation remains unchanged.

Passive evidence and QSO opportunities remain diagnostic. Unknown or derived
observations cannot create independent confirmation. Passive opportunities are
rebuilt per slot instead of retaining an old high-SNR hint indefinitely.

## Bounded cross cancellation

Each direction may run once per slot, with at most three seeds. A seed must have
finite, in-band frequency and DT, a recognizable sender, no unresolved/hash
identity, and the original decoder's existing tone sequence. Re-encoding only
verifies that sequence; missing tones are never reconstructed for subtraction.

The orchestration layer checks data-symbol waveform support against the input.
Correct-tone energy relative to the strongest alternative must reach 0.70. It
then calls the existing subtraction function on the private input, with the
native `DT + 0.5 s` convention. Non-finite output or an energy increase above 1%
rejects the subtraction. These gates control seed use; they do not change a
native decoder's acceptance thresholds or prove signal truth.

JTDX-to-WSJT-X seeds additionally require native Regular provenance. WSJT-X's
ordinary path does not expose its internal AP subtype, so its provenance stays
Unknown and seed admission relies on the separate waveform gate. Recovery rows
are terminal outputs and cannot become new cancellation seeds.

Cross cancellation remains between WSJT-X and JTDX/noAGC only. The AGC path
contributes output without changing peer observations or cancellation seeds.
Its tagged evidence records the `jtdx-agc` variant, but both JTDX variants count
as one decoder family; agreement between them cannot create independent quorum.

Seed selection uses messages not yet seen from the peer, including the verified
payload comparison above. It does not start additional recovery work just to
reproduce an unresolved display of an already-decoded payload. The native-tone
and waveform gates, one attempt per direction, three-seed limit and three-second
start budget are unchanged. Diagnostics report each baseline's completion time
(WSJT-X, noAGC, AGC) and starts skipped by that budget.

Cross cancellation is enabled by default. CLI `--no-hybrid-sic` and the GUI
Cross cancellation checkbox disable it while retaining all three baseline sessions.
Generic per-call-pair replay, AP callbook imports and SameParityA7 feedback are
not enabled.

## Scheduling and output

Monitor mode preserves WSJT-X's existing N41 early decode and N47 subtraction.
At N50 it runs the WSJT-X final stage and both full JTDX baselines concurrently.
An eligible direction can start when its source baseline finishes, using peer
rows already seen. No new recovery starts after three seconds of N50 processing;
active original calls finish normally.

File mode waits for the WSJT-X/noAGC baseline pair before choosing seeds, keeps
their canonical ordering and submits recovery rows in fixed direction order,
then merges the AGC output. Real-time
and file scheduling need not select identical additional opportunities on every
recording.

Real-time rows are forwarded immediately. Worker completion does not hold back
an already discovered row. GUI markers `?` and `sic` indicate unknown native
provenance and isolated cancellation recovery; N41 follows the same provenance
rules. Callback errors are returned after started workers have been drained.

The product only listens. Evaluate message arrival by the next slot +2/+3 seconds
separately from total worker completion. The start budget cannot interrupt an
active original decoder, so throughput and next-slot queueing must also be
checked under the intended machine load.

## Partition and SWL configuration

For receive-only operation that accepts occasional false-positive candidates,
the default is `jtdx_threads=1` for both Hybrid JTDX sessions and standalone
JTDX. Local replay retained previously emitted signals while
reducing processing time and CPU cost. This is an engineering choice for the
current implementation and tested recordings, not evidence that one partition
has universally higher sensitivity.

One partition still retains both WSJT-X and JTDX, cross cancellation, and their
concurrent workers. It does not turn Hybrid into a single-decoder or
single-threaded mode. Upstream JTDX uses OpenMP to process frequency partitions
in parallel; the current Rust session processes its logical partitions serially.
Increasing the band count does not create additional OS threads here. It changes
candidate processing and state evolution, and repeats synchronization work, so
it can change both decoded messages and cost without a partition-parallel speedup.

Explicit `jtdx_threads=0` selects the original CPU-dependent automatic mode;
positive values select a logical partition count. CLI accepts
`--jtdx-threads 0` to opt into automatic selection. This is separate from FFT
thread configuration. The pure JTDX four-partition calibration below remains
independent and unchanged.

The existing GUI SWL checkbox and CLI `--swl` now control both JTDX sessions:

| SWL switch | JTDX/noAGC | JTDX/AGC |
|---|---|---|
| Off | `nagcc=false`, `swl=false` | `nagcc=true`, `swl=false` |
| On | `nagcc=false`, `swl=true` | `nagcc=true`, `swl=true` |

The WSJT-X path retains its native behavior. Reconfiguration rebuilds both JTDX
sessions together. Their original mutable state is never shared or migrated
between the two branches.

## Validation and measured scope

Run the shared local suite with a private 2022 CallDB:

```sh
scripts/test-decoders.sh fast
scripts/test-decoders.sh release --include-ignored
```

The full runner includes three-path integration tests, historical recordings,
native signal/channel fixtures, degraded-recording retention and staged versus
one-shot file decoding. Use the engine's separate ignored actor gate when
changing staged engine delivery; see [development checks](../../docs/DEVELOPMENT.md).

Preserve every previous signal under the same recording, configuration, CallDB
and build conditions. Additions cannot compensate for losses. SWL-on and SWL-off
change both native JTDX searches and are not guaranteed supersets of each other.
Pure JTDX's four-partition long-recording gate remains separate from Hybrid.

### Historical partition and three-path evidence (2026-10-28)

The following are dated Release/RustFFT measurements on a 12-core Apple Silicon
machine, using the shipped CallDB (SHA-256
`17140773605b0d2a7a5a31458c11384d9efc413bf6288b262b7dd1f391ec32ea`),
the 19-slot `230208_140300.wav` recording and persistent sessions. They are
separate from the current 2022-CallDB regression point. The earlier two-path
Hybrid kept all 466 original output rows at every JTDX partition setting:

| Logical partitions | Raw rows | Wall time | CPU time | Latest callback |
|---|---:|---:|---:|---:|
| Auto (9 on that machine) | 466 | 68.87 s | 98.63 s | +2.09–2.12 s |
| 4 | 466 | 56.73 s | 82.13 s | +1.58–1.59 s |
| 1 | 467 | 50.42 s | 71.61 s | +1.50–1.51 s |

The one extra row was `CQ Z3BZW FL21`, later reviewed as a wrong decode. One
partition reduced processing cost in these recordings without a confirmed
message gain. In the later three-path implementation replay, the SWL-off
Hybrid kept all 466 earlier rows and produced 481 raw rows; with SWL on it kept
all 465 distinct earlier messages and produced 482 raw rows. SWL changes both
JTDX searches, so its on/off outputs are not a superset relation. All Hybrid
callbacks in those runs arrived within the next-slot +3 s window, while all
workers could finish later. The SWL-off three-path run used more total CPU than
the old two-path run despite a shorter wall time. These are historical run
measurements, not general performance guarantees.

Two earlier unresolved hash displays at 14:03:30 and 14:03:45 had resolved
counterparts with identical complete 79-tone payloads within each slot. Their
resolved/unresolved presentations came from separate native hash books; the
two slots did not have the same complete payload. This is why the ledger checks
the full payload before deduplicating a hash-display expansion and never treats
a short hash or similar text alone as identity evidence.

### Reviewed continuous-recording comparison (2026-10-30)

The homepage chart uses the same 19-slot WAV (SHA-256
`5b4efad0432b7e51a17645ec2723306bf1acd58c29994883b61d471f9841bc2e`),
one persistent file session per mode, 200–3000 Hz, one JTDX partition and the
historical `ALLCALL7.TXT.2022` database (SHA-256
`e647033c0ed4ef2d7c3c56839cfbd819f224805bcd7e0689ac656a0e8b22b298`).
The decoder binary used for the comparison has SHA-256
`cfd1d9d65b989482862a0ca5eaea541a828a9970c8cf988f0c4b13a42d5aa08b`.
Each output row counts, including repeated displays in different slots. CSV
matches and the user's per-row review count as true:

| Mode | Raw | True | Wrong |
|---|---:|---:|---:|
| WSJT-X | 434 | 434 | 0 |
| JTDX | 455 | 453 | 2 |
| Hybrid | 478 | 476 | 2 |
| Hybrid + experiment | 481 | 479 | 2 |

The reviewed wrong rows were `CQ Z3BZW FL21` at 14:04:00 and
`CQ TL1XFI FK48` at 14:04:45. Both occurred in standalone JTDX, Hybrid and
Hybrid + experiment; the latter two modes introduced no additional wrong rows
in this run. The user reviewed 12 other previously unmatched slot/message rows
as real, and they are now included in `tests/ft8/230208_140300.csv`:

| UTC slot | Reviewed real message |
|---|---|
| 14:03:45 | `KB3Z DD0ZL JO40` |
| 14:04:45 | `CQ M0BEN IO92` |
| 14:05:00 | `CQ OH5NBJ KP41`; `4S6NCH UA6ADA KN95` |
| 14:05:30 | `CQ OH5NBJ KP41`; `E25CRF DL1R 73` |
| 14:06:00 | `CQ IW1PUR JN44` |
| 14:06:30 | `CQ F1PPH JN07` |
| 14:07:00 | `CQ F1PPH JN07` |
| 14:07:15 | `VE7ON IU0IFR JN61` |
| 14:07:30 | `4S6NCH IW0RLC 73`; `CQ F1PPH JN07` |

These labels are review judgments for this finite recording, not an estimated
field error rate.
Current regression counts and operating points are in
[validation](../../docs/VALIDATION.md).
