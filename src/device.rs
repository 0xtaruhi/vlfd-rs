use crate::config::Config;
use crate::constants;
use crate::error::{Error, Result};
use crate::usb::{Endpoint, UsbDevice};
use std::time::{Duration, Instant};

const SYNC_TIMEOUT: Duration = Duration::from_secs(1);

/// High-level interface for talking to the SMIMS VLFD device.
///
/// The type owns the underlying USB session and keeps the remote configuration
/// cached locally, providing ergonomic helpers around the device's
/// configuration and command protocols.
pub struct Device {
    usb: UsbDevice,
    config: Config,
    encryption: EncryptionState,
}

impl Device {
    pub fn new() -> Result<Self> {
        Ok(Self {
            usb: UsbDevice::new()?,
            config: Config::new(),
            encryption: EncryptionState::default(),
        })
    }

    pub fn connect() -> Result<Self> {
        let mut device = Self::new()?;
        device.open()?;
        device.initialize()?;
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

    pub fn open(&mut self) -> Result<()> {
        self.usb.open(constants::DW_VID, constants::DW_PID)
    }

    pub fn close(&mut self) -> Result<()> {
        self.usb.close()
    }

    pub fn initialize(&mut self) -> Result<()> {
        self.read_encrypt_table()?;
        self.encryption.decode_table();
        self.read_config()?;
        Ok(())
    }

    pub fn reset_engine(&self) -> Result<()> {
        self.usb.write_bytes(Endpoint::Command, &[0x02])
    }

    pub fn ensure_session(&mut self) -> Result<()> {
        if !self.is_open() {
            self.open()?;
        }
        self.initialize()
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

        self.write_config()?;
        self.activate_vericomm()?;
        Ok(())
    }

    pub fn transfer_io(&mut self, write_buffer: &mut [u16], read_buffer: &mut [u16]) -> Result<()> {
        self.encrypt(write_buffer);
        self.fifo_write(write_buffer)?;
        self.fifo_read(read_buffer)?;
        self.decrypt(read_buffer);
        Ok(())
    }

    pub fn exit_io_mode(&mut self) -> Result<()> {
        if !self.is_open() {
            return Ok(());
        }

        self.command_active()?;
        self.close()
    }

    pub fn fifo_write(&self, buffer: &[u16]) -> Result<()> {
        self.usb.write_words(Endpoint::FifoWrite, buffer)
    }

    pub fn fifo_read(&self, buffer: &mut [u16]) -> Result<()> {
        self.usb.read_words(Endpoint::FifoRead, buffer)
    }

    pub fn sync_delay(&self) -> Result<()> {
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

    pub fn command_active(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x00])
    }

    pub fn read_config(&mut self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x01])?;

        let mut words = [0u16; Config::WORD_COUNT];
        self.usb.read_words(Endpoint::FifoRead, &mut words)?;
        self.command_active()?;
        self.decrypt(&mut words);
        self.config = Config::from_words(words);
        Ok(())
    }

    pub fn write_config(&mut self) -> Result<()> {
        self.sync_delay()?;
        let mut words = *self.config.words();
        self.encrypt(&mut words);
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x11])?;
        self.usb.write_words(Endpoint::FifoWrite, &words)?;
        self.command_active()
    }

    pub fn activate_fpga_programmer(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x02])
    }

    pub fn activate_vericomm(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x03])
    }

    pub fn activate_veri_instrument(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x08])
    }

    pub fn activate_verilink(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x09])
    }

    pub fn activate_veri_soc(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x0a])
    }

    pub fn activate_vericomm_pro(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x0b])
    }

    pub fn activate_veri_sdk(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x04])
    }

    pub fn activate_flash_read(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x05])
    }

    pub fn activate_flash_write(&self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x15])
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

    fn read_encrypt_table(&mut self) -> Result<()> {
        self.sync_delay()?;
        self.usb.write_bytes(Endpoint::Command, &[0x01, 0x0f])?;
        self.usb
            .read_words(Endpoint::FifoRead, self.encryption.table_mut())
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
        if self.table.is_empty() {
            return;
        }
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
