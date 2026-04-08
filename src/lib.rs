//! # vlfd-rs
//!
//! `vlfd-rs` 2.x models the device around explicit sessions instead of a
//! single stateful façade. Open a [`Board`] to inspect and configure the
//! hardware, then create dedicated sessions for I/O or programming.
//!
//! ```no_run
//! use vlfd_rs::{Board, IoConfig, Result};
//!
//! fn main() -> Result<()> {
//!     let mut board = Board::open()?;
//!     let mut io = board.configure_io(&IoConfig::default())?;
//!
//!     let tx = [0x1234u16; 4];
//!     let mut rx = [0u16; 4];
//!     io.transfer(&tx, &mut rx)?;
//!     io.finish()?;
//!     Ok(())
//! }
//! ```
//!
//! ```no_run
//! use std::path::Path;
//! use vlfd_rs::{Programmer, Result};
//!
//! fn main() -> Result<()> {
//!     let mut programmer = Programmer::open()?;
//!     programmer.program(Path::new("path/to/bitstream.txt"))?;
//!     programmer.close()?;
//!     Ok(())
//! }
//! ```

pub mod constants;

mod config;
mod error;
mod program;
mod session;
mod usb;

pub use config::Config;
pub use error::{Error, Result};
pub use program::{Programmer, load_bitfile, load_bitfile_from_reader};
pub use session::{Board, BoardMode, IoConfig, IoSession, ProgramSession};
pub use usb::{
    HotplugDeviceInfo, HotplugEvent, HotplugEventKind, HotplugOptions, HotplugRegistration, Probe,
    TransportConfig,
};
