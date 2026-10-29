# Single-slot receiver research — working record

This is an experiment, not a production decoder or an accepted sensitivity claim.
The requested deliverable is a feasible independent receiver design supported by
single-slot sensitivity, interference, negative-control and timing measurements.

## Constraints and evidence rules

- No changes to either native mirror, Hybrid, DX, accepted fixtures or CallDB.
- No additional equipment, learned model, external training data or cross-slot context.
- The independent DSP uses unchanged native packing/waveform/FEC calls as controlled
  subsystems. This isolates receive-side experiments; it is not a new LDPC invention.
- Native baseline disables AP, searches 200–3000 Hz at depth 3, fresh call per slot.
- All compilation and experimental processes are sequential, optimized Rust builds.
- Oracle runs provide the actual frequency, timing and drift to the independent
  demapper. They are component feasibility tests, never blind sensitivity results.
- Full-band prototypes derive candidates from their own spectra. No injected
  message, target call, target frequency, timing or transmitted bits enter search.
- Truth is used exclusively by the generator and evaluator. Later harness versions
  require text, frequency within 5 Hz and DT within 0.3 s to match injection.
- Synthetic SNR is active-signal power relative to AWGN in 2500 Hz. No peak counting
  or unannotated real-recording output is called a confirmed sensitivity gain.
- Fixed seeds are paired across SNR and methods; repeated waveforms are not new
  independent negative trials. Development and untouched holdout roots are separate.

## Reproduction and isolation

`prepare.py` copies the current core to `/tmp/ft8rs-single-slot-lab/core`, adds an
experiment-only bridge and executable, and verifies hashes of all 82 native source
files. It builds offline, with its own target directory and private unchanged DB.
`run.py NAME TRIALS ROOT_SEED SCENARIO [SNR_LIST]` runs one executable at a time,
preserving the exact receiver source, build inputs, hashes, raw rows, stderr,
wall/process CPU totals and a mechanically generated summary.

Do not execute prepare/compile while an experiment is active. Do not overwrite
existing run directories. Nothing in this directory is registered in the product.

## Versions and observations so far

- v1: oracle 1–4-symbol block demapping: normalized amplitude/power max metrics,
  log-sum-exp power and fixed-amplitude phase-marginalized Bessel likelihood.
  Same unchanged BP/OSD for all. Release codegen=1/thin-LTO.
- v2: per-tone robust variance, robust pilot amplitude estimate, independent
  full-band Costas candidate scan, local sync refinement and up to four hypotheses.
  Release codegen=16/no-LTO thereafter (do not compare v1/v2 timings as identical builds).
- v3: global/local/interpolated coherent demapping from this frame's 21 pilots.
- v4: blind coherent pilot refinement, with noncoherent fallback. In 14 fixed-
  coordinate development frames, 2/2 at -22 dB were recovered by the phase branch;
  the native baseline and earlier blind branch each recovered 0/2. This tiny sample
  only motivates larger tests.
- v5: frequency/time-varying development coordinates, positional truth checks,
  spectral whitening in candidate discovery, waveform fitting and two cancellation
  variants (unchanged native subtractor vs independent constant-channel LS).

Initial oracle AWGN development screen (32 paired noises/SNR, 8 messages):

| Method | -21 dB | -22 dB | -23 dB | -24 dB |
|---|---:|---:|---:|---:|
| Native blind full-band | 15 | 3 | 0 | 0 |
| Oracle 3-symbol amplitude | 23 | 7 | 1 | 0 |
| Oracle 4-symbol Bessel | 26 | 11 | 2 | 0 |
| Oracle global coherent | 31 | 25 | 7 | 3 |

These columns do not isolate gains against native demapping: oracle synchronization
is an advantage. Log-sum-exp/Bessel alone did not consistently beat amplitude max.
Unmitigated narrowband/CW and near-frequency strong FT8 interference defeated
these longer-block demappers. Robust per-tone likelihood restored 15/16 at -20 dB
and 5/16 at -21 dB in one *oracle* CW screen; native gave 0 in those cases.

