//! Deterministic test images. The same name always produces the same bytes on
//! every machine (no system RNG, no floating point), so fixture files and golden
//! files can be pinned by SHA-256 in tests/fixtures/golden/MANIFEST.json.

use crate::raster::Bitmap;

/// A4 page raster as captdriver sends it on our unit (capt::lbp2900, FACT from the VM):
/// line_size 595 bytes = 4760 dots, 6780 lines.
pub const A4_WIDTH: usize = 4760;
pub const A4_HEIGHT: usize = 6780;
/// A5 page raster: line_size 420 bytes = 3360 dots, 4722 lines.
pub const A5_WIDTH: usize = 3360;
pub const A5_HEIGHT: usize = 4722;
/// What CUPS/Ghostscript renders for A4 with the LBP2900 PPD (cupsWidth 4722, 591
/// bytes/line); hardware fixtures use it so the captdriver-style centring is exercised.
pub const A4_CUPS_WIDTH: usize = 4722;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pattern {
    Blank,
    Black,
    /// 0xAA / 0x55 alternating rows (every other pixel black, shifted per row)
    Alternating,
    SinglePixel,
    HorizontalLine,
    VerticalLine,
    Checkerboard(usize),
    Random(u64),
    Sparse(u64),
    RepeatedRows,
    RepeatedBlocks,
    Text,
    ImageLike,
    /// Border, 1-px lines, checkerboard patch and "TEST" — for hardware alignment checks.
    TestPage,
}

#[derive(Clone, Copy, Debug)]
pub struct FixtureSpec {
    pub name: &'static str,
    pub width: usize,
    pub height: usize,
    pub pattern: Pattern,
}

impl FixtureSpec {
    pub fn generate(&self) -> Bitmap {
        generate(self.pattern, self.width, self.height)
    }
}

const fn spec(name: &'static str, width: usize, height: usize, pattern: Pattern) -> FixtureSpec {
    FixtureSpec {
        name,
        width,
        height,
        pattern,
    }
}

/// Small fixtures, committed as tests/fixtures/<name>.pbm.
pub const SMALL: &[FixtureSpec] = &[
    spec("blank_1x1", 1, 1, Pattern::Blank),
    spec("black_1x1", 1, 1, Pattern::Black),
    spec("blank_8x8", 8, 8, Pattern::Blank),
    spec("black_8x8", 8, 8, Pattern::Black),
    spec("alternating_16x16", 16, 16, Pattern::Alternating),
    spec("single_pixel_64x64", 64, 64, Pattern::SinglePixel),
    spec("horizontal_line_64x64", 64, 64, Pattern::HorizontalLine),
    spec("vertical_line_64x64", 64, 64, Pattern::VerticalLine),
    spec("checkerboard_64x64", 64, 64, Pattern::Checkerboard(1)),
    spec("random_64x64", 64, 64, Pattern::Random(0x2676)),
    spec("odd_width_61x75", 61, 75, Pattern::Random(61)),
    spec("sparse_600x600", 600, 600, Pattern::Sparse(7)),
    spec("repeated_rows_600x600", 600, 600, Pattern::RepeatedRows),
    spec("repeated_blocks_600x600", 600, 600, Pattern::RepeatedBlocks),
    spec("checkerboard_600x600", 600, 600, Pattern::Checkerboard(8)),
    spec("text_test_600x600", 600, 600, Pattern::Text),
    spec("image_like_600x600", 600, 600, Pattern::ImageLike),
];

/// Page-sized fixtures (~4 MB each): generated on demand, not committed.
pub const PAGES: &[FixtureSpec] = &[
    spec("a4_blank", A4_WIDTH, A4_HEIGHT, Pattern::Blank),
    spec("a4_test_page", A4_WIDTH, A4_HEIGHT, Pattern::TestPage),
    spec("a5_test_page", A5_WIDTH, A5_HEIGHT, Pattern::TestPage),
];

