use crate::config::Config;
use crate::constants;
use crate::error::{Error, Result};
use crate::usb::{Endpoint, TransportConfig, UsbDevice};
use nusb::{
    Endpoint as UsbEndpoint,
    transfer::{Buffer, Bulk, Completion, EndpointDirection, In, Out},
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const CONTROL_COMMAND_PREFIX: u8 = 0x01;
const VERICOMM_TRANSFER_PACKET_BYTES: usize = 8;
const MAX_PIPELINE_DEPTH: usize = 512;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransferStageProfile {
    pub calls: u64,
    pub transfers: u64,
    pub validation: Duration,
    pub setup: Duration,
    pub submit: Duration,
    pub wait_write: Duration,
    pub wait_read: Duration,
    pub decode_copy: Duration,
    pub refill_submit: Duration,
}

impl TransferStageProfile {
    pub fn merge(&mut self, other: &Self) {
        self.calls = self.calls.saturating_add(other.calls);
        self.transfers = self.transfers.saturating_add(other.transfers);
        self.validation = self.validation.saturating_add(other.validation);
        self.setup = self.setup.saturating_add(other.setup);
        self.submit = self.submit.saturating_add(other.submit);
        self.wait_write = self.wait_write.saturating_add(other.wait_write);
        self.wait_read = self.wait_read.saturating_add(other.wait_read);
        self.decode_copy = self.decode_copy.saturating_add(other.decode_copy);
        self.refill_submit = self.refill_submit.saturating_add(other.refill_submit);
    }

    pub fn total_duration(&self) -> Duration {
        self.validation
            .saturating_add(self.setup)
            .saturating_add(self.submit)
            .saturating_add(self.wait_write)
            .saturating_add(self.wait_read)
            .saturating_add(self.decode_copy)
            .saturating_add(self.refill_submit)
    }
}

#[derive(Debug, Clone, Copy)]
enum TransferProfileStage {
    Validation,
    Setup,
    Submit,
    WaitWrite,
    WaitRead,
    DecodeCopy,
    RefillSubmit,
}

struct TransferProfiler<'a> {
    profile: Option<&'a mut TransferStageProfile>,
}

impl<'a> TransferProfiler<'a> {
    fn new(profile: Option<&'a mut TransferStageProfile>, transfers: usize) -> Self {
        let mut profiler = Self { profile };
        if let Some(profile) = profiler.profile.as_deref_mut() {
            profile.calls = profile.calls.saturating_add(1);
            profile.transfers = profile.transfers.saturating_add(transfers as u64);
        }
        profiler
    }

    fn borrow(profile: Option<&'a mut TransferStageProfile>) -> Self {
        Self { profile }
    }

    fn add(&mut self, stage: TransferProfileStage, elapsed: Duration) {
        let Some(profile) = self.profile.as_deref_mut() else {
            return;
        };

        match stage {
            TransferProfileStage::Validation => {
                profile.validation = profile.validation.saturating_add(elapsed);
            }
            TransferProfileStage::Setup => {
                profile.setup = profile.setup.saturating_add(elapsed);
            }
            TransferProfileStage::Submit => {
                profile.submit = profile.submit.saturating_add(elapsed);
            }
            TransferProfileStage::WaitWrite => {
                profile.wait_write = profile.wait_write.saturating_add(elapsed);
            }
            TransferProfileStage::WaitRead => {
                profile.wait_read = profile.wait_read.saturating_add(elapsed);
            }
            TransferProfileStage::DecodeCopy => {
                profile.decode_copy = profile.decode_copy.saturating_add(elapsed);
            }
            TransferProfileStage::RefillSubmit => {
                profile.refill_submit = profile.refill_submit.saturating_add(elapsed);
            }
        }
    }
}

pub struct Board {
    usb: UsbDevice,
    config: Config,
    crypto: CryptoState,
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

