//! Hi-SCoA band encoder and decoder.
//!
//! Port of captdriver (GPL-3.0-or-later):
//! - encoder: `src/hiscoa-compress.c`, Copyright (C) 2013 Alexey Galakhov
//! - decoder: `tests/hiscoa-decompress.c`
//!
//! The encoder follows the C code routine by routine (same greedy match search, same
//! command selection order, same padding) so its output is byte-identical to
//! captdriver's; this is checked against golden files produced by the C code
//! (`golden check`, docs/testing.md). Format summary: docs/protocol.md §6.
//!
//! Deliberate differences from the C code, none of which change valid output:
//! - Output bits are collected in a `Vec` and XORed with 0x43 at the end instead of
//!   XOR-read-modify-write into a caller buffer (equivalent because every output byte
//!   is fully written: the stream always ends on a 32-bit boundary).
//! - Where C silently truncates at the output buffer size (2 × line_size × 70 in
//!   rastertocapt) this port returns an error. It cannot happen for real bands
//!   (worst case is 1.5 × input + 4 bytes).
//! - The decoder returns an error for malformed input (copy before the start of the
//!   band, output larger than the band) where the C test decoder writes 0xCC.

use crate::error::{CanonError, Result};

/// Lines per band, from the LBP2900 PPD (`cupsRowCount 70`). FACT (captdriver).
pub const BAND_LINES: usize = 70;

/// Every output byte is XORed with this value. FACT (hiscoa-compress.c:210).
const XOR: u8 = 0x43;

/// Longest match: C stops at `input_pos + 512 + 127` (hiscoa-compress.c:75).
const MAX_MATCH: usize = 512 + 127;

/// Hi-SCoA parameters, same fields as captdriver `struct hiscoa_params`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub origin_3: i32,
    pub origin_5: i32,
    pub origin_0: i32,
    pub origin_2: i32,
    pub origin_4: i32,
}

/// captdriver `hiscoa_default_params`. FACT (hiscoa-common.c:24-30).
pub const DEFAULT_PARAMS: Params = Params {
    origin_3: 1,
    origin_5: 4,
    origin_0: 0,
    origin_2: -7,
    origin_4: 0,
};

/// End-of-band type (captdriver `enum hiscoa_eob_type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eob {
    Normal = 0,
    Last = 1,
}

/// The six copy offsets. C stores them as `unsigned` computed with wrapping
/// arithmetic (`params->origin_2 + line_size`); a narrow band therefore gets a huge
/// offset that never matches, exactly like the C code. 0 disables a command.
fn origins(params: &Params, line_size: usize) -> [u32; 6] {
    let ls = line_size as u32;
    [
        (params.origin_0 as u32).wrapping_add(ls),
        0,
        (params.origin_2 as u32).wrapping_add(ls),
        params.origin_3 as u32,
        params.origin_4 as u32,
        params.origin_5 as u32,
    ]
}

/// D0A4 payload (captdriver `hiscoa_format_params`, hiscoa-compress.c:249-266).
pub fn format_params(p: &Params) -> [u8; 8] {
    let o4 = (p.origin_4 as i16 as u16).to_le_bytes();
    [
        p.origin_3 as i8 as u8,
        p.origin_5 as i8 as u8,
        0x01,
        0x01,
        p.origin_0 as i8 as u8,
        p.origin_2 as i8 as u8,
        o4[0],
        o4[1],
    ]
}

/// Number of significant bits (captdriver `find_msb`): 0 → 0, 1 → 1, 5 → 3.
fn find_msb(v: u32) -> u32 {
    32 - v.leading_zeros()
}

struct BitWriter {
    bytes: Vec<u8>,
    bitpos: usize,
}

impl BitWriter {
    /// Append the low `count` bits of `bits`, most significant first
    /// (same effect as captdriver `push_bits`).
    fn push(&mut self, bits: u32, count: u32) {
        for i in (0..count).rev() {
            if self.bitpos % 8 == 0 {
                self.bytes.push(0);
            }
            if (bits >> i) & 1 != 0 {
                let last = self.bytes.len() - 1;
                self.bytes[last] |= 0x80 >> (self.bitpos % 8);
            }
            self.bitpos += 1;
        }
    }
}

