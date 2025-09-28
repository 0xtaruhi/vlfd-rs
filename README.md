# vlfd-rs

`vlfd-rs` is a modern Rust driver for a VeriComm-compatible USB interface board. It exposes ergonomic APIs for bringing the hardware online, exchanging FIFO data, and programming the onboard FPGA fabric.

## Features
- Automatic USB session and endpoint management with sensible timeouts
- Transparent encryption/decryption for VeriComm FIFO transfers
- High-level configuration getters and setters with caching
- Bitstream upload support for the integrated FPGA programmer
- Hotplug callbacks powered by libusb's hotplug subsystem

## Quick Start
```rust
use vlfd_rs::{Device, IoSettings, Result};

fn main() -> Result<()> {
    let mut device = Device::connect()?;

    let mut settings = IoSettings::default();
    device.enter_io_mode(&settings)?;

    let mut tx = [0x1234u16; 4];
    let mut rx = [0u16; 4];
    device.transfer_io(&mut tx, &mut rx)?;

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
    let _registration = device.usb().register_hotplug_callback(
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

## License
Apache-2.0
