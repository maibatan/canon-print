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
- USB diagnose: implemented (`canon-print diagnose|inspect|list`, `check-hardware.sh`); run on
  the real LBP2900 (VMware, Ubuntu 22.04): all checks PASS except the expected `usblp` WARN.
- Session / state machine: tested against `MockTransport` and `FakePrinter` (busy after IDENT,
  UNINIT until UPLOAD_2, page counters, out of paper, ignored FIRE, multi-page, timeouts).
- Real printer: `status` and `print` verified on the real LBP2900 — 1-page PBM, 2-page PDF;
  FIRE (paper starts moving) ~2.5 s after start. Physical pages checked by eye only.

## PDF rendering and tone

PDF pages are rendered by Ghostscript as 8-bit gray (`pgmraw`) and halftoned by
`src/raster/halftone.rs` with Ghostscript's default 600 dpi screen + transfer, applied once.

| Check | Result |
|---|---|
| Screen transcription (`GS_SCREEN_600`) | measured from a 256-level ramp, identical in gs 9.55.0 and 10.02.1 |
| Unit tests | coverage of flat grays equals the gs ramp measurement; 0/255 stay solid; monotone |
| `tone_does_not_depend_on_transparency` | ramp prints bit-identically with and without a soft-masked image on the page (old pipeline on gs 10: 1.6 % of the ramp dots differed, ~13 % lighter) |
| `matches_ghostscript_halftone_on_pages_without_transparency` | vs `gs -sDEVICE=pbmraw`: 0.013 % of dots differ |
| `broken_pdf_reports_ghostscript_error` | Ghostscript's message reaches the user (gs text kept off stdout with `-sstdout=%stderr`) |
| Two real reports (not committed: patient data) | vs what the VM prints today (gs 9.55 pbmraw): 0.012 % of dots differ; image-column coverage 0.5604 vs 0.5601 and 0.2652 vs 0.2648 (old pipeline on gs 10: 0.5413 / 0.2442) |
| Speed / memory | 0.2–0.35 s per A4 page; pages are streamed (6-page PDF: 58 MB RSS) |

Fixtures: `tests/fixtures/tone_ramp.pdf`, `tone_ramp_smask.pdf` (synthetic, written by
`scripts/make-tone-fixtures.py`).

Ghostscript failures (unix tests run a fake `gs` shell script, no real Ghostscript needed):

| Test | Checks |
|---|---|
| `stuck_ghostscript_times_out_as_render_stage` | no output for the idle limit (120 s; 0.5 s in the test) → gs is stopped, `Timeout` with stage `render` (nothing sent: HHH-Bridge falls back to CUPS) |
| `stuck_wrapper_child_is_stopped_too` | a wrapper script's child holding the pipes is stopped as well (own process group) |
| `wrong_size_header_fails_before_reading_the_body` | a page of the wrong size fails at the header, without reading its body |
| `ghostscript_crash_is_reported_as_such` | gs killed by a signal mid-page → "Ghostscript failed (signal …)", not just "truncated" |
| `non_utf8_stderr_is_kept`, `gs_text_keeps_the_end_and_replaces_invalid_utf8` | gs messages with raw PDF bytes survive; long text keeps its last 4000 bytes |
| `warnings_of_a_successful_render_are_returned` | complaints of a successful render reach `job.render_warnings` in the JSON summary |
| `detects_pdf_and_loads_pbm` | `%PDF-` found after leading junk; a PBM magic wins over a chance `%PDF-` in pixel data; empty file → input error |
