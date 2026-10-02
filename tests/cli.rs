//! End-to-end CLI behaviour: exit codes and machine-readable JSON. No printer needed,
//! and nothing here ever opens USB for writing.

use serde_json::Value;
use std::process::Command;

fn run(args: &[&str]) -> (i32, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_canon-print"))
        .args(args)
        .output()
        .expect("run canon-print");
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (out.status.code().expect("exit code"), json)
}

#[test]
fn print_missing_file_is_input_error() {
    let (code, j) = run(&["print", "tests/fixtures/does-not-exist.pbm", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(j["success"], false);
    assert_eq!(j["stage"], "input");
    assert_eq!(j["error"], "input");
    assert_eq!(j["stages"]["input"], "FAIL");
}

#[test]
fn dry_run_encodes_without_usb() {
    let (code, j) = run(&[
        "print",
        "tests/fixtures/text_test_600x600.pbm",
        "--json",
        "--dry-run",
    ]);
    assert_eq!(code, 0);
    assert_eq!(j["success"], true);
    assert_eq!(j["stages"]["input"], "PASS");
    assert_eq!(j["stages"]["encode"], "PASS");
    // fitted into the A4 page geometry captdriver uses on our unit
    assert_eq!(j["job"]["paper"], "A4");
    assert_eq!(j["job"]["line_size"], 595);
    assert_eq!(j["job"]["lines"], 6780);
    assert_eq!(j["job"]["bands_per_page"], 97);
    assert_eq!(j["job"]["sheets"], 1);
    assert!(j["job"]["compressed_bytes_per_page"][0].as_u64().unwrap() > 0);
    assert_eq!(j["physical_page"], "NOT VERIFIED");
}

#[test]
fn dry_run_a5_and_bad_paper() {
    let (code, j) = run(&[
        "print",
        "tests/fixtures/black_8x8.pbm",
        "--json",
        "--dry-run",
        "--paper",
        "a5",
    ]);
    assert_eq!(code, 0);
    assert_eq!(j["job"]["line_size"], 420);
    assert_eq!(j["job"]["bands_per_page"], 68); // 4722 / 70 rounded up
    let (code, j) = run(&[
        "print",
        "tests/fixtures/black_8x8.pbm",
        "--json",
        "--paper",
        "b4",
    ]);
    assert_eq!(code, 1);
    assert_eq!(j["stage"], "input");
}

#[test]
fn real_print_without_printer_stops_before_usb_io() {
    // This machine has no LBP2900 attached (hardware tests are opt-in via
    // scripts/agent-test.sh --hardware); if one is attached, skip rather than print.
    let (_, probe) = run(&["inspect", "--json"]);
    if probe["hardware_available"] == true {
        return;
    }
    let (code, j) = run(&["print", "tests/fixtures/black_8x8.pbm", "--json"]);
    assert!(code == 2 || code == 5, "unexpected exit {code}: {j}");
    assert_eq!(j["success"], false);
    assert_eq!(j["stages"]["encode"], "PASS");
    assert_eq!(j["physical_page"], "NOT VERIFIED");
}

#[test]
fn print_rejects_non_pbm() {
    let (code, j) = run(&["print", "Cargo.toml", "--json"]);
    assert_eq!(code, 1);
    assert_eq!(j["stage"], "input");
    assert_eq!(j["error"], "raster");
}

#[test]
fn diagnose_json_has_findings() {
    let (code, j) = run(&["diagnose", "--json"]);
    // 0 = diagnosed; 5 = USB enumeration impossible here (e.g. container without usbfs)
    assert!(code == 0 || code == 5, "unexpected exit {code}");
    assert!(j["findings"].as_array().is_some_and(|f| !f.is_empty()));
    assert!(j["hardware_available"].is_boolean());
    assert!(j["environment"]["environment"].is_string());
}

#[test]
fn status_without_printer_fails_cleanly() {
    let (code, j) = run(&["status", "--json"]);
    if j["success"] == true {
        return; // a real printer is attached to this machine
    }
    // 2 = no printer (USB), 5 = environment (no USB stack / permission / usblp)
    assert!(code == 2 || code == 5, "unexpected exit {code}: {j}");
    assert!(j["stage"].is_string());
    assert!(j["message"].as_str().is_some_and(|m| !m.is_empty()));
}

#[test]
fn dry_run_pdf_with_copies() {
    if !std::process::Command::new("gs")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("SKIPPED: Ghostscript not installed");
        return;
    }
    let (code, j) = run(&[
        "print",
        "tests/fixtures/two_pages.pdf",
        "--json",
        "--dry-run",
        "--copies",
        "2",
    ]);
    assert_eq!(code, 0, "{j}");
    assert_eq!(j["job"]["document_pages"], 2);
    assert_eq!(j["job"]["sheets"], 4);
    assert_eq!(j["job"]["input_size"], serde_json::json!([4722, 6780]));
}

#[test]
fn copies_out_of_range_is_input_error() {
    let (code, j) = run(&[
        "print",
        "tests/fixtures/black_8x8.pbm",
        "--json",
        "--dry-run",
        "--copies",
        "0",
    ]);
    assert_eq!(code, 1);
    assert_eq!(j["stage"], "input");
}