## Remaining research before drawing a final proposal

1. Varied-coordinate blind tests and paired retention/gain/loss, including all errors.
2. Cancellation ablation: parameter fitting, native vs independent cancellation,
   time-varying channel model, and a few jointly refitted interference signals.
3. Actual code-feedback refinement, if useful beyond pilot-only multiple hypotheses.
4. Untouched holdout roots after freezing variants, including noise, blocker-only,
   drift/fading and CW; limit conclusions to observed exposure.
5. Existing native-generated channel WAVs and historical real recordings; unlabelled
   rows remain candidates. Preserve baseline union in the proposed product integration.
6. Full single-slot runtime, stage/discovery timestamps and tails, under serial load.
   Oracle demapper-only times exclude front-end cost and must not be reported as
   end-to-end decoding latency. No GPU acceleration result is claimed.
7. Recheck original-source/input hashes, retain failed versions, and deliver a report
   plus reproducible code/data bundle with limitations and a completion audit.

## Frozen confirmation now in progress

v8 was frozen before touching root `2026102991` and negative roots
`2026103041`, `2026103051`, `2026103061`. Source SHA-256:
`4c77d905d3f98b21b89d3e999fa71d0c4a9b7dad39824919326742d043bdb5ea`.
Binary SHA-256:
`f4a64aa9ae3bf28a7ca7a2f2095a581068a16c4d3e0cf3d560aa9371ed4920f4`.
No algorithm edits are permitted during this confirmation campaign. Analysis,
reporting and replay scripts can be improved without rebuilding the executable.
The complete immutable task matrix is preserved in `confirm.py`.

Selected receiver: automatic persistent narrow-line LS subtraction; independent
whitened Costas discovery; amplitude/block likelihood variants and pilot-coherent
fallback; fitted-waveform score >40; one smooth cancellation round with at most
three initial seeds. Feedback and two-round joint refitting remain ablations.

Important development evidence:

- A raw CRC/FEC candidate `BM4YEZ/P YL2GAI/P LE66` was wrong in a v5 mixed-signal
  frame. v6's waveform-gated replay of the two development frames at -22 dB had
  no errors. This is a regression observation, not a false-positive-rate guarantee.
- Interpolating 200-Hz observations biased timing toward integer sample positions.
  The fitter now interpolates the known 12-kHz template instead and refines to
  0.0005 Hz / 0.00025 s grid spacing.
- The constant-channel canceler failed on drift/fading blockers; smooth channel
  reconstruction retained 3/4 target recoveries, matching the native baseline on
  those four development cases. Joint refitting offered no addition in four
  two-blocker development frames, so it was not selected.
- Autonomous persistent-line subtraction recovered 8/8 at -20 dB in the v7 CW
  development screen, versus 0/8 for native and the earlier verified phase head.
- Existing native channel WAVs: native and receiver both 14/16, all outputs match
  the one injected truth; the two other WAVs yielded no output in either method.
- Real 2021 recording: Hybrid 21/21; standalone receiver 17/21; unchanged Hybrid
  plus terminal research additions 21/21 with no extra. A replacement decoder is
  not supported by this result; the proposed integration must retain the baseline.

Active serial batch was launched with exec session `30979`; poll that handle,
never restart it just because output is quiet. It executes eight holdout jobs and
the 20-slot historical replay, all using the frozen v8 executable. Completed run
directories have `run.json` with returncode 0; an unfinished synthetic job has
only growing `rows.tsv` and `stderr.txt`. No other benchmark or compiler may run
while this batch is live.

After the batch: regenerate `analyse.py` outputs; inspect per-signal gains/losses,
all known-truth errors, unknown real rows, timings/callbacks; inspect historical
baseline retention; run appropriate source/script checks; package frozen source,
raw data and report. Then perform the goal completion audit before marking done.

## Important v8 confirmation failure and new hypothesis

