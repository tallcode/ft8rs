# Updating FT8 reference data

This is an explicit maintenance operation. Daily tests reuse stored files and do
not invoke these generators. Commands run from the repository root.
The accepted corpus is `crates/ft8rs-core/tests/decoder/data`.

## Decide what changed

```sh
python3 scripts/decoder-references.py status --decoder wsjtx --source ../wsjtx
python3 scripts/decoder-references.py status --decoder jtdx --source ../jtdx
```

The status command compares recorded native source hashes, not just Git commits.
Unrelated upstream changes need no regeneration. Decoder algorithm changes still
need the accepted audio regressions even when fixture-generating sources did not
change. Compare source and provenance changes before choosing a layer to update.

| Change | Candidate preparation |
|---|---|
| Only Rust decoder/orchestration changes | Reuse all accepted data; compare old/new decoding, do not regenerate expectations |
| Only original raw-call/channel/degradation inputs change | Reuse accepted base files and generate a fresh extension candidate |
| Base protocol/encoder/waveform sources or base corpus change | Generate a fresh base first, then generate extensions using that base |

Keep previous reports and a known decoder build. Never regenerate a required
message away to make a new implementation pass. Candidate retention capture is a
proposed baseline, not permission to replace earlier identities or bless extras.

## Native build requirements

Both generators need:

- The selected original WSJT-X/JTDX source roots and a Fortran compiler.
- A C/C++ toolchain and Boost headers containing `boost/crc.hpp`.
- Python 3.9+; the scripts use its standard library.

The extension generator additionally needs **single-precision static FFTW with
Fortran wrappers**. `--fftw-prefix` must contain `include/fftw3.f03` and
`lib/libfftw3f.a`. A normal FFTW source build with `--enable-float --enable-static
--disable-shared` supplies these; do not disable Fortran wrappers. This differs
from the application's optional double-precision runtime FFTW feature.

A relocated macOS Fortran toolchain can pass
`--runtime-dir /path/to/lib/gcc/<triplet>/<version>` for static runtime linking.
Set `MACOSX_DEPLOYMENT_TARGET=11.0` when using the relocated macOS ARM compiler
used for the recorded fixtures. System toolchains normally do not need those
options. Both generators accept the compiler/header paths explicitly and never
download a toolchain.

Native C `rand` and FFTW floating-point results depend on platform/toolchain.
Provenance records source revision/hashes, compiler/flags, platform, seeds,
drivers/generator and artifact hashes. Reproducibility claims must retain those
conditions; normal tests use the fixed stored output.

## 1. Prepare the base

If base sources/data are unchanged, select the accepted base without generating
anything:

```sh
FT8_REFERENCE_BASE=crates/ft8rs-core/tests/decoder/data
```

If base sources changed, use a fresh directory and generate both original engines:

```sh
python3 scripts/generate-ft8-reference.py \
  --decoder wsjtx --source ../wsjtx \
  --messages ../wsjtx/lib/ft8/ft8_testmsg.f90 \
  --fc /path/to/gfortran --boost-include /path/to/boost \
  --output /tmp/ft8-base-candidate
python3 scripts/generate-ft8-reference.py \
  --decoder jtdx --source ../jtdx \
  --messages ../wsjtx/lib/ft8/ft8_testmsg.f90 \
  --fc /path/to/gfortran --boost-include /path/to/boost \
  --output /tmp/ft8-base-candidate
FT8_REFERENCE_BASE=/tmp/ft8-base-candidate
```

The common message list currently has 49 cases. The generator checks upstream
`NTEST`; future additions need review and corresponding schema/test changes.
It refuses to overwrite an existing backend's vectors. Base files alone are not
a complete candidate for the current full test suite.

## 2. Generate the extension candidate

Use a different, fresh output directory. The extension tool copies only the
selected base's `protocol`, `audio`, `waveforms` and `provenance` directories
byte-for-byte. It does not rerun the base generator. Generate both sources into
the same candidate root:

```sh
python3 scripts/generate-ft8-extended-reference.py \
  --base-data "$FT8_REFERENCE_BASE" \
  --decoder wsjtx --source ../wsjtx \
  --calls-corpus ../wsjtx/lib/77bit/test28.txt \
  --fc /path/to/gfortran --boost-include /path/to/boost \
  --fftw-prefix /path/to/single-precision-fftw \
  --output /tmp/ft8-candidate
python3 scripts/generate-ft8-extended-reference.py \
  --base-data "$FT8_REFERENCE_BASE" \
  --decoder jtdx --source ../jtdx \
  --calls-corpus ../wsjtx/lib/77bit/test28.txt \
  --fc /path/to/gfortran --boost-include /path/to/boost \
  --fftw-prefix /path/to/single-precision-fftw \
  --output /tmp/ft8-candidate
python3 scripts/decoder-references.py verify \
  --data /tmp/ft8-candidate --fixtures-only
```