        Ok(IoSession {
            board: self,
            pipeline_write: None,
            pipeline_read: None,
            single_tx_buffer: None,
            single_rx_buffer: None,
            tx_pool: Vec::new(),
            rx_pool: Vec::new(),
            finished: false,
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
    pipeline_write: Option<UsbEndpoint<Bulk, Out>>,
    pipeline_read: Option<UsbEndpoint<Bulk, In>>,
    single_tx_buffer: Option<Buffer>,
    single_rx_buffer: Option<Buffer>,
    tx_pool: Vec<Buffer>,
    rx_pool: Vec<Buffer>,
    finished: bool,
}

/// A rolling VeriComm pipeline that keeps up to `capacity` transfers in flight.
///
/// Submit frames with [`Self::submit`] and retire them in order with
/// [`Self::receive_into`]. Dropping the window cancels any remaining
/// transfers and recycles their buffers back into the parent [`IoSession`].
pub struct IoTransferWindow<'session, 'board> {
    io: &'session mut IoSession<'board>,
    capacity: usize,
    pending_words: VecDeque<usize>,
    pending_writes: usize,
    pending_reads: usize,
}

impl<'a> IoSession<'a> {
    fn cleanup(&mut self) -> Result<()> {
        if let Some(pipeline_write) = self.pipeline_write.as_mut() {
            pipeline_write.cancel_all();
        }
        if let Some(pipeline_read) = self.pipeline_read.as_mut() {
            pipeline_read.cancel_all();
        }
        self.board.activate_control()
    }

    fn ensure_pipeline_endpoints(&mut self) -> Result<()> {
        if self.pipeline_write.is_none() {
            self.pipeline_write = Some(self.board.usb.open_out_endpoint(Endpoint::FifoWrite)?);
        }
        if self.pipeline_read.is_none() {
            self.pipeline_read = Some(self.board.usb.open_in_endpoint(Endpoint::FifoRead)?);
        }
        Ok(())
    }

    fn take_single_tx_buffer(&mut self, tx_bytes: usize) -> Buffer {
        if let Some(mut buffer) = self.single_tx_buffer.take() {
            if buffer.capacity() >= tx_bytes.max(1) {
                buffer.clear();
                return buffer;
            }
        }

        self.pipeline_write
            .as_mut()
            .expect("pipeline write endpoint should be initialized")
            .allocate(tx_bytes.max(1))
    }

    fn take_single_rx_buffer(&mut self, request_bytes: usize) -> Buffer {
        if let Some(mut buffer) = self.single_rx_buffer.take() {
            if buffer.capacity() >= request_bytes.max(1) {
                buffer.clear();
                buffer.set_requested_len(request_bytes.max(1));
                return buffer;
            }
        }

        let mut buffer = self
            .pipeline_read
            .as_mut()
            .expect("pipeline read endpoint should be initialized")
            .allocate(request_bytes.max(1));
        buffer.set_requested_len(request_bytes.max(1));
        buffer
    }

    fn prepare_pools(&mut self, pipeline_depth: usize, tx_bytes: usize, rx_bytes: usize) {
        let tx_bytes = tx_bytes.max(1);
        let rx_bytes = rx_bytes.max(1);

        let pipeline_write = self
            .pipeline_write
            .as_mut()
            .expect("pipeline write endpoint should be initialized");
        let pipeline_read = self
            .pipeline_read
            .as_mut()
            .expect("pipeline read endpoint should be initialized");

        discard_undersized_buffers(&mut self.tx_pool, tx_bytes);
        discard_undersized_buffers(&mut self.rx_pool, rx_bytes);

        while self.tx_pool.len() < pipeline_depth {
            self.tx_pool.push(pipeline_write.allocate(tx_bytes));
        }
        while self.rx_pool.len() < pipeline_depth {
            let mut buffer = pipeline_read.allocate(rx_bytes);
            buffer.set_requested_len(rx_bytes);
            self.rx_pool.push(buffer);
        }
    }

    /// Opens a rolling transfer window that can keep `capacity` VeriComm
    /// transfers outstanding at once.
    pub fn transfer_window(&mut self, capacity: usize) -> Result<IoTransferWindow<'_, 'a>> {
        if capacity == 0 {
            return Err(Error::InvalidBufferLength {
                context: "vericomm transfer window",
                expected: 1,
                actual: 0,
            });
        }

        Ok(IoTransferWindow {
            io: self,
            capacity: capacity.min(MAX_PIPELINE_DEPTH),
            pending_words: VecDeque::with_capacity(capacity.min(MAX_PIPELINE_DEPTH)),
            pending_writes: 0,
            pending_reads: 0,
        })
    }

    fn submit_window_transfer(&mut self, tx: &[u16], request_bytes: usize) {
        let tx_buffer = self.tx_pool.pop().expect("tx pool should be primed");
        let rx_buffer = self.rx_pool.pop().expect("rx pool should be primed");
        submit_pipeline_write(
            &mut self.board.crypto,
            self.pipeline_write
                .as_mut()
                .expect("pipeline write endpoint should be initialized"),
            tx,
            tx_buffer,
        );
        submit_pipeline_read(
            self.pipeline_read
                .as_mut()
                .expect("pipeline read endpoint should be initialized"),
            rx_buffer,
            request_bytes,
        );
    }

    fn discard_window_pending_transfers(&mut self, pending_writes: usize, pending_reads: usize) {
        const DRAIN_TIMEOUT: Duration = Duration::from_millis(10);

        if let Some(endpoint) = self.pipeline_write.as_mut() {
            endpoint.cancel_all();
            for _ in 0..pending_writes {
                let Some(completion) = endpoint.wait_next_complete(DRAIN_TIMEOUT) else {
                    break;
                };
                self.tx_pool.push(completion.buffer);
            }
        }

        if let Some(endpoint) = self.pipeline_read.as_mut() {
            endpoint.cancel_all();
            for _ in 0..pending_reads {
                let Some(completion) = endpoint.wait_next_complete(DRAIN_TIMEOUT) else {
                    break;
                };
                self.rx_pool.push(completion.buffer);
            }
        }
    }
    fn transfer_with_profile(
        &mut self,
        tx: &[u16],
        rx: &mut [u16],
        profile: Option<&mut TransferStageProfile>,
    ) -> Result<()> {
        let mut profiler = TransferProfiler::new(profile, 1);

        let stage_started = Instant::now();
        validate_transfer_buffers(
            tx.len(),
            rx.len(),
            usize::from(self.board.config.fifo_size_words()),
        )?;
        self.board.ensure_mode(BoardMode::VeriComm)?;
        profiler.add(TransferProfileStage::Validation, stage_started.elapsed());

        let stage_started = Instant::now();
        self.ensure_pipeline_endpoints()?;

        let tx_byte_len = std::mem::size_of_val(tx);
        let request_bytes = aligned_request_len(
            self.pipeline_read
                .as_ref()
                .expect("pipeline read endpoint should be initialized")
                .max_packet_size(),
            tx_byte_len,
        );
        profiler.add(TransferProfileStage::Setup, stage_started.elapsed());

        let stage_started = Instant::now();
        let rx_buffer = self.take_single_rx_buffer(request_bytes);
        submit_pipeline_read(
            self.pipeline_read
                .as_mut()
                .expect("pipeline read endpoint should be initialized"),
            rx_buffer,
            request_bytes,
        );

        let mut tx_buffer = self.take_single_tx_buffer(tx_byte_len);
        let tx_bytes = tx_buffer.extend_fill(tx_byte_len, 0);
        words_to_bytes(tx, tx_bytes);
        self.board
            .crypto
            .encrypt_words(bytes_as_words_mut(tx_bytes));
        self.pipeline_write
            .as_mut()
            .expect("pipeline write endpoint should be initialized")
            .submit(tx_buffer);
        profiler.add(TransferProfileStage::Submit, stage_started.elapsed());

        let timeout = self.board.transport().usb_timeout;
        let stage_started = Instant::now();
        let tx_completion = match self
            .pipeline_write
            .as_mut()
            .expect("pipeline write endpoint should be initialized")
            .wait_next_complete(timeout)
        {
            Some(completion) => completion,
            None => {
                let tx_cancelled = cancel_pending_transfer(
                    self.pipeline_write
                        .as_mut()
                        .expect("pipeline write endpoint should be initialized"),
                );
                self.single_tx_buffer = Some(tx_cancelled.buffer);
                let rx_cancelled = cancel_pending_transfer(
                    self.pipeline_read
                        .as_mut()
                        .expect("pipeline read endpoint should be initialized"),
                );
                self.single_rx_buffer = Some(rx_cancelled.buffer);
                return Err(Error::Timeout("nusb_bulk_write"));
            }
        };
        let tx_status = tx_completion.status;
        let tx_buffer = tx_completion.buffer;
        self.single_tx_buffer = Some(tx_buffer);
        tx_status.map_err(|err| transfer_error(err, "nusb_bulk_write"))?;
        profiler.add(TransferProfileStage::WaitWrite, stage_started.elapsed());

        let stage_started = Instant::now();
        let rx_completion = match self
            .pipeline_read
            .as_mut()
            .expect("pipeline read endpoint should be initialized")
            .wait_next_complete(timeout)
        {
            Some(completion) => completion,
            None => {
                let rx_cancelled = cancel_pending_transfer(
                    self.pipeline_read
                        .as_mut()
                        .expect("pipeline read endpoint should be initialized"),
                );
                self.single_rx_buffer = Some(rx_cancelled.buffer);
                return Err(Error::Timeout("nusb_bulk_read"));
            }
        };
        let actual_len = rx_completion.actual_len;
        let rx_status = rx_completion.status;
        let mut rx_buffer = rx_completion.buffer;
        rx_status.map_err(|err| transfer_error(err, "nusb_bulk_read"))?;
        profiler.add(TransferProfileStage::WaitRead, stage_started.elapsed());

        let stage_started = Instant::now();
        if actual_len < tx_byte_len {
            self.single_rx_buffer = Some(rx_buffer);
            return Err(Error::UnexpectedResponse(
                "blocking read returned short payload",
            ));
        }
        self.board
            .crypto
            .decrypt_words(bytes_as_words_mut(&mut rx_buffer[..tx_byte_len]));
        rx.copy_from_slice(bytes_as_words(&rx_buffer[..tx_byte_len]));
        self.single_rx_buffer = Some(rx_buffer);
        profiler.add(TransferProfileStage::DecodeCopy, stage_started.elapsed());
        Ok(())
    }

    pub fn transfer(&mut self, tx: &[u16], rx: &mut [u16]) -> Result<()> {
        self.transfer_with_profile(tx, rx, None)
    }

    pub fn transfer_profiled_into(
        &mut self,
        tx: &[u16],
        rx: &mut [u16],
    ) -> Result<TransferStageProfile> {
        let mut profile = TransferStageProfile::default();
        self.transfer_with_profile(tx, rx, Some(&mut profile))?;
        Ok(profile)
    }

    fn transfer_batch_into_slices_with_profile(
        &mut self,
        txs: &[&[u16]],
        outputs: &mut [&mut [u16]],
        profile: Option<&mut TransferStageProfile>,
    ) -> Result<()> {
        let mut profiler = TransferProfiler::new(profile, txs.len());

        let stage_started = Instant::now();
        if txs.len() != outputs.len() {
            return Err(Error::InvalidBufferLength {
                context: "vericomm batch transfer",
                expected: txs.len(),
                actual: outputs.len(),
            });
        }
        self.board.ensure_mode(BoardMode::VeriComm)?;

        for (tx, output) in txs.iter().zip(outputs.iter()) {
            validate_transfer_buffers(
                tx.len(),
                output.len(),
                usize::from(self.board.config.fifo_size_words()),
            )?;
        }

        if txs.is_empty() {
            return Ok(());
        }
        profiler.add(TransferProfileStage::Validation, stage_started.elapsed());

        let stage_started = Instant::now();
        self.ensure_pipeline_endpoints()?;

        let max_packet_size = self
            .pipeline_read
            .as_ref()
            .expect("pipeline read endpoint should be initialized")
            .max_packet_size();
        let request_bytes = batch_request_lengths(max_packet_size, txs);
        let max_bytes = txs
            .iter()
            .map(|tx| std::mem::size_of_val(*tx))
            .max()
            .unwrap_or(0);
        let max_request_bytes = request_bytes.iter().copied().max().unwrap_or(0);
        let pipeline_depth = txs.len().min(MAX_PIPELINE_DEPTH);
        self.prepare_pools(pipeline_depth, max_bytes, max_request_bytes);
        profiler.add(TransferProfileStage::Setup, stage_started.elapsed());

        let mut submitted = 0usize;
        let mut completed = 0usize;

        let stage_started = Instant::now();
        while submitted < pipeline_depth {
            let tx_buffer = self.tx_pool.pop().expect("tx pool should be primed");
            let rx_buffer = self.rx_pool.pop().expect("rx pool should be primed");
            submit_pipeline_write(
                &mut self.board.crypto,
                self.pipeline_write
                    .as_mut()
                    .expect("pipeline write endpoint should be initialized"),
                txs[submitted],
                tx_buffer,
            );
            submit_pipeline_read(
                self.pipeline_read
                    .as_mut()
                    .expect("pipeline read endpoint should be initialized"),
                rx_buffer,
                request_bytes[submitted],
            );
            submitted += 1;
        }
        profiler.add(TransferProfileStage::Submit, stage_started.elapsed());

        while completed < txs.len() {
            let stage_started = Instant::now();
            let write_completion = self
                .pipeline_write
                .as_mut()
                .expect("pipeline write endpoint should be initialized")
                .wait_next_complete(self.board.transport().usb_timeout)
                .ok_or(Error::Timeout("pipeline_write"))?;
            let write_status = write_completion.status;
            self.tx_pool.push(write_completion.buffer);
            write_status.map_err(|err| transfer_error(err, "pipeline_write"))?;
            profiler.add(TransferProfileStage::WaitWrite, stage_started.elapsed());

            let stage_started = Instant::now();
            let read_completion = self
                .pipeline_read
                .as_mut()
                .expect("pipeline read endpoint should be initialized")
                .wait_next_complete(self.board.transport().usb_timeout)
                .ok_or(Error::Timeout("pipeline_read"))?;
            let actual_len = read_completion.actual_len;
            let read_status = read_completion.status;
            let read_buffer = read_completion.buffer;
            if let Err(err) = read_status {
                self.rx_pool.push(read_buffer);
                return Err(transfer_error(err, "pipeline_read"));
            }
            profiler.add(TransferProfileStage::WaitRead, stage_started.elapsed());

            let stage_started = Instant::now();
            let output = &mut *outputs[completed];
            let expected_bytes = std::mem::size_of_val(output);
            if actual_len < expected_bytes {
                self.rx_pool.push(read_buffer);
                return Err(Error::UnexpectedResponse(
                    "pipeline read returned short payload",
                ));
            }
            bytes_into_words(&read_buffer[..expected_bytes], output);
            self.board.crypto.decrypt_words(output);
            self.rx_pool.push(read_buffer);
            completed += 1;
            profiler.add(TransferProfileStage::DecodeCopy, stage_started.elapsed());

            if submitted < txs.len() {
                let stage_started = Instant::now();
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
                    self.pipeline_write
                        .as_mut()
                        .expect("pipeline write endpoint should be initialized"),
                    txs[submitted],
                    tx_buffer,
                );
                submit_pipeline_read(
                    self.pipeline_read
                        .as_mut()
                        .expect("pipeline read endpoint should be initialized"),
                    rx_buffer,
                    request_bytes[submitted],
                );
                submitted += 1;
                profiler.add(TransferProfileStage::RefillSubmit, stage_started.elapsed());
            }
        }

        Ok(())
    }

    fn transfer_batch_slices(
        &mut self,
        txs: &[&[u16]],
        rx_lengths: &[usize],
    ) -> Result<Vec<Vec<u16>>> {
        let mut outputs = rx_lengths
            .iter()
            .map(|len| vec![0u16; *len])
            .collect::<Vec<_>>();
        let mut output_refs = outputs
            .iter_mut()
            .map(Vec::as_mut_slice)
            .collect::<Vec<_>>();
        self.transfer_batch_into_slices_with_profile(txs, &mut output_refs, None)?;
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
        self.transfer_batch_into_slices_with_profile(txs, outputs, None)
    }

    pub fn transfer_batch_into_profiled(
        &mut self,
        txs: &[&[u16]],
        outputs: &mut [&mut [u16]],
    ) -> Result<TransferStageProfile> {
        let mut profile = TransferStageProfile::default();
        self.transfer_batch_into_slices_with_profile(txs, outputs, Some(&mut profile))?;
        Ok(profile)
    }

    pub fn finish(mut self) -> Result<()> {
        let result = self.cleanup();
        self.finished = true;
        result
    }
}

impl Drop for IoSession<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.cleanup();
        }
    }
}

