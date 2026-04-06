use crate::config::Config;
use crate::constants;
use crate::error::{Error, Result};
use crate::usb::{Endpoint, UsbDevice};
use std::{
    cell::Cell,
    time::{Duration, Instant},
};

const SYNC_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROL_COMMAND_PREFIX: u8 = 0x01;

/// High-level interface for talking to the SMIMS VLFD device.
///
/// The type owns the underlying USB session and keeps the remote configuration
/// cached locally, providing ergonomic helpers around the device's
/// configuration and command protocols.
pub struct Device {
    usb: UsbDevice,
    config: Config,
    encryption: EncryptionState,
    transfer_scratch: Vec<u16>,
    session: Cell<SessionState>,
}

impl Device {
    pub fn new() -> Result<Self> {
        Ok(Self {
            usb: UsbDevice::new()?,
            config: Config::new(),
            encryption: EncryptionState::default(),
            transfer_scratch: Vec::new(),
            session: Cell::new(SessionState::default()),
        })
    }

    pub fn connect() -> Result<Self> {
        let mut device = Self::new()?;
        device.ensure_session()?;
        Ok(device)
    }

    pub fn usb(&self) -> &UsbDevice {
        &self.usb
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    pub fn is_open(&self) -> bool {
        self.usb.is_open()
    }

    pub fn is_initialized(&self) -> bool {
        self.session.get().initialized
    }

    pub fn mode(&self) -> DeviceMode {
        self.session.get().mode
    }

    pub fn open(&mut self) -> Result<()> {
        if self.is_open() {
            return Ok(());
        }
        self.usb.open(constants::DW_VID, constants::DW_PID)?;
        self.reset_local_state(SessionState::opened());
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        self.usb.close()?;
        self.reset_local_state(SessionState::default());
        Ok(())
    }

    pub fn initialize(&mut self) -> Result<()> {
        self.ensure_open()?;
        self.initialize_unchecked()
    }

    pub fn reset_engine(&self) -> Result<()> {
        self.usb.write_bytes(Endpoint::Command, &[0x02])
    }

    pub fn ensure_session(&mut self) -> Result<()> {
        self.ensure_open()?;
        if !self.is_initialized() {
            self.initialize_unchecked()?;
        }
        Ok(())
    }

    pub fn enter_io_mode(&mut self, settings: &IoSettings) -> Result<()> {
        self.ensure_session()?;

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

        self.write_config_unchecked()?;
        self.activate_mode_checked(DeviceMode::VeriComm)
    }

    /// Performs a VeriComm FIFO round-trip without mutating `write_buffer`.
    pub fn transfer_io(&mut self, write_buffer: &mut [u16], read_buffer: &mut [u16]) -> Result<()> {
        self.transfer_io_words(write_buffer, read_buffer)
    }

    /// Performs a VeriComm FIFO round-trip without mutating `write_buffer`.
    pub fn transfer_io_words(
        &mut self,
        write_buffer: &[u16],
        read_buffer: &mut [u16],
    ) -> Result<()> {
        self.ensure_session()?;
        self.ensure_mode(DeviceMode::VeriComm)?;
        validate_transfer_buffers(
            write_buffer.len(),
            read_buffer.len(),
            usize::from(self.config.fifo_size_words()),
        )?;

        let usb = &self.usb;
        let encrypted = prepare_encrypted_write_buffer(
            &mut self.encryption,
            &mut self.transfer_scratch,
            write_buffer,
        );
        usb.write_words(Endpoint::FifoWrite, encrypted)?;
        self.fifo_read(read_buffer)?;
        self.decrypt(read_buffer);
        Ok(())
    }

    pub fn exit_io_mode(&mut self) -> Result<()> {
        if !self.is_open() {
            return Ok(());
        }

        self.command_active_unchecked()?;
        self.close()
    }

    pub fn fifo_capacity_words(&self) -> Option<usize> {
        self.is_initialized()
            .then(|| usize::from(self.config.fifo_size_words()))
    }

    pub fn fifo_write(&self, buffer: &[u16]) -> Result<()> {
        self.usb.write_words(Endpoint::FifoWrite, buffer)
    }

    pub fn fifo_read(&self, buffer: &mut [u16]) -> Result<()> {
        self.usb.read_words(Endpoint::FifoRead, buffer)
    }

    pub fn sync_delay(&self) -> Result<()> {
        if !self.is_open() {
            return Err(Error::DeviceNotOpen);
        }
        self.sync_delay_unchecked()
    }

    pub fn command_active(&self) -> Result<()> {
        if !self.is_open() {
            return Err(Error::DeviceNotOpen);
        }
        self.command_active_unchecked()
    }

    pub fn read_config(&mut self) -> Result<()> {
        if !self.is_initialized() {
            self.ensure_session()?;
            return Ok(());
        }

        self.read_config_unchecked()
    }

    pub fn write_config(&mut self) -> Result<()> {
        self.ensure_session()?;
        self.write_config_unchecked()
    }

    pub fn activate_fpga_programmer(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::FpgaProgrammer)
    }

