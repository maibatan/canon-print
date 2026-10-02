# Canon LBP2900 — CAPT protocol map (Phase 0)

Status of every claim:

- **FACT** — read directly in source code / SPECS / a hardware log, with a reference.
- **INFERRED** — deduced from facts; plausible but not proven.
- **UNKNOWN** — no evidence found.

Reference abbreviations (all cloned under `research/`, read-only):

| Tag | Source | Commit | License |
|---|---|---|---|
| CD | agalakhov/captdriver (+ identical mounaiban fork `62719249`) | 175f8ff | GPL-3.0 |
| SC | mounaiban/studycapt | 422fa84 | per file (GPL-3 / CC0 / Apache-2 / none) |
| LC | darkvision77/libcapt | 8f1fd50 | BSD-2-Clause |
| MAC | duy12i1i7/canon-LBP2900-for-macOS | 12953b0 | GPL-3.0 |
| TK | brucenguyen1102/Canon-LBP2900-CAPT-Toolkit | 4b8c50c | none (all rights reserved) |
| HW | our own CUPS debug logs from a real LBP2900 (jobs 41–73, Sep 2026) | — | — |

---

## 1. Device identity

| Item | Value | Status / evidence |
|---|---|---|
| USB VID:PID | `04a9:2676` | FACT — HW `lsusb`: `ID 04a9:2676 Canon, Inc. LBP2900`; SC `usb_a1a1.c:79`; TK `captd.c:64` |
| IEEE-1284 ID | `MFG:Canon;MDL:LBP2900;CMD:CAPT;VER:2.1;CLS:PRINTER;DES:Canon LBP2900` | FACT as claimed by TK `capt-backend.c:54-59` (“captured from live trace”); not yet verified by us |
| Protocol generation | **CAPT 2.1** | INFERRED (strong): IEEE ID `VER:2.1`; A1A1 reply byte 5 = `0x0B` (“CAPT 2.1+”, SC `diss_capt.py:93-96`) and byte 52 = `0x15`; SC `capt-lbp-devices.json:10` lists 2676 without the “S” (SCoA/CAPT 1) flag |
| Compression | **Hi-SCoA** (not SCoA) | FACT for what works in practice: CD sends D0A4 Hi-SCoA params + Hi-SCoA bands and prints (HW jobs 60–72). SC marks LBP2900 as Hi-SCoA. LC (SCoA, CAPT v1) never mentions LBP2900 |
| Product string | `Canon CAPT USB Device` (manufacturer `Canon`) | FACT — HW `check-hardware.sh`, 2026-09-30 (not "LBP2900") |
| Interface / endpoints | interface 0, class `07/01/02` (USB printer, bidirectional); bulk **OUT `0x01`**, bulk **IN `0x82`** | FACT — HW sysfs on our unit (VMware, Ubuntu 22.04, kernel 6.8), same as SC `usb_a1a1.c:134-135` |
| Max packet size | **512 B** on both bulk endpoints (USB 2.0 high speed) | FACT — HW. Note: CD's reply reassembly assumes 64-byte chunks (`received % 64 == 6`), Rust must reassemble by the length field |
| Kernel driver | `usblp` may claim the interface | FACT (TK detaches it: `captd.c:381-383`; CUPS usb backend detaches/reattaches it) |

A1A1 reply captured from an LBP2900 (FACT, SC `docs/a1a1.py:36`, CC0):

```
A1 A1 38 00 00 0B 31 2A 01 01 F0 FF 40 00 04 00 41 00 01 00 D0 02 00 00
6F 08 00 00 E4 0D 00 00 00 00 00 00 FA 02 00 00 F6 04 00 00 28 3C 32 32
58 02 58 02 15 03 02 02
```

INFERRED decode (SC `diss_capt.py:89-167`, packet offsets): info format `0x0B`; product `0x2A31`
(same as the `30 2A`/`31 2A` model word in D0A0 — see §5); block size `0xFFF0`; 64 buffers;
720 pages/h (12 ppm); max 215.9×355.6 mm; min 76.2×127 mm; margins T/B/L/R 4.0/6.0/5.0/5.0 mm;
600×600 dpi. This vector is a Phase 3 golden test for the A1A1 parser.

---

## 2. Framing

