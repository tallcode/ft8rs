#!/usr/bin/env bash
# Run local historical profile gates with the 2022 CallDB beside a private
# executable. The live database and build-directory database are never swapped.
# Usage: ./scripts/acceptance-gates.sh [test-name-filter]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORE="$ROOT/crates/ft8rs-core"
CALIBRATED="$CORE/ALLCALL7.TXT.2022"
CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ft8rs-acceptance.XXXXXX")"
trap 'rm -rf "$CHECK_DIR"' EXIT

if [ ! -f "$CALIBRATED" ]; then
    echo "error: calibrated DB not found: $CALIBRATED" >&2
    exit 1
fi

cd "$ROOT"
cargo test --release -p ft8rs-core --test stream_decode_test --no-run \
    --message-format=json > "$CHECK_DIR/build.jsonl"
python3 - "$CHECK_DIR/build.jsonl" "$CHECK_DIR/stream_decode_test" <<'PYTHON'
import json
import shutil
import sys
for line in open(sys.argv[1]):
    artifact = json.loads(line)
    if artifact.get("reason") == "compiler-artifact" and artifact.get("executable"):
        if artifact["target"]["name"] == "stream_decode_test":
            shutil.copy2(artifact["executable"], sys.argv[2])
            break
else:
    raise SystemExit("stream_decode_test executable was not produced")
PYTHON
cp "$CALIBRATED" "$CHECK_DIR/ALLCALL7.TXT"
cd "$CORE"
"$CHECK_DIR/stream_decode_test" --ignored --test-threads=1 "$@"
