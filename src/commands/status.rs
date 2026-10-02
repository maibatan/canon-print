//! `canon-print status`: talk to the printer without printing.
//!
//! Stages: detect → open (+ claim interface) → drain stale IN data → IDENT (A1A1)
//! → basic status (E0A0) → extended status (A0A8). No job is reserved (no A2A0),
//! nothing is printed. Every exchange has a timeout; with `--dump-protocol DIR`
//! every packet is saved.

use super::Outcome;
use crate::capt::session::{Dumper, Session};
use crate::capt::status::{flag, BasicStatus};
use crate::error::CanonError;
use crate::usb_transport::RusbTransport;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3);

fn status0_flags(s: &BasicStatus) -> Vec<&'static str> {
    [
        (flag::READY1, "READY1"),
        (flag::JOBSTAT_CHNG, "JOBSTAT_CHNG"),
        (flag::XSTATUS_CHNG, "XSTATUS_CHNG"),
        (flag::BUSY, "BUSY"),
        (flag::UNINIT1, "UNINIT1"),
        (flag::UNINIT2, "UNINIT2"),
        (flag::BUFFERFULL, "BUFFERFULL"),
        (flag::NOPAPER1, "NOPAPER1"),
        (flag::PROCESSING, "PROCESSING"),
    ]
    .iter()
    .filter(|(f, _)| s.has(*f))
    .map(|(_, n)| *n)
    .collect()
}

pub fn run(detach: bool, dump: Option<&Path>, verbose: bool) -> Outcome {
    let mut stages = Map::new();
    let mut result = Map::new();
    let fail = |stages: Map<String, Value>,
                result: Map<String, Value>,
                stage: &str,
                e: CanonError| Outcome {
        text: format!("FAIL at: {stage}\n{e}\n"),
        exit_code: e.exit_code(),
        json: json!({
            "success": false,
            "stage": stage,
            "error": e.kind(),
            "message": e.to_string(),
            "stages": stages,
            "result": result,
        }),
    };

    let dumper = match dump.map(Dumper::create).transpose() {
        Ok(d) => d,
        Err(e) => return fail(stages, result, "setup", e),
    };

    let transport = match RusbTransport::open(detach) {
        Ok(t) => {
            for s in ["detect", "open", "claim_interface"] {
                stages.insert(s.into(), json!("PASS"));
            }
            t
        }
        Err(e) => {
            let stage = match &e {
                CanonError::Usb(m) if m.contains("no Canon") => "detect",
                CanonError::Permission(m) if m.contains("claimed") || m.contains("busy") => {
                    "claim_interface"
                }
                _ => "open",
            };
            stages.insert(stage.into(), json!("FAIL"));
            return fail(stages, result, stage, e);
        }
    };
    let (iface, ep_in, ep_out) = transport.endpoints();
    result.insert(
        "printer".into(),
        json!({
            "vid": transport.info.vid,
            "pid": transport.info.pid,
            "product": transport.info.product,
            "interface": iface,
            "endpoint_in": format!("0x{ep_in:02x}"),
            "endpoint_out": format!("0x{ep_out:02x}"),
            "detached_kernel_driver": transport.detached_kernel_driver(),
        }),
    );

    let mut s = Session::new(transport, TIMEOUT)
        .with_dump(dumper)
        .verbose(verbose);

    match s.drain_stale(Duration::from_millis(100), 50) {
        Ok(n) => {
            stages.insert("drain".into(), json!("PASS"));
            result.insert("stale_bytes_drained".into(), json!(n));
        }
        Err(e) => {
            stages.insert("drain".into(), json!("FAIL"));
            return fail(stages, result, "drain", e);
        }
    }

    match s.identify() {
        Ok(info) => {
            stages.insert("identify".into(), json!("PASS"));
            result.insert("info".into(), json!(info));
        }
        Err(e) => {
            stages.insert("identify".into(), json!("FAIL"));
            return fail(stages, result, "identify", e);
        }
    }

    match s.basic_status() {
        Ok(b) => {
            stages.insert("status".into(), json!("PASS"));
            result.insert(
                "basic_status".into(),
                json!({
                    "status0": format!("0x{:04X}", b.status0),
                    "flags": status0_flags(&b),
                    "rest": crate::capt::packet::hex(&b.rest),
                }),
            );
        }
        Err(e) => {
            stages.insert("status".into(), json!("FAIL"));
            return fail(stages, result, "status", e);
        }
    }

    match s.extended_status() {
        Ok(x) => {
            stages.insert("extended_status".into(), json!("PASS"));
            result.insert(
                "extended_status".into(),
                json!({
                    "status": x.status.iter().map(|w| format!("0x{w:04X}")).collect::<Vec<_>>(),
                    "status6": x.status6.map(|w| format!("0x{w:04X}")),
                    "pages": x.pages,
                    "no_paper": x.no_paper(),
                    "printing": x.printing(),
                }),
            );
        }
        Err(e) => {
            stages.insert("extended_status".into(), json!("FAIL"));
            return fail(stages, result, "extended_status", e);
        }
    }

    let text = format!(
        "PASS: identify + status\n{}\n",
        serde_json::to_string_pretty(&Value::Object(result.clone())).expect("serializable")
    );
    Outcome {
        text,
        exit_code: 0,
        json: json!({ "success": true, "stages": stages, "result": result }),
    }
}
