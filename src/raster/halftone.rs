//! 8-bit gray → 1 bit with Ghostscript's own default halftone for 600 dpi, reproduced
//! here so that canon-print prints the same on every Ghostscript version.
//!
//! Why not let Ghostscript halftone (`-sDEVICE=pbmraw`, as before): for 1-bit devices
//! below 800 dpi Ghostscript installs the transfer `{0.8 exp}` ("lighten up the grays",
//! gs_init.ps `.sethiresscreen`). Ghostscript ≥ 9.56 with the new PDF interpreter applies
//! that transfer twice on pages that contain transparency (images with a soft mask, as in
//! the hospital's signed reports), so photos printed ~0.13 coverage lighter on Ubuntu
//! 24.04 (gs 10.02) than on Ubuntu 22.04 (gs 9.55) and than CUPS/captdriver.
//! Ghostscript now renders 8-bit gray (`pgmraw`: no transfer, no halftone, on every
//! version) and [`gs_screen`] applies the transfer once and the same screen.
//!
//! Verified on two real reports (docs/testing.md): gs 9.55 `pgmraw` + [`gs_screen`]
//! differs from gs 9.55 `pbmraw` in 0.008 % of the dots, gs 10.02 `pgmraw` +
//! [`gs_screen`] in 0.012 %; gs 10.02 `pbmraw` (old pipeline) differed in 0.25 %.

use super::Bitmap;

/// Smallest 8-bit gray value (0 = black, 255 = white) at which each device pixel of the
/// repeating 8 × 8 tile is printed white, i.e. a dot is black when `gray < threshold`.
///
/// FACT: measured from a 256-level ramp rendered with Ghostscript 9.55.0 and 10.02.1
/// (`-sDEVICE=pbmraw -r600`, default transfer `{0.8 exp}`); both versions give exactly
/// this array. Screen: 106 lpi at 45°, two round dots per tile, 32 levels, anchored at
/// the top-left pixel of the raster. Row index = y % 8, column index = x % 8.
pub const GS_SCREEN_600: [[u8; 8]; 8] = [
    [58, 88, 120, 154, 163, 128, 96, 65],
    [73, 37, 25, 51, 145, 190, 208, 172],
    [104, 13, 4, 19, 112, 227, 246, 217],
    [137, 44, 8, 31, 80, 181, 236, 199],
    [163, 128, 96, 65, 58, 88, 120, 154],
    [145, 190, 208, 172, 73, 37, 25, 51],
    [112, 227, 246, 217, 104, 13, 4, 19],
    [80, 181, 236, 199, 137, 44, 8, 31],
];

/// Halftone a `width` × `height` 8-bit gray raster (one byte per pixel, rows packed).
pub fn gs_screen(gray: &[u8], width: usize, height: usize) -> Bitmap {
    assert_eq!(gray.len(), width * height, "gray raster size");
    let mut bmp = Bitmap::new(width, height);
    if width == 0 {
        return bmp;
    }
    for (y, (src, dst)) in gray
        .chunks_exact(width)
        .zip(bmp.data.chunks_exact_mut(bmp.stride))
        .enumerate()
    {
        let thr = &GS_SCREEN_600[y % 8];
        // x % 8 == bit position inside the output byte, so each byte uses the row once
        for (byte, px) in dst.iter_mut().zip(src.chunks(8)) {
            let mut b = 0u8;
            for (i, &v) in px.iter().enumerate() {
                if v < thr[i] {
                    b |= 0x80 >> i;
                }
            }
            *byte = b;
        }
    }
    bmp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcription_is_a_two_dot_threshold_array() {
        // 32 levels, each used by exactly two pixels (one per dot), rows 4..8 = rows 0..4
        // shifted by half a tile
        let mut values: Vec<u8> = GS_SCREEN_600.iter().flatten().copied().collect();
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 32);
        for y in 0..4 {
            for x in 0..8 {
                assert_eq!(GS_SCREEN_600[y][x], GS_SCREEN_600[y + 4][(x + 4) % 8]);
            }
        }
    }

    #[test]
    fn coverage_matches_ghostscript_ramp() {
        // FACT: black coverage of an 8x8-aligned flat patch rendered by gs 9.55 / 10.02
        // pbmraw at 600 dpi for gray k = 0, 32, ..., 224, 255.
        let measured = [
            1.000, 0.812, 0.688, 0.531, 0.406, 0.312, 0.188, 0.094, 0.000,
        ];
        for (i, &k) in [0u8, 32, 64, 96, 128, 160, 192, 224, 255]
            .iter()
            .enumerate()
        {
            let b = gs_screen(&[k; 64 * 64], 64, 64);
            let cov = b.black_pixel_count() as f64 / (64.0 * 64.0);
            assert!(
                (cov - measured[i]).abs() < 0.001,
                "gray {k}: {cov} vs {}",
                measured[i]
            );
        }
    }

    #[test]
    fn solid_black_and_white_stay_solid() {
        assert_eq!(gs_screen(&[0; 37 * 5], 37, 5).black_pixel_count(), 37 * 5);
        assert_eq!(gs_screen(&[255; 37 * 5], 37, 5).black_pixel_count(), 0);
    }

    #[test]
    fn padding_bits_stay_white_and_phase_is_top_left() {
        let b = gs_screen(&[0; 13 * 2], 13, 2);
        assert_eq!(b.data[1] & 0x07, 0, "padding must stay 0");
        // pixel (2,2) has the lowest threshold (4): black only below 4
        let mut g = vec![255u8; 8 * 8];
        g[2 * 8 + 2] = 3;
        assert!(gs_screen(&g, 8, 8).get(2, 2));
        g[2 * 8 + 2] = 4;
        assert!(!gs_screen(&g, 8, 8).get(2, 2));
    }

    #[test]
    fn darker_gray_never_has_fewer_black_dots() {
        let mut prev = usize::MAX;
        for k in 0..=255u8 {
            let n = gs_screen(&[k; 64], 8, 8).black_pixel_count();
            assert!(n <= prev, "gray {k}");
            prev = n;
        }
    }
}
