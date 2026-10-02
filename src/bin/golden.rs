//! Developer tool for golden files.
//!
//!   golden manifest --dir tests/fixtures/golden --oracle-commit <sha>
//!   golden integrity [--json]      pinned hashes + file format
//!   golden check     [--json]      Rust encoder vs oracle, byte by byte
//!   golden roundtrip [--json]      decode(encode(x)) == x, determinism
//!
//! Exit: 0 when the suite status is PASS or SKIPPED, 1 on FAIL.
//! The status itself is in the JSON ("status") and on the last text line.

use canon_print::fixtures;
use canon_print::golden::BandStream;
use canon_print::golden_suite::{self, Entry, Manifest, Oracle, Status, SuiteResult};
use canon_print::sha256_hex;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
struct Args {
    #[arg(long, global = true, default_value = "tests/fixtures/golden")]
    dir: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Manifest {
        #[arg(long)]
        oracle_commit: String,
    },
    Integrity,
    Check,
    Roundtrip,
}

fn label(s: Status) -> &'static str {
    match s {
        Status::Pass => "PASS",
        Status::Fail => "FAIL",
        Status::Skipped => "SKIPPED",
    }
}

fn report(r: SuiteResult, json: bool) -> ! {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&r).expect("serializable")
        );
    } else {
        for c in &r.cases {
            println!(
                "[{}] {}: {}",
                label(c.status),
                c.name,
                c.detail.replace('\n', "\n    ")
            );
        }
        if let Some(reason) = &r.reason {
            println!("reason: {reason}");
        }
        println!("STATUS: {}", label(r.status));
    }
    std::process::exit(if r.status == Status::Fail { 1 } else { 0 });
}

fn main() {
    let args = Args::parse();
    match args.cmd {
        Cmd::Manifest { oracle_commit } => {
            let mut entries = Vec::new();
            for spec in fixtures::SMALL.iter().chain(fixtures::PAGES) {
                let golden = format!("{}.hiscoa", spec.name);
                let Ok(data) = std::fs::read(args.dir.join(&golden)) else {
                    eprintln!("warning: no golden for {}", spec.name);
                    continue;
                };
                let stream =
                    BandStream::from_bytes(&data).unwrap_or_else(|e| panic!("{golden}: {e}"));
                entries.push(Entry {
                    name: spec.name.to_string(),
                    width: spec.width,
                    height: spec.height,
                    fixture_sha256: sha256_hex(&golden_suite::fixture_bytes(spec)),
                    golden_sha256: sha256_hex(&data),
                    bands: stream.bands.len(),
                    golden,
                });
            }
            let manifest = Manifest {
                format: "HSC1".into(),
                oracle: Oracle {
                    source: "agalakhov/captdriver".into(),
                    commit: oracle_commit,
                    files: vec!["src/hiscoa-compress.c".into(), "src/hiscoa-common.c".into()],
                    params: "hiscoa_default_params".into(),
                    band_lines: 70,
                    eob: "HISCOA_EOB_NORMAL for every band (as generic-ops.c)".into(),
                },
                entries,
            };
            let text = serde_json::to_string_pretty(&manifest).expect("serializable") + "\n";
            std::fs::write(args.dir.join(golden_suite::MANIFEST), text).expect("write manifest");
            println!("wrote {} entries", manifest.entries.len());
        }
        Cmd::Integrity => report(golden_suite::check_integrity(&args.dir), args.json),
        Cmd::Check => report(golden_suite::check_golden(&args.dir), args.json),
        Cmd::Roundtrip => report(golden_suite::check_roundtrip(), args.json),
    }
}