struct Encoder<'a> {
    input: &'a [u8],
    pos: usize,
    line_size: usize,
    out: BitWriter,
    nbytes: usize,
    stash: [u8; 16],
    origin: [u32; 6],
}

impl Encoder<'_> {
    /// captdriver `try_match`.
    fn try_match(&self, diff: u32) -> usize {
        let diff = diff as usize;
        let start = self.pos;
        if start < diff {
            return 0;
        }
        let mut pos = start;
        while self.input[pos] == self.input[pos - diff] {
            pos += 1;
            if pos >= self.input.len() || pos >= start + MAX_MATCH || pos % self.line_size == 0 {
                break;
            }
        }
        pos - start
    }

    /// captdriver `try_write_longrepeat`.
    fn try_write_longrepeat(&mut self) -> bool {
        let mut best_cmd = 0usize;
        let mut best_len = 0usize;
        for cmd in 0..6 {
            if self.origin[cmd] != 0 {
                let len = self.try_match(self.origin[cmd]);
                if len > best_len {
                    best_cmd = cmd;
                    best_len = len;
                }
            }
        }
        if best_len <= 1 {
            return false;
        }

        let mut len = best_len as u32;
        if len > 127 {
            let val = len / 128;
            let nbits = find_msb(val) - 1;
            self.out.push(0xFC, 8); // prefix
            self.out.push(nbits, 2);
            self.out.push(!val, nbits);
            len %= 128;
        }

        self.out.push(0xFFFF_FFFE, best_cmd as u32 + 1); // longrep<cmd>
        if best_cmd == 2 {
            self.out.push(0, 1); // subcommand 0
        }
        match len {
            0 => self.out.push(0x3F, 6),
            1 => self.out.push(0x0, 2),
            2 => self.out.push(0x3, 3),
            3 => self.out.push(0x2, 3),
            _ => {
                let nbits = find_msb(len) - 1;
                self.out.push(0xFFFF_FFFE, nbits);
                self.out.push(!len, nbits);
            }
        }
        if best_cmd == 2 {
            self.origin.swap(2, 0);
        }
        if best_cmd == 5 {
            self.origin.swap(5, 3);
        }
        self.pos += best_len;
        true
    }

    /// captdriver `try_write_byterepeat`.
    fn try_write_byterepeat(&mut self) -> bool {
        let byte = self.input[self.pos];
        for i in 0..self.nbytes {
            if self.stash[i] == byte {
                self.stash.copy_within(0..i, 1);
                self.stash[0] = byte;
                self.out.push(0x20 | ((15 - i as u32) & 0xF), 6);
                self.pos += 1;
                return true;
            }
        }
        false
    }

    /// captdriver `write_simple_byte`.
    fn write_simple_byte(&mut self) {
        let byte = self.input[self.pos];
        if self.nbytes < 16 {
            self.nbytes += 1;
        }
        self.stash.copy_within(0..self.nbytes - 1, 1);
        self.stash[0] = byte;
        if byte == 0 {
            self.out.push(0xFD, 8); // zero byte
        } else {
            self.out.push(0xD00 | byte as u32, 12); // byte
        }
        self.pos += 1;
    }
}

