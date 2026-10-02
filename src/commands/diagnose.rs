//! `canon-print diagnose`: environment + USB checks with PASS/WARN/FAIL/SKIP findings.
//! Diagnose only: it never unloads modules, detaches drivers or changes permissions.

use super::Outcome;
use crate::env::{self, Environment};
use crate::error::{EXIT_ENVIRONMENT, EXIT_OK};
use crate::usb::{self, DeviceInfo};
use serde::Serialize;
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Level {
    Pass,
    Warn,
    Fail,
    Skip,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub level: Level,
    pub check: String,
    pub detail: String,
}

fn f(level: Level, check: &str, detail: impl Into<String>) -> Finding {
    Finding {
        level,
        check: check.to_string(),
        detail: detail.into(),
    }
}

#[derive(Debug, Serialize)]
pub struct Diagnosis {
    pub environment: Environment,
    pub usb_devices_total: usize,
    pub printers: Vec<DeviceInfo>,
    pub findings: Vec<Finding>,
    pub hardware_available: bool,
}

/// Pure evaluation of the collected facts (unit tested without hardware).
pub fn evaluate(environment: Environment, devices: Result<Vec<DeviceInfo>, String>) -> Diagnosis {
    let mut findings = Vec::new();
    if environment.os == "linux" {
        findings.push(f(
            Level::Pass,
            "Linux detected",
            format!(
                "{} / kernel {} / {} / environment={}",
                environment
                    .distribution
                    .as_deref()
                    .unwrap_or("unknown distribution"),
                environment.kernel.as_deref().unwrap_or("?"),
                environment.arch,
                environment.environment
            ),
        ));
    } else {
        findings.push(f(
            Level::Fail,
            "Linux detected",
            format!("os = {}", environment.os),
        ));
    }
    findings.push(f(
        Level::Pass,
        "libusb detected",
        format!("libusb {}", environment.libusb),
    ));
    if environment.wsl {
        findings.push(f(
            Level::Warn,
            "WSL",
            "running under WSL: USB only via usbipd; results are not equivalent to native/VMware",
        ));
    }

    let devices = match devices {
        Ok(d) => {
            findings.push(f(
                Level::Pass,
                "USB enumeration",
                format!("{} device(s)", d.len()),
            ));
            d
        }
        Err(e) => {
            findings.push(f(Level::Fail, "USB enumeration", e));
            Vec::new()
        }
    };
    let total = devices.len();
    let printers: Vec<DeviceInfo> = devices.into_iter().filter(|d| d.is_candidate).collect();

    if printers.is_empty() {
        findings.push(f(
            Level::Skip,
            "Canon LBP printer",
            "no Canon LBP device found (VID 04a9) — hardware tests will be SKIPPED",
        ));
    }
    for p in &printers {
        let id = format!("{}:{} bus {:03} dev {:03}", p.vid, p.pid, p.bus, p.address);
        match p.known_model {
            Some(m) => findings.push(f(
                Level::Pass,
                "Canon LBP printer",
                format!("Canon {m} ({id})"),
            )),
            None => findings.push(f(
                Level::Warn,
                "Canon LBP printer",
                format!(
                    "unknown product id ({id}, product {:?}) — not a verified revision",
                    p.product
                ),
            )),
        }
        if p.permission.writable {
            findings.push(f(
                Level::Pass,
                "USB permission",
                format!("{} is writable", p.permission.device_node),
            ));
        } else {
            findings.push(f(
                Level::Fail,
                "USB permission",
                format!(
                    "{} is not writable by this user — install the udev rule (README 'Linux permissions')",
                    p.permission.device_node
                ),
            ));
        }
        let (ep_in, ep_out) = p.bulk_endpoints();
        match (ep_in, ep_out) {
            (Some(i), Some(o)) => findings.push(f(
                Level::Pass,
                "Bulk endpoints",
                format!(
                    "IN {} (max {} B), OUT {} (max {} B)",
                    i.address, i.max_packet_size, o.address, o.max_packet_size
                ),
            )),
            _ => findings.push(f(
                Level::Fail,
                "Bulk endpoints",
                "no bulk IN/OUT pair found",
            )),
        }
        let drivers = p.kernel_drivers();
        if drivers.is_empty() {
            findings.push(f(
                Level::Pass,
                "Kernel driver",
                "interface not claimed by a kernel driver",
            ));
        } else {
            findings.push(f(
                Level::Warn,
                "Kernel driver",
                format!(
                    "interface claimed by {} — canon-print must detach it or you unload it manually \
                     (sudo modprobe -r usblp); this tool never does it automatically",
                    drivers.join(", ")
                ),
            ));
        }
    }

    Diagnosis {
        environment,
        usb_devices_total: total,
        hardware_available: !printers.is_empty(),
        printers,
        findings,
    }
}

