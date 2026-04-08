use crate::config::Config;
use crate::constants;
use crate::error::{Error, Result};
use crate::usb::{Endpoint, TransportConfig, UsbDevice};
use nusb::{
    Endpoint as UsbEndpoint,
    io::{EndpointRead, EndpointWrite},
    transfer::{Buffer, Bulk, In, Out},
};
use std::io::{Read, Write};
use std::time::Instant;

const CONTROL_COMMAND_PREFIX: u8 = 0x01;

pub struct Board {
    usb: UsbDevice,
    config: Config,
    crypto: CryptoState,
    transfer_scratch: Vec<u16>,
    initialized: bool,
    mode: BoardMode,
}

impl Board {
    pub fn open() -> Result<Self> {
        Self::open_with_transport(TransportConfig::default())
    }

    pub fn open_with_transport(transport: TransportConfig) -> Result<Self> {
        let mut usb = UsbDevice::with_transport_config(transport)?;
        usb.open(constants::DW_VID, constants::DW_PID)?;

        let mut board = Self {
            usb,
            config: Config::new(),
            crypto: CryptoState::default(),
            transfer_scratch: Vec::new(),
            initialized: false,
            mode: BoardMode::Unknown,
        };
        board.initialize()?;
        Ok(board)
    }

    pub fn transport(&self) -> &TransportConfig {
        self.usb.transport_config()
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn mode(&self) -> BoardMode {
        self.mode
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn initialize(&mut self) -> Result<()> {
        self.read_encrypt_table()?;
        self.crypto.decode_table();
        self.refresh_config()?;
        Ok(())
    }

    pub fn refresh_config(&mut self) -> Result<&Config> {
        self.sync_delay()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x01])?;

        let mut words = [0u16; Config::WORD_COUNT];
        self.usb.read_words(Endpoint::FifoRead, &mut words)?;
        self.activate_control()?;
        self.crypto.decrypt_words(&mut words);
        self.config = Config::from_words(words);
        self.initialized = true;
        self.mode = BoardMode::Control;
        Ok(&self.config)
    }

