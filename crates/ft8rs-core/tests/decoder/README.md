# FT8 decoder regression framework

Daily tests read versioned reference data. They do not run a simulator, download
a toolchain or require an upstream checkout. Explicit updates use the preserved
native generators described in [UPDATING.md](UPDATING.md).
This framework covers FT8 reception only.

## Run the suite

From the repository root:

```sh
scripts/test-decoders.sh fast
scripts/test-decoders.sh release --include-ignored
```

The runner verifies fixtures, builds first, copies core test executables and the
2022 CallDB into a private temporary directory, then runs processes and cases
serially. It never overwrites the runtime or target-directory database. The
second command adds all ignored gates, including historical profile checks, DX
known-truth waveforms, original-recording snapshots and long-noise retention.
The engine's long actor test is separate; see
[development checks](../../../../docs/DEVELOPMENT.md).

Narrow checks can use Cargo with the normal CallDB lookup:

```sh
cargo test --profile fast -p ft8rs-core --lib decoder_protocol_tests -- --test-threads=1
cargo test --profile fast -p ft8rs-core --lib original_test28_calls_and_integer_boundaries -- --test-threads=1
cargo test --profile fast -p ft8rs-core --test decoder_test -- --test-threads=1 --nocapture
```

The `test28` checks live inside private native modules, so they are not selected
by the `decoder_protocol_tests` filter alone. Decode tests require an optimized
profile. Ordinary `cargo test` with debug assertions is not the acceptance path.

## Code and data layout

Paths below are relative to `crates/ft8rs-core/`:

```text
tests/decoder_test.rs               public audio integration-test entry
src/lib.rs → decoder/protocol.rs    cfg(test) access to private coding routines
examples/decoder_retention_probe.rs explicit recording/report tool
tests/decoder/
  adapters.rs                      audio interface, factories and capabilities
  protocol_adapters.rs             private pack/CRC/LDPC backend adapters
  support.rs                       replay, matching and callback/completion timing
  data.rs                          accepted/candidate data-root selection
  protocol.rs                      coding, hash, raw-call and subtraction checks
  audio.rs                         waveform, context, noise and session tests
  extended.rs                      native channels and real-recording retention
  random.rs                        deterministic coding experiments
  reference/
    probe.F90                      base native protocol/waveform driver
    calls.F90                      original raw pack28/unpack28 driver
    channels.F90                   original Watterson/degrade_snr driver
  data/ft8/
    protocol/                      native messages, bits, CRC, codewords, tones, hashes
    audio/                         base scenario manifests and fixed noisy WAVs
    waveforms/                     clean native carriers for subtraction checks
    provenance/                    base source/tool/driver/artifact hashes
    calls/                         original test28 inputs and per-source vectors
    channels/                      native Watterson manifests and fixed WAVs
    degraded/                      noisy recordings and per-row retention baseline
    extended-provenance/           extension source/tool/driver/artifact hashes
```

Historical WAVs/CSVs and older input/profile/source-audit tests remain under
`tests/ft8/` and `tests/`. The serial runner includes them without changing their
historical meaning. Current counts and calibration conditions are centralized in
[validation](../../../../docs/VALIDATION.md).

## Shared adapters

A new FT8 engine implements `adapters::Decoder` and registers its factory and
capabilities in `ADAPTERS`. It then receives the same audio, known messages,
matching tolerances and callback checks. The shared recording tool emits the
registry in `adapters.tsv` for candidate-retention preparation.

An engine with its own channel-code implementation can add a private
`protocol_adapters` backend. Hybrid/DX compose native channel codes and are
covered at the audio/orchestration layer. Capability flags preserve differences:
DX returns target-related messages, while JTDX lacks WSJT-X's a8 list recovery.
Future engines must declare capabilities rather than weakening shared assertions.

## Coverage and evidence

| Layer | Checks |
|---|---|
| Native protocol vectors | All 49 upstream FT8 test messages per engine: 77-bit packing, CRC, 174-bit codeword, 79 tones and receive unpack with explicit hash context |
| Callsign hashes | Fixed 10/12/22-bit values from WSJT-X unit tests |
| Coding experiments | Every message/CRC bit flip, deterministic BP/OSD soft errors and AP masks, seeded AWGN/block-Rayleigh correct and undetected outputs |
| Original `test28` | 35 inputs + 14 integer boundaries per engine; cold/seeded unpack text, validity and pack values |
| Base audio | 13 native scenarios per source, offsets, known content, weak signals, synthetic echo/fading, clean subtraction and crowded-band checks |
| Negative/session cases | Silence, seeded noise, wrong/partial context, subsequent empty slots, callback errors and supported a8 behavior |
| Native Watterson | 16 stored WAVs across both original models and two seeds per condition |
| Real-recording degradation | Six fixed WAVs through all adapters, with individual CSV-target retention requirements |

