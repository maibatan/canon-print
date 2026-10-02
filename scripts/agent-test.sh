#!/usr/bin/env bash
# Single entry point for validating canon-print after a change.
#
#   scripts/agent-test.sh [--hardware] [--detach-kernel-driver] [--no-docker] [--allow-multiple-hardware-jobs]
#   CANON_HARDWARE_TEST=1 scripts/agent-test.sh
#
# Levels: 1 software, 2 golden/compression, Docker, 3 USB + real printer.
# A real print job is sent ONLY with --hardware / CANON_HARDWARE_TEST=1, at most one
# job per run. Missing hardware is SKIPPED, never PASS.
# Output: artifacts/test-report.json, artifacts/test-report.txt
# Exit: 0 PASS or PARTIAL, 1 FAIL.
set -uo pipefail
cd "$(dirname "$0")/.."

HARDWARE_REQUESTED=${CANON_HARDWARE_TEST:-0}
DETACH=""
USE_DOCKER=1
MAX_JOBS=1
for a in "$@"; do
    case "$a" in
        --hardware) HARDWARE_REQUESTED=1 ;;
        --no-docker) USE_DOCKER=0 ;;
        --allow-multiple-hardware-jobs) MAX_JOBS=3 ;;
        --detach-kernel-driver) DETACH=--detach-kernel-driver ;;
        *) echo "unknown option $a" >&2; exit 1 ;;
    esac
done
[ "$HARDWARE_REQUESTED" = 1 ] || HARDWARE_REQUESTED=0

ART=artifacts
rm -rf "$ART/logs" "$ART/hardware"
mkdir -p "$ART/logs" "$ART/hardware"
STARTED=$(date -u +%Y-%m-%dT%H:%M:%SZ)
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "no-git")
DIRTY=false
[ -n "$(git status --porcelain 2>/dev/null)" ] && DIRTY=true

field() { grep -o "\"$2\": *\"[^\"]*\"" "$1" 2>/dev/null | head -1 | sed "s/\"$2\": *\"//; s/\"$//"; }

# ── 1. environment ──────────────────────────────────────────────
KERNEL=$(uname -r)
if [ "$(uname -s)" != Linux ]; then ENVIRONMENT=other
elif [ -f /.dockerenv ]; then ENVIRONMENT=container
elif echo "$KERNEL" | grep -qiE 'microsoft|wsl'; then ENVIRONMENT=wsl
elif grep -qi vmware /sys/class/dmi/id/sys_vendor 2>/dev/null; then ENVIRONMENT=vmware
else ENVIRONMENT=native-linux; fi
DOCKER_OK=0
command -v docker >/dev/null && docker info >/dev/null 2>&1 && DOCKER_OK=1
echo "== environment: $ENVIRONMENT, kernel $KERNEL, docker=$DOCKER_OK, commit $COMMIT (dirty=$DIRTY)"

# ── 2-3. LEVEL 1 + LEVEL 2 ───────────────────────────────────────
bash scripts/software-tests.sh --out "$ART"
SOFTWARE=$(field "$ART/software.json" software); SOFTWARE=${SOFTWARE:-FAIL}
GOLDEN=$(field "$ART/software.json" golden); GOLDEN=${GOLDEN:-FAIL}
COMPRESSION=$(field "$ART/software.json" compression); COMPRESSION=${COMPRESSION:-FAIL}
GOLDEN_REASON=$(field "$ART/software.json" golden_reason)

# ── 4. Docker ────────────────────────────────────────────────────
DOCKER_REASON=""
if [ "$USE_DOCKER" = 0 ]; then DOCKER=SKIPPED; DOCKER_REASON="--no-docker"
elif [ "$DOCKER_OK" = 0 ]; then DOCKER=SKIPPED; DOCKER_REASON="Docker daemon not available"
else
    echo "== Docker"
    bash scripts/test-docker.sh >"$ART/logs/docker.log" 2>&1
    case $? in 0) DOCKER=PASS ;; 10) DOCKER=SKIPPED; DOCKER_REASON="Docker unavailable" ;; *) DOCKER=FAIL ;; esac
fi
echo "   docker                $DOCKER ${DOCKER_REASON:+($DOCKER_REASON)}"

# ── 5. USB / hardware detection ──────────────────────────────────
echo "== LEVEL 3: USB"
bash scripts/check-hardware.sh --json "$ART/hardware/check.json" >"$ART/hardware/check.txt" 2>&1
case $? in
    0) USB=PASS; AVAILABLE=YES ;;
    1) USB=FAIL; AVAILABLE=YES ;;
    *) USB=SKIPPED; AVAILABLE=NO ;;
esac
sed 's/^/   /' "$ART/hardware/check.txt"

# ── 6. real printer (explicit only) ──────────────────────────────
HARDWARE_REASON=""; FAIL_STAGE=""
if [ "$HARDWARE_REQUESTED" = 0 ]; then
    PRINTER=SKIPPED; HARDWARE_REASON="not requested (use --hardware)"
elif [ "$AVAILABLE" = NO ]; then
    PRINTER=SKIPPED; HARDWARE_REASON="no Canon LBP printer detected"
elif [ "$USB" = FAIL ]; then
    PRINTER=SKIPPED; HARDWARE_REASON="USB environment problem (see hardware/check.txt), no job sent"
elif [ "$SOFTWARE" != PASS ]; then
    PRINTER=SKIPPED; HARDWARE_REASON="software tests failing, no job sent"
