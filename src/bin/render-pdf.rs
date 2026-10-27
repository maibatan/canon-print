//! Developer tool: render a PDF (or PBM) exactly like `canon-print print` does and write
//! the 1-bit pages as PBM files, to look at what the printer gets without printing.
//!
//!   render-pdf <input> [--paper a4|a5] [--out-prefix page] [--page-raster]
//!
//! Writes <prefix>-1.pbm, <prefix>-2.pbm, ... By default each file is the rendered page
//! (A4 4722 x 6780, same size as Ghostscript's own output); with --page-raster it is the
//! page raster actually sent to the printer (centred into line_size x num_lines).

use canon_print::capt::lbp2900::Paper;
use canon_print::raster::{pbm, pdf, Bitmap};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    input: PathBuf,
    #[arg(long, default_value = "a4")]
    paper: String,
    #[arg(long, default_value = "page")]
    out_prefix: String,
    #[arg(long)]
    page_raster: bool,
}

fn main() {
    let args = Args::parse();
    let fail = |e: canon_print::error::CanonError| -> ! {
        eprintln!("error: {e}");
        std::process::exit(e.exit_code())
    };
    let paper = Paper::parse(&args.paper).unwrap_or_else(|e| fail(e));
    let loaded = pdf::load_pages(&args.input, paper).unwrap_or_else(|e| fail(e));
    if let Some(w) = &loaded.warnings {
        eprintln!("Ghostscript warnings: {w}");
    }
    let pages = loaded.pages;
    let params = paper.page_params();
    for (i, page) in pages.iter().enumerate() {
        let out = if args.page_raster {
            let (ls, lines) = (params.line_size as usize, params.num_lines as usize);
            Bitmap {
                width: ls * 8,
                height: lines,
                stride: ls,
                data: page.to_page_raster(ls, lines),
            }
        } else {
            page.clone()
        };
        let path = format!("{}-{}.pbm", args.out_prefix, i + 1);
        std::fs::write(&path, pbm::write_p4(&out)).unwrap_or_else(|e| {
            eprintln!("cannot write {path}: {e}");
            std::process::exit(1)
        });
        println!(
            "{path} ({}x{}, {} black dots)",
            out.width,
            out.height,
            out.black_pixel_count()
        );
    }
}