    pub fn activate_vericomm(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::VeriComm)
    }

    pub fn activate_veri_instrument(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::VeriInstrument)
    }

    pub fn activate_verilink(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::VeriLink)
    }

    pub fn activate_veri_soc(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::VeriSoc)
    }

    pub fn activate_vericomm_pro(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::VeriCommPro)
    }

    pub fn activate_veri_sdk(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::VeriSdk)
    }

    pub fn activate_flash_read(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::FlashRead)
    }

    pub fn activate_flash_write(&self) -> Result<()> {
        self.activate_mode_raw(DeviceMode::FlashWrite)
    }

    pub fn encrypt(&mut self, buffer: &mut [u16]) {
        self.encryption.encrypt_words(buffer);
    }

    pub fn decrypt(&mut self, buffer: &mut [u16]) {
        self.encryption.decrypt_words(buffer);
    }

    pub fn licence_gen(&self, security_key: u16, customer_id: u16) -> u16 {
        licence_gen(security_key, customer_id)
    }

    pub(crate) fn activate_fpga_programmer_checked(&mut self) -> Result<()> {
        self.activate_mode_checked(DeviceMode::FpgaProgrammer)
    }

    fn ensure_open(&mut self) -> Result<()> {
        if !self.is_open() {
            self.open()?;
        }
        Ok(())
    }

    fn initialize_unchecked(&mut self) -> Result<()> {
        self.read_encrypt_table_unchecked()?;
        self.encryption.decode_table();
        self.read_config_unchecked()
    }

    fn ensure_mode(&self, expected: DeviceMode) -> Result<()> {
        let actual = self.mode();
        if actual != expected {
            return Err(Error::InvalidMode {
                expected: expected.as_str(),
                actual: actual.as_str(),
            });
        }
        Ok(())
    }

    fn reset_local_state(&mut self, session: SessionState) {
        self.config = Config::new();
        self.encryption = EncryptionState::default();
        self.transfer_scratch.clear();
        self.session.set(session);
    }

    fn sync_delay_unchecked(&self) -> Result<()> {
        let start = Instant::now();
        let mut buffer = [0u8; 1];

        while start.elapsed() <= SYNC_TIMEOUT {
            self.usb.write_bytes(Endpoint::Command, &buffer)?;
            self.usb.read_bytes(Endpoint::Sync, &mut buffer)?;
            if buffer[0] != 0 {
                return Ok(());
            }
        }

        Err(Error::Timeout("sync_delay"))
    }

    fn command_active_unchecked(&self) -> Result<()> {
        self.sync_delay_unchecked()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x00])?;
        self.session
            .set(self.session.get().with_mode(DeviceMode::Control));
        Ok(())
    }

    fn read_config_unchecked(&mut self) -> Result<()> {
        self.sync_delay_unchecked()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x01])?;

        let mut words = [0u16; Config::WORD_COUNT];
        self.usb.read_words(Endpoint::FifoRead, &mut words)?;
        self.command_active_unchecked()?;
        self.decrypt(&mut words);
        self.config = Config::from_words(words);
        self.session
            .set(SessionState::initialized(DeviceMode::Control));
        Ok(())
    }

    fn write_config_unchecked(&mut self) -> Result<()> {
        self.sync_delay_unchecked()?;
        let mut words = *self.config.words();
        self.encrypt(&mut words);
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x11])?;
        self.usb.write_words(Endpoint::FifoWrite, &words)?;
        self.command_active_unchecked()?;
        self.session
            .set(SessionState::initialized(DeviceMode::Control));
        Ok(())
    }

    fn activate_mode_checked(&mut self, mode: DeviceMode) -> Result<()> {
        self.ensure_session()?;
        self.activate_mode_raw(mode)
    }

    fn activate_mode_raw(&self, mode: DeviceMode) -> Result<()> {
        let Some(command) = mode.command_byte() else {
            return Err(Error::UnexpectedResponse("unsupported mode command"));
        };
        self.sync_delay()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, command])?;
        self.session.set(self.session.get().with_mode(mode));
        Ok(())
    }

    fn read_encrypt_table_unchecked(&mut self) -> Result<()> {
        self.sync_delay_unchecked()?;
        self.usb
            .write_bytes(Endpoint::Command, &[CONTROL_COMMAND_PREFIX, 0x0f])?;
        self.usb
            .read_words(Endpoint::FifoRead, self.encryption.table_mut())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMode {
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

impl DeviceMode {
    fn as_str(self) -> &'static str {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionState {
    initialized: bool,
    mode: DeviceMode,
}

impl SessionState {
    const fn opened() -> Self {
        Self {
            initialized: false,
            mode: DeviceMode::Unknown,
        }
    }

    const fn initialized(mode: DeviceMode) -> Self {
        Self {
            initialized: true,
            mode,
        }
    }

    const fn with_mode(self, mode: DeviceMode) -> Self {
        Self {
            initialized: self.initialized,
            mode,
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            initialized: false,
            mode: DeviceMode::Closed,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EncryptionState {
    table: [u16; 32],
    encode_index: usize,
    decode_index: usize,
}

impl EncryptionState {
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

fn prepare_encrypted_write_buffer<'a>(
    encryption: &mut EncryptionState,
    scratch: &'a mut Vec<u16>,
    write_buffer: &[u16],
) -> &'a [u16] {
    scratch.clear();
    scratch.extend_from_slice(write_buffer);
    encryption.encrypt_words(scratch);
    scratch.as_slice()
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

/// Fine-grained tuning options when switching the device into VeriComm I/O
/// mode.
#[derive(Debug, Clone)]
pub struct IoSettings {
    pub clock_high_delay: u16,
    pub clock_low_delay: u16,
    pub vericomm_isv: u8,
    pub clock_check_enabled: bool,
    pub mode_selector: u8,
    pub licence_key: Option<u16>,
}

impl Default for IoSettings {
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

fn licence_gen(security_key: u16, customer_id: u16) -> u16 {
    let mut temp: u32 = 0;

    let mut i: u16 = security_key & 0x0003;
    let mut j: u16 = (customer_id & 0x000f) << 4;
    j >>= i;
    j = (j >> 4) | (j & 0x000f);
    temp |= (j as u32) << 16;

    i = (security_key & 0x0030) >> 4;
    j = customer_id & 0x00f0;
    j >>= i;
    j = (j >> 4) | (j & 0x000f);
    temp |= (j as u32) << 20;

    i = (security_key & 0x0300) >> 8;
    j = (customer_id & 0x0f00) >> 4;
    j >>= i;
    j = (j >> 4) | (j & 0x000f);
    temp |= (j as u32) << 24;

    i = (security_key & 0x3000) >> 12;
    j = (customer_id & 0xf000) >> 8;
    j >>= i;
    j = (j >> 4) | (j & 0x000f);
    temp |= (j as u32) << 28;

    temp >>= 11;
    !((temp >> 16) | (temp & 0x0000ffff)) as u16
}

#[cfg(test)]
mod tests {
    use super::{
        Device, DeviceMode, EncryptionState, Error, prepare_encrypted_write_buffer,
        validate_transfer_buffers,
    };

    #[test]
    fn new_device_starts_closed_and_uninitialized() {
        let device = Device::new().expect("failed to initialise USB context");
        assert!(!device.is_open());
        assert!(!device.is_initialized());
        assert_eq!(device.mode(), DeviceMode::Closed);
    }

    #[test]
    fn encrypted_transfer_buffer_is_copied_before_mutation() {
        let mut encryption = EncryptionState::default();
        let mut scratch = Vec::new();
        encryption.table[0] = 0x00ff;
        let input = [0x1234u16, 0xabcd];
        let encrypted = prepare_encrypted_write_buffer(&mut encryption, &mut scratch, &input);

        assert_eq!(input, [0x1234, 0xabcd]);
        assert_eq!(encrypted, &[0x12cb, 0xabcd]);
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
}
