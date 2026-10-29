# Hybrid experiment full development — 2026-10-29

The pre-development checkpoint `e692a160e7d14fa1a27afa147be9b0aa7706c7d7`
was committed and pushed before product work. Campaigns 01 and 02 are unchanged.
The new work and all successful/failed measurement builds belong to
[campaign 03](../../../../../experiments/single-slot/campaigns/03-experiment-development/README.md).
The earlier [campaign 02 report](EXPERIMENTS.md) remains historical evidence.

## Delivered behavior

`experiment` is an independent receiver module scheduled only by Hybrid. It has
no standalone profile or public decoder entry point. CLI/config opt-in remains
available; the GUI now has a persisted **Experimental decoder** setting at the
bottom of the Decode settings, after Deep decode, for Hybrid. Toggling it rebuilds the session at the next slot boundary, without
restarting capture. Default remains off. Display adds `exp`; payload and UDP
messages remain unchanged.

The native WSJT-X/JTDX source trees, thresholds and algorithms remain untouched.
Hybrid forwards all native callbacks before supplemental processing. The
experimental receiver receives immutable samples and native baseline rows;
original audio and native mutable sessions are never used as scratch space.
Experimental results cannot update native AP/hash memory or create independent
passive confirmation. Native errors are preserved along with native correct rows.

## Algorithms completed in this version

- Shared full-band FFT/subband discovery, locally whitened Costas candidates,
  noncoherent and pilot-coherent local timing/frequency hypotheses, block
  amplitude/Bessel/robust soft metrics and unchanged native CRC/LDPC/OSD calls.
- Up to eight original-tone native waveform models, selected by native SNR then
  independently fitted/checked against the observed samples. Missing/invalid
  tones are skipped; display text and unresolved calls are never re-encoded.
  Native recovery rows and experimental rows are not supplied as native seeds.
- Waveform frequency/timing refinement and conditional admission after nearby
  explained waveforms are removed from private copies. Full-frame score >160;
  data-only score >=100; positive coherent support >=5 in all three data time
  groups (20/20/18 symbols, >=128 observed subband samples per group).
- Stationary-line removal and smooth complex channel subtraction, with a global
  channel prior of 960 ridge units in each local fit. This constrains absorption
  of unrelated weak energy; it does not establish lossless separation.
- One residual search seeded by at most eight strongest accepted models. Terminal
  recoveries cannot recursively seed another search. Maximum 4096 FEC calls and a
  soft deadline ten seconds from Hybrid slot processing start. Coarse fit steps
  also check the deadline. Native work can independently exceed the deadline.
- Counts of native models/rejections, data support rejections, accepted full/data
  scores and stage durations added to `HybridSlotDiagnostics.experiment`.

These are conditional sequential models. Joint nuisance-projected GLRT, SISO
feedback, general phase/drift tracking and GPU kernels are not implemented or
claimed. The data gate excludes public sync, but uses the same recording that
selected the candidate; it is not an independent statistical holdout or p-value.

## Admission and cancellation details

For an aligned complex reference `v` and subband observations `y`, data-only
support uses `|sum(conj(v)*y)|² / (noise_variance * sum(|v|²))`, excluding all
21 Costas symbols. Data score 100 is a conservative rounded fraction of the
full-frame development gate (58/79 * 160 is about 117); group score 5 requires
support distributed across the frame. These values were fixed before fresh
confirmation, not optimized on confirmation labels.

The channel is a real two-column cosine/sine least-squares fit. Local coefficients
are regularized toward the global full-frame fit:

`theta_local = inverse(X' W X + 960 I) * (X' W y + 960 theta_global)`.

A 0.32-second raised-cosine window estimates each local gain, followed by linear
interpolation between 0.16-second knots. For the unit-amplitude reference, 960 is
approximately one symbol's quadrature energy. This limits the flexibility of
local channel tracking. It is a controlled tradeoff: fast fading can remain in
the residual rather than being fully tracked. Candidate evidence is measured
again after nearby already-explained signals are removed; residual-only ranking
is never sufficient for emission.

## Measurement conditions

The isolated harness freezes the current core, unchanged protocol-only synthesis
helpers, source hashes, release build, private shipped CallDB and compiler/machine
information. Product stream defaults are used, with `cooperative_realtime=false`
and one logical JTDX partition. `exp` is the Hybrid configuration with the branch
enabled. All four methods receive the identical float32 input. Truth coordinates
and messages are used for grading only. All measurement processes run serially;
there is no concurrent compilation or other benchmark initiated by this task.