The original calls list is shared; raw expected text and validity remain
source-specific. Each source generates eight Watterson files through its own
original API. WSJT-X supplies the six degraded historical recordings shared by
all adapters. WSJT-X Watterson takes `NSIG`; JTDX does not, and its spread/shift
and normalization behavior also differ. The drivers preserve those differences.

`--fixtures-only` verifies native artifacts before retention expectations exist.
It is a preparation check, not the final acceptance command. The tool refuses
output/base overlap and existing backend extension metadata. After a failed
partial generation, use another fresh candidate directory.

## 3. Capture candidate retention

Use a known decoder build and a fresh report directory:

```sh
scripts/capture-ft8-retention.sh /tmp/ft8-candidate /tmp/ft8-retention-report fast
python3 scripts/prepare-ft8-retention.py \
  --report /tmp/ft8-retention-report --candidate /tmp/ft8-candidate \
  --calldb crates/ft8rs-core/ALLCALL7.TXT.2022 --profile fast
```

`release` can be used instead, but use the same profile for capture and
preparation. The capture script builds the report example, copies it and the
2022 CallDB into a private directory and runs recording/adapter combinations
serially. It does not alter the accepted expectations or runtime database.

The report contains:

| File | Contents |
|---|---|
| `adapters.tsv` | Adapter registry and target-only capability |
| `observations.tsv` | Every observed row, slot and target/other-reference/unlabelled class |
| `curves.tsv` | Per-case reference coverage, unlabelled count, first-nonempty-slot callback and summed completion time |
| `baseline/` | Eight original-audio controls, with decoder configuration and numeric fields |
| `conditions.json` | Profile, CallDB, native recording-manifest and executable hashes |

Preparation independently checks labels against the original profile-appropriate
CSVs. Only matching target rows become candidate `retention.tsv` requirements.
`retention-cases.tsv` records matrix coverage and counts, including cases with no
retained targets. `retention.json` records selection, source/report hashes and
explicitly identifies these as Rust regression expectations, not native-decoder
truth. Native WAV generation and Rust expectation capture remain separate.

Do not capture a weaker implementation and silently adopt its missing rows. If
the native inputs changed, review which lost identities are explained by that
input change; if inputs did not change, preserve accepted per-message requirements.
Unlabelled candidates require separate evidence and are never automatically
promoted to required true messages.

Nominal `degrade_snr` dB is relative to mixed-audio total power, not each station's
measured SNR loss. Do not turn the curve into a claimed dB sensitivity gain or
require monotonic per-message recovery across fixed random trials.

## 4. Review and run the complete candidate

```sh
python3 scripts/decoder-references.py verify --data /tmp/ft8-candidate
python3 scripts/decoder-references.py diff --candidate /tmp/ft8-candidate
FT8RS_DECODER_DATA_DIR=/tmp/ft8-candidate \
  scripts/test-decoders.sh release --include-ignored
```

Review protocol bits/text/validity changes, native source/artifact differences,
retention identities and every unlabelled observation. Unchanged base files
should remain byte-identical for an extension-only update.

Compare original-audio controls with a previous capture made under the same
profile, configuration, input and CallDB. After independently checking the two
capture `conditions.json` files, compare their `baseline/` directories:

```sh
python3 scripts/decoder-references.py compare-runs \
  --before /tmp/ft8-retention-before/baseline \
  --after /tmp/ft8-retention-report/baseline
```

The baseline directories contain per-file configuration headers; their parent
conditions files must also be reviewed. For decoder-change snapshots produced
by `test-decoders.sh`, compare the full report roots instead so the tool also
checks their `conditions.json` automatically. Do not mix those report formats
or fast/release conditions without explicitly separating the comparison.

Never run CPU benchmarks or another compilation during timed replay. Record
first/last callbacks separately from worker completion; offline timing excludes
real capture and GUI latency.

## 5. Adopt the reviewed files

After understanding changes and passing all required gates, replace the reviewed
accepted layer directories from the complete candidate, including intentional
removals shown by `diff`. Keep generated data, provenance, drivers and justified
test/schema changes together. Verify the accepted copy again:

```sh
python3 scripts/decoder-references.py verify
```

Normal tests, status checks, diffs and comparison commands never replace accepted
data automatically. Keep detailed local logs under the ignored
`target/decoder-validation/`; record durable conditions/results in
[validation](../../../../docs/VALIDATION.md). A new upstream oracle does not
justify deleting old required messages, lowering gates or blessing unknown extras.