Completed AWGN confirmation: Hybrid off/on 31,17,7 /32 at -20,-21,-22;
receiver 32,32,23 /32, zero receiver errors. Hybrid SWL-on had one wrong output
at -21 dB. Receiver max worker completion about 4.8 s.

Mixed one-blocker confirmation exposed a **receiver error**, despite recovering
the correct target too. Seed `2026133012`, -20 dB, true target
`F1GPL PA8SAO RR73` at 994.988315 Hz / DT 0.340259. False candidate:
`CQ AJSCWK0XVRV` at 1007.1215 Hz / DT 0.8875, unconditioned fitted score 55.074.
It is about -12.5 Hz and +0.64 s relative to the strong decoded blocker. The
current waveform check can be fooled by strong-signal cross-correlation support.
Do not call v8 ready for trusted autonomous output or hide this failure.

`receiver_conditional.rs` is a separate, as-yet UNBUILT v9 hypothesis. It ranks
tentative candidates by waveform support, subtracts already-explained overlapping
signals before admission, retains the same score threshold 40, and makes exactly
one residual recovery search. It is not an exact nuisance-projected GLRT, and
must be tested rather than described as proven. It also preserves v8 methods as
controls. Original `receiver.rs` and the running v8 binary stay frozen.

After exec session `30979` fully completes, build with
`python3 experiments/single-slot/prepare.py receiver_conditional.rs`, replay the
known v8 failure as v9 DEVELOPMENT, and check development AWGN/CW/fading/short real
recording. Only then freeze v9 and confirm on untouched root `2026103991` and new
negative roots. V8 roots are no longer holdout evidence for v9. Do not compile
while v8 is still live.

Additional useful confirmation, still pending after v8: extend frozen sensitivity
to -23/-24 dB; run `degrade_channels.py` with the bundled Python (numpy/scipy), then
`run_files.py ... degraded ...` to compare weak target propagation channels.
This adds known AWGN to 12 existing native channel traces using noise estimated
in a separate 1600–2600 Hz band; the 10/12 dB noise increase is not an independently
measured per-signal SNR. It does not change decoder algorithms or source fixtures.

## Current authoritative state after v9 and observability correction

- V8 full batch `30979` completed and was archived under
  `/tmp/ft8rs-single-slot-lab/frozen-v8` with its original binary/source.
- V8 confirmation also found one CW-only noise false candidate and one far-away
  blocker-only false candidate, plus a repeated false on a fading blocker. Preserve
  all five erroneous outputs. Pure noise control was 0/32 errors; this does not
  make the mixed/CW failures harmless.
- V9 source `receiver_conditional.rs` carries original FEC tones, handles unknown
  hash identity conservatively, and uses conditional support plus score 80. The
  margin was calibrated on V8 null failures; no call-format or time-window filter
  was added. Development regressions removed the known false candidates while
  preserving the correct weak messages. Example: original-tone ghost score
  322.130 -> conditioned 11.981; correct weak payload remains about 400.
- V9's complete 32-case/SNR AWGN confirmation at root `2026103991` was preserved.
  Batch `22259` intentionally stopped at the next job boundary because another
  implementation flaw was found. It is terminal, not a live wait. The stop reason
  is `/tmp/ft8rs-single-slot-lab/stop-v9-confirmation.txt`. V9 source/binary archived
  under `frozen-v9`. Its remaining seven confirmation jobs were NOT run.
- The flaw: `variance` included all-zero *unobserved* Costas rows for early/clipped
  hypotheses. Missing one 7-symbol pilot group can bias the median noise estimate
  downward by about 2.4x. This explains why some deeply negative-DT false candidates
  appeared more significant. This is our research frontend's bug, not a native edit.
- New source `receiver_observed.rs` (v10) attaches a sample-validity mask to every
  symbol, estimates noise/pilot channel only from fully observed symbols, and uses
  observed sample counts in block likelihood normalization. The score remains 80;
  there is no further parameter fitting to the next confirmation data.
