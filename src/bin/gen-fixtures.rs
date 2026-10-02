//! Write deterministic PBM fixtures.
//!
//!   gen-fixtures --small tests/fixtures --pages target/fixtures/pages --hardware tests/hardware-fixtures

use canon_print::fixtures::{FixtureSpec, HARDWARE, PAGES, SMALL};
use canon_print::golden_suite::fixture_bytes;
use clap::Parser;
use std::path::{Path, PathBuf};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    small: Option<PathBuf>,
    #[arg(long)]
    pages: Option<PathBuf>,
    #[arg(long)]
    hardware: Option<PathBuf>,
}

fn write_all(dir: &Path, specs: &[FixtureSpec]) {
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create {}: {e}", dir.display()));
    for s in specs {
        let path = dir.join(format!("{}.pbm", s.name));
        std::fs::write(&path, fixture_bytes(s))
            .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!("{}", path.display());
    }
}

fn main() {
    let args = Args::parse();
    if let Some(d) = args.small {
        write_all(&d, SMALL);
    }
    if let Some(d) = args.pages {
        write_all(&d, PAGES);
    }
    if let Some(d) = args.hardware {
        write_all(&d, HARDWARE);
    }
}