    pub fn write_config(&mut self) -> Result<()> {
        self.sync_delay()?;
        let mut words = *self.config.words();
        self.crypto.encrypt_words(&mut words);
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x11])?;
        self.usb.write_words(Endpoint::FifoWrite, &words)?;
        self.activate_control()?;
        self.initialized = true;
        self.mode = BoardMode::Control;
        Ok(())
    }

    pub fn configure_io(&mut self, settings: &IoConfig) -> Result<IoSession<'_>> {
        self.ensure_ready()?;

        let actual_version = self.config.smims_version_raw();
        if actual_version < constants::SMIMS_VERSION {
            return Err(Error::VersionMismatch {
                expected: constants::SMIMS_VERSION,
                actual: actual_version,
            });
        }
        if !self.config.is_programmed() {
            return Err(Error::NotProgrammed);
        }
        if !self.config.vericomm_ability() {
            return Err(Error::FeatureUnavailable("vericomm"));
        }

        if let Some(licence_key) = settings.licence_key {
            self.config.set_licence_key(licence_key);
        }
        self.config
            .set_vericomm_clock_high_delay(settings.clock_high_delay);
        self.config
            .set_vericomm_clock_low_delay(settings.clock_low_delay);
        self.config.set_vericomm_isv(settings.vericomm_isv);
        self.config
            .set_vericomm_clock_check_enabled(settings.clock_check_enabled);
        self.config.set_mode_selector(settings.mode_selector);
        self.write_config()?;
        self.activate_mode(BoardMode::VeriComm)?;

        let fifo_write = self.usb.open_out_writer(Endpoint::FifoWrite)?;
        let fifo_read = self.usb.open_in_reader(Endpoint::FifoRead)?;

        let pipeline_write = self.usb.open_out_endpoint(Endpoint::FifoWrite)?;
        let pipeline_read = self.usb.open_in_endpoint(Endpoint::FifoRead)?;

        Ok(IoSession {
            board: self,
            fifo_write,
            fifo_read,
            pipeline_write,
            pipeline_read,
            tx_pool: Vec::new(),
            rx_pool: Vec::new(),
        })
    }

    pub fn programmer(&mut self) -> Result<ProgramSession<'_>> {
        self.ensure_ready()?;
        self.activate_mode(BoardMode::FpgaProgrammer)?;
        Ok(ProgramSession { board: self })
    }

    pub fn close(mut self) -> Result<()> {
        self.usb.close()
    }

    pub(crate) fn encrypt_words(&mut self, words: &mut [u16]) {
        self.crypto.encrypt_words(words);
    }

    pub(crate) fn fifo_write_words(&self, words: &[u16]) -> Result<()> {
        self.usb.write_words(Endpoint::FifoWrite, words)
    }

    pub(crate) fn command_active(&mut self) -> Result<()> {
        self.activate_control()
    }

    pub(crate) fn activate_control(&mut self) -> Result<()> {
        self.sync_delay()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x00])?;
        self.mode = BoardMode::Control;
        Ok(())
    }

    fn ensure_ready(&mut self) -> Result<()> {
        if !self.initialized {
            self.initialize()?;
        }
        Ok(())
    }

    fn ensure_mode(&self, expected: BoardMode) -> Result<()> {
        if self.mode != expected {
            return Err(Error::InvalidMode {
                expected: expected.as_str(),
                actual: self.mode.as_str(),
            });
        }
        Ok(())
    }

    fn activate_mode(&mut self, mode: BoardMode) -> Result<()> {
        let Some(command) = mode.command_byte() else {
            return Err(Error::UnexpectedResponse("unsupported mode command"));
        };
        self.sync_delay()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, command])?;
        self.mode = mode;
        Ok(())
    }

    fn read_encrypt_table(&mut self) -> Result<()> {
        self.sync_delay()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x0f])?;
        self.usb
            .read_words(Endpoint::FifoRead, self.crypto.table_mut())
    }

    fn sync_delay(&self) -> Result<()> {
        let start = Instant::now();
        let sync_timeout = self.transport().sync_timeout;
        let mut buffer = [0u8; 1];

        while start.elapsed() <= sync_timeout {
            self.usb.write_bytes(Endpoint::Command, &buffer)?;
            self.usb.read_bytes(Endpoint::Sync, &mut buffer)?;
            if buffer[0] != 0 {
                return Ok(());
            }
        }

        Err(Error::Timeout("sync_delay"))
    }
}

pub struct IoSession<'a> {
    board: &'a mut Board,
    fifo_write: EndpointWrite<Bulk>,
    fifo_read: EndpointRead<Bulk>,
    pipeline_write: UsbEndpoint<Bulk, Out>,
    pipeline_read: UsbEndpoint<Bulk, In>,
    tx_pool: Vec<Buffer>,
    rx_pool: Vec<Buffer>,
}

