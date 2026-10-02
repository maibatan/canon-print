#!/usr/bin/env python3
"""Show the SET_PARM_PAGE (D0A0) bytes captdriver sends for a CUPS raster file.

captdriver logs only the first 16 bytes of the SET_PARMS packet, so the page
geometry that worked on the real printer is recovered by applying captdriver's
own rules (paper.c page_set_dims + prn_lbp2900.c pageparms, commit 62719249) to
the CUPS raster header of a job:

    cupsfilter -p /usr/share/ppd/captdriver/CanonLBP-2900-3000.ppd -m application/vnd.cups-raster \
        -o media=A4 /tmp/t.txt > /tmp/a4.ras 2>/dev/null
    python3 scripts/cups-raster-params.py /tmp/a4.ras

Header offsets: struct cups_page_header2_s (CUPS raster v2/v3, 1796 bytes).
Reads only the first page header. No dependency outside the standard library.
"""
import json
import struct
import sys

SIZES = [("A4", 0x02), ("A5", 0x03), ("B5", 0x07), ("Executive", 0x0A), ("Legal", 0x0C),
         ("Letter", 0x0D), ("EnvC5", 0x15), ("Env10", 0x16), ("EnvMonarch", 0x17),
         ("EnvDL", 0x18), ("3x5", 0x40), ("PRC16K", 0xD4)]
FUSER = {0: 0x01, 2: 0x01, 1: 0x01, 3: 0x02, 4: 0x13, 5: 0x14, 6: 0x1C}


def main(path):
    with open(path, "rb") as f:
        data = f.read(4 + 1796)
    sync = data[:4]
    # The sync word is the integer 0x52615333 written in the writer's byte order:
    # bytes "RaS3" = big-endian header, "3SaR" = little-endian header.
    if sync in (b"RaS2", b"RaS3", b"RaSt"):
        e = ">"
    elif sync in (b"2SaR", b"3SaR", b"tSaR"):
        e = "<"
    else:
        sys.exit(f"{path}: not a CUPS raster file (sync {sync!r})")
    h = data[4:]
    u = lambda off: struct.unpack_from(e + "I", h, off)[0]
    s = lambda off: h[off:off + 64].split(b"\0", 1)[0].decode("latin-1")

    hdr = {
        "MediaType": s(128),
        "HWResolution": [u(276), u(280)],
        "ImagingBoundingBox": [u(284), u(288), u(292), u(296)],
        "Margins": [u(312), u(316)],
        "PageSize": [u(352), u(356)],
        "cupsWidth": u(372),
        "cupsHeight": u(376),
        "cupsMediaType": u(380),
        "cupsBitsPerPixel": u(388),
        "cupsBytesPerLine": u(392),
        "cupsRowCount": u(408),
        "cupsInteger0_toner_save": u(452),
        "cupsInteger1_ink_k": u(456),
        "cupsPageSizeName": h[1796 - 64:].split(b"\0", 1)[0].decode("latin-1"),
    }

    # A raster made with the LBP2900 PPD is 1 bit per pixel with 70-line bands
    # (canon-lbp.drv: ColorModel Gray k chunky, Resolution k 1 70 ...). Anything else
    # means the PPD was not used (e.g. /etc/cups/ppd/*.ppd not readable by this user).
    problems = []
    if hdr["cupsBitsPerPixel"] != 1:
        problems.append(f"cupsBitsPerPixel={hdr['cupsBitsPerPixel']} (LBP2900 PPD gives 1)")
    if hdr["cupsRowCount"] != 70:
        problems.append(f"cupsRowCount={hdr['cupsRowCount']} (LBP2900 PPD gives 70)")
    if problems:
        print("WARNING: this raster was NOT produced with the LBP2900 PPD: " + "; ".join(problems)
              + " — values below are not what captdriver sends.", file=sys.stderr)

    # captdriver paper.c + prn_lbp2900.c (note: size code comes from MediaType, a bug)
    size_code = next((c for n, c in SIZES if hdr["MediaType"].startswith(n)), 0x02)
    fm = FUSER.get(hdr["cupsMediaType"], 0x01)
    p = bytearray(40)
    p[2:4] = b"\x30\x2A"
    p[4] = size_code
    p[8] = (hdr["cupsInteger1_ink_k"] << 2) & 0xFF
    p[9:12] = b"\x1C\x1C\x1C"
    p[12] = hdr["cupsMediaType"] & 0xFF
    p[13] = 0x81 if hdr["HWResolution"][1] == 400 else 0x11
    p[14:18] = b"\x04\x00\x01\x01"
    p[18] = 0x02
    p[19] = hdr["cupsInteger0_toner_save"] & 0xFF
    for off, val in ((22, hdr["Margins"][0]), (24, hdr["Margins"][1]), (26, hdr["PageSize"][0]),
                     (28, hdr["cupsHeight"]), (30, hdr["cupsWidth"]), (32, hdr["cupsHeight"])):
        struct.pack_into("<H", p, off, val & 0xFFFF)
    p[36] = fm

    derived = {
        "size_code": size_code,
        "margin_height": hdr["Margins"][0],
        "margin_width": hdr["Margins"][1],
        "line_size": hdr["PageSize"][0],
        "num_lines": hdr["cupsHeight"],
        "paper_width": hdr["cupsWidth"],
        "paper_height": hdr["cupsHeight"],
        "fuser_mode": fm,
        "raster_bytes_per_line": hdr["cupsBytesPerLine"],
        "line_handling": ("cut" if hdr["cupsBytesPerLine"] > hdr["PageSize"][0] else
                          "pad" if hdr["cupsBytesPerLine"] < hdr["PageSize"][0] else "exact"),
    }
    print(json.dumps({"valid_lbp2900_raster": not problems, "header": hdr, "captdriver_d0a0": derived,
                      "d0a0_hex": " ".join(f"{b:02X}" for b in p)}, indent=2))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