- Optimized v10 `self-check` passed: 64 Gaussian noises per timing offset, corrected
  noise estimates 0.5662/0.5412/0.5442/0.5378 versus theoretical 0.5333 for DT
  -2.1/-0.7/+0.1/+1.9. Including padding gives 0.2368 at -2.1. Silence rejection,
  unresolved/full hash display identity, conflicting calls and different payloads
  with identical unresolved text are checked too. Log: `self-check-v10.tsv`.
- ACTIVE now: exec session `8635`, v10 short DEVELOPMENT regressions (known mixed
  failure, exact negative seeds, eight old-root AWGN cases). Poll before doing
  anything that compiles or benchmarks. No other experiment process is active.

After v10 development passes: freeze v10, run the complete confirmation matrix
on untouched root `2026104991` and negative roots `2026105041/5051/5061`; include
AWGN -21/-22/-23 dB (32 each), mixtures/CW/fading/two blockers, 32 negatives of
each type and the full 20-slot historical replay. Use methods `hybrid`,
`hybrid-swl` for AWGN, `conditional`, and `hybrid-conditional` for union retention.
Do not treat old V8/V9 roots as v10 holdout. Then perform degraded native-channel
checks and a target-drift scope check if useful, rebuild final analysis and plots,
write the local report, audit protected hashes and completed evidence, archive the
reproducible materials, and only then complete the goal.

## Preservation checkpoint on 2026 10 29

This entry supersedes earlier active-session and next-step instructions above.
The user has requested design discussion only, without development or experiments.
The experiment goal is paused. Do not resume any batch from these historical notes
without a new user instruction authorizing experiments.

The final v10 batch used exec session 70818 and was interrupted with exit code 130.
All nine v10 synthetic confirmation directories have successful completion metadata.
The 16-task channel replay also completed. Historical replay has four successfully
recorded tasks out of 20; output prefix 004 is unfinalized and must not be counted
as a completed task. Existing raw output, including the partial task, is preserved.

An inventory of all 58 run directories and counts read from their existing TSVs
is saved in `campaigns/01-single-slot-research/checkpoint-2026-10-29.json`. These are indexing operations,
not new decoder runs or a final acceptance audit. The existing analysis JSON/TSV
are preserved unchanged and may predate some later completed runs.

All 82 protected native source hashes were checked against the saved environment
for both the repository and private snapshot; no mismatches were found.
See `campaigns/01-single-slot-research/checkpoint-2026-10-29.md` for the durable archive, contents, known
gaps and the distinction between preserved measurements and unexecuted proposals.

## Second campaign authorized on 2026 10 29

The user explicitly authorized implementation of the diagrammed receive DSP only
as a Hybrid branch, new experiments, and development-document updates. Partial
standalone coverage is acceptable; declared specialties need actual known-truth
gains and erroneous output must be counted separately. Both campaigns must be
fully retained. This authorization supersedes the earlier discussion-only pause
for the new campaign; historical runs and their completion limits stay unchanged.

Implementation: private `decode/hybrid/specialist.rs`, opt-in
`StreamDecodeConfig.hybrid_specialist` / `--profile hybrid --hybrid-specialist`.
No standalone profile. Original worker callbacks complete before the DSP begins;
remaining work is limited by ten seconds from Hybrid start and 4096 FEC calls.
Supplementary evidence cannot upgrade native confidence or enter AP/hash memory.
Protected mirrored files remain unchanged. The CPU frontend shares one full-band
FFT between subbands. The original research sources remain unmodified.

The development root was 2026106991. Three 8-case development jobs (AWGN, CW and
one blocker) are preserved, including their entire source snapshots. Earlier
v10's incorrect codeword scored 103.186; a declared 50 percent margin rounded up
sets the new admission threshold to 160. This uses prior development information,
not the new confirmation outcomes. Conditional neighboring-signal admission and
one residual search remain in place. It is not an implemented exact GLRT or
fully soft-feedback joint decoder.

