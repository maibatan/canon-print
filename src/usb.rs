//! USB enumeration and descriptor inspection (read-only: never claims an
//! interface, never detaches a kernel driver, never sends data).

use crate::error::Result;
use rusb::UsbContext;
use serde::Serialize;

pub const CANON_VID: u16 = 0x04a9;

/// Known CAPT models by product id. FACT: 0x2676 = LBP2900 (lsusb on our unit,
/// studycapt usb_a1a1.c:79). Other entries are not needed yet.
pub const KNOWN_MODELS: &[(u16, &str)] = &[(0x2676, "LBP2900")];

#[derive(Clone, Debug, Serialize)]
pub struct EndpointInfo {
    pub address: String,
    pub direction: &'static str,
    pub transfer_type: &'static str,
    pub max_packet_size: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct InterfaceInfo {
    pub number: u8,
    pub alt_setting: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<EndpointInfo>,
    /// Kernel driver bound to this interface according to sysfs (e.g. "usblp").
    pub kernel_driver: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigInfo {
    pub number: u8,
    pub interfaces: Vec<InterfaceInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Permission {
    pub device_node: String,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    pub bus: u8,
    pub address: u8,
    pub vid: String,
    pub pid: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub known_model: Option<&'static str>,
    /// Canon device that looks like an LBP printer (known PID or "LBP" in product string).
    pub is_candidate: bool,
    pub configs: Vec<ConfigInfo>,
    pub permission: Permission,
    /// Why strings could not be read (usually missing permission).
    pub open_error: Option<String>,
    #[serde(skip)]
    pub vid_raw: u16,
    #[serde(skip)]
    pub pid_raw: u16,
}

impl DeviceInfo {
    pub fn bulk_endpoints(&self) -> (Option<&EndpointInfo>, Option<&EndpointInfo>) {
        let eps = self
            .configs
            .iter()
            .flat_map(|c| &c.interfaces)
            .flat_map(|i| &i.endpoints)
            .filter(|e| e.transfer_type == "bulk");
        let mut input = None;
        let mut output = None;
        for e in eps {
            if e.direction == "in" && input.is_none() {
                input = Some(e);
            } else if e.direction == "out" && output.is_none() {
                output = Some(e);
            }
        }
        (input, output)
    }

    pub fn kernel_drivers(&self) -> Vec<String> {
        self.configs
            .iter()
            .flat_map(|c| &c.interfaces)
            .filter_map(|i| i.kernel_driver.clone())
            .collect()
    }
}

/// A libusb context. rusb's global context panics when libusb cannot initialise
/// (seen with the statically linked libusb inside WSL); this returns an error instead.
pub fn context() -> Result<rusb::Context> {
    rusb::Context::new().map_err(|e| {
        crate::error::CanonError::Permission(format!(
            "cannot initialise libusb ({e}): no USB support in this environment (WSL or container without USB passthrough?)"
        ))
    })
}

pub fn known_model(vid: u16, pid: u16) -> Option<&'static str> {
    if vid != CANON_VID {
        return None;
    }
    KNOWN_MODELS
        .iter()
        .find(|(p, _)| *p == pid)
        .map(|(_, m)| *m)
}

pub fn is_candidate(vid: u16, pid: u16, product: Option<&str>) -> bool {
    vid == CANON_VID
        && (known_model(vid, pid).is_some()
            // FACT: our LBP2900 reports product "Canon CAPT USB Device"
            || product.is_some_and(|p| {
                let p = p.to_ascii_uppercase();
                p.contains("LBP") || p.contains("CAPT")
            }))
}

fn sysfs_driver(bus: u8, ports: &[u8], config: u8, iface: u8) -> Option<String> {
    if ports.is_empty() {
        return None;
    }
    let path: Vec<String> = ports.iter().map(u8::to_string).collect();
    let link = format!(
        "/sys/bus/usb/devices/{bus}-{}:{config}.{iface}/driver",
        path.join(".")
    );
    std::fs::read_link(link)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
}

fn permission(bus: u8, address: u8) -> Permission {
    let node = format!("/dev/bus/usb/{bus:03}/{address:03}");
    let readable = std::fs::File::open(&node).is_ok();
    let writable = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .is_ok();
    Permission {
        device_node: node,
        readable,
        writable,
    }
}

fn direction(d: rusb::Direction) -> &'static str {
    match d {
        rusb::Direction::In => "in",
        rusb::Direction::Out => "out",
    }
}

fn transfer(t: rusb::TransferType) -> &'static str {
    match t {
        rusb::TransferType::Control => "control",
        rusb::TransferType::Isochronous => "isochronous",
        rusb::TransferType::Bulk => "bulk",
        rusb::TransferType::Interrupt => "interrupt",
    }
}

