//! Golden (reference) and round-trip test suites, shared by the `golden` binary,
//! `cargo test` and scripts/agent-test.sh.
//!
//! Golden files are produced by the captdriver C code (tools/oracle, scripts/gen-golden.sh)
//! and pinned by SHA-256 in `MANIFEST.json`. Status values are PASS / FAIL / SKIPPED;
//! SKIPPED is never reported as PASS.

use crate::compression::hiscoa::{self, BAND_LINES, DEFAULT_PARAMS};
use crate::error::CanonError;
use crate::fixtures::{self, FixtureSpec};
use crate::golden::{self, BandStream};
use crate::sha256_hex;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MANIFEST: &str = "MANIFEST.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Oracle {
    pub source: String,
    pub commit: String,
    pub files: Vec<String>,
    pub params: String,
    pub band_lines: usize,
    pub eob: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub fixture_sha256: String,
    pub golden: String,
    pub golden_sha256: String,
    pub bands: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub oracle: Oracle,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Status {
    Pass,
    Fail,
    Skipped,
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub status: Status,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct SuiteResult {
    pub suite: String,
    pub status: Status,
    pub reason: Option<String>,
    pub cases: Vec<CaseResult>,
}

fn summarize(suite: &str, cases: Vec<CaseResult>, reason: Option<String>) -> SuiteResult {
    let status = if cases.iter().any(|c| c.status == Status::Fail) {
        Status::Fail
    } else if cases.is_empty() || cases.iter().any(|c| c.status == Status::Skipped) {
        Status::Skipped
    } else {
        Status::Pass
    };
    SuiteResult {
        suite: suite.to_string(),
        status,
        reason,
        cases,
    }
}

fn case(name: &str, status: Status, detail: impl Into<String>) -> CaseResult {
    CaseResult {
        name: name.to_string(),
        status,
        detail: detail.into(),
    }
}

pub fn fixture_bytes(spec: &FixtureSpec) -> Vec<u8> {
    crate::raster::pbm::write_p4(&spec.generate())
}

pub fn load_manifest(dir: &Path) -> Result<Manifest, CanonError> {
    let path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CanonError::Input(format!("cannot read {}: {e}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|e| CanonError::Input(format!("bad {}: {e}", path.display())))
}

/// Integrity only: fixtures regenerate to the pinned hash, golden files are unmodified
/// and well-formed. Does not need the Rust encoder.
pub fn check_integrity(dir: &Path) -> SuiteResult {
    let manifest = match load_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            return summarize(
                "golden-integrity",
                vec![case("manifest", Status::Fail, e.to_string())],
                None,
            )
        }
    };
    let mut cases = Vec::new();
    for entry in &manifest.entries {
        cases.push(
            check_entry_integrity(dir, entry)
                .unwrap_or_else(|why| case(&entry.name, Status::Fail, why)),
        );
    }
    summarize("golden-integrity", cases, None)
}

fn check_entry_integrity(dir: &Path, entry: &Entry) -> Result<CaseResult, String> {
    let spec = fixtures::find(&entry.name).ok_or("fixture no longer exists in src/fixtures.rs")?;
    let bmp = spec.generate();
    let got = sha256_hex(&crate::raster::pbm::write_p4(&bmp));
    if got != entry.fixture_sha256 {
        return Err(format!(
            "fixture generator output changed (sha256 {got}, manifest {}): regenerate goldens",
            entry.fixture_sha256
        ));
    }
    let data =
        std::fs::read(dir.join(&entry.golden)).map_err(|e| format!("cannot read golden: {e}"))?;
    let gsha = sha256_hex(&data);
    if gsha != entry.golden_sha256 {
        return Err(format!(
            "golden file modified (sha256 {gsha}, manifest {})",
            entry.golden_sha256
        ));
    }
    let stream = BandStream::from_bytes(&data).map_err(|e| e.to_string())?;
    if stream.line_size as usize != bmp.stride || stream.lines as usize != bmp.height {
        return Err(format!(
            "golden geometry {}x{} does not match fixture {}x{}",
            stream.line_size, stream.lines, bmp.stride, bmp.height
        ));
    }
    let expected_bands = bmp.height.div_ceil(BAND_LINES);
    if stream.bands.len() != expected_bands || stream.band_lines as usize != BAND_LINES {
        return Err(format!(
            "golden has {} bands, expected {expected_bands}",
            stream.bands.len()
        ));
    }
    Ok(case(
        &entry.name,
        Status::Pass,
        format!("{} bands, {} bytes", stream.bands.len(), data.len()),
    ))
}

/// The Rust decoder must turn the oracle's (C-encoded) bands back into the fixture.
fn decode_oracle(stream: &BandStream, raster: &[u8], line_size: usize) -> Result<(), String> {
    let mut decoded = Vec::with_capacity(raster.len());
    for (i, band) in stream.bands.iter().enumerate() {
        let d = hiscoa::decode_band(band, line_size, &DEFAULT_PARAMS)
            .map_err(|e| format!("band {i}: {e}"))?;
        if d.eob != Some(0) || !d.fully_consumed(band) {
            return Err(format!(
                "band {i}: eob {:?}, consumed {}/{}",
                d.eob,
                d.consumed,
                band.len()
            ));
        }
        decoded.extend_from_slice(&d.data);
    }
    match golden::diff(raster, &decoded, &[]) {
        None => Ok(()),
        Some(d) => Err(d.to_string()),
    }
}

/// Byte-for-byte comparison of the Rust encoder against the oracle golden files,
/// plus the Rust decoder run on the oracle files.
pub fn check_golden(dir: &Path) -> SuiteResult {
    let manifest = match load_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            return summarize(
                "golden",
                vec![case("manifest", Status::Fail, e.to_string())],
                None,
            )
        }
    };
    let mut cases = Vec::new();
    let mut reason = None;
    for entry in &manifest.entries {
        if let Err(why) = check_entry_integrity(dir, entry) {
            cases.push(case(&entry.name, Status::Fail, why));
            continue;
        }
        let spec = fixtures::find(&entry.name).expect("checked above");
        let bmp = spec.generate();
        let expected = std::fs::read(dir.join(&entry.golden)).expect("checked above");
        let expected_stream = BandStream::from_bytes(&expected).expect("checked above");
        match hiscoa::encode_page(&bmp.data, bmp.stride, bmp.height, &DEFAULT_PARAMS) {
            Err(CanonError::NotImplemented(what)) => {
                reason = Some(format!(
                    "{what} — golden files verified, comparison skipped"
                ));
                cases.push(case(
                    &entry.name,
                    Status::Skipped,
                    "encoder not implemented",
                ));
            }
            Err(e) => cases.push(case(
                &entry.name,
                Status::Fail,
                format!("encoder error: {e}"),
            )),
            Ok(bands) => {
                let actual = BandStream {
                    line_size: bmp.stride as u32,
                    lines: bmp.height as u32,
                    band_lines: BAND_LINES as u32,
                    bands,
                }
                .to_bytes();
                match golden::diff(&expected, &actual, &expected_stream.regions()) {
                    Some(d) => cases.push(case(
                        &entry.name,
                        Status::Fail,
                        format!("encoder differs from oracle: {d}"),
                    )),
                    None => match decode_oracle(&expected_stream, &bmp.data, bmp.stride) {
                        Err(why) => cases.push(case(
                            &entry.name,
                            Status::Fail,
                            format!("encoder identical, but decoding the oracle file failed: {why}"),
                        )),
                        Ok(()) => cases.push(case(
                            &entry.name,
                            Status::Pass,
                            format!(
                                "encoder: {} bytes identical to oracle; decoder: oracle file decodes to the fixture",
                                expected.len()
                            ),
                        )),
                    },
                }
            }
        }
    }
    summarize("golden", cases, reason)
}