pub fn run() -> Outcome {
    let environment = env::detect();
    let devices = usb::enumerate().map_err(|e| e.to_string());
    let usb_failed = devices.is_err();
    let d = evaluate(environment, devices);

    let mut text = String::from("Environment:\n");
    let e = &d.environment;
    text += &format!(
        "  OS: {}\n  Kernel: {}\n  Arch: {}\n  Environment: {}\n  WSL: {}\n  Container: {}\n  libusb: {}\n\n",
        e.distribution.as_deref().unwrap_or(&e.os),
        e.kernel.as_deref().unwrap_or("?"),
        e.arch,
        e.environment,
        e.wsl,
        e.container,
        e.libusb
    );
    text += "Checks:\n";
    for fi in &d.findings {
        let tag = match fi.level {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
            Level::Skip => "SKIP",
        };
        text += &format!("  [{tag}] {}: {}\n", fi.check, fi.detail);
    }
    text += &format!(
        "\nHARDWARE_AVAILABLE = {}\n",
        if d.hardware_available { "YES" } else { "NO" }
    );

    Outcome {
        json: json!(d),
        text,
        exit_code: if usb_failed {
            EXIT_ENVIRONMENT
        } else {
            EXIT_OK
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usb::{ConfigInfo, EndpointInfo, InterfaceInfo, Permission};

    fn linux_env() -> Environment {
        Environment {
            os: "linux".into(),
            distribution: Some("Ubuntu 24.04".into()),
            kernel: Some("6.8.0".into()),
            arch: "x86_64".into(),
            environment: "vmware".into(),
            wsl: false,
            container: false,
            vm_vendor: Some("vmware".into()),
            libusb: "1.0.27.0".into(),
        }
    }

    fn lbp2900(writable: bool, driver: Option<&str>) -> DeviceInfo {
        DeviceInfo {
            bus: 2,
            address: 3,
            vid: "04a9".into(),
            pid: "2676".into(),
            manufacturer: Some("Canon".into()),
            product: Some("LBP2900".into()),
            serial: None,
            known_model: Some("LBP2900"),
            is_candidate: true,
            configs: vec![ConfigInfo {
                number: 1,
                interfaces: vec![InterfaceInfo {
                    number: 0,
                    alt_setting: 0,
                    class: 7,
                    subclass: 1,
                    protocol: 2,
                    endpoints: vec![
                        EndpointInfo {
                            address: "0x01".into(),
                            direction: "out",
                            transfer_type: "bulk",
                            max_packet_size: 64,
                        },
                        EndpointInfo {
                            address: "0x82".into(),
                            direction: "in",
                            transfer_type: "bulk",
                            max_packet_size: 64,
                        },
                    ],
                    kernel_driver: driver.map(str::to_string),
                }],
            }],
            permission: Permission {
                device_node: "/dev/bus/usb/002/003".into(),
                readable: true,
                writable,
            },
            open_error: None,
            vid_raw: 0x04a9,
            pid_raw: 0x2676,
        }
    }

    fn level_of(d: &Diagnosis, check: &str) -> Level {
        d.findings.iter().find(|f| f.check == check).unwrap().level
    }

    #[test]
    fn no_printer_means_skip_not_pass() {
        let d = evaluate(linux_env(), Ok(vec![]));
        assert!(!d.hardware_available);
        assert_eq!(level_of(&d, "Canon LBP printer"), Level::Skip);
    }

    #[test]
    fn printer_with_usblp_warns() {
        let d = evaluate(linux_env(), Ok(vec![lbp2900(true, Some("usblp"))]));
        assert!(d.hardware_available);
        assert_eq!(level_of(&d, "Canon LBP printer"), Level::Pass);
        assert_eq!(level_of(&d, "USB permission"), Level::Pass);
        assert_eq!(level_of(&d, "Bulk endpoints"), Level::Pass);
        assert_eq!(level_of(&d, "Kernel driver"), Level::Warn);
    }

    #[test]
    fn missing_permission_fails() {
        let d = evaluate(linux_env(), Ok(vec![lbp2900(false, None)]));
        assert_eq!(level_of(&d, "USB permission"), Level::Fail);
        assert_eq!(level_of(&d, "Kernel driver"), Level::Pass);
    }

    #[test]
    fn enumeration_error_is_reported() {
        let d = evaluate(linux_env(), Err("LIBUSB_ERROR_OTHER".into()));
        assert_eq!(level_of(&d, "USB enumeration"), Level::Fail);
        assert!(!d.hardware_available);
    }
}
