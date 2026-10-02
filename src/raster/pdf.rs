//! PDF → 1-bit page rasters, rendered by Ghostscript at runtime (optional dependency:
//! only needed for PDF input; PBM input needs nothing).
//!
//! The raster size is the imageable area CUPS/Ghostscript produced for the LBP2900 PPD
//! on our VM (scripts/cups-raster-params.py: A4 4722 × 6780, A5 3264 × 4722 at 600 dpi,
//! 14 pt hardware margins), and pages are scaled to fit (`-dPDFFitPage`, like the
//! `fit-to-page` option HHH-Bridge passed to CUPS). The result is then centred into the
//! page line exactly like captdriver (Bitmap::to_page_raster).

use super::{pbm, Bitmap};
use crate::capt::lbp2900::Paper;
use crate::error::{CanonError, Result};
use std::path::Path;
use std::process::Command;

/// Rendered raster size per paper: (width, height) in dots at 600 dpi.
pub fn render_size(paper: Paper) -> (usize, usize) {
    match paper {
        Paper::A4 => (4722, 6780),
        Paper::A5 => (3264, 4722),
    }
}

pub fn is_pdf(data: &[u8]) -> bool {
    data.starts_with(b"%PDF")
}

/// Render every page of `path`.
pub fn render(path: &Path, paper: Paper) -> Result<Vec<Bitmap>> {
    let (w, h) = render_size(paper);
    let out = Command::new("gs")
        .args([
            "-q",
            "-dSAFER",
            "-dBATCH",
            "-dNOPAUSE",
            "-sDEVICE=pbmraw",
            "-r600",
            &format!("-g{w}x{h}"),
            "-dFIXEDMEDIA",
            "-dPDFFitPage",
            "-sOutputFile=-",
            "--",
        ])
        .arg(path)
        .output()
        .map_err(|e| {
            CanonError::Permission(format!(
                "cannot run Ghostscript ('gs') to render the PDF: {e} (sudo apt install ghostscript)"
            ))
        })?;
    if !out.status.success() {
        return Err(CanonError::Raster(format!(
            "Ghostscript failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let pages = pbm::parse_all(&out.stdout)?;
    for (i, p) in pages.iter().enumerate() {
        if (p.width, p.height) != (w, h) {
            return Err(CanonError::Raster(format!(
                "page {} rendered as {}x{}, expected {w}x{h}",
                i + 1,
                p.width,
                p.height
            )));
        }
    }
    Ok(pages)
}

/// Load an input file: PDF (via Ghostscript) or PBM (one or more P4 images).
pub fn load_pages(path: &Path, paper: Paper) -> Result<Vec<Bitmap>> {
    let head = std::fs::read(path)
        .map_err(|e| CanonError::Input(format!("cannot read {}: {e}", path.display())))?;
    if is_pdf(&head) {
        render(path, paper)
    } else {
        pbm::parse_all(&head)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gs_available() -> bool {
        Command::new("gs")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[test]
    fn renders_two_page_pdf_at_the_cups_size() {
        if !gs_available() {
            eprintln!("SKIPPED: Ghostscript not installed");
            return;
        }
        let pages = render(Path::new("tests/fixtures/two_pages.pdf"), Paper::A4).unwrap();
        assert_eq!(pages.len(), 2);
        for p in &pages {
            assert_eq!((p.width, p.height), (4722, 6780));
            assert!(
                p.black_pixel_count() > 1000,
                "text must render black pixels"
            );
        }
        let a5 = render(Path::new("tests/fixtures/two_pages.pdf"), Paper::A5).unwrap();
        assert_eq!((a5[0].width, a5[0].height), (3264, 4722));
    }

    #[test]
    fn detects_pdf_and_loads_pbm() {
        assert!(is_pdf(b"%PDF-1.7\n"));
        assert!(!is_pdf(b"P4\n1 1\n\x00"));
        let pages = load_pages(Path::new("tests/fixtures/black_8x8.pbm"), Paper::A4).unwrap();
        assert_eq!(pages.len(), 1);
    }
}
