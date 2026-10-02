#!/usr/bin/env bash
# Regenerate golden files from the captdriver C reference (oracle).
# Needs: Linux, gcc, cargo, and the reference clone research/agalakhov_captdriver
# (see docs/porting-plan.md). Normal test runs do NOT need this script: the golden
# files are committed and pinned by SHA-256 in tests/fixtures/golden/MANIFEST.json.
set -euo pipefail
cd "$(dirname "$0")/.."

REF=research/agalakhov_captdriver
[ -f "$REF/src/hiscoa-compress.c" ] || { echo "missing $REF (git clone https://github.com/agalakhov/captdriver $REF)"; exit 5; }
COMMIT=$(git -C "$REF" rev-parse HEAD)

mkdir -p target/oracle target/fixtures/pages tests/fixtures/golden
gcc -std=c99 -O2 -Wall -I"$REF/src" -o target/oracle/hiscoa_oracle \
    tools/oracle/hiscoa_oracle.c "$REF/src/hiscoa-compress.c" "$REF/src/hiscoa-common.c"

gcc -std=c99 -O2 -Wall -I"$REF/tests" -I"$REF/src" -o target/oracle/hiscoa_verify \
    tools/oracle/hiscoa_verify.c "$REF/tests/hiscoa-decompress.c" "$REF/src/hiscoa-common.c"

cargo run --quiet --bin gen-fixtures -- --small tests/fixtures --pages target/fixtures/pages >/dev/null

for pbm in tests/fixtures/*.pbm target/fixtures/pages/*.pbm; do
    name=$(basename "$pbm" .pbm)
    target/oracle/hiscoa_oracle "$pbm" "tests/fixtures/golden/$name.hiscoa"
    # cross-check with captdriver's own decoder: every band must decode back to the PBM
    target/oracle/hiscoa_verify "$pbm" "tests/fixtures/golden/$name.hiscoa" 2>/dev/null
done

cargo run --quiet --bin golden -- manifest --oracle-commit "$COMMIT"
echo "golden files regenerated from $REF @ $COMMIT"
