# Hybrid Decode

`--profile hybrid` combines independent WSJT-X, JTDX/noAGC and JTDX/AGC sessions
and one bounded round of cross cancellation. It is orchestration over the
existing decoders. Hybrid does not alter the native algorithms under
`lib_wsjtx/` or `lib_jtdx/`; source-alignment corrections are reviewed separately.

Related notes: [WSJT-X](WSJTX.md), [JTDX](JTDX.md), [DX](DX.md),
[shared architecture](COOPERATIVE_DECODER_DESIGN.md).

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
Pure JTDX's four-partition 430/431 calibration remains separate from Hybrid.

The [2026-10-28 report](../../docs/reports/2026-10-28-hybrid.md) records partition
experiments, payload-aware hash-display matching, and the implemented three-path
replay. It retained all previous distinct messages in the measured SWL settings.
Unannotated additions remain candidates; the finite recordings do not establish
a universal sensitivity gain or real-time deadline. Current regression counts
and operating points are in [validation](../../docs/VALIDATION.md).
