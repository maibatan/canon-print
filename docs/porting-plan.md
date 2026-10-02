# canon-print — porting plan (Phase 0 output)

See `protocol.md` for the protocol facts this plan relies on. FACT / INFERRED / UNKNOWN as defined there.

## 1. Decisions that need the project owner

### 1.1 License — DECIDED: GPL-3.0-or-later (project owner, 2026-09-30)

| Source | License | What we may take |
|---|---|---|
| captdriver (CD) | GPL-3.0 | Everything, **if canon-print is GPL-3.0-or-later**. Otherwise facts only (clean-room). |
| canon-LBP2900-for-macOS (MAC) | GPL-3.0 | same as CD |
| studycapt (SC) | per file | CC0 files (`usb_a1a1.c`, `scoa.py`, `captstream.py`, `docs/a1a1.py` …) freely; `capt.lua` is GPL-3 (derived from CD) |
| libcapt (LC) | BSD-2-Clause | code allowed with copyright notice; **never** vendor its `scripts/filter/captfilter` (proprietary Canon binary, redistributed without license) |
| CAPT-Toolkit (TK) | none | facts only; no code |
| Canon CAPT driver v2.71 | proprietary | observe behaviour only (usbmon traces); no code, no binaries in repo |

**Recommendation: license canon-print as GPL-3.0-or-later.**
Reasons: the only complete LBP2900 implementation (CD) and the only Hi-SCoA codec are GPL-3; a GPL project can port
them routine-by-routine and verify byte-identical output against the C oracle, which is exactly the method this
project requires (binary comparison). A clean-room reimplementation from SPECS is possible but SPECS is incomplete and
contains a known error (PREFIX formula), so it would rely on black-box behaviour of the C code anyway.
Consequence for HHH-Bridge: Bridge invokes `canon-print` as a separate process (CLI) → Bridge itself is not affected.

→ If the owner requires a permissive license instead, switch the plan to clean-room: Hi-SCoA written from
`protocol.md` §6 + black-box golden vectors produced by the C code; no C code read by the person writing the Rust.

### 1.2 Where the project lives
Currently `C:\Users\tanmb\Downloads\canon-print`. `research/` holds reference clones and is git-ignored.

## 2. C → Rust mapping

| C (captdriver) | Responsibility | Rust module | Port type |
|---|---|---|---|
| `word.h` (LO/HI/WORD/BCD) | LE helpers, BCD length | `capt/packet.rs` | rewrite (trivial) |
| `capt-command.c/.h` (`capt_send`, `capt_sendrecv`, `capt_multi_*`, `capt_recv_buf`) | framing, reply reassembly | `capt/packet.rs`, `capt/protocol.rs` | port logic, **fix** length/copy bugs (§4.3 of protocol.md) |
| `capt-command.h` enum | opcodes | `capt/commands.rs` (`CaptCommand`) | rewrite with names from LC/SC where known |
| `capt-status.c/.h` | E0A0/A0A8 decode, flags, wait loops | `capt/status.rs` | port offsets; **decode counters only from A0A8**; waits get timeouts |
| `prn_lbp2900.c` (lbp2900 ops) | job/page state machine, D0A0 params, GPIO, JOB_SETUP | `capt/lbp2900.rs`, `capt/session.rs` | port sequence; all waits → `wait_*(timeout)` returning `PrinterError::Timeout{stage,last_status}` |
| `hiscoa-compress.c`, `hiscoa-common.c/.h` | Hi-SCoA encoder + params | `compression/hiscoa.rs`, `compression/bitstream.rs` | **port 1:1** (same search order) so output is byte-identical |
| `tests/hiscoa-decompress.c` | Hi-SCoA decoder | `compression/hiscoa.rs` (`decode_band`) | port 1:1; used by tests and `--verify` |
| `tests/captdefilter.c` | CAPT stream → PBM | `commands/decode.rs` (dev tool) | port; regression tool for `--output-capt` files |
| `paper.c` | CUPS header → page dims | `raster/mod.rs` (`PrintSettings` → `PageGeometry`) | **rewrite** (CUPS-specific, and buggy: MediaType as size, points as bytes) |
| `rastertocapt.c` | CUPS raster reading, band compression, main loop | `commands/print.rs`, `raster/pbm.rs` | rewrite (no CUPS) |
| `generic-ops.c` | band compress/send, 0xFF00 chunking, wait every 16 packets | `capt/session.rs` | port |
| `printer.c` | model detection via side-channel | `printer.rs` | rewrite: detect via USB VID/PID + A1A1 + (optional) IEEE ID |
| SC `usb_a1a1.c` (CC0) | libusb A1A1 exchange | `usb.rs` | reference only (fix: detach usblp, check errors, discover endpoints) |
| — (CUPS usb backend role) | open/claim/detach, bulk IO, timeouts | `transport.rs` (`trait UsbTransport`), `usb.rs` (`RusbTransport`) | new |