/// Hardware fixtures (A4, 600 dpi, 1 page each), written to tests/hardware-fixtures/.
pub const HARDWARE: &[FixtureSpec] = &[
    spec("test_1page", A4_CUPS_WIDTH, A4_HEIGHT, Pattern::TestPage),
    spec(
        "test_single_line",
        A4_CUPS_WIDTH,
        A4_HEIGHT,
        Pattern::HorizontalLine,
    ),
    spec(
        "test_checkerboard",
        A4_CUPS_WIDTH,
        A4_HEIGHT,
        Pattern::Checkerboard(16),
    ),
    spec("test_text", A4_CUPS_WIDTH, A4_HEIGHT, Pattern::Text),
];

pub fn find(name: &str) -> Option<FixtureSpec> {
    SMALL
        .iter()
        .chain(PAGES)
        .chain(HARDWARE)
        .find(|s| s.name == name)
        .copied()
}

/// xorshift64* — deterministic, platform independent.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

pub fn generate(pattern: Pattern, width: usize, height: usize) -> Bitmap {
    let mut b = Bitmap::new(width, height);
    match pattern {
        Pattern::Blank => {}
        Pattern::Black => b.data.fill(0xFF),
        Pattern::Alternating => {
            for y in 0..height {
                let v = if y % 2 == 0 { 0xAA } else { 0x55 };
                b.data[y * b.stride..(y + 1) * b.stride].fill(v);
            }
        }
        Pattern::SinglePixel => b.set(width / 2, height / 2, true),
        Pattern::HorizontalLine => {
            let y = height / 2;
            for x in 0..width {
                b.set(x, y, true);
            }
        }
        Pattern::VerticalLine => {
            let x = width / 2;
            for y in 0..height {
                b.set(x, y, true);
            }
        }
        Pattern::Checkerboard(cell) => {
            let cell = cell.max(1);
            for y in 0..height {
                for x in 0..width {
                    if (x / cell + y / cell) % 2 == 0 {
                        b.set(x, y, true);
                    }
                }
            }
        }
        Pattern::Random(seed) => {
            let mut rng = Rng::new(seed);
            for byte in b.data.iter_mut() {
                *byte = rng.next() as u8;
            }
        }
        Pattern::Sparse(seed) => {
            // ~0.5% black pixels, like scattered specks on a mostly white page
            let mut rng = Rng::new(seed);
            let count = (width * height) / 200;
            for _ in 0..count {
                let x = (rng.next() % width as u64) as usize;
                let y = (rng.next() % height as u64) as usize;
                b.set(x, y, true);
            }
        }
        Pattern::RepeatedRows => {
            // one random row repeated: exercises "copy from line above"
            let mut rng = Rng::new(42);
            let row: Vec<u8> = (0..b.stride).map(|_| rng.next() as u8).collect();
            for y in 0..height {
                b.data[y * b.stride..(y + 1) * b.stride].copy_from_slice(&row);
            }
        }
        Pattern::RepeatedBlocks => {
            // a random 8-byte block repeated along each row: exercises short-offset repeats
            let mut rng = Rng::new(43);
            let block: Vec<u8> = (0..8).map(|_| rng.next() as u8).collect();
            for (i, byte) in b.data.iter_mut().enumerate() {
                *byte = block[(i % b.stride) % block.len()];
            }
        }
        Pattern::Text => {
            let scale = (width / 120).max(1);
            let mut y = scale * 4;
            let lines = ["TEST 0123456789", "CANON LBP2900", "CAPT HISCOA"];
            while y + 8 * scale < height {
                for line in lines {
                    if y + 8 * scale >= height {
                        break;
                    }
                    draw_text(&mut b, scale * 2, y, scale, line);
                    y += 10 * scale;
                }
            }
        }
        Pattern::ImageLike => {
            // ordered-dither (Bayer 4x4) of a radial gradient: typical halftoned photo data
            const BAYER: [[u32; 4]; 4] =
                [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
            let (cx, cy) = (width as i64 / 2, height as i64 / 2);
            let max = (cx * cx + cy * cy).max(1) as u64;
            for y in 0..height {
                for x in 0..width {
                    let dx = x as i64 - cx;
                    let dy = y as i64 - cy;
                    let d2 = (dx * dx + dy * dy) as u64;
                    let level = (16 * d2 / max) as u32; // 0..16
                    if level > BAYER[y % 4][x % 4] {
                        b.set(x, y, true);
                    }
                }
            }
        }
        Pattern::TestPage => draw_test_page(&mut b),
    }
    b.normalize_padding();
    b
}

fn fill_rect(b: &mut Bitmap, x0: usize, y0: usize, w: usize, h: usize) {
    for y in y0..(y0 + h).min(b.height) {
        for x in x0..(x0 + w).min(b.width) {
            b.set(x, y, true);
        }
    }
}

fn draw_test_page(b: &mut Bitmap) {
    let (w, h) = (b.width, b.height);
    let margin = (w / 40).max(1);
    let t = (w / 1000).max(2); // border thickness
                               // border
    fill_rect(b, margin, margin, w.saturating_sub(2 * margin), t);
    fill_rect(
        b,
        margin,
        h.saturating_sub(margin + t),
        w.saturating_sub(2 * margin),
        t,
    );
    fill_rect(b, margin, margin, t, h.saturating_sub(2 * margin));
    fill_rect(
        b,
        w.saturating_sub(margin + t),
        margin,
        t,
        h.saturating_sub(2 * margin),
    );
    // 1-pixel lines through the first third (bit order / offset check)
    for x in margin..w.saturating_sub(margin) {
        b.set(x, h / 3, true);
    }
    for y in margin..h.saturating_sub(margin) {
        b.set(w / 3, y, true);
    }
    // checkerboard patch, bottom right quarter
    let cell = (w / 150).max(1);
    let (px, py, size) = (w * 5 / 8, h * 5 / 8, w / 4);
    for y in py..(py + size).min(h) {
        for x in px..(px + size).min(w) {
            if ((x - px) / cell + (y - py) / cell) % 2 == 0 {
                b.set(x, y, true);
            }
        }
    }
    // "TEST" in the middle
    let scale = (w / 60).max(1);
    let text_w = 4 * 6 * scale;
    draw_text(
        b,
        (w.saturating_sub(text_w)) / 2,
        h / 2 - 4 * scale,
        scale,
        "TEST",
    );
}

/// 5x7 glyphs, rows top to bottom, 5 low bits per row (bit 4 = leftmost).
fn glyph(c: char) -> [u8; 7] {
    match c {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        _ => [0; 7],
    }
}

pub fn draw_text(b: &mut Bitmap, x0: usize, y0: usize, scale: usize, text: &str) {
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        let gx = x0 + i * 6 * scale;
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) != 0 {
                    fill_rect(b, gx + col * scale, y0 + row * scale, scale, scale);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_findable() {
        let all: Vec<_> = SMALL.iter().chain(PAGES).chain(HARDWARE).collect();
        for s in &all {
            assert_eq!(
                all.iter().filter(|o| o.name == s.name).count(),
                1,
                "{}",
                s.name
            );
            assert!(find(s.name).is_some());
        }
    }

    #[test]
    fn generation_is_deterministic() {
        for s in SMALL {
            assert_eq!(s.generate(), s.generate(), "{}", s.name);
        }
    }

    #[test]
    fn patterns_have_expected_content() {
        assert_eq!(find("blank_8x8").unwrap().generate().black_pixel_count(), 0);
        assert_eq!(
            find("black_8x8").unwrap().generate().black_pixel_count(),
            64
        );
        assert_eq!(find("black_1x1").unwrap().generate().data, vec![0x80]);
        assert_eq!(
            find("single_pixel_64x64")
                .unwrap()
                .generate()
                .black_pixel_count(),
            1
        );
        assert_eq!(
            find("horizontal_line_64x64")
                .unwrap()
                .generate()
                .black_pixel_count(),
            64
        );
        assert_eq!(
            find("checkerboard_64x64")
                .unwrap()
                .generate()
                .black_pixel_count(),
            2048
        );
        let text = find("text_test_600x600").unwrap().generate();
        assert!(text.black_pixel_count() > 1000);
    }

    #[test]
    fn padding_stays_zero_for_odd_width() {
        let b = find("odd_width_61x75").unwrap().generate();
        let mask = b.last_byte_mask();
        for y in 0..b.height {
            assert_eq!(b.row(y)[b.stride - 1] & !mask, 0);
        }
    }
}
