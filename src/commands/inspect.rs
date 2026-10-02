//! `canon-print inspect`: full descriptor dump of every Canon LBP candidate.
//! Exit 0 when a printer is found, 2 (USB) when none is attached.

use super::Outcome;
use crate::error::{CanonError, EXIT_OK, EXIT_USB};
use crate::usb;
use serde_json::json;

pub fn run() -> Result<Outcome, CanonError> {
    let printers: Vec<_> = usb::enumerate()?
        .into_iter()
        .filter(|d| d.is_candidate)
        .collect();
    let mut text = String::new();
    if printers.is_empty() {
        text += "HARDWARE_AVAILABLE = NO (no Canon LBP device on USB)\n";
    }
    for p in &printers {
        text += &format!(
            "Canon printer: {}:{}  model: {}\n",
            p.vid,
            p.pid,
            p.known_model.unwrap_or("UNKNOWN revision (not verified)")
        );
        text += &format!(
            "  manufacturer: {}\n  product: {}\n  serial: {}\n",
            p.manufacturer.as_deref().unwrap_or("?"),
            p.product.as_deref().unwrap_or("?"),
            p.serial.as_deref().unwrap_or("?")
        );
        if let Some(e) = &p.open_error {
            text += &format!("  (strings unavailable: {e})\n");
        }
        text += &format!(
            "  device node: {} readable={} writable={}\n",
            p.permission.device_node, p.permission.readable, p.permission.writable
        );
        for c in &p.configs {
            text += &format!("  configuration {}\n", c.number);
            for i in &c.interfaces {
                text += &format!(
                    "    interface {} alt {} class {:02x}/{:02x}/{:02x} kernel driver: {}\n",
                    i.number,
                    i.alt_setting,
                    i.class,
                    i.subclass,
                    i.protocol,
                    i.kernel_driver.as_deref().unwrap_or("none")
                );
                for e in &i.endpoints {
                    text += &format!(
                        "      endpoint {} {} {} max packet {}\n",
                        e.address, e.direction, e.transfer_type, e.max_packet_size
                    );
                }
            }
        }
        text += "HARDWARE_AVAILABLE = YES\n";
    }
    let found = !printers.is_empty();
    Ok(Outcome {
        json: json!({ "hardware_available": found, "printers": printers }),
        text,
        exit_code: if found { EXIT_OK } else { EXIT_USB },
    })
}
