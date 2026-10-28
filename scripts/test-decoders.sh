#!/usr/bin/env bash
# Serial local FT8 verification with a private executable/CallDB snapshot.
# Usage: scripts/test-decoders.sh [fast|release] [--include-ignored]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="${1:-fast}"
case "$PROFILE" in fast|release) ;; *) echo "expected fast or release" >&2; exit 2 ;; esac
INCLUDE_IGNORED="${2:-}"
case "$INCLUDE_IGNORED" in ''|--include-ignored) ;; *) echo "unknown argument $INCLUDE_IGNORED" >&2; exit 2 ;; esac
CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ft8rs-decoders.XXXXXX")"
trap 'rm -rf "$CHECK_DIR"' EXIT
cd "$ROOT"
python3 "$ROOT/scripts/decoder-references.py" verify \
    --data "${FT8RS_DECODER_DATA_DIR:-$ROOT/crates/ft8rs-core/tests/decoder/data}"
cargo test --profile "$PROFILE" -p ft8rs-core --lib --tests --no-run \
    --message-format=json > "$CHECK_DIR/build.jsonl"
python3 - "$ROOT" "$CHECK_DIR" "$INCLUDE_IGNORED" "$PROFILE" <<'PYTHON'
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

root, work = map(Path, sys.argv[1:3])
core = root / 'crates/ft8rs-core'
# The historical long-profile gates were calibrated against this database.
# Never replace a live or target-directory database to run them.
database = core / 'ALLCALL7.TXT.2022'
shutil.copy2(database, work / 'ALLCALL7.TXT')
print(f'CallDB: {database.name} sha256={hashlib.sha256(database.read_bytes()).hexdigest()}', flush=True)
if os.environ.get('FT8RS_DECODER_REPORT_DIR'):
    report = Path(os.environ['FT8RS_DECODER_REPORT_DIR'])
    report.mkdir(parents=True, exist_ok=True)
    conditions = {'profile': sys.argv[4], 'calldb_sha256': hashlib.sha256(database.read_bytes()).hexdigest(),
                  'inputs': {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                             for p in sorted((core / 'tests/ft8').glob('*')) if p.is_file()}}
    conditions_file = report / 'conditions.json'
    if conditions_file.exists() and json.loads(conditions_file.read_text()) != conditions:
        raise SystemExit('report directory contains results from different conditions; choose a fresh directory')
    conditions_file.write_text(json.dumps(conditions, indent=2) + '\n')
executables = {}
for line in (work / 'build.jsonl').read_text().splitlines():
    item = json.loads(line)
    if item.get('reason') == 'compiler-artifact' and item.get('executable') and item['profile']['test']:
        executables[item['target']['name']] = Path(item['executable'])
assert 'decoder_test' in executables and 'ft8rs' in executables
for name, path in sorted(executables.items()):
    target = work / name
    shutil.copy2(path, target)
    print(f'Running {name} (serial)', flush=True)
    subprocess.run([str(target), '--test-threads=1', '--nocapture'], cwd=core, check=True)
    if sys.argv[3]:
        subprocess.run([str(target), '--ignored', '--test-threads=1', '--nocapture'], cwd=core, check=True)
PYTHON
