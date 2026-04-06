# vlfd-rs

`vlfd-rs` is a modern Rust driver for a VeriComm-compatible USB interface board. It exposes ergonomic APIs for bringing the hardware online, exchanging FIFO data, and programming the onboard FPGA fabric.

## Features
- Automatic USB session and endpoint management with sensible timeouts
- Transparent encryption/decryption for VeriComm FIFO transfers
- High-level configuration getters and setters with caching
- Bitstream upload support for the integrated FPGA programmer
- Hotplug callbacks powered by libusb's hotplug subsystem
- Session/mode tracking so higher-level APIs can validate driver state
- Configurable transport knobs for timeout and open behavior tuning

## Quick Start
```rust
use vlfd_rs::{Device, IoSettings, Result, TransportConfig};

fn main() -> Result<()> {
    let transport = TransportConfig::default();
    let mut device = Device::connect_with_transport_config(transport)?;

    let mut settings = IoSettings::default();
    device.enter_io_mode(&settings)?;

    let tx = [0x1234u16; 4];
    let mut rx = [0u16; 4];
    device.transfer_io_words(&tx, &mut rx)?;

    device.exit_io_mode()?;
    Ok(())
}
```

## Installation
Add the crate to your `Cargo.toml`:
```toml
[dependencies]
vlfd-rs = "1.0"
```

## Hotplug Example
```rust
use vlfd_rs::{Device, HotplugEventKind, HotplugOptions};

fn main() -> vlfd_rs::Result<()> {
    let device = Device::new()?;
    let _registration = device.register_hotplug_callback(
        HotplugOptions::default(),
        |event| match event.kind {
            HotplugEventKind::Arrived => println!("Device arrived: {:?}", event.device),
            HotplugEventKind::Left => println!("Device left: {:?}", event.device),
        },
    )?;

    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
```

## Benchmarking

Two example harnesses are included:

```bash
cargo run --example bench_transfer -- cpu --words 1024 --iterations 200000
cargo run --example bench_transfer -- device --words 512 --iterations 1000
```

## License
Apache-2.0
