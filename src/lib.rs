//! # vlfd-rs
//!
//! `vlfd-rs` 3.x models the device around explicit sessions instead of a
//! single stateful façade. Open a [`Board`] to inspect and configure the
//! hardware, then create dedicated sessions for I/O or programming.
//!
//! ```no_run
//! use vlfd_rs::{Board, IoConfig, Licence, Result, VeriCommFrame};
//!
//! fn main() -> Result<()> {
//!     let mut board = Board::open()?;
//!     let mut io = board.configure_io(&IoConfig::new(Licence::CustomerId(0x1234)))?;
//!
//!     let tx = VeriCommFrame::from_bits(0x1234);
//!     let rx = io.transfer_frame(tx)?;
//!     assert_eq!(rx.words().len(), VeriCommFrame::WORDS);
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
mod frame;
mod licence;
mod program;
mod session;
mod usb;

pub use config::Config;
pub use error::{Error, Result};
pub use frame::VeriCommFrame;
pub use licence::Licence;
pub use program::{Programmer, load_bitfile, load_bitfile_from_reader};
pub use session::{
    Board, BoardMode, IoConfig, IoSession, IoTransferWindow, ProgramSession, TransferStageProfile,
};
pub use usb::{
    BoardInfo, BoardSelector, HotplugDeviceInfo, HotplugEvent, HotplugEventKind, HotplugOptions,
    HotplugRegistration, Probe, TransportConfig, UsbLocation,
};
