//! Committed fixtures must equal what the generator produces today (determinism),
//! and the golden files must still match their pinned hashes.

use canon_print::fixtures::SMALL;
use canon_print::golden_suite::{self, fixture_bytes, Status};
use std::path::Path;

#[test]
fn committed_small_fixtures_match_generator() {
    for spec in SMALL {
        let path = Path::new("tests/fixtures").join(format!("{}.pbm", spec.name));
        let committed = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} (run: cargo run --bin gen-fixtures -- --small tests/fixtures)",
                path.display()
            )
        });
        assert!(
            committed == fixture_bytes(spec),
            "{} differs from the generator output",
            path.display()
        );
    }
}

#[test]
fn generator_is_deterministic_across_calls() {
    for spec in SMALL {
        assert_eq!(fixture_bytes(spec), fixture_bytes(spec), "{}", spec.name);
    }
}

#[test]
fn golden_files_intact() {
    let r = golden_suite::check_integrity(Path::new("tests/fixtures/golden"));
    let failures: Vec<_> = r
        .cases
        .iter()
        .filter(|c| c.status != Status::Pass)
        .map(|c| format!("{}: {}", c.name, c.detail))
        .collect();
    assert!(
        failures.is_empty(),
        "golden integrity failures:\n{}",
        failures.join("\n")
    );
    assert_eq!(r.status, Status::Pass);
    assert!(r.cases.len() >= 19, "expected at least 19 golden entries");
}

#[test]
fn a4_golden_has_97_bands() {
    // 6776 lines / 70 = 97 bands; matches the 97 PRINT_DATA packets seen in the
    // CUPS debug log of a real LBP2900 A4 job (docs/protocol.md §7).
    let m = golden_suite::load_manifest(Path::new("tests/fixtures/golden")).unwrap();
    let a4 = m.entries.iter().find(|e| e.name == "a4_blank").unwrap();
    assert_eq!(a4.bands, 97);
}