else
    echo "== hardware status (identify + status, nothing printed)"
    cargo run --quiet --release --bin canon-print -- status --json $DETACH         --dump-protocol "$ART/capt/status" >"$ART/hardware/status.json" 2>"$ART/hardware/status.log"
    if grep -q '"success": true' "$ART/hardware/status.json"; then HW_STATUS=PASS
    else HW_STATUS=FAIL; fi
    echo "   printer status        $HW_STATUS (artifacts/hardware/status.json)"
    echo "== hardware print (max $MAX_JOBS job)"
    cargo run --quiet --release --bin gen-fixtures -- --hardware tests/hardware-fixtures >/dev/null
    cargo run --quiet --release --bin canon-print -- print tests/hardware-fixtures/test_1page.pbm \
        --json --dump-protocol "$ART/capt" >"$ART/hardware/print.json" 2>"$ART/hardware/print.log"
    CODE=$?
    if [ "$HW_STATUS" = FAIL ]; then
        PRINTER=FAIL
        FAIL_STAGE="status: $(field "$ART/hardware/status.json" stage)"
        HARDWARE_REASON="printer status failed at stage '$FAIL_STAGE' (see artifacts/hardware/status.json, artifacts/capt/status/)"
    elif [ $CODE = 0 ] && grep -q '"success": true' "$ART/hardware/print.json"; then
        PRINTER=PASS
    elif grep -q '"error": "not_implemented"' "$ART/hardware/print.json"; then
        PRINTER=SKIPPED
        HARDWARE_REASON="printer path not implemented yet (stage $(field "$ART/hardware/print.json" stage))"
    else
        PRINTER=FAIL
        FAIL_STAGE=$(field "$ART/hardware/print.json" stage)
        FDIR="$ART/failure-$(date -u +%Y%m%dT%H%M%SZ)"
        mkdir -p "$FDIR"
        cp "$ART/hardware/print.json" "$ART/hardware/print.log" "$ART/hardware/check.json" "$FDIR/" 2>/dev/null
        [ -d "$ART/capt" ] && cp -r "$ART/capt" "$FDIR/"
        echo "{\"commit\":\"$COMMIT\",\"dirty\":$DIRTY,\"input\":\"tests/hardware-fixtures/test_1page.pbm\",\"exit_code\":$CODE,\"stage\":\"$FAIL_STAGE\"}" >"$FDIR/context.json"
        HARDWARE_REASON="failed at stage '$FAIL_STAGE' (exit $CODE), evidence in $FDIR"
    fi
fi
PHYSICAL="NOT VERIFIED"

# ── 7. report ────────────────────────────────────────────────────
OVERALL=PASS
for s in "$SOFTWARE" "$GOLDEN" "$COMPRESSION" "$DOCKER" "$USB" "$PRINTER"; do
    case "$s" in FAIL) OVERALL=FAIL ;; SKIPPED) [ "$OVERALL" = PASS ] && OVERALL=PARTIAL ;; esac
done

esc() { local s=${1//\\/\\\\}; printf '%s' "${s//\"/\\\"}"; }
cat >"$ART/test-report.json" <<EOF
{
  "started": "$STARTED",
  "commit": "$COMMIT",
  "dirty": $DIRTY,
  "environment": "$ENVIRONMENT",
  "software": "$SOFTWARE",
  "golden": "$GOLDEN",
  "golden_reason": "$(esc "$GOLDEN_REASON")",
  "compression": "$COMPRESSION",
  "docker": "$DOCKER",
  "docker_reason": "$(esc "$DOCKER_REASON")",
  "usb": "$USB",
  "hardware_available": "$AVAILABLE",
  "hardware": "$PRINTER",
  "hardware_reason": "$(esc "$HARDWARE_REASON")",
  "failed_stage": "$FAIL_STAGE",
  "physical": "$PHYSICAL",
  "overall": "$OVERALL"
}
EOF

{
    echo "===================================="
    echo "CANON PRINT TEST REPORT"
    echo "===================================="
    echo
    printf '%-14s %s\n' Environment "$ENVIRONMENT (commit $COMMIT, dirty=$DIRTY)"
    printf '%-14s %s\n' Software "$SOFTWARE"
    printf '%-14s %s\n' Golden "$GOLDEN${GOLDEN_REASON:+  ($GOLDEN_REASON)}"
    printf '%-14s %s\n' Compression "$COMPRESSION"
    printf '%-14s %s\n' Docker "$DOCKER${DOCKER_REASON:+  ($DOCKER_REASON)}"
    printf '%-14s %s\n' USB "$USB  (HARDWARE_AVAILABLE=$AVAILABLE)"
    printf '%-14s %s\n' Printer "$PRINTER${HARDWARE_REASON:+  ($HARDWARE_REASON)}"
    printf '%-14s %s\n' Physical "$PHYSICAL"
    echo
    printf '%-14s %s\n' Overall "$OVERALL"
    echo
    case "$OVERALL/$PRINTER" in
        FAIL/*) echo "Verdict: FAILED — see artifacts/logs/ and the failing level above." ;;
        */PASS) echo "Verdict: software + protocol + real printer job completed (physical page NOT verified)." ;;
        *) echo "Verdict: software validated; real printer not tested." ;;
    esac
    echo
    echo "Report:"
    echo "  $ART/test-report.json"
} | tee "$ART/test-report.txt"

[ "$OVERALL" = FAIL ] && exit 1
exit 0