## 3. What can be ported directly vs. must be specified/implemented

- **Direct port (algorithm fully in code, deterministic):** Hi-SCoA encoder/decoder, D0A4 param serialisation,
  packet framing, D0A9 container, JOB_SETUP layout, E0A0/A0A8 offsets, CAPT stream decoder.
- **Port + fix:** reply length/copy handling, status decoding (no uninitialised reads), every wait loop (timeouts,
  no busy spin in the NOPAPER branch).
- **Must be specified by us / verified on hardware:** A4/A5 geometry and D0A0 size codes, margins, line_size,
  polling strategy (always-A0A8 vs conditional; interval), the delay before FIRE, error-bit mapping (jam/cover/toner),
  stale-IN draining between jobs, USB endpoint discovery.

## 4. Test strategy (oracle = captdriver C code, built in WSL)

1. **Oracle harness** (`tools/oracle/`, built in WSL from the pinned CD commit): small C programs linking CD sources
   that read a PBM and write (a) Hi-SCoA bands with default params, (b) the full CAPT byte stream a job would send
   (stubbing the CUPS I/O and status replies). CD has no automated tests and no fixtures — we create them.
2. **Fixtures** (`tests/fixtures/`, generated by a script, deterministic): all-zero, all-one, alternating bits, single
   pixel, 1-px horizontal line, 1-px vertical line, checkerboard, seeded random, sparse text-like, repeated rows,
   full A4 page (4736×6776 @600 dpi), plus the “TEST + border + lines” page for hardware.
3. **Golden tests:** `rust_encode(fixture) == oracle_encode(fixture)` byte-for-byte; `decode(encode(x)) == x`;
   packet serialisation vs. hand-checked vectors (A1A1 reply from SC `docs/a1a1.py`, D0A4 `01 04 01 01 00 F9 00 00`,
   our HW-logged JOB_SETUP / FIRE bytes).
4. **Mock transport:** scripted E0A0/A0A8 replies to exercise the state machine (including the stale-counter case
   and timeouts) without a printer.
5. **Hardware:** only after 1–4 pass, step by step (inspect → A1A1 → status → reserve → one tiny page).

## 5. Phases (as specified), with concrete exit criteria

| Phase | Deliverable | Exit criterion (evidence) |
|---|---|---|
| 1 | `canon-print list / inspect / diagnose` | output on the real LBP2900 showing interface, endpoints, max packet size, kernel driver |
| 2 | Hi-SCoA encoder + decoder | `cargo test` green; byte-identical to oracle on all fixtures |
| 3 | packet serializer/parser | golden vectors pass |
| 4 | connect / A1A1 / E0A0 / A0A8 | real replies parsed on HW; A1A1 matches the SC capture layout |
| 5 | one A4 PBM page | physical page printed; `--output-capt` stream decodes back to the input PBM |
| 6 | PDF → raster | via external `pdftoppm`/Ghostscript first (runtime-optional), no CUPS |
| 7 | robustness | timeouts, no-paper, cancel, multi-page, stale-IN drain, protocol dump |

## 6. Risks

- **Status polling:** our HW tests showed hangs with fast polling; MAC shows fast polling works with always-A0A8.
  Plan: implement always-A0A8 + timeouts, make the interval configurable, measure.
- **“5th job” wedge (TK claim)** after repeated open/close — our CLI opens USB per job like CUPS did. Mitigation:
  drain stale IN data, optional long-running mode later. UNVERIFIED.
- **VM USB passthrough** adds latency/instability; final validation should also run on a physical Linux machine.
- **Page geometry** mistakes produce shifted/clipped output, not errors — the TEST/border/line fixture exists for this.
