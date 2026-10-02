# canon-print

Rust CLI to print on a **Canon LBP2900 / LBP2900B** directly over USB (CAPT 2.1 + Hi-SCoA),
without CUPS, cupsd, ccpd, lp or lpr.

**Status: work in progress.** USB diagnosis, the test infrastructure and the Hi-SCoA encoder
(byte-identical to captdriver) work; talking to the printer does not yet. See [docs/testing.md](docs/testing.md) for exactly what is verified.

```
canon-print list [--all] [--json]      Canon LBP printers on USB
canon-print inspect [--json]           descriptors, interfaces, endpoints, kernel driver
canon-print diagnose [--json]          environment + USB checks (never changes the host)
canon-print print FILE.pbm [--json]    staged print (currently stops at stage "detect")
canon-print print FILE.pbm --dry-run   parse + Hi-SCoA encode only, never opens USB
```

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