/// Encode one band (captdriver `hiscoa_compress_band`) with an explicit end type.
pub fn encode_band_eob(
    band: &[u8],
    line_size: usize,
    lines: usize,
    eob: Eob,
    params: &Params,
) -> Result<Vec<u8>> {
    if line_size == 0 {
        return Err(CanonError::Compression("line_size must be > 0".into()));
    }
    let size = line_size
        .checked_mul(lines)
        .ok_or_else(|| CanonError::Compression("band size overflow".into()))?;
    if band.len() < size {
        return Err(CanonError::Compression(format!(
            "band has {} bytes, expected {line_size} x {lines}",
            band.len()
        )));
    }
    let mut e = Encoder {
        input: &band[..size],
        pos: 0,
        line_size,
        out: BitWriter {
            bytes: Vec::with_capacity(size + size / 2 + 8),
            bitpos: 0,
        },
        nbytes: 0,
        stash: [0; 16],
        origin: origins(params, line_size),
    };
    while e.pos < size {
        if e.try_write_longrepeat() {
            continue;
        }
        if e.try_write_byterepeat() {
            continue;
        }
        e.write_simple_byte();
    }
    e.out.push(0xFE, 8); // end
    e.out.push(eob as u32, 2);
    // C always pads, even when already aligned (the `if` is commented out,
    // hiscoa-compress.c:242): an aligned stream gets 32 extra 1-bits.
    let pad = 32 - (e.out.bitpos % 32) as u32;
    e.out.push(0xFFFF_FFFF, pad);

    let limit = 2 * line_size * BAND_LINES;
    if e.out.bytes.len() > limit {
        return Err(CanonError::Compression(format!(
            "encoded band is {} bytes, larger than captdriver's {limit}-byte buffer",
            e.out.bytes.len()
        )));
    }
    let mut out = e.out.bytes;
    out.iter_mut().for_each(|b| *b ^= XOR);
    Ok(out)
}

/// Encode one band the way captdriver's LBP2900 path does (`HISCOA_EOB_NORMAL`).
pub fn encode_band(
    band: &[u8],
    line_size: usize,
    lines: usize,
    params: &Params,
) -> Result<Vec<u8>> {
    encode_band_eob(band, line_size, lines, Eob::Normal, params)
}

/// Split a page into 70-line bands exactly like captdriver's rastertocapt
/// (last band shorter) and encode each one.
pub fn encode_page(
    data: &[u8],
    line_size: usize,
    lines: usize,
    params: &Params,
) -> Result<Vec<Vec<u8>>> {
    if data.len() != line_size * lines {
        return Err(CanonError::Compression(format!(
            "raster is {} bytes, expected {line_size} x {lines}",
            data.len()
        )));
    }
    (0..lines)
        .step_by(BAND_LINES)
        .map(|start| {
            let n = BAND_LINES.min(lines - start);
            encode_band(
                &data[start * line_size..(start + n) * line_size],
                line_size,
                n,
                params,
            )
        })
        .collect()
}

// ─────────────────────────────── decoder ───────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedBand {
    pub data: Vec<u8>,
    /// Bytes consumed, including the padding up to the 32-bit boundary after END.
    pub consumed: usize,
    /// END type, `None` if the input ended without an END command.
    pub eob: Option<u8>,
}

impl DecodedBand {
    /// True when everything after `consumed` is the one extra padding word captdriver
    /// adds to an already aligned stream (`BC BC BC BC`), or nothing.
    pub fn fully_consumed(&self, input: &[u8]) -> bool {
        let rest = &input[self.consumed.min(input.len())..];
        rest.is_empty() || rest == [0xBC; 4]
    }
}

struct BitReader<'a> {
    input: &'a [u8],
    bitpos: usize,
}

impl BitReader<'_> {
    /// Like the C `read_bit`: past the end it returns 1 without advancing.
    fn bit(&mut self) -> bool {
        match self.input.get(self.bitpos / 8) {
            None => true,
            Some(b) => {
                let v = (b ^ XOR) & (0x80 >> (self.bitpos % 8)) != 0;
                self.bitpos += 1;
                v
            }
        }
    }
    fn ones(&mut self, limit: u32) -> u32 {
        let mut n = 0;
        while n < limit && self.bit() {
            n += 1;
        }
        n
    }
    fn number(&mut self, bits: u32) -> u32 {
        (0..bits).fold(0, |acc, _| (acc << 1) | self.bit() as u32)
    }
    fn long_number(&mut self) -> u32 {
        let order = self.ones(6);
        match order {
            6 => 0,
            0 => {
                if !self.bit() {
                    1
                } else if self.bit() {
                    2
                } else {
                    3
                }
            }
            _ => (1u32 << (order + 2)) - 1 - self.number(order + 1),
        }
    }
    fn prefix(&mut self) -> u32 {
        let nbits = self.number(2);
        let val = self.number(nbits);
        128 * ((1u32 << (nbits + 1)) - 1 - val)
    }
}