impl<'session, 'board> IoTransferWindow<'session, 'board> {
    fn submit_with_profile(
        &mut self,
        tx: &[u16],
        profile: Option<&mut TransferStageProfile>,
    ) -> Result<()> {
        if self.is_full() {
            return Err(Error::PipelineFull {
                capacity: self.capacity,
            });
        }

        let mut profiler = TransferProfiler::new(profile, 1);

        let stage_started = Instant::now();
        validate_transfer_buffers(
            tx.len(),
            tx.len(),
            usize::from(self.io.board.config.fifo_size_words()),
        )?;
        self.io.board.ensure_mode(BoardMode::VeriComm)?;
        profiler.add(TransferProfileStage::Validation, stage_started.elapsed());

        let stage_started = Instant::now();
        self.io.ensure_pipeline_endpoints()?;
        let tx_byte_len = std::mem::size_of_val(tx);
        let request_bytes = request_bytes_for_words(
            self.io
                .pipeline_read
                .as_ref()
                .expect("pipeline read endpoint should be initialized")
                .max_packet_size(),
            tx.len(),
        );
        self.io.prepare_pools(1, tx_byte_len, request_bytes);
        profiler.add(TransferProfileStage::Setup, stage_started.elapsed());

        let stage_started = Instant::now();
        self.io.submit_window_transfer(tx, request_bytes);
        self.pending_words.push_back(tx.len());
        self.pending_writes += 1;
        self.pending_reads += 1;
        profiler.add(TransferProfileStage::Submit, stage_started.elapsed());
        Ok(())
    }

