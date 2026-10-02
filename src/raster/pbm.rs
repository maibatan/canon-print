//! PBM reader/writer (P4 binary and P1 ASCII). PBM is the primary test input format:
//! it is deterministic and maps 1:1 onto the printer's 1-bit raster (1 = black).

use super::Bitmap;
use crate::error::{CanonError, Result};

/// Largest accepted dimension. A4 at 600 dpi is ~4960 x 7016 dots; anything far
/// beyond that is a corrupt header, not a page.
const MAX_DIM: usize = 20_000;

struct Header {
    magic: [u8; 2],
    width: usize,
    height: usize,
    data_start: usize,
}

fn parse_header(input: &[u8]) -> Result<Header> {
    if input.len() < 2 || input[0] != b'P' || (input[1] != b'4' && input[1] != b'1') {
        return Err(CanonError::Raster(
            "not a PBM file (expected magic 'P4' or 'P1')".into(),
        ));
    }
    let mut pos = 2;
    let mut nums = [0usize; 2];
    for n in &mut nums {
        // skip whitespace and comments
        loop {
            match input.get(pos) {
                Some(c) if c.is_ascii_whitespace() => pos += 1,
                Some(b'#') => {
                    while let Some(&c) = input.get(pos) {
                        pos += 1;
                        if c == b'\n' || c == b'\r' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
        let start = pos;
        while input.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        if start == pos {
            return Err(CanonError::Raster("PBM header: expected a number".into()));
        }
        let text = std::str::from_utf8(&input[start..pos]).expect("ascii digits");
        *n = text
            .parse()
            .map_err(|_| CanonError::Raster(format!("PBM header: bad number '{text}'")))?;
    }
    // exactly one whitespace byte separates the header from P4 data
    match input.get(pos) {
        Some(c) if c.is_ascii_whitespace() => pos += 1,
        _ => {
            return Err(CanonError::Raster(
                "PBM header: missing whitespace after height".into(),
            ))
        }
    }
    let [width, height] = nums;
    if width == 0 || height == 0 {
        return Err(CanonError::Raster(format!(
            "PBM has empty size {width}x{height}"
        )));
    }
    if width > MAX_DIM || height > MAX_DIM {
        return Err(CanonError::Raster(format!(
            "PBM size {width}x{height} exceeds the {MAX_DIM} limit"
        )));
    }
    Ok(Header {
        magic: [input[0], input[1]],
        width,
        height,
        data_start: pos,
    })
}

pub fn parse(input: &[u8]) -> Result<Bitmap> {
    let h = parse_header(input)?;
    let mut bmp = Bitmap::new(h.width, h.height);
    let data = &input[h.data_start..];
    if h.magic[1] == b'4' {
        let need = bmp.stride * h.height;
        if data.len() < need {
            return Err(CanonError::Raster(format!(
                "PBM truncated: need {need} data bytes, got {}",
                data.len()
            )));
        }
        bmp.data.copy_from_slice(&data[..need]);
        bmp.normalize_padding();
    } else {
        let mut bits = data
            .iter()
            .filter(|c| !c.is_ascii_whitespace())
            .map(|&c| match c {
                b'0' => Ok(false),
                b'1' => Ok(true),
                other => Err(CanonError::Raster(format!(
                    "P1 data: unexpected byte 0x{other:02X}"
                ))),
            });
        for y in 0..h.height {
            for x in 0..h.width {
                match bits.next() {
                    Some(bit) => bmp.set(x, y, bit?),
                    None => {
                        return Err(CanonError::Raster(format!(
                            "P1 truncated at pixel ({x},{y})"
                        )))
                    }
                }
            }
        }
    }
    Ok(bmp)
}

/// Every image of a multi-image P4 stream (the PBM format allows images to follow each
/// other; Ghostscript's pbmraw device writes one per PDF page this way).
pub fn parse_all(input: &[u8]) -> Result<Vec<Bitmap>> {
    let mut pages = Vec::new();
    let mut rest = input;
    loop {
        while rest.first().is_some_and(u8::is_ascii_whitespace) {
            rest = &rest[1..];
        }
        if rest.is_empty() {
            break;
        }
        let h = parse_header(rest)?;
        if h.magic[1] != b'4' {
            // P1 has no reliable end marker: only a single image is supported
            if !pages.is_empty() {
                return Err(CanonError::Raster(
                    "P1 images cannot follow other images".into(),
                ));
            }
            return Ok(vec![parse(rest)?]);
        }
        let bmp = parse(rest)?;
        rest = &rest[h.data_start + bmp.stride * bmp.height..];
        pages.push(bmp);
    }
    if pages.is_empty() {
        return Err(CanonError::Raster("no image in PBM data".into()));
    }
    Ok(pages)
}

/// Binary P4 with a minimal header ("P4\n<w> <h>\n"), no comments: deterministic bytes.
pub fn write_p4(bmp: &Bitmap) -> Vec<u8> {
    let mut out = format!("P4\n{} {}\n", bmp.width, bmp.height).into_bytes();
    out.extend_from_slice(&bmp.data);
    out
}

pub fn read_file(path: &std::path::Path) -> Result<Bitmap> {
    let bytes = std::fs::read(path)
        .map_err(|e| CanonError::Input(format!("cannot read {}: {e}", path.display())))?;
    parse(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p4_roundtrip() {
        let mut b = Bitmap::new(10, 3);
        b.set(0, 0, true);
        b.set(9, 2, true);
        let bytes = write_p4(&b);
        assert_eq!(&bytes[..9], b"P4\n10 3\n\x80");
        assert_eq!(parse(&bytes).unwrap(), b);
    }

    #[test]
    fn p4_padding_bits_are_cleared() {
        let bytes = b"P4\n3 1\n\xFF".to_vec();
        let b = parse(&bytes).unwrap();
        assert_eq!(b.data, vec![0xE0]);
    }

    #[test]
    fn comments_and_whitespace() {
        let bytes = b"P4 # comment\n# another\n 8\t2\n\xAA\x55".to_vec();
        let b = parse(&bytes).unwrap();
        assert_eq!((b.width, b.height), (8, 2));
        assert_eq!(b.data, vec![0xAA, 0x55]);
    }

    #[test]
    fn p1_ascii() {
        let b = parse(b"P1\n3 2\n1 0 1\n0 1 0\n").unwrap();
        assert_eq!(b.data, vec![0b1010_0000, 0b0100_0000]);
    }

    #[test]
    fn multi_image_stream() {
        let mut a = Bitmap::new(8, 1);
        a.data[0] = 0x81;
        let b = Bitmap::new(16, 2);
        let mut bytes = write_p4(&a);
        bytes.extend(write_p4(&b));
        bytes.push(b'\n');
        let pages = parse_all(&bytes).unwrap();
        assert_eq!(pages, vec![a.clone(), b]);
        assert_eq!(parse_all(&write_p4(&a)).unwrap(), vec![a]);
        assert!(parse_all(b"  ").is_err());
        let mut truncated = write_p4(&Bitmap::new(8, 1));
        truncated.extend(b"P4\n8 4\n\x00");
        assert!(parse_all(&truncated).is_err());
    }

    #[test]
    fn errors_do_not_panic() {
        for bad in [
            &b""[..],
            b"P5\n1 1\n\x00",
            b"P4\n",
            b"P4\n0 5\n",
            b"P4\n8 2\n\x00",
            b"P4\n99999 1\n",
            b"P4\n8 1",
            b"P1\n2 1\n1",
            b"P1\n1 1\n2\n",
        ] {
            assert!(parse(bad).is_err(), "should reject {bad:?}");
        }
    }
}