The build uses release fat LTO, one codegen unit and panic abort. Measurement is
on macOS arm64 with 12 visible CPUs; no i5 or GPU result is inferred. Each callback
records message, f, DT, SNR, sync, original tones, provenance and callback time.
Every original Hybrid row must remain identical at the recorded twelve-decimal
field precision (tones/provenance/text exact), independently of any added rows.
Decode completion and callback time exclude session construction; process wall
time is also saved and includes the series of profile runs and initialization.

Development root is 2026102911001; fresh confirmation root is 2026102951001.
Scenario/SNR groups have disjoint offsets and trials use a 10007 stride. Payloads,
frequencies and timing vary. Algorithm/binary was frozen after development, before
confirmation; only documentation and a test-only clone-to-reference cleanup changed
later. Generated audio is retained locally with hashes, not added to Git.

The 32 development cases comprised 16 target cases and 16 controls. Hybrid target
recovery was 6/16 versus 13/16 with the branch, with zero introduced wrong rows and
zero baseline losses. Two native wrong rows remained two wrong rows with exp.
The initial harness compile failure (callback count mistaken for a vector) is
retained under `frozen/development-v2`; it produced no measurements. The corrected
release build is `frozen/development-v2-fixed-harness`.

## Fresh target confirmation

These are correct target counts, not all decoded station counts. Other legitimate
blocker messages are graded separately from wrong rows.

| Scenario | Target SNR | Cases | WSJT-X | JTDX | Hybrid | Hybrid + exp | Added targets |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Stable AWGN | -21 dB | 8 | 3 | 1 | 3 | 7 | +4 |
| Stable AWGN | -22 dB | 8 | 0 | 2 | 2 | 5 | +3 |
| Stable AWGN | -23 dB | 8 | 0 | 0 | 0 | 3 | +3 |
| Stationary CW | -20 dB | 8 | 0 | 0 | 0 | 8 | +8 |
| Stationary CW | -22 dB | 8 | 0 | 0 | 0 | 5 | +5 |
| One FT8 blocker | -20 dB | 8 | 8 | 2 | 8 | 8 | 0 |
| Two FT8 blockers | -20 dB | 8 | 7 | 2 | 7 | 8 | +1 |
| Fading/drifting blocker | -20 dB | 4 | 3 | 1 | 3 | 3 | 0 |
| Target drift 0.25 Hz/s | -20 dB | 4 | 3 | 1 | 3 | 3 | 0 |

The target total is 26/64 to 50/64 (+24). Clear supported specialties in this fresh
batch are stable weak signals and stationary CW. The two-FT8 result adds one
known target; the one-FT8 sample saturates the baseline and shows no incremental
benefit here. No additional fading/drift capability is established. These small
counts do not establish a universal dB improvement or zero false-alarm rate.

All 116 confirmation cases completed. All methods produced zero wrong rows in
this fresh batch, including all 52 controls: 12 noise-only, 12 CW-only, 12 single
FT8 blocker-only, eight two-blocker-only and eight public-sync-only cases.
Hybrid's 82 original rows are retained and the 24 added rows match known target
truth. Development still retains its two inherited native errors; that earlier
result is not rewritten by this fresh zero-error batch.

The maximum newly added target callback was **5.929 s**. Maximum all-worker decode
completion was **9.078 s**; these are different measurements. The native Hybrid
configuration completed within 1.106 s in this batch. Processing remains a soft
ten-second budget, with no hard real-time guarantee on other hardware or load.
No GPU acceleration is needed to meet this measured operating point.

Release binary SHA-256:
`c1d2bdfaa4c3dfcd42cadf49f65499cf141e4328e05a66820c5a8fdfddd91a86`.
Private CallDB SHA-256:
`17140773605b0d2a7a5a31458c11384d9efc413bf6288b262b7dd1f391ec32ea`.
Compiler: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.
Raw evidence: campaign 03 `runs/confirm-v2/`; derived report:
`analysis/confirmation.json`; frozen pre-confirmation record:
`analysis/confirmation-freeze.json`.

## Original-recording replay

The same two stored recordings were replayed as 20 cold slot jobs using the
private shipped CallDB and the same fixed harness timestamp for both methods.
This is a comparison of isolated slot processing, not a warm live AP-memory run.
All **444 native Hybrid rows** were retained at recorded field precision.
Hybrid + exp produced 447 rows, with these three additions now accepted as
user-reviewed supplemental annotations:

| Long recording slot | Message | Frequency | DT |
| ---: | --- | ---: | ---: |
| 1 | G5AT SP9WZO R-10 | 1870.4145 Hz | -0.19225 s |
| 3 | ON7RED CU3HN HM68 | 1705.0460 Hz | 0.50050 s |
| 13 | EA1HBW CU3HN HM68 | 1704.8795 Hz | 0.49800 s |