Native generation compiles the selected upstream routines unchanged; drivers
call their original APIs. The original source revisions, compiler/platform,
source hashes, RNG contract and generated-file hashes are retained. The base
Python echo/fading transforms are synthetic test-layer scenarios, separate from
the original Watterson models. Near-threshold results on fixed seeds are not a
statistical sensitivity curve.

All-zero is a valid CRC/LDPC codeword. Coding tests accept it; the full signal
and message validation layer must reject digitally silent audio. JTDX OSD
candidates with negative `nharderror` are rejected as its original caller requires.

Raw-call vectors preserve native validity differences. Reserved values
`532444..2063591` remain unresolved rather than wrapping into directed CQ. `A0`
raw packing and JTDX's raw validation are separate from WSJT-X `callok`.
Only `CQ_` versus `CQ ` is normalized as a display convention. Passing raw unpack
is not evidence of a valid on-air message or independent station identity.

Watterson generation uses explicit source-specific interfaces: WSJT-X takes
`NSIG`; JTDX does not. Their spread scaling, shift and RMS conventions differ.
Receiver AWGN has a fixed seed and active-signal 2500 Hz SNR reference. MQ/MM/MD
cases require known-message recovery; disturbed cases record recovery and reject
wrong content without requiring universal success over random channels.

WSJT-X `degrade_snr` operates unchanged on each <=180000-sample slot. A fresh
native process and C `srand` seed are used per recording, continuously across its
slots, with unchanged `gran.c`. `bw=6000` represents the 12 kHz Nyquist band.
The nominal added dB is based on **total mixed-audio power**, not a measured SNR
loss for each station. Strong signals can dominate the noise scale. Individual
message retention need not be monotonic across these fixed random trials.

Each recording/scenario starts an independent adapter session. Long recordings
keep that session across 19 slots for AP history. The native JTDX long case fixes
four partitions and the 2022 CallDB. Short-noise checks run by default; the long
matrix is an ignored gate selected by the complete release runner.
`retention.tsv` / `retention-cases.tsv` hold individual requirements and matrix
coverage; `retention.json` explicitly labels them as **Rust regression baselines,
not a complete native-decoder oracle**. Unlabelled/other-reference observations
remain in capture reports and are not automatically promoted to truth.

## Before/after message preservation

The ignored snapshot test exports all registered adapters on both historical
recordings. Keep input, configuration, CallDB, FFT backend and build profile the
same before and after a decoder change:

```sh
FT8RS_DECODER_REPORT_DIR=/tmp/ft8-before scripts/test-decoders.sh fast --include-ignored
# Apply the decoder change while retaining the same test conditions.
FT8RS_DECODER_REPORT_DIR=/tmp/ft8-after scripts/test-decoders.sh fast --include-ignored
python3 scripts/decoder-references.py compare-runs --before /tmp/ft8-before --after /tmp/ft8-after
```

Use fresh report directories. `conditions.json` records the profile, CallDB hash
and historical-input hashes; file headers record decoder configuration. The
comparison requires matching conditions and matches rows one-to-one by slot,
normalized message, frequency within 5 Hz and DT within 0.3 seconds. It fails on
any lost row and reports additions separately. Previous unlabelled outputs are
also preserved, without calling them confirmed gains. Exported SNR is available
for stricter numerical comparison but is not part of that identity tolerance.

The known JTDX long miss is `140700 F1MLZ UA3QNA -04`; a new message cannot
compensate for losing another previously retained reference row. Pure native
profiles keep their own checks; Hybrid output cannot substitute for them.

Each replay separates first/last message callbacks from completion of all work.
The retention curve records the first callback in the first nonempty slot and
summed slot completion time separately. Neither is a live end-to-end latency
measurement. Do not run compilation or another CPU benchmark during timed replay.

## Updating or inspecting data

```sh
python3 scripts/decoder-references.py verify
python3 scripts/decoder-references.py status --decoder wsjtx --source ../wsjtx
python3 scripts/decoder-references.py status --decoder jtdx --source ../jtdx
```

A different upstream commit does not require regeneration when the recorded
source hashes are unchanged. Algorithm changes still require accepted audio
regressions. `decoder-references.py` is read-only; normal tests never replace
accepted fixtures.

Use [the update guide](UPDATING.md) for base versus extension regeneration,
Fortran/Boost/FFTW requirements, recording capture, review and adoption. A complete
candidate must include both base and extension layers before running the full
suite; generating base protocol vectors alone is insufficient.

The `hybrid-exp` adapter is the existing Hybrid profile with
`hybrid_experiment=true`. It participates in the same native retention,
invalid-input, callback and audio checks. For calibrated degraded recordings it
must retain every existing Hybrid golden identity; new unlabelled outputs do not
become goldens. It does not add a fifth product profile.
Specialty sensitivity and negative controls remain separately recorded in the
[third campaign](../../../../experiments/single-slot/campaigns/03-experiment-development/README.md).
