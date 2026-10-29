# Experimental decoder foundation

The `experiment` module owns the retained receive DSP foundation. Hybrid schedules
it through a crate-private boundary; algorithm ownership is independent of Hybrid.
It has no standalone `DecodeProfile`, executable or public decoder constructor.
The initial integration is opt-in with `StreamDecodeConfig.hybrid_experiment` or
CLI `--profile hybrid --hybrid-experiment`; it is disabled by default. There is
currently no GUI switch. The [2026-10-29 report](EXPERIMENTS.md)
records frozen known-truth confirmation and original-recording observations.

## Current layout

- `mod.rs` defines the internal decoder boundary and diagnostic export.
- `receiver.rs` retains the validated prototype as a development foundation.
- `tests.rs` keeps the existing input/budget/CW regressions outside runtime code.
- `EXPERIMENTS.md` records the pre-layout frozen measurement, not a new benchmark.

No new algorithm capability is introduced by this cleanup. The unused power/LSE
metric and unused phase interpolation variants were removed. The callable path
keeps its previous operations and thresholds.

## Scope and native boundary

The implementation lives in the private `decode/experiment/receiver.rs` module.
It reuses unchanged native CRC/LDPC, message unpacking, codeword-to-tone mapping
and GFSK synthesis. All candidate discovery, likelihoods, waveform admission,
line removal, channel fitting and residual search are independent ft8rs code
derived from the preserved single-slot research harness. No mirrored native
source, threshold, candidate order, AP scheduling or arithmetic is changed.

This work introduces an explicitly authorized independent DSP branch. The native
Hybrid paths continue to be orchestration over their original implementations;
DX does not use the specialist. The specialist never accesses a mutable baseline
session, AP memory, hash book or runtime CallDB. FEC uses an all-zero AP mask.

## Receive path

One full-band FFT supplies 28 overlapping complex subbands at 200 samples/second.
The configured search is limited to the intersection with 200–3000 Hz. Candidate
Costas power is locally whitened. Up to 96 candidates receive four noncoherent
local hypotheses. Up to 32 candidates can receive eight pilot-coherent hypotheses.
FEC uses the existing BP+OSD entry with its existing argument value 2; this is not
a claim that the argument changes the native OSD flip order.

Initial metrics include one/three-symbol amplitude, four-symbol Bessel likelihood
and three-symbol per-tone robust likelihood. Pilot-coherent detection uses only
fully observed sync symbols. Effective sample counts are carried through block
metrics; missing samples do not become low-noise observations. Input lengths
between 120000 and 180000 samples are accepted, with other lengths skipped.

Up to three strongly persistent spectral lines can be fitted on the private copy.
The original audio remains available to all native paths. This is a stationary
line model, not a general-purpose jammer or multipath estimator.

Tentative FEC codewords keep the original 79 tones. Reference-waveform fitting
refines frequency and timing; it interpolates the 12 kHz reference rather than
low-rate observations. Candidates are ranked by support, then refitted after
already explained overlapping signals are subtracted. The minimum score is 160,
chosen using earlier development failures. It is an empirical gate, not a
calibrated false-alarm probability or a proof of signal truth.

Accepted models use smooth complex gain estimation with 0.16-second knots and
0.32-second raised-cosine estimation windows. At most three initial models seed
one residual discovery search. Newly recovered messages never start another
search. This is conditional sequential fitting, not an implemented joint GLRT,
soft-feedback receiver or joint refit of every overlapping station.

## Scheduling and output

All native workers and their existing recovery complete first. Their callbacks
are forwarded before specialist work starts. The specialist uses the remaining
time until ten seconds after Hybrid slot processing began and at most 4096 FEC
calls across its searches. Checks occur between candidates, FEC calls and fits;
an active FEC/fit is not preempted, so ten seconds is a soft work limit. Existing
native work can exceed that time independently, in which case the specialist
skips. No GPU or external training data is required.

Supplementary results pass through the existing payload-aware ledger after all
native rows, preserving original representatives. They carry
`StreamDecodeProvenance::Experiment` (CLI/GUI suffix `exp`). Their passive
evidence is `Provenance::Experiment`, excluded from independent confirmation and
AP/hash import eligibility. Duplicate variants do not manufacture corroboration.
Caught specialist worker errors are recorded in diagnostics without discarding
baseline rows. The shipped panic-abort build cannot recover from a process abort.
User callback failures retain the normal error behavior.

`HybridSlotDiagnostics.experiment` records FEC calls, tentative/rejected/accepted
rows, accepted scores and methods, residual searches, budget exhaustion, elapsed
time and a worker error if any. Estimated SNR is derived from fitted waveform
support and should not be treated as an independently measured station SNR.

## Acceptance

Evaluate stable weak signals, stationary-CW interference, single/multiple FT8
blockers, fading/drift scope limits, and noise/CW/blocker-only controls separately.
A branch can lose standalone coverage; its retained value is verified incremental
recovery in a declared scenario. The Hybrid baseline is still preserved.

Record all incorrect outputs even when the correct payload is also present.
Report baseline errors separately from newly introduced errors. Unannotated real
recording additions remain candidates. A finite zero-error result does not prove
zero error on arbitrary interference. Measure callback arrival separately from
all-worker completion, and run timed processes serially with the same input,
configuration, CallDB and build conditions.

## Experiment tools

Historical tools and Rust harness sources live with their campaigns under
`experiments/single-slot/campaigns/01-single-slot-research/{tools,sources}` and
`02-hybrid-specialist/{tools,sources}`. See the repository's experiment index for
replay conditions. Frozen product sources and measured metadata retain their old
names; the current module/flag/provenance names are `experiment`,
`--hybrid-experiment`, and `Experiment`. Unit-test tooling can be consolidated in
a later development step.

## Cleanup verification

The foundation cleanup passed 197 core fast tests, workspace library/binary tests,
all-target Clippy and formatting checks. Three existing stored inputs (AWGN, CW,
and two FT8 blockers) reproduced the frozen message sets after CLI rounding,
with `exp` only on experimental additions. No new random samples or sensitivity
claim were produced. The local check record is in the second campaign's
`checks/layout-cleanup-2026-10-29/cleanup-manifest.json`.
