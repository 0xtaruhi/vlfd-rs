//! # vlfd-rs
//!
//! Rust bindings for the SMIMS VLFD board. The crate exposes two high-level
//! entry points: [`Device`] for day-to-day interaction with VeriComm I/O and
//! [`Programmer`] for uploading FPGA bitstreams. The following example opens
//! the device, switches to VeriComm mode, and performs a single FIFO
//! transaction:
//!
//! ```no_run
//! use vlfd_rs::{Device, IoSettings, Result};
//!
//! fn main() -> Result<()> {
//!     // Establish a connection and load the remote configuration/encryption tables.
//!     let mut device = Device::connect()?;
//!
//!     // Override default VeriComm timing before entering I/O mode.
//!     let mut settings = IoSettings::default();
//!     settings.clock_high_delay = 8;
//!     settings.clock_low_delay = 8;
//!     device.enter_io_mode(&settings)?;
//!
//!     // Perform a 4-word FIFO round-trip with transparent encryption.
//!     let mut tx = [0x1234u16, 0x5678, 0x9abc, 0xdef0];
//!     let mut rx = [0u16; 4];
//!     device.transfer_io(&mut tx, &mut rx)?;
//!
//!     device.exit_io_mode()?;
//!     Ok(())
//! }
//! ```
//!
//! To reprogram the FPGA, construct a [`Programmer`]:
//!
//! ```no_run
//! use std::path::Path;
//! use vlfd_rs::{Programmer, Result};
//!
//! fn main() -> Result<()> {
//!     let mut programmer = Programmer::connect()?;
//!     programmer.program(Path::new("path/to/bitstream.txt"))?;
//!     programmer.close()?;
//!     Ok(())
//! }
//! ```
//!
//! Both examples are tagged with `no_run`, so they compile during `cargo test`
//! but do not touch live hardware.
pub mod constants;

mod config;
mod device;
mod error;
mod program;
mod usb;

pub use config::Config;
pub use device::{Device, IoSettings};
pub use error::{Error, Result};
pub use program::Programmer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_is_created_closed() {
        let device = Device::new().expect("failed to initialise USB context");
        assert!(!device.is_open());
    }

    #[test]
    fn config_mutation_roundtrip() {
        let mut device = Device::new().expect("failed to initialise USB context");
        device.config_mut().set_vericomm_clock_high_delay(42);
        assert_eq!(device.config().vericomm_clock_high_delay(), 42);
    }

    #[test]
    fn programmer_wraps_device() {
        let device = Device::new().expect("failed to initialise USB context");
        let programmer = Programmer::new(device);
        assert!(!programmer.device().is_open());
    }
}