- **FACT** (CD SPECS:26-38, `capt-command.c:122-137`; LC `PacketHeader.cpp:10-16`; SC `captstream.py`):
  `[opcode u16 LE][length u16 LE, includes the 4-byte header][payload]`. All multi-byte payload fields are LE.
  Example: IDENT request = `A1 A1 04 00`.
- **FACT**: a reply echoes the request opcode, same framing. LC treats a mismatch as a protocol error.
- **FACT** (CD SPECS:49-50, `capt-command.c:146-172`; SC `capt.lua:53-63`; TK val `lbp3000-protocol.md:1513`):
  over USB the printer returns the **first 6 bytes as one bulk transfer**, then the rest.
  CD reads 6 bytes, then continues reading while chunks are multiples of 64.
  → Rust transport must reassemble replies by the length field, not assume one transfer = one packet.
- **FACT** (CD SPECS:52-54, `word.h:39-49`): some firmware encodes the length as BCD (57 → `57 00`); CD accepts either.
- **FACT** (CD `capt-command.c:187-204`): **D0A9** is a container: outer header + complete sub-packets, each with its own header. No reply.
- **FACT** (TK `captd.c:313-333`, TK README): a late reply from a previous exchange can remain in the IN pipe
  and desync the next job; TK drains stale IN data (300 ms reads) before/after each job and resyncs on header mismatch.

---

## 3. Commands used by the working LBP2900 flow

Names: CD name / Canon name where known (LC, SC `captstream.py:223-239`). Semantics beyond CD usage are INFERRED.

| Code | CD name | Canon/LC name | Request payload | Reply | Evidence |
|---|---|---|---|---|---|
| A1A1 | IDENT | PC_GET_PRINTER_INFO | — | 56 B info (see §1) | CD `prn_lbp2900.c:130`; LC `Protocol.cpp:94-112` |
| E0A0 | CHKSTATUS | PCR_GET_BASIC_STATUS | — | **8 B** payload on LBP2900: STATUS0 u16 + 6 bytes (HW: `A0 E0 0C 00 00 8A 00 00 0F 00 00 00`) | CD `capt-status.c`; LC `Protocol.cpp:114-125`; HW job 65 |
| A0A8 | CHKXSTATUS | (extended status, CAPT 2) | — | extended status record (§4) | CD `capt-status.c:100-112` |
| A3A2 | START_0 | UNKNOWN | — | echo | CD `:135` |
| A2A0 | JOB_BEGIN | PC_RESERVE_UNIT | `00 00 1E 00 00 00 00 00` | payload[2..3] = job id | CD `:52-54,136-137`; LC `Protocol.cpp:168` |
| E1A2 | GPIO | UNKNOWN | 12 B (LBP2900 table) or 16 B (LBP3010 table — **what CD HEAD actually sends in the LBP2900 prologue**) | echo | CD `:61-83,139` |
| E1A1 | JOB_SETUP | (SetJobInfo) | 72 B: page u16 @4, fg @16, `01` @17, job id @18, `C4 FF 88 FF` @20, date/time @24-30, `01` @31 | echo | CD `:101-122`; SC `capt.lua:597-662` |
| E0A3/E0A2/E0A4 | START_1/2/3 | PCR_CLEAR_MISPRINT / CLEAR_ERROR / DISCARD_DATA | — | echo | CD `:302-305`; LC `Protocol.cpp:169-173` (INFERRED same meaning on LBP2900) |
| E0A5 | UPLOAD_2 | PCR_GO_ONLINE | `EE DB EA AD` + page u16 + `00 00` (CD sends 12×00 after the magic) | echo | CD `:56-59`; LC `Protocol.cpp:127-140` |
| D0A9 | SET_PARMS | container | {D0A0, D0A4, D0A1, D0A2} | none | CD `:319-325` |
| D0A0 | SET_PARM_PAGE | IC_BEGIN_PAGE | 40 B (§5) | none | CD `:279-297` |
| D0A4 | SET_PARM_HISCOA | IC_BLACK_PLANE | `01 04 01 01 00 F9 00 00` (default params) | none | CD `hiscoa-compress.c:249-266` |
| D0A1 / D0A2 | SET_PARM_1/2 | IC_BEGIN_DATA / IC_END_PAGE | — | none | CD; SC `captstream.py` |
| C0A0 | PRINT_DATA | IC_VIDEO_DATA | Hi-SCoA bytes, ≤ 0xFF00 per packet | none | CD `generic-ops.c:40-54` |
| C0A4 | PRINT_DATA_END | IC_BLACK_END | — | none | CD `:335` |
| E0A7 | FIRE | UNKNOWN | page u16 (= page_decoding) | echo | CD `:347-348` |
| E0A9 | JOB_END | PCR_RELEASE_UNIT | job id u16 | echo | CD `:370,380`; LC `Protocol.cpp:172` |

