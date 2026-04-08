use crate::error::{Error, Result};
use crate::session::Board;
use crate::usb::TransportConfig;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

pub struct Programmer {
    board: Board,
}

impl Programmer {
    pub fn open() -> Result<Self> {
        Self::open_with_transport(TransportConfig::default())
    }

    pub fn open_with_transport(transport: TransportConfig) -> Result<Self> {
        Ok(Self {
            board: Board::open_with_transport(transport)?,
        })
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn board_mut(&mut self) -> &mut Board {
        &mut self.board
    }

    pub fn program(&mut self, bitfile: impl AsRef<Path>) -> Result<()> {
        let words = load_bitfile(bitfile.as_ref())?;
        let mut session = self.board.programmer()?;
        session.write_bitstream_words(&words)?;
        session.finish()
    }

    pub fn close(self) -> Result<()> {
        self.board.close()
    }
}

pub fn load_bitfile(path: &Path) -> Result<Vec<u16>> {
    let file = File::open(path)?;
    load_bitfile_from_reader(BufReader::new(file))
}

pub fn load_bitfile_from_reader<R: BufRead>(reader: R) -> Result<Vec<u16>> {
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

#[cfg(test)]
mod tests {
    use super::load_bitfile_from_reader;
    use crate::Error;
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
}
