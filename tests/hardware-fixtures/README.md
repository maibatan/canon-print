# Hardware fixtures

A4 as CUPS renders it for the LBP2900 PPD (4722 × 6780 dots, 600 dpi; centred into the 595-byte page line like captdriver), monochrome, 1 page each. Generated, not committed (~4 MB each):

```
cargo run --release --bin gen-fixtures -- --hardware tests/hardware-fixtures
```

| File | Checks |
|---|---|
| test_1page.pbm | border, 1-px horizontal + vertical line, checkerboard patch, "TEST" in the centre |
| test_single_line.pbm | one horizontal line: vertical offset / band boundaries |
| test_checkerboard.pbm | 16-px checkerboard: bit order, compression worst case |
| test_text.pbm | text rows: glyph shapes, left margin |

A completed job only proves that the printer accepted the data. Whether the page looks right
must be checked by eye, camera or scanner (`physical = NOT VERIFIED` in reports).