JOB_SETUP `fg` values (FACT usage, INFERRED meaning, CD): `1` job start (page 0), `2` page data complete
(before FIRE), `6` after FIRE, `4` job finished / cancel.

HW log confirming the wire bytes (job 72): `send A1 E1 4C 00 … 02 00 …` (JOB_SETUP fg=2 page 2) → `send A7 E0 06 00 02 00` (FIRE page 2).

---

## 4. Status

### 4.1 Basic status (E0A0) — STATUS0 bits (FACT: CD `capt-status.h:34-63`)

| Bit | Flag | Note |
|---|---|---|
| 15 | READY1 | ? |
| 9 | JOBSTAT_CHNG | A0A1 result changed |
| 8 | XSTATUS_CHNG | extended status changed |
| 7 | BUSY | retry later |
| 5, 4 | UNINIT1/2 | cleared by E0A5 |
| 2 | BUFFERFULL | |
| 1 | NOPAPER1 | |
| 0 | PROCESSING | |

HW observation: idle/after-page STATUS0 = `0x8A00` or `0x8B00` (bits 15, 11, 9 [+8]).

HW FACT (canon-print status, 2026-09-30, printer idle after power-on, right after IDENT): E0A0 reply
is **6 bytes** `A0 E0 06 00 88 00` → STATUS0 = `0x0088` = BUSY (+ bit 3). This matches captdriver's
fixed 1 s wait after IDENT (the printer is busy right after IDENT).

HW FACT (job 65, during a job): the E0A0 reply is 12 bytes total (8-byte payload `00 8A 00 00 0F 00 00 00`), not the
6 bytes (2-byte payload) CD's code comments suggest. Bytes 2..7 of the payload: meaning UNKNOWN
(CD reads `status[1]` from payload offset 8, i.e. past this reply — see pitfall 1).

HW FACT (canon-print status, 2026-09-30): the A0A8 reply on our unit is **56 bytes** (52-byte payload),
first bytes `A8 A0 38 00 31 8A 00 00 0F 00 …`; status[0] = `0x8A31` (READY1, bit 11, JOBSTAT_CHNG,
**UNINIT1, UNINIT2**, PROCESSING) → printer not initialised yet, the first job must run
START_1/2/3 + UPLOAD_2. status[2] = `0x0080`, status[4] = `0x0056`, all page counters 0.
IN replies arrived ~100 ms after each request (A0A8: 2 transfers, ~200 ms) when reading with 64 KiB bulk
transfers. FACT (same VM, 2026-09-30): reading with **max-packet (512-byte) transfers**, as the CUPS usb backend
does (`readbuffer[512]`, usb-libusb.c:1822), every reply arrives **2-3 ms** after the request. A pending
read alone did not help. canon-print therefore reads 512 bytes at a time from a background thread.

### 4.2 Extended status (A0A8) — payload offsets (FACT: CD `capt-status.c:53-79`, SPECS:80-106)

`status[0]`=0..1, `status[1]`=8..9, `status[2]`=10..11, `status[3]`=12..13,
**page counters**: `decoding`=14..15, `printing`=16..17, `out`=18..19, `completed`=20..21,
`status[4]`=24..25, `status[5]`=30..31, **`received`=34..35**, `status[6]`=38..39.
SPECS also names: 28-29 job number, 33 E0A5 count, 40-41 pages in job, 44-45 pages printed.

### 4.3 Known status pitfalls (critical for the state machine)

1. **FACT (CD bug):** `decode_status` gets the reply length *including* the header while `buf` holds the payload;
   a 2-byte E0A0 reply still “decodes” `status[1]` and the page counters from **uninitialized stack memory**
   (`capt-status.c:81-87` + `capt-command.c:181-184`). Rust must decode counters **only** from A0A8 replies of sufficient length.