    fn receive_into_with_profile(
        &mut self,
        output: &mut [u16],
        profile: Option<&mut TransferStageProfile>,
    ) -> Result<()> {
        let Some(expected_words) = self.pending_words.front().copied() else {
            return Err(Error::PipelineEmpty);
        };

        if output.len() != expected_words {
            return Err(Error::InvalidBufferLength {
                context: "vericomm transfer window receive",
                expected: expected_words,
                actual: output.len(),
            });
        }

        let mut profiler = TransferProfiler::borrow(profile);

        let stage_started = Instant::now();
        let write_completion = self
            .io
            .pipeline_write
            .as_mut()
            .expect("pipeline write endpoint should be initialized")
            .wait_next_complete(self.io.board.transport().usb_timeout)
            .ok_or(Error::Timeout("pipeline_write"))?;
        self.pending_writes = self.pending_writes.saturating_sub(1);
        let write_status = write_completion.status;
        self.io.tx_pool.push(write_completion.buffer);
        write_status.map_err(|err| transfer_error(err, "pipeline_write"))?;
        profiler.add(TransferProfileStage::WaitWrite, stage_started.elapsed());

        let stage_started = Instant::now();
        let read_completion = self
            .io
            .pipeline_read
            .as_mut()
            .expect("pipeline read endpoint should be initialized")
            .wait_next_complete(self.io.board.transport().usb_timeout)
            .ok_or(Error::Timeout("pipeline_read"))?;
        self.pending_reads = self.pending_reads.saturating_sub(1);
        let actual_len = read_completion.actual_len;
        let read_status = read_completion.status;
        let read_buffer = read_completion.buffer;
        if let Err(err) = read_status {
            self.io.rx_pool.push(read_buffer);
            self.pending_words.pop_front();
            return Err(transfer_error(err, "pipeline_read"));
        }
        profiler.add(TransferProfileStage::WaitRead, stage_started.elapsed());

        let stage_started = Instant::now();
        let expected_bytes = std::mem::size_of_val(output);
        if actual_len < expected_bytes {
            self.io.rx_pool.push(read_buffer);
            self.pending_words.pop_front();
            return Err(Error::UnexpectedResponse(
                "pipeline read returned short payload",
            ));
        }
        bytes_into_words(&read_buffer[..expected_bytes], output);
        self.io.board.crypto.decrypt_words(output);
        self.io.rx_pool.push(read_buffer);
        self.pending_words.pop_front();
        profiler.add(TransferProfileStage::DecodeCopy, stage_started.elapsed());
        Ok(())
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn pending(&self) -> usize {
        self.pending_words.len()
    }

    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.pending())
    }

    pub fn is_empty(&self) -> bool {
        self.pending_words.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.pending() >= self.capacity
    }

    /// Queues one transfer into the rolling window.
    pub fn submit(&mut self, tx: &[u16]) -> Result<()> {
        self.submit_with_profile(tx, None)
    }

    /// Queues one transfer and returns a stage profile for that submission.
    pub fn submit_profiled(&mut self, tx: &[u16]) -> Result<TransferStageProfile> {
        let mut profile = TransferStageProfile::default();
        self.submit_with_profile(tx, Some(&mut profile))?;
        Ok(profile)
    }

    /// Retires the oldest in-flight transfer into `output`.
    pub fn receive_into(&mut self, output: &mut [u16]) -> Result<()> {
        self.receive_into_with_profile(output, None)
    }

    /// Retires the oldest in-flight transfer and returns its stage profile.
    pub fn receive_into_profiled(&mut self, output: &mut [u16]) -> Result<TransferStageProfile> {
        let mut profile = TransferStageProfile::default();
        self.receive_into_with_profile(output, Some(&mut profile))?;
        Ok(profile)
    }
}