On 2026-10-29 the user checked the callsigns and matching grids and explicitly
accepted all three rows as annotations. The recording result is therefore
**three annotated incremental decodes**, with all 444 native Hybrid rows retained.
Annotation origin is recorded as `user-review`; the original dated CSV remains
unchanged. The exact file hash, slot, message, frequency and DT are bound in
`annotations/recordings-user-review.json`. This is a retrospective label update;
the decoder, raw callbacks, timing measurements and synthetic results are unchanged.

The pre-review interpretation remains in `analysis/recordings-before-user-review.json`.
Whole-recording correct/wrong totals remain null because this supplemental review
does not relabel every native output. The earlier summary that summed nulls to
zero is also retained as aggregation history.

Compared with campaign 02's own supplemental outputs, the stricter current branch
no longer recovers its annotated `F1MLZ UA3QNA -04` row, nor its unlabelled
`N1F US7UK -09` and `ET5BVW/R 2Z1SBW/R JQ96` rows. It retains the two CU3HN
rows and adds the G5AT row; all three now have user-reviewed annotations. This loss of earlier experimental coverage
is explicit; it is not a loss of native Hybrid output. The earlier results and
sources remain available. No thresholds were relaxed after examining this replay.

Maximum recording all-worker decode time was **8.136 s**, and the latest added
message callback was **7.524 s**. No i5 timing is inferred from this macOS run.
Full raw evidence is in campaign 03 `runs/files-v2/`; annotation comparison is
`analysis/recordings.json`.

## Product verification

All applicable local checks passed:

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `RUSTC_WRAPPER= scripts/test-decoders.sh fast`: **199 passed, 0 failed**;
  11 pre-existing ignored cases remain ignored. This includes the `hybrid-exp`
  adapter in the common upstream-waveform, channel, session and short degraded
  recording checks, using the test script's private 2022 calibration CallDB.
- Remaining workspace library/binary tests, optimized and serial:
  `RUSTC_WRAPPER= cargo test --profile fast --workspace --exclude ft8rs-core --lib --bins -- --test-threads=1`:
  **63 passed, 0 failed**, one pre-existing ignored case. Core tests are excluded
  here because they already passed in the preceding serial run. UDP loopback
  tests ran outside the socket-restricted sandbox; no test result was bypassed.
- Python measurement tooling compilation and 24 local documentation links.
- Current CLI and GUI both built with `--profile fast`. A private CLI/CallDB copy
  replayed an existing confirmation CW input, produced `CQ N0QVY KL05 exp` and
  returned success. This functional smoke is not an extra independent sample;
  stdout/stderr, binary/database fingerprints and command are in `checks/cli-smoke.*`.
- Completion audit verifies all frozen sources/binary/database fingerprints,
  all raw stdout/stderr hashes, all 148 retained generated WAV hashes and all
  550 baseline rows across development/confirmation/recordings, with zero losses.
  Runtime source matches the measured freeze; documentation, the test-only clone-to-reference cleanup and the subsequent
  GUI label/ordering update differ from its source snapshot. The latter is
  recorded separately from the measured receiver source.
- The 82 protected native files and the original two campaign directories have
  no changes relative to checkpoint `e692a16`.

No ignored long matrix is represented as newly tested. The separate release
confirmation (116 cases) and release cold-recording comparisons (20 slots) above
are the current development's algorithm measurements. Historical campaigns retain
their original conditions and outcomes.

Check logs live under campaign 03 `checks/`; the read-only integrity result is
`analysis/completion-audit.json`. New algorithm/data paths are opt-in and the
current branch's verified role is supplementary stable-weak/CW recovery. The three real-recording additions have explicit user-reviewed labels.
Fading/drift scope and slower-CPU timing remain explicit limits.

## User review and presentation update — 2026-10-29

The user accepted the three recording additions after checking callsigns and
grids. Supplemental labels and the pre-review interpretation are retained as
separate records. The GUI label is exactly **Experimental decoder**, placed
after Deep decode as the last Decode setting row for Hybrid. GUI all-target
Clippy, formatting, fast build, 27 local documentation links and the evidence
audit passed. The audit permits only this exact GUI label/row relocation when
comparing against the measured source; receiver/native code and raw measurements
are unchanged. No decode experiment was rerun for this annotation/presentation
update. See campaign 03 `analysis/user-review-update.json` and
`analysis/user-review-audit.json`.
