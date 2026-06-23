#!/usr/bin/env bash
#
# Run the manual acceptance gates against the *calibrated* callsign database.
#
# The shipped `crates/ft8rs-core/ALLCALL7.TXT` is updated over time to recognize
# new callsigns (better real-world decoding). The #[ignore]d acceptance baselines
# (hybrid == 465, the weak DX recovery of `F1MLZ UA3QNA -04`, etc.) were
# calibrated against the 2022 snapshot `ALLCALL7.TXT.2022`. A larger DB shifts the
# JTDX AP-candidate ordering, which can drop a very weak signal — a data-driven
# change, not a code regression.
#
# The JTDX decoder reads "ALLCALL7.TXT" via an aligned, untouchable path with a
# process-global cache, so we can't pick the DB per test. Instead we temporarily
# swap the calibrated DB into place, run the gates, and always restore the live
# DB on exit (even on Ctrl-C / failure).
#
# Usage:
#   ./scripts/acceptance-gates.sh                       # all manual gates
#   ./scripts/acceptance-gates.sh test_dx_profile_long_ua3qna   # one gate

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORE="$ROOT/crates/ft8rs-core"
LIVE="$CORE/ALLCALL7.TXT"
CALIBRATED="$CORE/ALLCALL7.TXT.2022"
SAVED="$CORE/.ALLCALL7.live.bak"

if [ ! -f "$CALIBRATED" ]; then
    echo "error: calibrated DB not found: $CALIBRATED" >&2
    exit 1
fi

cp "$LIVE" "$SAVED"
restore() { mv -f "$SAVED" "$LIVE" 2>/dev/null || true; }
trap restore EXIT
cp "$CALIBRATED" "$LIVE"

echo "==> Running acceptance gates against calibrated ALLCALL7 (2022 snapshot)"
cargo test --release -p ft8rs-core -- --ignored "$@"