impl IoSession<'_> {
    fn prepare_pools(&mut self, pipeline_depth: usize, tx_bytes: usize, rx_bytes: usize) {
        while self.tx_pool.len() < pipeline_depth {
            self.tx_pool
                .push(self.pipeline_write.allocate(tx_bytes.max(1)));
        }
        while self.rx_pool.len() < pipeline_depth {
            let mut buffer = self.pipeline_read.allocate(rx_bytes.max(1));
            buffer.set_requested_len(rx_bytes.max(1));
            self.rx_pool.push(buffer);
        }
    }
    pub fn transfer(&mut self, tx: &[u16], rx: &mut [u16]) -> Result<()> {
        validate_transfer_buffers(
            tx.len(),
            rx.len(),
            usize::from(self.board.config.fifo_size_words()),
        )?;
        self.board.ensure_mode(BoardMode::VeriComm)?;

        self.board.transfer_scratch.clear();
        self.board.transfer_scratch.extend_from_slice(tx);
        self.board
            .crypto
            .encrypt_words(&mut self.board.transfer_scratch);

        let tx_bytes = words_as_bytes(&self.board.transfer_scratch);
        let rx_bytes = words_as_bytes_mut(rx);

        self.fifo_write
            .write_all(tx_bytes)
            .map_err(|err| io_error(err, "nusb_bulk_write"))?;
        self.fifo_write
            .flush()
            .map_err(|err| io_error(err, "nusb_bulk_flush"))?;
        self.fifo_read
            .read_exact(rx_bytes)
            .map_err(|err| io_error(err, "nusb_bulk_read"))?;

        self.board.crypto.decrypt_words(rx);
        Ok(())
    }

    fn transfer_batch_slices(
        &mut self,
        txs: &[&[u16]],
        rx_lengths: &[usize],
    ) -> Result<Vec<Vec<u16>>> {
        if txs.len() != rx_lengths.len() {
            return Err(Error::InvalidBufferLength {
                context: "vericomm batch transfer",
                expected: txs.len(),
                actual: rx_lengths.len(),
            });
        }
        self.board.ensure_mode(BoardMode::VeriComm)?;

        for (tx, rx_len) in txs.iter().zip(rx_lengths.iter().copied()) {
            validate_transfer_buffers(
                tx.len(),
                rx_len,
                usize::from(self.board.config.fifo_size_words()),
            )?;
        }

        if txs.is_empty() {
            return Ok(Vec::new());
        }

        let max_bytes = txs
            .iter()
            .map(|tx| std::mem::size_of_val(*tx))
            .max()
            .unwrap_or(0);
        let request_bytes = aligned_request_len(self.pipeline_read.max_packet_size(), max_bytes);
        let pipeline_depth = txs.len().min(4);
        self.prepare_pools(pipeline_depth, max_bytes, request_bytes);
        let mut outputs = rx_lengths
            .iter()
            .map(|len| vec![0u16; *len])
            .collect::<Vec<_>>();

        let mut submitted = 0usize;
        let mut completed = 0usize;

        while submitted < pipeline_depth {
            let tx_buffer = self.tx_pool.pop().expect("tx pool should be primed");
            let rx_buffer = self.rx_pool.pop().expect("rx pool should be primed");
            submit_pipeline_write(
                &mut self.board.crypto,
                &mut self.pipeline_write,
                txs[submitted],
                tx_buffer,
            );
            submit_pipeline_read(&mut self.pipeline_read, rx_buffer, request_bytes);
            submitted += 1;
        }

        while completed < txs.len() {
            let write_completion = self
                .pipeline_write
                .wait_next_complete(self.board.transport().usb_timeout)
                .ok_or(Error::Timeout("pipeline_write"))?;
            write_completion
                .status
                .map_err(|err| transfer_error(err, "pipeline_write"))?;

            let read_completion = self
                .pipeline_read
                .wait_next_complete(self.board.transport().usb_timeout)
                .ok_or(Error::Timeout("pipeline_read"))?;
            read_completion
                .status
                .map_err(|err| transfer_error(err, "pipeline_read"))?;

            let expected_bytes = outputs[completed].len() * std::mem::size_of::<u16>();
            if read_completion.actual_len < expected_bytes {
                return Err(Error::UnexpectedResponse(
                    "pipeline read returned short payload",
                ));
            }
            bytes_into_words(
                &read_completion.buffer[..expected_bytes],
                &mut outputs[completed],
            );
            self.board.crypto.decrypt_words(&mut outputs[completed]);
            completed += 1;

            if submitted < txs.len() {
                let tx_buffer = self
                    .tx_pool
                    .pop()
                    .expect("tx pool should contain recycled buffers");
                let rx_buffer = self
                    .rx_pool
                    .pop()
                    .expect("rx pool should contain recycled buffers");
                submit_pipeline_write(
                    &mut self.board.crypto,
                    &mut self.pipeline_write,
                    txs[submitted],
                    tx_buffer,
                );
                submit_pipeline_read(&mut self.pipeline_read, rx_buffer, request_bytes);
                submitted += 1;
            }
        }

        Ok(outputs)
    }

    pub fn transfer_batch(&mut self, txs: &[Vec<u16>]) -> Result<Vec<Vec<u16>>> {
        let inputs = txs.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let rx_lengths = txs.iter().map(Vec::len).collect::<Vec<_>>();
        self.transfer_batch_slices(&inputs, &rx_lengths)
    }

    pub fn transfer_into(&mut self, tx: &[u16], rx: &mut [u16]) -> Result<()> {
        self.transfer(tx, rx)
    }

    pub fn transfer_batch_into(
        &mut self,
        txs: &[&[u16]],
        outputs: &mut [&mut [u16]],
    ) -> Result<()> {
        if txs.len() != outputs.len() {
            return Err(Error::InvalidBufferLength {
                context: "vericomm batch in-place transfer",
                expected: txs.len(),
                actual: outputs.len(),
            });
        }

        let rx_lengths = outputs.iter().map(|out| out.len()).collect::<Vec<_>>();
        let results = self.transfer_batch_slices(txs, &rx_lengths)?;
        for (result, out) in results.into_iter().zip(outputs.iter_mut()) {
            out.copy_from_slice(&result);
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.pipeline_write.cancel_all();
        self.pipeline_read.cancel_all();
        self.board.activate_control()
    }
}

