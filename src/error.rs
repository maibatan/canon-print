//! Error model and process exit codes.
//!
//! Exit codes (stable, relied on by scripts/agent-test.sh):
//! 0 success, 1 input/software, 2 USB, 3 printer/protocol, 4 timeout,
//! 5 permission/environment.

use std::time::Duration;

pub const EXIT_OK: i32 = 0;
pub const EXIT_INPUT: i32 = 1;
pub const EXIT_USB: i32 = 2;
pub const EXIT_PROTOCOL: i32 = 3;
pub const EXIT_TIMEOUT: i32 = 4;
pub const EXIT_ENVIRONMENT: i32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum CanonError {
    #[error("input error: {0}")]
    Input(String),
    #[error("raster error: {0}")]
    Raster(String),
    #[error("compression error: {0}")]
    Compression(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("USB error: {0}")]
    Usb(String),
    #[error("permission/environment problem: {0}")]
    Permission(String),
    #[error("printer/protocol error: {0}")]
    Protocol(String),
    #[error("timeout in stage '{stage}' after {elapsed:?} (last status: {last_status})")]
    Timeout {
        stage: String,
        elapsed: Duration,
        last_status: String,
    },
}

impl CanonError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CanonError::Input(_)
            | CanonError::Raster(_)
            | CanonError::Compression(_)
            | CanonError::NotImplemented(_) => EXIT_INPUT,
            CanonError::Usb(_) => EXIT_USB,
            CanonError::Protocol(_) => EXIT_PROTOCOL,
            CanonError::Timeout { .. } => EXIT_TIMEOUT,
            CanonError::Permission(_) => EXIT_ENVIRONMENT,
        }
    }

    /// Short machine-readable kind, used in `--json` output.
    pub fn kind(&self) -> &'static str {
        match self {
            CanonError::Input(_) => "input",
            CanonError::Raster(_) => "raster",
            CanonError::Compression(_) => "compression",
            CanonError::NotImplemented(_) => "not_implemented",
            CanonError::Usb(_) => "usb",
            CanonError::Permission(_) => "permission",
            CanonError::Protocol(_) => "protocol",
            CanonError::Timeout { .. } => "timeout",
        }
    }
}

impl From<rusb::Error> for CanonError {
    fn from(e: rusb::Error) -> Self {
        match e {
            rusb::Error::Access => CanonError::Permission(format!(
                "{e} (no access to the USB device node; see README 'Linux permissions')"
            )),
            rusb::Error::Timeout => CanonError::Timeout {
                stage: "usb".into(),
                elapsed: Duration::ZERO,
                last_status: "n/a".into(),
            },
            other => CanonError::Usb(other.to_string()),
        }
    }
}

pub type Result<T> = std::result::Result<T, CanonError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_distinct_per_class() {
        assert_eq!(CanonError::Input("x".into()).exit_code(), 1);
        assert_eq!(CanonError::NotImplemented("x".into()).exit_code(), 1);
        assert_eq!(CanonError::Usb("x".into()).exit_code(), 2);
        assert_eq!(CanonError::Protocol("x".into()).exit_code(), 3);
        let t = CanonError::Timeout {
            stage: "transfer".into(),
            elapsed: Duration::from_secs(3),
            last_status: "8A00".into(),
        };
        assert_eq!(t.exit_code(), 4);
        assert_eq!(CanonError::Permission("x".into()).exit_code(), 5);
    }

    #[test]
    fn rusb_access_maps_to_permission() {
        let e: CanonError = rusb::Error::Access.into();
        assert_eq!(e.exit_code(), EXIT_ENVIRONMENT);
        let e: CanonError = rusb::Error::NoDevice.into();
        assert_eq!(e.exit_code(), EXIT_USB);
    }
}
