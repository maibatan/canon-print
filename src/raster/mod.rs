//! Monochrome raster pages.
//!
//! Convention (same as PBM P4 and the CUPS K colour space used by captdriver):
//! bit value 1 = black, most significant bit = leftmost pixel, every row padded
//! to a whole byte. Padding bits are always kept at 0 so that identical images
//! always have identical bytes (determinism).

pub mod halftone;
pub mod pbm;
pub mod pdf;
pub mod pgm;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    /// Bytes per row = ceil(width / 8).
    pub stride: usize,
    pub data: Vec<u8>,
}

impl Bitmap {
    /// All-white bitmap.
    pub fn new(width: usize, height: usize) -> Bitmap {
        let stride = width.div_ceil(8);
        Bitmap {
            width,
            height,
            stride,
            data: vec![0; stride * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> bool {
        assert!(x < self.width && y < self.height, "pixel out of range");
        self.data[y * self.stride + x / 8] & (0x80 >> (x % 8)) != 0
    }

    pub fn set(&mut self, x: usize, y: usize, black: bool) {
        assert!(x < self.width && y < self.height, "pixel out of range");
        let byte = &mut self.data[y * self.stride + x / 8];
        let mask = 0x80u8 >> (x % 8);
        if black {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }

    /// Set a pixel if it is inside the bitmap; ignore it otherwise.
    pub fn set_clipped(&mut self, x: i64, y: i64, black: bool) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            self.set(x as usize, y as usize, black);
        }
    }

    pub fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.stride..(y + 1) * self.stride]
    }

    /// Mask of the valid bits in the last byte of a row (padding bits are 0).
    pub fn last_byte_mask(&self) -> u8 {
        match self.width % 8 {
            0 => 0xFF,
            r => 0xFFu8 << (8 - r),
        }
    }

    /// Clear padding bits (keeps the "padding is always 0" invariant).
    pub fn normalize_padding(&mut self) {
        if self.width % 8 == 0 || self.stride == 0 {
            return;
        }
        let mask = self.last_byte_mask();
        for y in 0..self.height {
            let i = y * self.stride + self.stride - 1;
            self.data[i] &= mask;
        }
    }

    pub fn black_pixel_count(&self) -> usize {
        self.data.iter().map(|b| b.count_ones() as usize).sum()
    }

    /// Fit into a `line_size` × `lines` page raster exactly like captdriver's
    /// rastertocapt.c (compress_page_data + center_pixels, 1 bpp): centred horizontally
    /// in whole bytes (narrower rows padded, wider rows cut equally on both sides),
    /// top-aligned vertically (missing lines white, extra lines dropped).
    pub fn to_page_raster(&self, line_size: usize, lines: usize) -> Vec<u8> {
        let mut out = vec![0u8; line_size * lines];
        let (dst_shift, src_shift, copy) = if self.stride < line_size {
            ((line_size - self.stride) / 2, 0, self.stride)
        } else {
            (0, (self.stride - line_size) / 2, line_size)
        };
        for y in 0..lines.min(self.height) {
            let src = &self.row(y)[src_shift..src_shift + copy];
            let d = y * line_size + dst_shift;
            out[d..d + copy].copy_from_slice(src);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_rounds_up() {
        assert_eq!(Bitmap::new(1, 1).stride, 1);
        assert_eq!(Bitmap::new(8, 1).stride, 1);
        assert_eq!(Bitmap::new(9, 1).stride, 2);
    }

    #[test]
    fn set_get_msb_first() {
        let mut b = Bitmap::new(16, 2);
        b.set(0, 0, true);
        b.set(9, 1, true);
        assert_eq!(b.data, vec![0x80, 0x00, 0x00, 0x40]);
        assert!(b.get(0, 0) && b.get(9, 1) && !b.get(1, 0));
        b.set(0, 0, false);
        assert_eq!(b.data[0], 0);
    }

    #[test]
    fn page_raster_pads_like_captdriver() {
        // CUPS A4 raster on our VM: 591 bytes/line into line_size 595 -> 2 bytes left pad
        let mut b = Bitmap::new(591 * 8, 2);
        b.data[0] = 0xAB;
        let p = b.to_page_raster(595, 3);
        assert_eq!(p.len(), 595 * 3);
        assert_eq!(&p[..3], &[0, 0, 0xAB]);
        assert!(p[595 * 2..].iter().all(|&x| x == 0)); // missing line is white
    }

    #[test]
    fn page_raster_cuts_centre() {
        let mut b = Bitmap::new(6 * 8, 1);
        b.data.copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(b.to_page_raster(2, 1), vec![3, 4]); // (6-2)/2 = 2 bytes cut on the left
        assert_eq!(b.to_page_raster(6, 1), b.data);
    }

    #[test]
    fn padding_is_normalized() {
        let mut b = Bitmap::new(3, 1);
        b.data[0] = 0xFF;
        b.normalize_padding();
        assert_eq!(b.data[0], 0xE0);
        assert_eq!(b.black_pixel_count(), 3);
    }
}
