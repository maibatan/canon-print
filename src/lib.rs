//! canon-print: direct USB printing for Canon LBP2900 (CAPT 2.1 + Hi-SCoA) without CUPS.
//!
//! Protocol notes: docs/protocol.md. Test strategy: docs/testing.md.

pub mod capt;
pub mod cli;
pub mod commands;
pub mod compression;
pub mod env;
pub mod error;
pub mod fixtures;
pub mod golden;
pub mod golden_suite;
pub mod raster;
pub mod transport;
pub mod usb;
pub mod usb_transport;

/// SHA-256 as lowercase hex (used for fixture/golden pinning).
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
