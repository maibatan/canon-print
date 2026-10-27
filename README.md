# canon-print

Rust CLI to print on a **Canon LBP2900 / LBP2900B** directly over USB (CAPT 2.1 + Hi-SCoA),
without CUPS, cupsd, ccpd, lp or lpr.

**Status:** prints A4/A5 PDF and PBM files on a real LBP2900 (one or more pages, copies in one
job). Used by HHH-Bridge on Linux. See [docs/testing.md](docs/testing.md) for exactly what is verified.

```
canon-print list [--all] [--json]          Canon LBP printers on USB
canon-print inspect [--json]               descriptors, interfaces, endpoints, kernel driver
canon-print diagnose [--json]              environment + USB checks (never changes the host)
canon-print status [--detach-kernel-driver] identify + status, prints nothing
canon-print print FILE [--paper a4|a5] [--copies N] [--detach-kernel-driver] [--json]
canon-print print FILE --dry-run           render + Hi-SCoA encode only, never opens USB
render-pdf FILE [--paper a4|a5]            (dev tool) write the 1-bit pages as PBM files
```

## How a PDF becomes dots

Ghostscript (`gs`, runtime dependency for PDF input only) renders each page to 8-bit gray at
600 dpi, scaled to fit the LBP2900's printable area (A4 4722 × 6780 dots, A5 3264 × 4722).
canon-print then halftones it with **Ghostscript's own default screen and transfer** (106 lpi,
45°, `{0.8 exp}`, transcribed in `src/raster/halftone.rs`). The result equals what Ghostscript
9.55 / CUPS + captdriver print (0.01 % of dots differ), but it no longer depends on the
installed Ghostscript: gs ≥ 9.56 applies its transfer twice on pages with transparency (e.g.
digitally signed reports with soft-masked images), which printed photos ~13 % lighter.
Black toner only: colour images always print as gray.

Exit codes: `0` success, `1` input/software, `2` USB, `3` printer/protocol, `4` timeout,
`5` permission/environment.

## Build

```
sudo apt install build-essential pkg-config libusb-1.0-0-dev
cargo build --release
```

## Tests

```
./scripts/agent-test.sh              # software + golden + Docker (if available) + USB checks
./scripts/agent-test.sh --hardware   # additionally ONE real print job (explicit action)
```

## Linux permissions

canon-print must not need root. Run `canon-print inspect` first to confirm the VID/PID of your
unit, then install a udev rule (our unit: `04a9:2676`):

```
echo 'SUBSYSTEM=="usb", ATTR{idVendor}=="04a9", ATTR{idProduct}=="2676", MODE="0660", TAG+="uaccess"' \
  | sudo tee /etc/udev/rules.d/99-canon-lbp2900.rules
sudo udevadm control --reload-rules
sudo udevadm trigger
```

If `diagnose` reports the interface is claimed by `usblp`, canon-print does not change that
for you. Either unload it manually (`sudo modprobe -r usblp`) or stop the program that uses it
(CUPS). No module is ever blacklisted automatically.

## Documentation

- [docs/protocol.md](docs/protocol.md): CAPT/LBP2900 protocol map (FACT / INFERRED / UNKNOWN)
- [docs/porting-plan.md](docs/porting-plan.md): C → Rust mapping, license constraints, phases
- [docs/testing.md](docs/testing.md): test levels, golden files, current state

## License

GPL-3.0-or-later (see `LICENSE`). The Hi-SCoA encoder/decoder is a port of captdriver
(Copyright (C) 2013 Alexey Galakhov, GPL-3.0-or-later). `tools/oracle/*` link captdriver sources.

## Third-party research / inspiration

- captdriver (GPL-3.0): https://github.com/agalakhov/captdriver, fork https://github.com/mounaiban/captdriver.
  Protocol reference and the oracle for golden files.
- studycapt: https://github.com/mounaiban/studycapt. CAPT research; per-file licenses (CC0 parts used as facts).
- libcapt (BSD-2-Clause): https://github.com/darkvision77/libcapt. CAPT v1 / SCoA; command names, test style.
- canon-LBP2900-for-macOS (GPL-3.0): https://github.com/duy12i1i7/canon-LBP2900-for-macOS. LBP2900 status handling notes.
- Canon-LBP2900-CAPT-Toolkit (no license): https://github.com/brucenguyen1102/Canon-LBP2900-CAPT-Toolkit. Facts only.
- Canon CAPT Printer Driver for Linux V2.71: https://vn.canon/en/support/0100459601. Behaviour reference only; no code or binaries used.
