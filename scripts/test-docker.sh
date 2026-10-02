#!/usr/bin/env bash
# Run LEVEL 1 + LEVEL 2 inside Docker (independent of host packages).
# Exit: 0 PASS (golden/compression may be SKIPPED), 1 FAIL, 10 Docker unavailable.
set -uo pipefail
cd "$(dirname "$0")/.."

if ! command -v docker >/dev/null || ! docker info >/dev/null 2>&1; then
    echo "Docker not available (daemon not running?) — Docker tests SKIPPED"
    exit 10
fi
mkdir -p artifacts/docker
docker build -q -t canon-print-test -f docker/Dockerfile . >artifacts/docker/build.log 2>&1 \
    || { echo "docker build FAILED (artifacts/docker/build.log)"; exit 1; }
docker run --rm -v "$PWD":/src -w /src canon-print-test \
    bash scripts/software-tests.sh --out artifacts/docker