`freeze-hybrid-specialist-v1.json` freezes source files and binary
`d110af710bbd4340747b2ce9f51436e0f22f47f1863fea0b79ac2ed2aee21bd1`.
Confirmation uses previously unused positive root 2026108991 and negative roots
2026109041, 2026109051 and 2026109061. The declared matrix has 192 synthetic cases,
16 channel tasks and 20 historical tasks. Algorithms stay frozen throughout.

The serial confirmation is exec session 85062. Do not start another compiler or
CPU benchmark until it finishes. Logs are written to
`/tmp/ft8rs-single-slot-lab/hybrid-own-confirm-v1.log`. Each complete synthetic
batch is copied before the next timed child starts to
`campaigns/02-hybrid-specialist/runs/`. The first campaign's full archive has an
additional filesystem link in `campaigns/01-single-slot-research/`; the original
copy and all original temporary data remain present.

Local checks before confirmation: all-target workspace Clippy passed; core fast
suite 198 passed, no failures; all three new specialist unit/regression tests
passed after the final frequency-bound check. Workspace fast tests passed except
the existing real-UDP socket test denied by the sandbox; that exact test passed
on the authorized local-loopback retry outside the sandbox. Logs and the explicit
failed/retried result are retained. Final release checks remain pending until
the serial confirmation ends.

## Second campaign completed

Session 85062 completed successfully. Frozen confirmation covered 192 synthetic
combinations with 192 saved float32 input WAVs, 16 native channel tasks and 20
historical tasks. The synthetic target count was 50 for the existing Hybrid and
102 for Hybrid with the specialist, with zero incorrect outputs and no lost
baseline rows. Historical output retained the exact stored fields of all 444
baseline rows; five additions include one non-E reference match and four
unannotated candidates. These candidates are not confirmed gain.

Core release checks including all ignored gates completed with 209 passes and
zero failures. The final audit verified 117 frozen source files, 82 protected
native files, executable/CallDB hashes and input/output completeness. Release
CLI build and help verification succeeded. No algorithm parameters changed
during confirmation. The feature stays opt-in, Hybrid-only and has no GUI switch.

Generated audio remains saved locally and ignored by Git; reproducible generator,
seeds, parameters, hashes and compact results are versioned research material.
Both campaign directories remain present. No completed historical run was deleted
or overwritten. See the dated report and `campaigns/02-hybrid-specialist/analysis/hybrid-specialist-v1-audit.json`.

## Shared result directory cleaned

At the user's request, useful shared result files were assigned to their owning
campaigns. Identical shared copies were verified before their redundant paths
were removed; the first archive bytes and every original run remain preserved.
Mixed analysis JSON/TSV were partitioned by run ownership and verified to recombine
without data loss. The obsolete directory and its now-unused ignore file were
removed. Active scripts and documentation point to the canonical campaign paths.
The migration record is in the second campaign's analysis directory.

## Development foundation cleanup

The user requested cleanup rather than further algorithm development. Current
receive code moved to `crates/ft8rs-core/src/decode/experiment/`; Hybrid only
schedules it. The current opt-in flag is `--hybrid-experiment` and the output
provenance is `Experiment`, rendered as `exp` in CLI/GUI. Message text and UDP
payloads are unchanged. Unused metric/phase alternatives and the uncalled public
SameParityA7 diagnostic injection API were removed. Existing useful input/budget/CW
tests moved out of the runtime algorithm file.

Dedicated implementation documentation and the dated measurement report now live
in the experiment module as README.md and EXPERIMENTS.md. Historical results retain
their original measured names. Python tools and Rust harness sources are assigned
to each campaign's tools/ and sources/. The exact original CLI and GUI source
snapshots were recovered by inverse rename and verified against the original
SHA-256 values; all 117 frozen historical source hashes still match. No frozen
result or input audio was changed, and no new sensitivity experiment was run.