/// Decode one band (port of captdriver `tests/hiscoa-decompress.c`).
/// `max_output` bounds the result (a real band is at most line_size × 70 bytes).
pub fn decode_band_limited(
    data: &[u8],
    line_size: usize,
    params: &Params,
    max_output: usize,
) -> Result<DecodedBand> {
    let mut r = BitReader {
        input: data,
        bitpos: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    let mut stash: [u8; 16] = [
        0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE, 0xFF, 0xDE, 0xAD, 0xBE, 0xEF, 0xDE, 0xAD, 0xBE,
        0xFF,
    ];
    let o = origins(params, line_size);
    let (mut o0, mut o2, mut o3, o4, mut o5) = (o[0], o[2], o[3], o[4], o[5]);
    let mut prefix = 0u32;
    let mut eob = None;

    let copy = |out: &mut Vec<u8>, r: &mut BitReader, origin: u32, prefix: u32| -> Result<()> {
        let len = (prefix + r.long_number()) as usize;
        let origin = origin as usize;
        for _ in 0..len {
            if origin == 0 || out.len() < origin {
                return Err(CanonError::Compression(format!(
                    "copy from offset {origin} before the start of the band (output position {})",
                    out.len()
                )));
            }
            if out.len() >= max_output {
                return Err(CanonError::Compression(format!(
                    "decoded band exceeds {max_output} bytes"
                )));
            }
            out.push(out[out.len() - origin]);
        }
        Ok(())
    };
    let push = |out: &mut Vec<u8>, b: u8| -> Result<()> {
        if out.len() >= max_output {
            return Err(CanonError::Compression(format!(
                "decoded band exceeds {max_output} bytes"
            )));
        }
        out.push(b);
        Ok(())
    };

    while eob.is_none() && r.bitpos < 8 * data.len() {
        let mut keep_prefix = false;
        match r.ones(8) {
            0 => copy(&mut out, &mut r, o0, prefix)?,
            1 => {
                let n = 15 - r.number(4) as usize;
                let b = stash[n];
                stash.copy_within(0..n, 1);
                stash[0] = b;
                push(&mut out, b)?;
            }
            2 => {
                if r.bit() {
                    let b = r.number(8) as u8;
                    stash.copy_within(0..15, 1);
                    stash[0] = b;
                    push(&mut out, b)?;
                } else {
                    copy(&mut out, &mut r, o2, prefix)?;
                    std::mem::swap(&mut o2, &mut o0);
                }
            }
            3 => copy(&mut out, &mut r, o3, prefix)?,
            4 => copy(&mut out, &mut r, o4, prefix)?,
            5 => {
                copy(&mut out, &mut r, o5, prefix)?;
                std::mem::swap(&mut o5, &mut o3);
            }
            6 => {
                if r.bit() {
                    stash.copy_within(0..15, 1);
                    stash[0] = 0;
                    push(&mut out, 0)?;
                } else {
                    prefix = r.prefix();
                    keep_prefix = true;
                }
            }
            7 => eob = Some(r.number(2) as u8),
            _ => {} // 8: NOP
        }
        if !keep_prefix {
            prefix = 0;
        }
    }
    while r.bitpos % 32 != 0 && r.bitpos / 8 < data.len() {
        r.bit();
    }
    Ok(DecodedBand {
        data: out,
        consumed: (r.bitpos / 8).min(data.len()),
        eob,
    })
}

/// Decode one band of a page with the given line size (output limited to 70 lines).
pub fn decode_band(data: &[u8], line_size: usize, params: &Params) -> Result<DecodedBand> {
    decode_band_limited(data, line_size, params, line_size * BAND_LINES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_serialize_like_captdriver() {
        // FACT: D0A4 payload 01 04 01 01 00 F9 00 00 (docs/protocol.md §3)
        assert_eq!(
            format_params(&DEFAULT_PARAMS),
            [0x01, 0x04, 0x01, 0x01, 0x00, 0xF9, 0x00, 0x00]
        );
    }

    #[test]
    fn find_msb_matches_c() {
        assert_eq!(find_msb(0), 0);
        assert_eq!(find_msb(1), 1);
        assert_eq!(find_msb(4), 3);
        assert_eq!(find_msb(127), 7);
        assert_eq!(find_msb(u32::MAX), 32);
    }

    #[test]
    fn bitwriter_is_msb_first() {
        let mut w = BitWriter {
            bytes: vec![],
            bitpos: 0,
        };
        w.push(0b101, 3);
        w.push(0x1F, 5);
        assert_eq!(w.bytes, vec![0b1011_1111]);
        w.push(0xFFFF_FFFE, 2); // low 2 bits "10"
        assert_eq!(w.bytes[1], 0b1000_0000);
        assert_eq!(w.bitpos, 10);
    }

    #[test]
    fn output_is_32bit_aligned_and_ends_with_end_marker() {
        for (band, ls, n) in [
            (vec![0u8], 1, 1),
            (vec![0xFF; 64], 8, 8),
            (vec![0xAA; 600], 75, 8),
        ] {
            let enc = encode_band(&band, ls, n, &DEFAULT_PARAMS).unwrap();
            assert_eq!(enc.len() % 4, 0);
            let dec = decode_band(&enc, ls, &DEFAULT_PARAMS).unwrap();
            assert_eq!(dec.data, band);
            assert_eq!(dec.eob, Some(0));
            assert!(dec.fully_consumed(&enc));
        }
    }

    #[test]
    fn eob_last_is_encoded() {
        let enc = encode_band_eob(&[1, 2, 3, 4], 4, 1, Eob::Last, &DEFAULT_PARAMS).unwrap();
        assert_eq!(decode_band(&enc, 4, &DEFAULT_PARAMS).unwrap().eob, Some(1));
    }

    #[test]
    fn long_runs_use_prefix_and_roundtrip() {
        // runs > 127 and up to the 639 limit exercise PREFIX
        let line = 700;
        let band = vec![0x5A; line * 2];
        let enc = encode_band(&band, line, 2, &DEFAULT_PARAMS).unwrap();
        assert_eq!(decode_band(&enc, line, &DEFAULT_PARAMS).unwrap().data, band);
    }

    #[test]
    fn narrow_bands_do_not_panic() {
        // line_size < 7 makes origin_2 wrap (C unsigned arithmetic): command disabled
        for ls in 1..10 {
            let band: Vec<u8> = (0..ls * 5).map(|i| (i * 37) as u8).collect();
            let enc = encode_band(&band, ls, 5, &DEFAULT_PARAMS).unwrap();
            assert_eq!(decode_band(&enc, ls, &DEFAULT_PARAMS).unwrap().data, band);
        }
    }

    #[test]
    fn bad_inputs_return_errors() {
        assert!(encode_band(&[0; 3], 0, 1, &DEFAULT_PARAMS).is_err());
        assert!(encode_band(&[0; 3], 4, 1, &DEFAULT_PARAMS).is_err());
        // LONGREP0 as the very first command: copy before the start of the band
        let bogus = [XOR; 4]; // all-zero bits after the XOR
        assert!(decode_band(&bogus, 8, &DEFAULT_PARAMS).is_err());
        // garbage must not panic, whatever it decodes to
        for seed in 0u32..256 {
            let junk: Vec<u8> = (0..64u32).map(|i| (seed * 31 + i * 7) as u8).collect();
            let _ = decode_band(&junk, 16, &DEFAULT_PARAMS);
        }
    }

    #[test]
    fn output_limit_is_enforced() {
        let enc = encode_band(&[0x11; 800], 80, 10, &DEFAULT_PARAMS).unwrap();
        assert!(decode_band_limited(&enc, 80, &DEFAULT_PARAMS, 100).is_err());
    }
}