fn describe(device: &rusb::Device<rusb::Context>) -> Result<DeviceInfo> {
    let desc = device.device_descriptor()?;
    let (vid, pid) = (desc.vendor_id(), desc.product_id());
    let (bus, address) = (device.bus_number(), device.address());
    let ports = device.port_numbers().unwrap_or_default();

    let (mut manufacturer, mut product, mut serial, mut open_error) = (None, None, None, None);
    match device.open() {
        Ok(h) => {
            manufacturer = h.read_manufacturer_string_ascii(&desc).ok();
            product = h.read_product_string_ascii(&desc).ok();
            serial = h.read_serial_number_string_ascii(&desc).ok();
        }
        Err(e) => open_error = Some(e.to_string()),
    }

    let mut configs = Vec::new();
    for i in 0..desc.num_configurations() {
        let Ok(cfg) = device.config_descriptor(i) else {
            continue;
        };
        let mut interfaces = Vec::new();
        for iface in cfg.interfaces() {
            for alt in iface.descriptors() {
                interfaces.push(InterfaceInfo {
                    number: alt.interface_number(),
                    alt_setting: alt.setting_number(),
                    class: alt.class_code(),
                    subclass: alt.sub_class_code(),
                    protocol: alt.protocol_code(),
                    endpoints: alt
                        .endpoint_descriptors()
                        .map(|e| EndpointInfo {
                            address: format!("0x{:02x}", e.address()),
                            direction: direction(e.direction()),
                            transfer_type: transfer(e.transfer_type()),
                            max_packet_size: e.max_packet_size(),
                        })
                        .collect(),
                    kernel_driver: sysfs_driver(bus, &ports, cfg.number(), alt.interface_number()),
                });
            }
        }
        configs.push(ConfigInfo {
            number: cfg.number(),
            interfaces,
        });
    }

    Ok(DeviceInfo {
        bus,
        address,
        vid: format!("{vid:04x}"),
        pid: format!("{pid:04x}"),
        known_model: known_model(vid, pid),
        is_candidate: is_candidate(vid, pid, product.as_deref()),
        manufacturer,
        product,
        serial,
        configs,
        permission: permission(bus, address),
        open_error,
        vid_raw: vid,
        pid_raw: pid,
    })
}

/// All USB devices visible to libusb.
pub fn enumerate() -> Result<Vec<DeviceInfo>> {
    let list = context()?.devices()?;
    let mut out = Vec::new();
    for device in list.iter() {
        if let Ok(info) = describe(&device) {
            out.push(info);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_detection() {
        assert!(is_candidate(0x04a9, 0x2676, None));
        assert_eq!(known_model(0x04a9, 0x2676), Some("LBP2900"));
        // unknown Canon PID but LBP product string: still a candidate (reported as unknown revision)
        assert!(is_candidate(0x04a9, 0x1234, Some("Canon LBP2900B")));
        assert!(is_candidate(0x04a9, 0x1234, Some("Canon CAPT USB Device")));
        assert!(!is_candidate(0x04a9, 0x1234, Some("Canon Scanner")));
        assert!(!is_candidate(0x1234, 0x2676, Some("LBP2900")));
    }
}
