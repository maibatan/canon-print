# Testing

One entry point: `./scripts/agent-test.sh` (Linux; on Windows run it inside WSL).

| Level | What | Needs | Script |
|---|---|---|---|
| 1 software | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` (unit + integration) | Rust, libusb-dev | `scripts/software-tests.sh` |
| 2 golden | golden files intact (pinned SHA-256) + Rust encoder vs oracle, byte by byte | nothing extra | `golden integrity`, `golden check` |
| 2 compression | `decode(encode(x)) == x`, `encode(x) == encode(x)` on every fixture | — | `golden roundtrip` |
| Docker | Level 1 + 2 in `docker/Dockerfile` | Docker daemon | `scripts/test-docker.sh` |
| 3 USB | environment, libusb, Canon VID/PID, interface, endpoints, permission, kernel driver | Linux + printer | `scripts/check-hardware.sh` |
| 3 printer | one staged print job of `tests/hardware-fixtures/test_1page.pbm` | `--hardware` | `agent-test.sh --hardware` |
| physical | page actually looks right | camera/scanner | not automated → always `NOT VERIFIED` |

Status values: `PASS`, `FAIL`, `SKIPPED`. SKIPPED is never turned into PASS.
Overall: `FAIL` if any level failed, `PASS` if every level passed, otherwise `PARTIAL`.
Reports: `artifacts/test-report.json`, `artifacts/test-report.txt`; per-step logs in `artifacts/logs/`.

## Safety rules for hardware

- A print job is sent only with `--hardware` or `CANON_HARDWARE_TEST=1`; `cargo test` never prints.
- At most one job per run (`--allow-multiple-hardware-jobs` raises the limit to 3).
- No job is sent when software tests fail or USB checks fail.
- Scripts never unload `usblp`, never change udev, VMware or USB passthrough settings.
- On failure the evidence goes to `artifacts/failure-<timestamp>/`: input path, JSON result,
  stderr, USB check, protocol dump (`--dump-protocol`), commit hash.

## Golden files

`tests/fixtures/golden/*.hiscoa` are produced by the captdriver C code, not by our Rust code:

```
scripts/gen-golden.sh     # needs research/agalakhov_captdriver + gcc (Linux/WSL)
```

1. `tools/oracle/hiscoa_oracle.c` calls captdriver's `hiscoa_compress_band()` exactly like
   `rastertocapt.c` (70-line bands, last band shorter, zeroed band buffer, output buffer
   `2 × line_size × 70`, `HISCOA_EOB_NORMAL`, `hiscoa_default_params`).
2. `tools/oracle/hiscoa_verify.c` decodes every golden band with captdriver's own decoder
   (`tests/hiscoa-decompress.c`) and requires it to match the source PBM exactly.
   Result on the current set: 20 files, 0 errors.
3. `golden manifest` pins the SHA-256 of every fixture and golden file in `MANIFEST.json`
   together with the oracle commit.

Evidence gathered while generating them:
- A4 (4736 × 6776) = **97 bands**, the same number of PRINT_DATA packets seen in the CUPS
  debug log of a real LBP2900 job (jobs 69/70).
- 4 bands end with an extra `BC BC BC BC`: captdriver pads a stream that is already
  32-bit aligned with one more word of 1-bits (`hiscoa-compress.c:242`, the `if` is commented
  out). The printer accepts this (the captdriver path prints). A byte-identical Rust encoder
  must reproduce it.

### HSC1 container

All integers u32 little endian:

```
"HSC1" | line_size | lines | band_lines (70) | band_count | { band_len | band bytes } × band_count
```

### When a golden comparison fails

`golden check` prints, per fixture: the first differing offset, expected and actual byte,
the region (`header` or `band N`, offset inside it), both sizes, the number of differing
bytes and 8 bytes of context on each side.

## Fixtures

Generated deterministically by `src/fixtures.rs` (xorshift RNG with fixed seeds, integer
math only). Small ones are committed in `tests/fixtures/`; a test fails if a committed file
differs from the generator. A4 pages (`a4_*`, `tests/hardware-fixtures/*`) are generated on
demand (`cargo run --bin gen-fixtures -- --pages … --hardware …`).

| Fixture | Purpose |
|---|---|
| blank/black 1×1, 8×8 | degenerate sizes |
| alternating_16x16 | 0xAA/0x55 rows |
| single_pixel / horizontal_line / vertical_line 64×64 | bit order, position |
| checkerboard 64×64 (1 px), 600×600 (8 px) | worst case / periodic |
| random_64x64, odd_width_61x75 | incompressible data, width not a multiple of 8 |
| sparse, repeated_rows, repeated_blocks 600×600 | copy-from-above and short repeats |
| text_test, image_like 600×600 | text-like and dithered photo-like data |
| a4_blank, a4_test_page (4760×6780), a5_test_page (3360×4722) | page rasters in the LBP2900 geometry |
| hardware: test_1page, test_single_line, test_checkerboard, test_text (4722×6780, like CUPS output) | first real prints |

## Current state (be precise)

- Level 1: implemented and passing.
- Level 2 golden integrity: implemented and passing.
- Level 2 golden comparison: **PASS** — the Rust Hi-SCoA encoder (`src/compression/hiscoa.rs`,
  port of captdriver `hiscoa-compress.c`) is byte-identical to the C oracle on all 19 fixtures,
  and the Rust decoder turns every oracle file back into its fixture.
  Negative check done: changing the match selection from `>` to `>=` makes `golden check` FAIL
  with offset/expected/actual/band details.
- Level 2 compression round trip: **PASS** (decode(encode(x)) == x, deterministic, END type 0,
  whole band consumed or exactly the `BC BC BC BC` padding word left).
- Docker: implemented; SKIPPED when no Docker daemon is running.
- USB diagnose: implemented (`canon-print diagnose|inspect|list`, `check-hardware.sh`);
  not yet run against the real printer.
- Mock transport / state machine tests: not implemented yet (Phase 6).
- `print --dry-run`: parses + encodes (A4 page ≈ 0.5 s CPU), never opens USB.
- Real printer job: not implemented (the `print` command stops at stage `detect`).