2. **FACT (CD):** LBP2900 ops refresh A0A8 only when E0A0 bit 8 (XSTATUS_CHNG) is set.
   **MAC claims** (patch comment, `prn_lbp2900.c:531-538`) this unit *never* sets it reliably, leaving counters stale and
   hanging the epilogue; MAC switches LBP2900 to **always send A0A8** (like CD’s LBP3010 config) and reports hardware success.
3. **HW FACT:** page counters persist across jobs within one power-on (job 72: `1/1/1/1` before the page, `2/1/1/1` after);
   after power-on they start at `0/0/0/0` (job 73).
4. **HW FACT:** right after C0A4 the printer reports `received == decoding` for the *new* page within ~0.1 s,
   but a FIRE sent that early was ignored (job 73: counters stuck at `1/0/0/0`). Upstream’s 1 s wait works.
   Whether this is a real engine requirement or an artefact of reading stale counters (pitfall 2) is **UNKNOWN**.
5. **HW FACT:** polling E0A0 every 100 ms in the BUSY wait (with CD’s conditional A0A8) made jobs hang (jobs 65/66).
   MAC polls every 100 ms **with always-A0A8** and reports success → the combination matters; to be tested.

---

## 5. SET_PARM_PAGE (D0A0, 40 bytes) — FACT layout from CD `prn_lbp2900.c:279-297`

| Byte | Value | Note |
|---|---|---|
| 2-3 | `30 2A` | model word (A1A1 product id is `31 2A` — UNKNOWN why they differ) |
| 4 | size code | A4 `02`, A5 `03`, B5 `07`, Letter `0D`, Legal `0C` … (codes from LBP3010 SPECS table; **CD bug**: fed from MediaType, so effectively always `02`) |
| 8 | toner density `<<2` | LBP2900 default per SPECS `0x1C`; CD HEAD sends 0 |
| 9-11 | `1C 1C 1C` | |
| 12 | media type | SPECS vs CD disagree for transparency/envelope — UNKNOWN |
| 13 | `0x11` | 600 dpi (`0x81` if 400) |
| 14-17 | `04 00 01 01` | |
| 18 | `02` | “automatic image refinement” (2016 code sent 00) |
| 19 | toner save 0/1 | |
| 22-23 / 24-25 | margins | CD: `Margins[0]`, `Margins[1]` (points!) — naming x/y disputed between SC files |
| 26-27 | line size (bytes) | also the Hi-SCoA LINESIZE |
| 28-29 | number of lines | |
| 30-31 / 32-33 | paper width/height in dots | |
| 36 | fuser mode | plain `01` |

**Values used by canon-print (FACT: what captdriver sends on our unit, recovered on the VM with
`scripts/cups-raster-params.py`, printer PPD, 2026-09-30; A4 prefix also matches CUPS log job 78):**

| Paper | size code | margins | line_size | lines | paper w × h | D0A0 hex |
|---|---|---|---|---|---|---|
| A4 | `02` | 14 / 14 | 595 | 6780 (97 bands) | 4722 × 6780 | `00 00 30 2A 02 00 00 00 00 1C 1C 1C 00 11 04 00 01 01 02 00 00 00 0E 00 0E 00 53 02 7C 1A 72 12 7C 1A 00 00 01 00 00 00` |
| A5 | `02` (!) | 14 / 14 | 420 | 4722 (68 bands) | 3264 × 4722 | `… 0E 00 0E 00 A4 01 72 12 C0 0C 72 12 00 00 01 00 00 00` |

CUPS renders A4 as 591 bytes/line (cupsWidth 4722); captdriver centres it in 595 bytes (2 bytes
padding each side, `center_pixels`); canon-print does the same (`Bitmap::to_page_raster`).
A5 gets size code `02` because captdriver takes the code from MediaType ("Plain"), not the page size.
It prints; whether `03` would be more correct is UNKNOWN (candidate experiment for the A4↔A5 slowness).

Values the fork used to **match the Windows driver** (CD fork commit aabd266, FACT in history):
A4 paper 4960×7014, margins `0x78`/`0x60`, num_lines 6776, line_size 592; A5 paper 3496×4960.
These are the starting point for our A4 fixture (Phase 5). Which exact set the printer requires: UNKNOWN until HW test.