pub struct ProgramSession<'a> {
    board: &'a mut Board,
}

impl ProgramSession<'_> {
    pub fn write_bitstream_words(&mut self, words: &[u16]) -> Result<()> {
        let chunk_len = bitstream_chunk_words(self.board.config())?;
        let mut encrypted = words.to_vec();
        self.board.encrypt_words(&mut encrypted);
        for chunk in encrypted.chunks(chunk_len) {
            self.board.fifo_write_words(chunk)?;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        self.board.command_active()?;
        self.board.refresh_config()?;
        if !self.board.config().is_programmed() {
            return Err(Error::NotProgrammed);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardMode {
    Closed,
    Unknown,
    Control,
    VeriComm,
    FpgaProgrammer,
    VeriInstrument,
    VeriLink,
    VeriSoc,
    VeriCommPro,
    VeriSdk,
    FlashRead,
    FlashWrite,
}

impl BoardMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Unknown => "unknown",
            Self::Control => "control",
            Self::VeriComm => "vericomm",
            Self::FpgaProgrammer => "fpga_programmer",
            Self::VeriInstrument => "veri_instrument",
            Self::VeriLink => "veri_link",
            Self::VeriSoc => "veri_soc",
            Self::VeriCommPro => "vericomm_pro",
            Self::VeriSdk => "veri_sdk",
            Self::FlashRead => "flash_read",
            Self::FlashWrite => "flash_write",
        }
    }

    fn command_byte(self) -> Option<u8> {
        Some(match self {
            Self::Control => 0x00,
            Self::FpgaProgrammer => 0x02,
            Self::VeriComm => 0x03,
            Self::VeriSdk => 0x04,
            Self::FlashRead => 0x05,
            Self::VeriInstrument => 0x08,
            Self::VeriLink => 0x09,
            Self::VeriSoc => 0x0a,
            Self::VeriCommPro => 0x0b,
            Self::FlashWrite => 0x15,
            Self::Closed | Self::Unknown => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct IoConfig {
    pub clock_high_delay: u16,
    pub clock_low_delay: u16,
    pub vericomm_isv: u8,
    pub clock_check_enabled: bool,
    pub mode_selector: u8,
    pub licence_key: Option<u16>,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            clock_high_delay: 11,
            clock_low_delay: 11,
            vericomm_isv: 0,
            clock_check_enabled: false,
            mode_selector: 0,
            licence_key: Some(0xff40),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CryptoState {
    table: [u16; 32],
    encode_index: usize,
    decode_index: usize,
}

impl CryptoState {
    fn table_mut(&mut self) -> &mut [u16; 32] {
        &mut self.table
    }

    fn decode_table(&mut self) {
        self.table[0] = !self.table[0];
        for idx in 1..self.table.len() {
            let prev = self.table[idx - 1];
            self.table[idx] ^= prev;
        }
        self.reset_indices();
    }

    fn encrypt_words(&mut self, buffer: &mut [u16]) {
        let key = &self.table[0..16];
        let mut index = self.encode_index;
        for word in buffer.iter_mut() {
            *word ^= key[index];
            index = (index + 1) & 0x0f;
        }
        self.encode_index = index;
    }

    fn decrypt_words(&mut self, buffer: &mut [u16]) {
        let key = &self.table[16..32];
        let mut index = self.decode_index;
        for word in buffer.iter_mut() {
            *word ^= key[index];
            index = (index + 1) & 0x0f;
        }
        self.decode_index = index;
    }

    fn reset_indices(&mut self) {
        self.encode_index = 0;
        self.decode_index = 0;
    }
}

fn validate_transfer_buffers(
    write_words: usize,
    read_words: usize,
    fifo_capacity_words: usize,
) -> Result<()> {
    if write_words != read_words {
        return Err(Error::InvalidBufferLength {
            context: "vericomm transfer",
            expected: write_words,
            actual: read_words,
        });
    }

    if write_words > fifo_capacity_words {
        return Err(Error::BufferTooLarge {
            context: "vericomm transfer",
            max_words: fifo_capacity_words,
            actual_words: write_words,
        });
    }

    Ok(())
}

pub(crate) fn bitstream_chunk_words(config: &Config) -> Result<usize> {
    let fifo_words = usize::from(config.fifo_size_words());
    if fifo_words == 0 {
        return Err(Error::UnexpectedResponse(
            "device reported zero-length programming FIFO",
        ));
    }
    Ok(fifo_words)
}

fn words_as_bytes(words: &[u16]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, std::mem::size_of_val(words)) }
}

fn words_as_bytes_mut(words: &mut [u16]) -> &mut [u8] {
    unsafe {
        std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, std::mem::size_of_val(words))
    }
}

fn io_error(err: std::io::Error, context: &'static str) -> Error {
    if err.kind() == std::io::ErrorKind::TimedOut {
        Error::Timeout(context)
    } else {
        Error::Usb {
            source: Box::new(err),
            context,
        }
    }
}

fn aligned_request_len(max_packet_size: usize, payload_bytes: usize) -> usize {
    let payload_bytes = payload_bytes.max(max_packet_size.max(1));
    let rem = payload_bytes % max_packet_size.max(1);
    if rem == 0 {
        payload_bytes
    } else {
        payload_bytes + (max_packet_size - rem)
    }
}

fn submit_pipeline_write(
    crypto: &mut CryptoState,
    endpoint: &mut UsbEndpoint<Bulk, Out>,
    tx: &[u16],
    mut buffer: Buffer,
) {
    buffer.clear();
    let byte_len = std::mem::size_of_val(tx);
    buffer.extend_fill(byte_len, 0);
    words_to_bytes(tx, &mut buffer[..byte_len]);
    crypto.encrypt_words(bytes_as_words_mut(&mut buffer[..byte_len]));
    endpoint.submit(buffer);
}

fn submit_pipeline_read(
    endpoint: &mut UsbEndpoint<Bulk, In>,
    mut buffer: Buffer,
    request_bytes: usize,
) {
    buffer.clear();
    buffer.set_requested_len(request_bytes);
    endpoint.submit(buffer);
}

fn bytes_into_words(bytes: &[u8], out: &mut [u16]) {
    for (index, chunk) in bytes.chunks_exact(2).take(out.len()).enumerate() {
        out[index] = u16::from_le_bytes([chunk[0], chunk[1]]);
    }
}

fn transfer_error(err: nusb::transfer::TransferError, context: &'static str) -> Error {
    Error::Usb {
        source: Box::new(std::io::Error::other(format!("{context}: {err}"))),
        context,
    }
}

fn words_to_bytes(words: &[u16], out: &mut [u8]) {
    for (index, word) in words.iter().copied().enumerate() {
        let [lo, hi] = word.to_le_bytes();
        out[index * 2] = lo;
        out[index * 2 + 1] = hi;
    }
}

fn bytes_as_words_mut(bytes: &mut [u8]) -> &mut [u16] {
    unsafe { std::slice::from_raw_parts_mut(bytes.as_mut_ptr() as *mut u16, bytes.len() / 2) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn words_to_bytes_roundtrip() {
        let words = [0x1234u16, 0xabcd];
        let mut bytes = [0u8; 4];
        super::words_to_bytes(&words, &mut bytes);
        assert_eq!(bytes, [0x34, 0x12, 0xcd, 0xab]);
    }

    #[test]
    fn aligned_request_len_rounds_up_to_packet_boundary() {
        assert_eq!(super::aligned_request_len(512, 513), 1024);
        assert_eq!(super::aligned_request_len(512, 512), 512);
    }

    #[test]
    fn batch_in_place_error_shape_is_stable() {
        let err = Error::InvalidBufferLength {
            context: "vericomm batch in-place transfer",
            expected: 1,
            actual: 0,
        };
        assert_eq!(
            err.to_string(),
            "invalid buffer length for `vericomm batch in-place transfer` (expected 1, got 0)"
        );
    }

    #[test]
    fn io_session_struct_caches_endpoint_adapters() {
        let type_name = std::any::type_name::<super::IoSession<'_>>();
        assert!(type_name.contains("IoSession"));
    }

    use super::{Board, BoardMode, CryptoState, IoConfig, validate_transfer_buffers};
    use crate::error::Error;
    use crate::usb::TransportConfig;
    use std::time::Duration;

    #[test]
    fn board_accepts_custom_transport_config() {
        let transport = TransportConfig {
            usb_timeout: Duration::from_millis(250),
            sync_timeout: Duration::from_millis(750),
            reset_on_open: true,
            clear_halt_on_open: false,
        };
        let board = Board::open_with_transport(transport);
        assert!(
            board.is_err()
                || board
                    .as_ref()
                    .map(|b| b.transport() == &transport)
                    .unwrap_or(false)
        );
    }

    #[test]
    fn encrypted_transfer_buffer_is_copied_before_mutation() {
        let mut crypto = CryptoState::default();
        crypto.table[0] = 0x00ff;
        let input = [0x1234u16, 0xabcd];
        let mut encrypted = input;
        crypto.encrypt_words(&mut encrypted);

        assert_eq!(input, [0x1234, 0xabcd]);
        assert_eq!(encrypted, [0x12cb, 0xabcd]);
    }

    #[test]
    fn vericomm_transfer_requires_matching_buffer_lengths() {
        let err = validate_transfer_buffers(4, 3, 16).expect_err("validation should fail");
        match err {
            Error::InvalidBufferLength {
                context,
                expected,
                actual,
            } => {
                assert_eq!(context, "vericomm transfer");
                assert_eq!(expected, 4);
                assert_eq!(actual, 3);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn vericomm_transfer_rejects_oversize_payloads() {
        let err = validate_transfer_buffers(17, 17, 16).expect_err("validation should fail");
        match err {
            Error::BufferTooLarge {
                context,
                max_words,
                actual_words,
            } => {
                assert_eq!(context, "vericomm transfer");
                assert_eq!(max_words, 16);
                assert_eq!(actual_words, 17);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn io_config_defaults_match_previous_tuning() {
        let cfg = IoConfig::default();
        assert_eq!(cfg.clock_high_delay, 11);
        assert_eq!(cfg.clock_low_delay, 11);
        assert_eq!(cfg.licence_key, Some(0xff40));
    }

    #[test]
    fn board_mode_labels_are_stable() {
        assert_eq!(BoardMode::Control.as_str(), "control");
        assert_eq!(BoardMode::VeriComm.as_str(), "vericomm");
    }
}
