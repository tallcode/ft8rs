# Hybrid experimental receiver

The `experiment` module owns the independent receive DSP. Hybrid schedules
it through a crate-private boundary; algorithm ownership is independent of Hybrid.
It has no standalone `DecodeProfile`, executable or public decoder constructor.
The initial integration is opt-in with `StreamDecodeConfig.hybrid_experiment` or
CLI `--profile hybrid --hybrid-experiment`; it is disabled by default. The GUI
exposes **Experimental decoder** at the bottom of Decode settings, after Deep
decode, when Hybrid is
selected. Its value is persisted and a change rebuilds the decode session at the
next slot boundary without restarting audio capture. The [2026-10-29 report](EXPERIMENTS.md)
records frozen known-truth confirmation and original-recording observations.

## Current layout

- `mod.rs` defines immutable slot inputs, the internal boundary and diagnostics.
- `receiver.rs` owns discovery, demapping, model selection and the bounded search.
- `waveform.rs` owns reference fitting, data support and regularized cancellation.
- `tests.rs` keeps input/budget/CW, sync-only and weak-retention regressions outside runtime code.
- `EXPERIMENTS.md` records the pre-layout frozen measurement, not a new benchmark.

The completed development adds native nuisance models, data-symbol admission,
regularized channel fitting and the GUI/reconfiguration path. The first two
campaigns and the pre-development measurement remain frozen. New development
and confirmation belong to the third campaign; see [development evidence](DEVELOPMENT.md).

## Scope and native boundary

The implementation lives in the private `decode/experiment/receiver.rs` module.
It reuses unchanged native CRC/LDPC, message unpacking, codeword-to-tone mapping
and GFSK synthesis. All candidate discovery, likelihoods, waveform admission,
line removal, channel fitting and residual search are independent ft8rs code
derived from the preserved single-slot research harness. No mirrored native
source, threshold, candidate order, AP scheduling or arithmetic is changed.

This work introduces an explicitly authorized independent DSP branch. The native
Hybrid paths continue to be orchestration over their original implementations;
DX does not use the experimental receiver. It never accesses a mutable baseline
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
between 120000 and 180000 samples are accepted by the receiver, with shorter
inputs skipped. The Hybrid-only module boundary limits longer live captures to
their first 180000 samples: whole driver blocks can make a valid live slot
slightly longer than 15 seconds. This is a read-only slice; native inputs are
unchanged and partial slots are never padded for experimental observation.

Up to three strongly persistent spectral lines can be fitted on the private copy.
The original audio remains available to all native paths. This is a stationary
line model, not a general-purpose jammer or multipath estimator.

Tentative FEC codewords keep the original 79 tones. Reference-waveform fitting
refines frequency and timing; it interpolates the 12 kHz reference rather than
low-rate observations. Candidates are ranked by support, then refitted after
already explained overlapping signals are subtracted. The full-frame score must
exceed 160 (calibrated from earlier failures). The 58 data symbols, excluding all
public sync symbols, must also meet a score of 100 and positive coherent
support of at least 5 in each of three time groups (20/20/18 data symbols). Each
group needs at least 128 observed subband samples. These are empirical admission
gates. They are not statistical independence, a calibrated false-alarm probability,
or a proof of truth: candidate selection and validation use the same recording.

Before discovery, up to eight native baseline rows are selected by reported SNR,
then fitted and checked against the same waveform/data gates on a private audio
copy. The original tones are mandatory; a missing/invalid codeword is skipped.
Display text (including hashes) is never re-encoded. Native models are nuisance
signals only, never evidence of an experimental message. Native cross-cancellation
recoveries and experimental results are not supplied as native seeds.

Accepted models use smooth complex gain estimation with 0.16-second knots and
0.32-second raised-cosine windows. Each local least-squares channel has 960 units
of ridge support toward a whole-frame complex gain, limiting absorption of weak
unrelated energy. This reduces overfitting but does not guarantee lossless source
separation. At most eight strongest models seed one residual discovery search.
Candidate admission subtracts relevant models within 80 Hz on a separate copy.
Terminal residual results never start another search. This is conditional
sequential fitting, not a joint GLRT, SISO feedback receiver or joint refit of
every overlapping station.

## Scheduling and output

All native workers and their existing recovery complete first. Their callbacks
are forwarded before experimental work starts. The receiver uses the remaining
time until ten seconds after Hybrid slot processing began and at most 4096 FEC
calls across its searches. Checks occur between candidates, FEC calls and coarse fit frequency steps;
an active FEC or FFT is not preempted, so ten seconds is a soft work limit. Existing
native work can exceed that time independently, in which case the specialist
skips. No GPU or external training data is required.

Supplementary results pass through the existing payload-aware ledger after all
native rows, preserving original representatives. They carry
`StreamDecodeProvenance::Experiment` (CLI/GUI suffix `exp`). Their passive
evidence is `Provenance::Experiment`, excluded from independent confirmation and
AP/hash import eligibility. Duplicate variants do not manufacture corroboration.
Caught experimental worker errors are recorded in diagnostics without discarding
baseline rows. The shipped panic-abort build cannot recover from a process abort.
User callback failures retain the normal error behavior.

`HybridSlotDiagnostics.experiment` records FEC calls, tentative/rejected/accepted
rows, full-frame/data scores and methods, native model counts/rejections,
data-support rejections, conditioning/initial/residual stage durations, residual
searches, budget exhaustion, elapsed time and a worker error if any. Estimated
SNR is derived from fitted waveform
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

Current product measurement tools and the isolated generator are in
`experiments/single-slot/campaigns/03-experiment-development/{tools,sources}`.
They freeze source and a release binary before serial measurement, use a private
CallDB, save raw per-row callbacks, exact baseline-field retention, truth/errors,
diagnostics and generated float32 audio. Output names are immutable.

Historical tools and Rust harness sources live with their campaigns under
`experiments/single-slot/campaigns/01-single-slot-research/{tools,sources}` and
`02-hybrid-specialist/{tools,sources}`. See the repository's experiment index for
replay conditions. Frozen product sources and measured metadata retain their old
names; the current module/flag/provenance names are `experiment`,
`--hybrid-experiment`, and `Experiment`. The common decoder test factory registers
`hybrid-exp` as a Hybrid configuration variant, not a standalone decode profile.

## Cleanup verification

The foundation cleanup passed 197 core fast tests, workspace library/binary tests,
all-target Clippy and formatting checks. Three existing stored inputs (AWGN, CW,
and two FT8 blockers) reproduced the frozen message sets after CLI rounding,
with `exp` only on experimental additions. No new random samples or sensitivity
claim were produced. The local check record is in the second campaign's
`checks/layout-cleanup-2026-10-29/cleanup-manifest.json`.

The subsequent full development passed 199 core checks plus 63 remaining workspace
checks, fresh release confirmation and original-recording baseline comparisons.
See [the full development report](DEVELOPMENT.md) for counts, callbacks, native
error handling and the experimental coverage traded away by this version.

The three campaign 03 recording additions were subsequently accepted as
supplemental annotations by the user after checking callsigns and grids. Their
`user-review` origin and the previous unlabelled interpretation are preserved;
see the recording section of [the development report](DEVELOPMENT.md).
