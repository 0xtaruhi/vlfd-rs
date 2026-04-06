use crate::config::Config;
use crate::device::Device;
use crate::error::{Error, Result};
use crate::usb::TransportConfig;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

/// Helper that manages FPGA bitstream uploads using a [`Device`].
pub struct Programmer {
    device: Device,
}

impl Programmer {
    pub fn new(device: Device) -> Self {
        Self { device }
    }

    pub fn connect() -> Result<Self> {
        Self::connect_with_transport_config(TransportConfig::default())
    }

    pub fn connect_with_transport_config(transport: TransportConfig) -> Result<Self> {
        let device = Device::connect_with_transport_config(transport)?;
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
        self.device.activate_fpga_programmer_checked()?;

        let chunk_len = bitstream_chunk_words(self.device.config())?;
        for chunk in program_data.chunks(chunk_len) {
            self.device.fifo_write(chunk)?;
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
    load_bitfile_from_reader(BufReader::new(file))
}

fn load_bitfile_from_reader<R: BufRead>(reader: R) -> Result<Vec<u16>> {
    let mut program_data = Vec::new();

    for (line_index, line) in reader.lines().enumerate() {
        let line_number = line_index + 1;
        let line = line?;
        let payload = line.split_whitespace().next().unwrap_or_default();

        if payload.is_empty() {
            continue;
        }

        for segment in payload.split('_') {
            if segment.is_empty() {
                return Err(Error::InvalidBitfileLine {
                    line: line_number,
                    reason: "empty word segment",
                });
            }

            let value =
                u16::from_str_radix(segment, 16).map_err(|_| Error::InvalidBitfileLine {
                    line: line_number,
                    reason: "bitfile contains non-hexadecimal characters",
                })?;
            program_data.push(value);
        }
    }

    if program_data.is_empty() {
        return Err(Error::InvalidBitfile("bitfile produced no data"));
    }

    Ok(program_data)
}

fn bitstream_chunk_words(config: &Config) -> Result<usize> {
    let fifo_words = usize::from(config.fifo_size_words());
    if fifo_words == 0 {
        return Err(Error::UnexpectedResponse(
            "device reported zero-length programming FIFO",
        ));
    }
    Ok(fifo_words)
}

#[cfg(test)]
mod tests {
    use super::{bitstream_chunk_words, load_bitfile_from_reader};
    use crate::{Config, Error};
    use std::io::Cursor;

    #[test]
    fn parses_cpp_style_bitfile_lines_into_words() {
        let data = "1234_abcd\n5678_9abc trailing\n";
        let words = load_bitfile_from_reader(Cursor::new(data)).expect("parse should succeed");
        assert_eq!(words, vec![0x1234, 0xabcd, 0x5678, 0x9abc]);
    }

    #[test]
    fn reports_invalid_bitfile_line_numbers() {
        let err =
            load_bitfile_from_reader(Cursor::new("1234_gggg\n")).expect_err("parse should fail");
        match err {
            Error::InvalidBitfileLine { line, reason } => {
                assert_eq!(line, 1);
                assert_eq!(reason, "bitfile contains non-hexadecimal characters");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn programming_chunk_size_uses_fifo_word_count_directly() {
        let mut words = [0u16; Config::WORD_COUNT];
        words[33] = 512;
        let config = Config::from_words(words);
        assert_eq!(bitstream_chunk_words(&config).unwrap(), 512);
    }
}
