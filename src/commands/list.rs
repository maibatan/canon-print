//! `canon-print list`: USB devices (Canon LBP candidates by default, `--all` for everything).

use super::Outcome;
use crate::error::{CanonError, EXIT_OK};
use crate::usb;
use serde_json::json;

pub fn run(all: bool) -> Result<Outcome, CanonError> {
    let devices = usb::enumerate()?;
    let shown: Vec<_> = devices.iter().filter(|d| all || d.is_candidate).collect();
    let mut text = String::new();
    if shown.is_empty() {
        text += "No Canon LBP printer found (use --all to list every USB device).\n";
    }
    for d in &shown {
        text += &format!(
            "Bus {:03} Device {:03}: ID {}:{} {} {}{}\n",
            d.bus,
            d.address,
            d.vid,
            d.pid,
            d.manufacturer.as_deref().unwrap_or("?"),
            d.product.as_deref().unwrap_or("?"),
            if d.is_candidate {
                "  <- Canon LBP candidate"
            } else {
                ""
            }
        );
    }
    Ok(Outcome {
        json: json!({ "devices": shown, "candidates": devices.iter().filter(|d| d.is_candidate).count() }),
        text,
        exit_code: EXIT_OK,
    })
}
