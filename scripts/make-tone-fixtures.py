#!/usr/bin/env python3
"""Write the synthetic PDF fixtures for the tone tests (no patient data, no deps).

tests/fixtures/tone_ramp.pdf        A4 page with an 8-bit DeviceGray ramp image (0..255)
tests/fixtures/tone_ramp_smask.pdf  same page + a small RGB image with a soft mask
                                    (SMask) far below the ramp: the page now contains
                                    transparency, like the hospital's signed reports.
Ghostscript >= 9.56 used to lighten the ramp on the second page (double transfer);
canon-print must print the ramp identically on both.

    python3 scripts/make-tone-fixtures.py
"""
import os
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "tests", "fixtures")


def pdf(objects):
    """objects: list of bytes bodies for objects 1..n (object 1 = catalog)."""
    out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"
    xref = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objects) + 1)
    for off in offsets:
        out += b"%010d 00000 n \n" % off
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objects) + 1, xref)
    return bytes(out)


def stream(dict_items, data):
    comp = zlib.compress(data, 9)
    return b"<< " + dict_items + b" /Filter /FlateDecode /Length %d >>\nstream\n" % len(comp) + comp + b"\nendstream"


# 256 x 16 ramp: column k has gray k (0 = black)
ramp = bytes(k for _ in range(16) for k in range(256))
# 32 x 32 red square, alpha = disc
img = bytes([220, 30, 30] * 32 * 32)
alpha = bytes(255 if (x - 15.5) ** 2 + (y - 15.5) ** 2 < 14 ** 2 else 0 for y in range(32) for x in range(32))

for name, with_smask in (("tone_ramp.pdf", False), ("tone_ramp_smask.pdf", True)):
    # ramp drawn 512 x 128 pt near the top of the page; mask image 60 x 60 pt near the bottom
    content = b"q 512 0 0 128 41.64 640 cm /Ramp Do Q\n"
    xobjects = b"/Ramp 5 0 R"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        None,  # page, filled below
        None,  # content stream
        stream(b"/Type /XObject /Subtype /Image /Width 256 /Height 16 /ColorSpace /DeviceGray /BitsPerComponent 8", ramp),
    ]
    if with_smask:
        content += b"q 60 0 0 60 267.64 80 cm /Img Do Q\n"
        xobjects += b" /Img 6 0 R"
        objs.append(stream(b"/Type /XObject /Subtype /Image /Width 32 /Height 32 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 7 0 R", img))
        objs.append(stream(b"/Type /XObject /Subtype /Image /Width 32 /Height 32 /ColorSpace /DeviceGray /BitsPerComponent 8", alpha))
    objs[2] = (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595.28 841.89] "
               b"/Resources << /XObject << " + xobjects + b" >> >> /Contents 4 0 R >>")
    objs[3] = stream(b"", content)
    path = os.path.join(OUT, name)
    with open(path, "wb") as f:
        f.write(pdf(objs))
    print(path)
