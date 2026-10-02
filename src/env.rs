//! Host environment detection (native Linux / WSL / VMware / container ...).
//! Only reads files; never changes anything on the host.

use serde::Serialize;
use std::path::Path;

#[derive(Clone, Debug, Serialize)]
pub struct Environment {
    pub os: String,
    pub distribution: Option<String>,
    pub kernel: Option<String>,
    pub arch: String,
    /// "native-linux", "wsl", "vmware", "virtualbox", "kvm", "hyperv", "container", "other"
    pub environment: String,
    pub wsl: bool,
    pub container: bool,
    pub vm_vendor: Option<String>,
    pub libusb: String,
}

fn read_trim(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn os_release_pretty() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
}

/// Classify from raw facts; kept pure so it can be unit tested.
pub fn classify(
    os: &str,
    kernel: Option<&str>,
    container: bool,
    sys_vendor: Option<&str>,
    product: Option<&str>,
) -> (String, bool, Option<String>) {
    let k = kernel.unwrap_or("").to_ascii_lowercase();
    let wsl = k.contains("microsoft") || k.contains("wsl");
    let vendor = sys_vendor.unwrap_or("");
    let product = product.unwrap_or("");
    let vm = if vendor.contains("VMware") || product.contains("VMware") {
        Some("vmware")
    } else if vendor.contains("innotek") || product.contains("VirtualBox") {
        Some("virtualbox")
    } else if vendor.contains("QEMU") || product.contains("KVM") {
        Some("kvm")
    } else if vendor.contains("Microsoft") && product.contains("Virtual Machine") {
        Some("hyperv")
    } else {
        None
    };
    let label = if os != "linux" {
        "other".to_string()
    } else if container {
        "container".to_string()
    } else if wsl {
        "wsl".to_string()
    } else if let Some(v) = vm {
        v.to_string()
    } else {
        "native-linux".to_string()
    };
    (label, wsl, vm.map(str::to_string))
}

pub fn detect() -> Environment {
    let os = std::env::consts::OS.to_string();
    let kernel = read_trim("/proc/sys/kernel/osrelease");
    let container = Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|c| c.contains("docker") || c.contains("containerd") || c.contains("kubepods"))
            .unwrap_or(false);
    let sys_vendor = read_trim("/sys/class/dmi/id/sys_vendor");
    let product = read_trim("/sys/class/dmi/id/product_name");
    let (environment, wsl, vm_vendor) = classify(
        &os,
        kernel.as_deref(),
        container,
        sys_vendor.as_deref(),
        product.as_deref(),
    );
    let v = rusb::version();
    Environment {
        os,
        distribution: os_release_pretty(),
        kernel,
        arch: std::env::consts::ARCH.to_string(),
        environment,
        wsl,
        container,
        vm_vendor,
        libusb: format!("{}.{}.{}.{}", v.major(), v.minor(), v.micro(), v.nano()),
    }
}

#[cfg(test)]
mod tests {
    use super::classify;

    #[test]
    fn classification() {
        let c = |os, k, cont, v, p| classify(os, k, cont, v, p).0;
        assert_eq!(
            c(
                "linux",
                Some("6.6.87.2-microsoft-standard-WSL2"),
                false,
                None,
                None
            ),
            "wsl"
        );
        assert_eq!(
            c(
                "linux",
                Some("6.8.0"),
                false,
                Some("VMware, Inc."),
                Some("VMware Virtual Platform")
            ),
            "vmware"
        );
        assert_eq!(
            c(
                "linux",
                Some("6.8.0"),
                false,
                Some("innotek GmbH"),
                Some("VirtualBox")
            ),
            "virtualbox"
        );
        assert_eq!(
            c("linux", Some("6.8.0"), true, Some("VMware, Inc."), None),
            "container"
        );
        assert_eq!(
            c("linux", Some("6.8.0"), false, Some("LENOVO"), Some("20XW")),
            "native-linux"
        );
        assert_eq!(c("windows", None, false, None, None), "other");
    }
}
