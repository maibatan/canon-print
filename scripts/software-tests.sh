#!/usr/bin/env bash
# LEVEL 1 (software) + LEVEL 2 (golden / compression) — no printer needed.
# Runs on the host or inside Docker (scripts/test-docker.sh).
#
#   scripts/software-tests.sh [--out DIR]      (default DIR = artifacts)
#
# Writes DIR/software.json and DIR/logs/*.log. Exit 1 if anything FAILED.
# SKIPPED stays SKIPPED: it is never reported as PASS.
set -uo pipefail
cd "$(dirname "$0")/.."

OUT=artifacts
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT=$2; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 1 ;;
    esac
done
LOGS="$OUT/logs"
mkdir -p "$LOGS"

step() { # step NAME CMD... -> prints PASS/FAIL, full output in $LOGS/NAME.log
    local name=$1; shift
    if "$@" >"$LOGS/$name.log" 2>&1; then echo PASS; else echo FAIL; fi
}

suite_status() { # first (top-level) "status" field of a suite JSON file
    grep -o '"status": *"[A-Z]*"' "$1" 2>/dev/null | head -1 | grep -o '[A-Z]*"$' | tr -d '"' || true
}

echo "== LEVEL 1: software"
FMT=$(step fmt cargo fmt --check)
echo "   cargo fmt --check     $FMT"
CLIPPY=$(step clippy cargo clippy --all-targets --all-features -- -D warnings)
echo "   cargo clippy          $CLIPPY"
TEST=$(step test cargo test --all-features)
echo "   cargo test            $TEST"
SOFTWARE=PASS
for s in $FMT $CLIPPY $TEST; do [ "$s" = PASS ] || SOFTWARE=FAIL; done

echo "== LEVEL 2: golden / compression"
BIN="cargo run --quiet --release --bin golden --"
$BIN integrity --json >"$OUT/golden-integrity.json" 2>"$LOGS/golden-integrity.log"
INTEGRITY=$(suite_status "$OUT/golden-integrity.json"); INTEGRITY=${INTEGRITY:-FAIL}
$BIN check --json >"$OUT/golden.json" 2>"$LOGS/golden.log"
GOLDEN=$(suite_status "$OUT/golden.json"); GOLDEN=${GOLDEN:-FAIL}
[ "$INTEGRITY" = FAIL ] && GOLDEN=FAIL
$BIN roundtrip --json >"$OUT/compression.json" 2>"$LOGS/compression.log"
COMPRESSION=$(suite_status "$OUT/compression.json"); COMPRESSION=${COMPRESSION:-FAIL}
echo "   golden integrity      $INTEGRITY"
echo "   golden (byte diff)    $GOLDEN"
echo "   compression roundtrip $COMPRESSION"

reason() { grep -o '"reason": *"[^"]*"' "$1" 2>/dev/null | head -1 | sed 's/"reason": *"//; s/"$//'; }

cat >"$OUT/software.json" <<EOF
{
  "software": "$SOFTWARE",
  "software_steps": { "fmt": "$FMT", "clippy": "$CLIPPY", "test": "$TEST" },
  "golden_integrity": "$INTEGRITY",
  "golden": "$GOLDEN",
  "golden_reason": "$(reason "$OUT/golden.json")",
  "compression": "$COMPRESSION",
  "compression_reason": "$(reason "$OUT/compression.json")"
}
EOF

for s in $SOFTWARE $GOLDEN $COMPRESSION; do [ "$s" = FAIL ] && exit 1; done
exit 0