/// decode(encode(x)) == x and encode(x) == encode(x) for every fixture.
pub fn check_roundtrip() -> SuiteResult {
    let mut cases = Vec::new();
    let mut reason = None;
    for spec in fixtures::SMALL.iter().chain(fixtures::PAGES) {
        let bmp = spec.generate();
        let first = hiscoa::encode_page(&bmp.data, bmp.stride, bmp.height, &DEFAULT_PARAMS);
        let bands = match first {
            Err(CanonError::NotImplemented(what)) => {
                reason = Some(what);
                cases.push(case(
                    spec.name,
                    Status::Skipped,
                    "encoder/decoder not implemented",
                ));
                continue;
            }
            Err(e) => {
                cases.push(case(spec.name, Status::Fail, format!("encode failed: {e}")));
                continue;
            }
            Ok(b) => b,
        };
        match hiscoa::encode_page(&bmp.data, bmp.stride, bmp.height, &DEFAULT_PARAMS) {
            Ok(again) if again == bands => {}
            _ => {
                cases.push(case(
                    spec.name,
                    Status::Fail,
                    "non-deterministic: second encoding differs",
                ));
                continue;
            }
        }
        let mut decoded = Vec::with_capacity(bmp.data.len());
        let mut failure = None;
        for (i, band) in bands.iter().enumerate() {
            match hiscoa::decode_band(band, bmp.stride, &DEFAULT_PARAMS) {
                Ok(d) if d.eob != Some(0) => {
                    failure = Some(format!("band {i}: END type {:?}, expected Some(0)", d.eob));
                    break;
                }
                Ok(d) if !d.fully_consumed(band) => {
                    failure = Some(format!(
                        "band {i}: only {} of {} bytes consumed",
                        d.consumed,
                        band.len()
                    ));
                    break;
                }
                Ok(d) => decoded.extend_from_slice(&d.data),
                Err(e) => {
                    failure = Some(format!("band {i}: decode failed: {e}"));
                    break;
                }
            }
        }
        let result = match failure {
            Some(f) => case(spec.name, Status::Fail, f),
            None => match golden::diff(&bmp.data, &decoded, &[]) {
                None => case(spec.name, Status::Pass, format!("{} bytes", bmp.data.len())),
                Some(d) => case(spec.name, Status::Fail, d.to_string()),
            },
        };
        cases.push(result);
    }
    summarize("compression-roundtrip", cases, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_never_turns_skipped_into_pass() {
        let s = summarize(
            "x",
            vec![case("a", Status::Pass, ""), case("b", Status::Skipped, "")],
            None,
        );
        assert_eq!(s.status, Status::Skipped);
        let s = summarize(
            "x",
            vec![case("a", Status::Skipped, ""), case("b", Status::Fail, "")],
            None,
        );
        assert_eq!(s.status, Status::Fail);
        assert_eq!(summarize("x", vec![], None).status, Status::Skipped);
        assert_eq!(
            summarize("x", vec![case("a", Status::Pass, "")], None).status,
            Status::Pass
        );
    }
}
