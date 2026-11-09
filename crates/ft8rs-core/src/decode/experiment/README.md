# Hybrid experimental receiver

The `experiment` module owns the independent receive DSP. Hybrid schedules
it through a crate-private boundary; algorithm ownership is independent of Hybrid.
It has no standalone `DecodeProfile`, executable or public decoder constructor.
The receiver is opt-in with `StreamDecodeConfig.hybrid_experiment` or
CLI `--profile hybrid --hybrid-experiment`; it is disabled by default. The GUI
exposes **Experimental decoder** at the bottom of Decode settings, after Deep
decode, when Hybrid is selected. Its value is persisted and a change rebuilds the
decode session at the next slot boundary without restarting audio capture. The
[2026-10-29 report](EXPERIMENTS.md)
records frozen known-truth confirmation and original-recording observations.

## Current layout

- `mod.rs` defines immutable slot inputs, the internal boundary and diagnostics.
- `receiver.rs` owns discovery, demapping, model selection and the bounded search.
- `waveform.rs` owns reference fitting, data support and regularized cancellation.
- `tests.rs` keeps input/budget/CW, sync-only and weak-retention regressions outside runtime code.
- `EXPERIMENTS.md` records the pre-layout frozen measurement, not a new benchmark.

The receiver includes native nuisance models, data-symbol admission and
regularized channel fitting. The research campaigns and their measurements are
historical records; see [development evidence](DEVELOPMENT.md).

## Scope and native boundary

The implementation lives in the private `decode/experiment/receiver.rs` module.
It reuses WSJT-X+ BP/OSD and the shared CRC, message unpacking, codeword-to-tone
mapping and GFSK synthesis. All candidate discovery, likelihoods, waveform admission,
line removal, channel fitting and residual search are independent ft8rs code
derived from the preserved single-slot research harness. No mirrored native
source, threshold, candidate order, AP scheduling or arithmetic is changed.

Hybrid supplies read-only audio and baseline observations to this private DSP
branch. Its FEC uses an all-zero AP mask. Each decode uses local state, independent
of the baseline sessions, AP memory, hash books and runtime CallDB.

## Receive path

One full-band FFT supplies 28 overlapping complex subbands at 200 samples/second.
The configured search is limited to the intersection with 200–3000 Hz. Candidate
Costas power is locally whitened. Up to 96 candidates receive four noncoherent
local hypotheses. Up to 32 candidates can receive eight pilot-coherent hypotheses.
FEC calls the Plus BP/OSD entry with argument value 2.

Initial metrics include one/three-symbol amplitude, four-symbol Bessel likelihood
and three-symbol per-tone robust likelihood. Bessel demapping reuses complex
prefix sums across tone patterns and identical exponential terms across bit
partitions. The metrics share noise and pilot statistics from each spectrum.
The candidate and summation order are preserved. Pilot-coherent detection uses only
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

### Broadband pulse recovery

After all existing Hybrid callbacks, including candidate retry, the experimental
branch can run one more search on a private, pulse-blanked copy. Overlapping
64-sample Hann windows at 12 kHz measure power in six groups of audio FFT bins.
A window triggers if at least four groups exceed four times their slot median,
with a floor tied to total median power to reject narrowband leakage. The mask
blanks short bursts and tapers the edges. It skips slots with a contiguous mask
over 160 ms or more than 15% of the samples blanked or tapered.

The terminal search takes at most 32 coarse candidates within the remaining
slot deadline and shared 4096-call FEC budget. Candidates must pass the same
full-frame and payload waveform checks on the blanked copy. Accepted rows can
supplement the output as `exp` and cannot replace an existing representative or
seed a subsequent search.

An optimized, same-input A/B run with the experimental decoder enabled in
both builds recovered 27 of 28 known targets under synthetic broadband pulses,
up from 10. The fresh cases covered periodic, irregular and band-limited
bursts. The 64 synthetic slots included 30 without a target and produced zero
incorrect rows. All 16 prior callbacks were retained. Two keyed narrowband
control slots triggered the pulse pass without adding messages.

With the same `fast` build profile, 200–3000 Hz search and private 2022 CallDB,
two historical recordings retained all 510 callbacks in order, including the
three existing `exp` rows. The pulse detector stayed inactive in those 20 slots.
In the synthetic runs the latest correct pulse recovery callback was 1.96 s
after processing began, and the slowest slot finished in 2.99 s.

## Scheduling and output

The experimental worker starts after all three native workers finish and their
streaming callbacks have been forwarded. It owns a separate audio copy and an
immutable baseline snapshot, and can compute alongside the remaining SIC work.
Its separate FIFO buffers results until all native/SIC publications complete;
file mode retains the existing fixed SIC-direction and AGC supplement order.
Only then may experimental results enter the output ledger. Native callbacks
never wait for the experimental worker. No native session, mutable audio or
recovery result is supplied to it. The receiver uses the remaining time until
ten seconds after Hybrid slot processing began and at most 4096 FEC
calls across its searches. Checks occur between candidates, FEC calls and coarse
fit frequency steps; an active FEC or FFT is not preempted, so ten seconds is a
soft work limit. Existing native work can exceed that time independently, in which case the specialist
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
searches, budget exhaustion, elapsed time and a worker error if any.
Hybrid diagnostics separately record native/SIC completion, experimental worker
start/finish and its publication gate.
These wall intervals can overlap and must not be added as serial stages. Estimated
SNR is derived from fitted waveform support and should not be treated as an
independently measured station SNR.

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

## Historical experiment records

The development measurement tools and isolated generator were archived locally in
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

The three campaign 03 recording additions were accepted as
supplemental annotations by the user after checking callsigns and grids. Their
`user-review` origin and the previous unlabelled interpretation are preserved;
see the recording section of [the development report](DEVELOPMENT.md).