---

## 6. Hi-SCoA (FACT: CD `hiscoa-compress.c`, `hiscoa-common.c`, `tests/hiscoa-decompress.c`)

- Bands of **70 lines** (PPD `cupsRowCount`), each compressed independently (no references outside the band),
  each terminated with END + `00` (CD always sends EOB_NORMAL, even for the last band; page end = C0A4).
- Bitstream MSB-first; **every output byte XOR 0x43**; after END pad with 1-bits to a 32-bit boundary
  (CD bug: an already-aligned stream gets 32 extra 1-bits).
- Default params `origin_3=1, origin_5=4, origin_0=0, origin_2=-7, origin_4=0` → D0A4 payload `01 04 01 01 00 F9 00 00`.
  (The CD test program uses *different* params: origin_2=-12, origin_4=340.)
- Opcodes: `0`+len LONGREP0 (byte above), `10`+4b REPBYTE (16-entry MRU stash), `1101`+8b BYTE,
  `1100`+len LONGREP2, `1110`+len LONGREP3, `11110`+len LONGREP4 (unused), `111110`+len LONGREP5,
  `11111101` ZEROBYTE, `11111100` PREFIX (len > 127), `11111110`+2b END, `11111111` NOP.
- Length code: 0→`111111`, 1→`00`, 2→`011`, 3→`010`, else Elias-like (see CD `:134-152`). Max match 639.
- Matches never cross a line boundary. Copies overlap → decoder copies byte by byte.
- **SPECS:367 PREFIX formula is wrong**; the decoder (`hiscoa-decompress.c:70-75`) is authoritative.
- Encoder is greedy; output depends on the exact match-selection order → a Rust encoder must reproduce
  CD’s search order to be byte-identical (needed for golden tests; the printer only needs *valid* streams).
- **No Hi-SCoA golden vectors exist anywhere** (CD, MAC, SC, LC). We must generate them by running CD’s C code (Phase 2).

---

## 7. Working LBP2900 sequence (FACT: CD HEAD, confirmed by HW jobs 60–72)

```
job:   A1A1 IDENT → wait 1 s → E0A0(+A0A8) → A3A2 → A2A0 (job id) → E1A2 GPIO → wait !BUSY
       → E1A1 fg=1 page=0 → wait !BUSY
page:  E0A0(+A0A8); if UNINIT: E0A3 E0A2 E0A4, wait, E0A5, wait
       → wait !BUFFERFULL → D0A9{D0A0, D0A4, D0A1, D0A2}
       → C0A0 × N (≤0xFF00 each; wait !BUSY before every 16th)
       → C0A4 → [wait ~1 s] until received == decoding
       → E1A1 fg=2 → wait → E0A7 FIRE(page_decoding) → wait → E1A1 fg=6
       → until out == decoding (NOPAPER → stop/wait user)
end:   until completed == decoding → E1A1 fg=4 → E0A9 JOB_END(job id)
```

HW timing (upstream driver, job 69/70): IDENT→page data ≈1.0 s (the fixed wait), 97 × C0A0 in 0.32 s,
C0A4→FIRE 1.0–2.0 s, FIRE→paper out ≈8–9 s.

TK’s alternative (UNVERIFIED by us): LBP2900 uses the LBP3000-style prologue, double A0A8 before A2A0,
ReserveUnit error recovery, JobInfo heartbeat every 500 ms, persistent USB session (fixes a claimed
“5th job after power-on fails with 0x8C”).

---

## 8. Open questions (UNKNOWN) to resolve before/while implementing

1. ~~Real endpoint addresses, max packet size, interface class~~ → answered by HW (§1).
2. Does always-A0A8 polling at 100 ms work on our unit (MAC says yes)? Is the ~1 s before FIRE still required?
3. Correct D0A0 size codes for LBP2900 (A5!), margins semantics, line_size 592 vs 595.
4. Meaning of A3A2, E1A2 payloads, JOB_SETUP constants (`C4 FF 88 FF`, bytes 17/31).
5. Whether GPIO 16-byte (LBP3010) vs 12-byte table matters.
6. Error handling: jam / cover / no cartridge bits (LC names exist for CAPT v1; LBP2900 mapping unknown).
