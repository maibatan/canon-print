//! Golden-file support: the HSC1 band container and a byte-level diff that says
//! exactly where two encodings diverge.
//!
//! HSC1 layout (all integers u32 little endian), see docs/testing.md:
//! ```text
//! "HSC1" | line_size | lines | band_lines | band_count | { band_len | band bytes } * band_count
//! ```
//! It stores the Hi-SCoA output of every band separately, exactly as captdriver
//! produces it (one `hiscoa_compress_band` call per band).

use crate::error::{CanonError, Result};
use std::fmt;

pub const MAGIC: &[u8; 4] = b"HSC1";
const HEADER_LEN: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BandStream {
    pub line_size: u32,
    pub lines: u32,
    pub band_lines: u32,
    pub bands: Vec<Vec<u8>>,
}

impl BandStream {
    pub fn to_bytes(&self) -> Vec<u8> {
        let total: usize = self.bands.iter().map(|b| b.len() + 4).sum();
        let mut out = Vec::with_capacity(HEADER_LEN + total);
        out.extend_from_slice(MAGIC);
        for v in [
            self.line_size,
            self.lines,
            self.band_lines,
            self.bands.len() as u32,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for band in &self.bands {
            out.extend_from_slice(&(band.len() as u32).to_le_bytes());
            out.extend_from_slice(band);
        }
        out
    }

    pub fn from_bytes(data: &[u8]) -> Result<BandStream> {
        let bad = |why: &str| CanonError::Input(format!("invalid HSC1 file: {why}"));
        if data.len() < HEADER_LEN || &data[..4] != MAGIC {
            return Err(bad("missing magic/header"));
        }
        let word = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().expect("4 bytes"));
        let (line_size, lines, band_lines, count) = (word(4), word(8), word(12), word(16));
        let mut pos = HEADER_LEN;
        let mut bands = Vec::with_capacity(count as usize);
        for i in 0..count {
            if pos + 4 > data.len() {
                return Err(bad(&format!("truncated before band {i} length")));
            }
            let len = word(pos) as usize;
            pos += 4;
            if pos + len > data.len() {
                return Err(bad(&format!("band {i} truncated")));
            }
            bands.push(data[pos..pos + len].to_vec());
            pos += len;
        }
        if pos != data.len() {
            return Err(bad("trailing bytes"));
        }
        Ok(BandStream {
            line_size,
            lines,
            band_lines,
            bands,
        })
    }

