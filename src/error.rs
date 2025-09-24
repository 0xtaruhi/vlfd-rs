use rusb::Error as UsbLibError;
use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    DeviceNotOpen,
    DeviceNotFound {
        vid: u16,
        pid: u16,
    },
    FeatureUnavailable(&'static str),
    InvalidBitfile(&'static str),
    NotProgrammed,
    Timeout(&'static str),
    UnexpectedResponse(&'static str),
    VersionMismatch {
        expected: u16,
        actual: u16,
    },
    Usb {
        source: UsbLibError,
        context: &'static str,
    },
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::DeviceNotOpen => write!(f, "device is not open"),
            Error::DeviceNotFound { vid, pid } => {
                write!(f, "device {vid:#06x}:{pid:#06x} not found")
            }
            Error::FeatureUnavailable(feature) => write!(f, "feature `{feature}` is unavailable"),
            Error::InvalidBitfile(reason) => write!(f, "invalid bitfile: {reason}"),
            Error::NotProgrammed => write!(f, "FPGA is not programmed"),
            Error::Timeout(context) => write!(f, "operation `{context}` timed out"),
            Error::UnexpectedResponse(context) => {
                write!(f, "unexpected response during `{context}`")
            }
            Error::VersionMismatch { expected, actual } => write!(
                f,
                "SMIMS version mismatch (expected {expected:#06x}, found {actual:#06x})"
            ),
            Error::Usb { source, context } => {
                write!(f, "usb error {source} in `{context}`")
            }
            Error::Io(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Usb { source, .. } => Some(source),
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
