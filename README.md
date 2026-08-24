# vlfd-rs

`vlfd-rs` is a Rust driver for a VeriComm-compatible USB interface board.
The 3.x API models board access through explicit sessions:

- `Board`: owns the USB connection and cached device state
- `IoSession`: handles VeriComm FIFO transfers
- `ProgramSession`: handles FPGA programming transfers
- `Programmer`: convenience wrapper for bitstream upload flows
- `VeriCommFrame`: names the fixture's four-word, 64-lane sample boundary

## Features
- Pure-Rust USB transport powered by `nusb`
- Explicit board / I/O / programming session boundaries
- Fixed-size rolling transfer windows for sustained VeriComm streaming
- Reusable-buffer output APIs for lower-allocation I/O paths
- High-level configuration refresh and write helpers
- Bitstream upload support for the integrated FPGA programmer
- Hotplug callbacks powered by a `nusb`-based polling watcher
- Deterministic board enumeration and selection by USB location or serial number

## Quick Start
```rust
use vlfd_rs::{Board, IoConfig, Result};

fn main() -> Result<()> {
    let mut board = Board::open()?;
    let mut io = board.configure_io(&IoConfig::default())?;

    let tx = vlfd_rs::VeriCommFrame::from_bits(0x1234);
    let rx = io.transfer_frame(tx)?;
    println!("rx={:#018x}", rx.bits());

    io.finish()?;
    Ok(())
}
```

## Programming Example
```rust
use std::path::Path;
use vlfd_rs::{Programmer, Result};

fn main() -> Result<()> {
    let mut programmer = Programmer::open()?;
    programmer.program(Path::new("path/to/bitstream.txt"))?;
    programmer.close()?;
    Ok(())
}
```

## Installation
Add the crate to your `Cargo.toml`:
```toml
[dependencies]
vlfd-rs = "3"
```

## API Notes
- The old monolithic `Device` API was removed in 3.0
- `Board::open()` requires exactly one connected board; use `Board::enumerate()`
  and `Board::open_selected()` when multiple boards are present
- Rolling windows are fixed-size: use `io.transfer_window(words, capacity)?`
- The old batch transfer helpers are removed in favor of the rolling window API
- Transport remains blocking from the public API perspective
- Internally the USB layer uses `nusb` and `MaybeFuture::wait()`

## Benchmarking
```bash
cargo run --example bench_transfer -- cpu --words 1024 --iterations 200000
cargo run --example bench_transfer -- device --words 512 --iterations 1000
```

## License
Apache-2.0