    /// Named regions of the serialized file, for locating a diff offset.
    pub fn regions(&self) -> Vec<Region> {
        let mut regions = vec![Region {
            name: "header".into(),
            start: 0,
            len: HEADER_LEN,
        }];
        let mut pos = HEADER_LEN;
        for (i, band) in self.bands.iter().enumerate() {
            regions.push(Region {
                name: format!("band {i} (length prefix + {} data bytes)", band.len()),
                start: pos,
                len: 4 + band.len(),
            });
            pos += 4 + band.len();
        }
        regions
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub name: String,
    pub start: usize,
    pub len: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteDiff {
    pub expected_len: usize,
    pub actual_len: usize,
    pub first_offset: usize,
    pub expected_byte: Option<u8>,
    pub actual_byte: Option<u8>,
    /// Differing positions within the common length plus the length difference.
    pub differing_bytes: usize,
    /// Region of the *expected* data containing the first difference, if known.
    pub region: Option<(String, usize)>,
    pub expected_context: Vec<u8>,
    pub actual_context: Vec<u8>,
    pub context_start: usize,
}

/// Compare two byte strings. `None` means identical.
pub fn diff(expected: &[u8], actual: &[u8], regions: &[Region]) -> Option<ByteDiff> {
    let common = expected.len().min(actual.len());
    let first = (0..common)
        .find(|&i| expected[i] != actual[i])
        .or((expected.len() != actual.len()).then_some(common))?;
    let differing = (0..common).filter(|&i| expected[i] != actual[i]).count()
        + expected.len().abs_diff(actual.len());
    let region = regions
        .iter()
        .find(|r| first >= r.start && first < r.start + r.len)
        .map(|r| (r.name.clone(), first - r.start));
    let context_start = first.saturating_sub(8);
    let ctx = |d: &[u8]| d[context_start.min(d.len())..(first + 8).min(d.len())].to_vec();
    Some(ByteDiff {
        expected_len: expected.len(),
        actual_len: actual.len(),
        first_offset: first,
        expected_byte: expected.get(first).copied(),
        actual_byte: actual.get(first).copied(),
        differing_bytes: differing,
        region,
        expected_context: ctx(expected),
        actual_context: ctx(actual),
        context_start,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn byte_or_eof(b: Option<u8>) -> String {
    b.map_or("<end of data>".to_string(), |v| format!("0x{v:02X}"))
}

impl fmt::Display for ByteDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "first difference at byte offset {} (0x{:X})",
            self.first_offset, self.first_offset
        )?;
        writeln!(f, "  expected byte: {}", byte_or_eof(self.expected_byte))?;
        writeln!(f, "  actual byte:   {}", byte_or_eof(self.actual_byte))?;
        if let Some((name, off)) = &self.region {
            writeln!(f, "  location:      {name}, offset {off} inside it")?;
        }
        writeln!(
            f,
            "  sizes:         expected {} bytes, actual {} bytes",
            self.expected_len, self.actual_len
        )?;
        writeln!(f, "  differing:     {} bytes", self.differing_bytes)?;
        writeln!(
            f,
            "  expected @{:#X}: {}",
            self.context_start,
            hex(&self.expected_context)
        )?;
        write!(
            f,
            "  actual   @{:#X}: {}",
            self.context_start,
            hex(&self.actual_context)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BandStream {
        BandStream {
            line_size: 8,
            lines: 3,
            band_lines: 70,
            bands: vec![vec![1, 2, 3], vec![]],
        }
    }

    #[test]
    fn container_roundtrip() {
        let s = sample();
        let bytes = s.to_bytes();
        assert_eq!(&bytes[..4], b"HSC1");
        assert_eq!(bytes.len(), 20 + 4 + 3 + 4);
        assert_eq!(BandStream::from_bytes(&bytes).unwrap(), s);
    }

    #[test]
    fn container_rejects_corruption() {
        let bytes = sample().to_bytes();
        assert!(BandStream::from_bytes(&bytes[..bytes.len() - 1]).is_err());
        assert!(BandStream::from_bytes(&[bytes.clone(), vec![0]].concat()).is_err());
        assert!(BandStream::from_bytes(b"XXXX").is_err());
        let mut huge = bytes.clone();
        huge[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(BandStream::from_bytes(&huge).is_err());
    }

    #[test]
    fn identical_is_none() {
        assert!(diff(b"abc", b"abc", &[]).is_none());
        assert!(diff(b"", b"", &[]).is_none());
    }

    #[test]
    fn reports_first_offset_bytes_and_region() {
        let s = sample();
        let expected = s.to_bytes();
        let mut actual = expected.clone();
        actual[25] ^= 0xFF; // band 0 data byte index 1
        actual[26] ^= 0x01;
        let d = diff(&expected, &actual, &s.regions()).unwrap();
        assert_eq!(d.first_offset, 25);
        assert_eq!(d.expected_byte, Some(2));
        assert_eq!(d.actual_byte, Some(0xFD));
        assert_eq!(d.differing_bytes, 2);
        assert_eq!(d.region.as_ref().unwrap().1, 5);
        assert!(d.region.as_ref().unwrap().0.starts_with("band 0"));
        let text = d.to_string();
        assert!(text.contains("offset 25") && text.contains("0x02") && text.contains("0xFD"));
    }

    #[test]
    fn length_mismatch_is_reported() {
        let d = diff(b"abcd", b"ab", &[]).unwrap();
        assert_eq!(d.first_offset, 2);
        assert_eq!(d.expected_byte, Some(b'c'));
        assert_eq!(d.actual_byte, None);
        assert_eq!(d.differing_bytes, 2);
        assert!(d.to_string().contains("<end of data>"));
    }
}