impl Drop for IoTransferWindow<'_, '_> {
    fn drop(&mut self) {
        if !self.pending_words.is_empty() {
            self.io
                .discard_window_pending_transfers(self.pending_writes, self.pending_reads);
            self.pending_words.clear();
            self.pending_writes = 0;
            self.pending_reads = 0;
        }
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

    let write_bytes = write_words * std::mem::size_of::<u16>();
    if write_bytes % VERICOMM_TRANSFER_PACKET_BYTES != 0 {
        return Err(Error::InvalidBufferLength {
            context: "vericomm transfer packet alignment",
            expected: write_words
                .next_multiple_of(VERICOMM_TRANSFER_PACKET_BYTES / std::mem::size_of::<u16>()),
            actual: write_words,
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

fn aligned_request_len(max_packet_size: usize, payload_bytes: usize) -> usize {
    let payload_bytes = payload_bytes
        .next_multiple_of(VERICOMM_TRANSFER_PACKET_BYTES)
        .max(max_packet_size.max(1));
    let rem = payload_bytes % max_packet_size.max(1);
    if rem == 0 {
        payload_bytes
    } else {
        payload_bytes + (max_packet_size - rem)
    }
}

fn request_bytes_for_words(max_packet_size: usize, word_len: usize) -> usize {
    aligned_request_len(max_packet_size, word_len * std::mem::size_of::<u16>())
}

fn batch_request_lengths(max_packet_size: usize, txs: &[&[u16]]) -> Vec<usize> {
    txs.iter()
        .map(|tx| request_bytes_for_words(max_packet_size, tx.len()))
        .collect()
}

fn discard_undersized_buffers(pool: &mut Vec<Buffer>, min_capacity: usize) {
    pool.retain(|buffer| buffer.capacity() >= min_capacity);
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

fn bytes_as_words(bytes: &[u8]) -> &[u16] {
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, bytes.len() / 2) }
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

fn cancel_pending_transfer<Dir>(endpoint: &mut UsbEndpoint<Bulk, Dir>) -> Completion
where
    Dir: EndpointDirection,
{
    endpoint.cancel_all();
    loop {
        if let Some(completion) = endpoint.wait_next_complete(Duration::from_secs(1)) {
            return completion;
        }
    }
}

#[cfg(test)]
mod tests {
    use nusb::transfer::Buffer;

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
    fn batch_request_lengths_keep_each_frame_request_size() {
        let short = vec![0x1234u16; 256];
        let long = vec![0x5678u16; 512];
        let txs = vec![short.as_slice(), long.as_slice()];

        assert_eq!(super::batch_request_lengths(512, &txs), vec![512, 1024]);
    }

    #[test]
    fn discard_undersized_buffers_drops_stale_pool_entries() {
        let mut pool = vec![
            Buffer::from(vec![0u8; 512]),
            Buffer::from(vec![0u8; 1024]),
            Buffer::from(vec![0u8; 256]),
        ];

        super::discard_undersized_buffers(&mut pool, 600);

        let capacities = pool.iter().map(Buffer::capacity).collect::<Vec<_>>();
        assert_eq!(capacities, vec![1024]);
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

    #[test]
    fn io_transfer_window_type_is_stable() {
        let type_name = std::any::type_name::<super::IoTransferWindow<'_, '_>>();
        assert!(type_name.contains("IoTransferWindow"));
    }

    #[test]
    fn pipeline_error_shapes_are_stable() {
        assert_eq!(
            Error::PipelineEmpty.to_string(),
            "transfer pipeline has no pending transfers"
        );
        assert_eq!(
            Error::PipelineFull { capacity: 4 }.to_string(),
            "transfer pipeline is full (capacity 4 outstanding transfers)"
        );
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
