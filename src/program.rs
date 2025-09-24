use crate::device::Device;
use crate::error::{Error, Result};
use crate::usb::Endpoint;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Helper that manages FPGA bitstream uploads using a [`Device`].
pub struct Programmer {
    device: Device,
}

impl Programmer {
    pub fn new(device: Device) -> Self {
        Self { device }
    }

    pub fn connect() -> Result<Self> {
        let device = Device::connect()?;
        Ok(Self { device })
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut Device {
        &mut self.device
    }

    pub fn close(mut self) -> Result<()> {
        self.device.close()
    }

    pub fn program(&mut self, bitfile: impl AsRef<Path>) -> Result<()> {
        let mut program_data = load_bitfile(bitfile.as_ref())?;

        self.device.ensure_session()?;
        self.device.encrypt(&mut program_data);
        self.device.activate_fpga_programmer()?;

        let fifo_words = usize::from(self.device.config().fifo_size()).saturating_mul(2);
        let chunk_len = fifo_words.max(1);

        for chunk in program_data.chunks(chunk_len) {
            self.device.usb().write_words(Endpoint::FifoWrite, chunk)?;
        }

        self.device.command_active()?;
        self.device.read_config()?;

        if !self.device.config().is_programmed() {
            return Err(Error::NotProgrammed);
        }

        Ok(())
    }
}

fn load_bitfile(path: &Path) -> Result<Vec<u16>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut program_data = Vec::new();

    for line in reader.lines() {
        let line = line?;
        let mut accumulator = 0u16;
        let mut has_nibble = false;

        for byte in line.bytes() {
            match byte {
                b'_' => {
                    program_data.push(accumulator);
                    accumulator = 0;
                    has_nibble = false;
                }
                b' ' | b'\t' => break,
                _ => {
                    let Some(nibble) = remap_hex(byte) else {
                        return Err(Error::InvalidBitfile(
                            "bitfile contains non-hexadecimal character",
                        ));
                    };
                    accumulator = (accumulator << 4) | u16::from(nibble);
                    has_nibble = true;
                }
            }
        }

        if has_nibble {
            program_data.push(accumulator);
        }
    }

    if program_data.is_empty() {
        return Err(Error::InvalidBitfile("bitfile produced no data"));
    }

    Ok(program_data)
}

fn remap_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}
