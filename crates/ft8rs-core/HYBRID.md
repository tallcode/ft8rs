# Hybrid Decode

`--profile hybrid` combines independent WSJT-X and JTDX sessions and one bounded
round of cross cancellation. It is orchestration over the existing decoders.
The source under `lib_wsjtx/` and `lib_jtdx/` is unchanged.

Related notes: [WSJT-X](WSJTX.md), [JTDX](JTDX.md), [DX](DX.md),
[shared architecture](COOPERATIVE_DECODER_DESIGN.md).

## Baseline and ownership

Each baseline decoder owns its hash book, AP/odd-even memory, workspaces and
residuals. Both receive the same immutable input audio. Persistent baseline
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
unresolved hashes are not silently equated with resolved identities.

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

Cross cancellation is enabled by default. CLI `--no-hybrid-sic` and the GUI
Cross cancellation checkbox disable it while retaining both baseline sessions.
Generic per-call-pair replay, AP callbook imports and SameParityA7 feedback are
not enabled.

## Scheduling and output

Monitor mode preserves WSJT-X's existing N41 early decode and N47 subtraction.
At N50 it runs the WSJT-X final stage and the full JTDX baseline concurrently.
An eligible direction can start when its source baseline finishes, using peer
rows already seen. No new recovery starts after three seconds of N50 processing;
active original calls finish normally.

File mode waits for both baseline results before choosing seeds, keeps canonical
baseline ordering and submits recovery rows in fixed direction order. Real-time
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

## Validation

Run optimized local tests. Preserve every old signal under the same recording,
configuration and CallDB; additional rows cannot compensate for lost messages.
CSV-unannotated rows remain unconfirmed candidates.

The existing profile gates use the calibrated database through
`./scripts/acceptance-gates.sh`. JTDX's 430/431 calibration uses four logical
partitions, as documented in [JTDX.md](JTDX.md). Pure profiles retain their own
native execution and are not validated by substituting Hybrid output.
