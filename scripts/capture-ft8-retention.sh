#!/usr/bin/env bash
# Capture observations and original-recording controls into a fresh report.
# Usage: scripts/capture-ft8-retention.sh DATA_ROOT REPORT_ROOT [fast|release]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA_ROOT="${1:?candidate data root required}"
REPORT_ROOT="${2:?fresh report directory required}"
PROFILE="${3:-fast}"
case "$PROFILE" in fast|release) ;; *) echo "expected fast or release" >&2; exit 2 ;; esac
CHECK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ft8rs-retention.XXXXXX")"
trap 'rm -rf "$CHECK_DIR"' EXIT
cd "$ROOT"
python3 scripts/decoder-references.py verify --data "$DATA_ROOT" --fixtures-only
cargo build --profile "$PROFILE" -p ft8rs-core --example decoder_retention_probe \
    --message-format=json > "$CHECK_DIR/build.jsonl"
python3 - "$ROOT" "$CHECK_DIR" "$DATA_ROOT" "$REPORT_ROOT" "$PROFILE" <<'PYTHON'
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
root,work,data,report=map(lambda value:Path(value).resolve(),sys.argv[1:5])
assert not report.exists(), 'choose a fresh report directory'
for line in (work/'build.jsonl').read_text().splitlines():
    item=json.loads(line)
    if item.get('reason')=='compiler-artifact' and item.get('executable') and item['target']['name']=='decoder_retention_probe':
        shutil.copy2(item['executable'],work/'probe')
        break
else:
    raise SystemExit('retention probe executable not found')
database=root/'crates/ft8rs-core/ALLCALL7.TXT.2022'
shutil.copy2(database,work/'ALLCALL7.TXT')
subprocess.run([str(work/'probe'),str(report)],cwd=root,check=True,
               env={**os.environ,'FT8RS_DECODER_DATA_DIR':str(data)})
conditions={'profile':sys.argv[5],'calldb_sha256':hashlib.sha256(database.read_bytes()).hexdigest(),
            'recordings_manifest_sha256':hashlib.sha256((data/'ft8/degraded/recordings.tsv').read_bytes()).hexdigest(),
            'executable_sha256':hashlib.sha256((work/'probe').read_bytes()).hexdigest()}
(report/'conditions.json').write_text(json.dumps(conditions,indent=2)+'\n')
print(f'Candidate observations saved to {report}; accepted expectations were not changed.')
PYTHON
