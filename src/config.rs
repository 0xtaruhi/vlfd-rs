#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    words: [u16; Self::WORD_COUNT],
}

impl Default for Config {
    fn default() -> Self {
        Self {
            words: [0u16; Self::WORD_COUNT],
        }
    }
}

impl Config {
    pub const WORD_COUNT: usize = 64;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_words(words: [u16; Self::WORD_COUNT]) -> Self {
        Self { words }
    }

    pub fn words(&self) -> &[u16; Self::WORD_COUNT] {
        &self.words
    }

    pub fn words_mut(&mut self) -> &mut [u16; Self::WORD_COUNT] {
        &mut self.words
    }

    pub fn vericomm_clock_high_delay(&self) -> u16 {
        self.words[0]
    }

    pub fn set_vericomm_clock_high_delay(&mut self, delay: u16) {
        self.words[0] = delay;
    }

    pub fn vericomm_clock_low_delay(&self) -> u16 {
        self.words[1]
    }

    pub fn set_vericomm_clock_low_delay(&mut self, delay: u16) {
        self.words[1] = delay;
    }

    pub fn vericomm_isv(&self) -> u8 {
        ((self.words[2] >> 4) & 0x000f) as u8
    }

    pub fn set_vericomm_isv(&mut self, value: u8) {
        let value = (value & 0x0f) as u16;
        let clock_check = self.words[2] & 0x0001;
        self.words[2] = (value << 4) | clock_check;
    }

    pub fn vericomm_clock_check_enabled(&self) -> bool {
        self.words[2] & 0x0001 != 0
    }

    pub fn set_vericomm_clock_check_enabled(&mut self, enabled: bool) {
        if enabled {
            self.words[2] |= 0x0001;
        } else {
            self.words[2] &= !0x0001;
        }
    }

    pub fn veri_sdk_channel_selector(&self) -> u8 {
        (self.words[3] & 0x00ff) as u8
    }

    pub fn set_veri_sdk_channel_selector(&mut self, channel: u8) {
        self.words[3] = (self.words[3] & 0xff00) | channel as u16;
    }

    pub fn mode_selector(&self) -> u8 {
        (self.words[3] >> 8) as u8
    }

    pub fn set_mode_selector(&mut self, mode: u8) {
        self.words[3] = (self.words[3] & 0x00ff) | ((mode as u16) << 8);
    }

    pub fn flash_begin_block_addr(&self) -> u16 {
        self.words[4]
    }

    pub fn set_flash_begin_block_addr(&mut self, addr: u16) {
        self.words[4] = addr;
    }

    pub fn flash_begin_cluster_addr(&self) -> u16 {
        self.words[5]
    }

    pub fn set_flash_begin_cluster_addr(&mut self, addr: u16) {
        self.words[5] = addr;
    }

    pub fn flash_read_end_block_addr(&self) -> u16 {
        self.words[6]
    }

    pub fn set_flash_read_end_block_addr(&mut self, addr: u16) {
        self.words[6] = addr;
    }

    pub fn flash_read_end_cluster_addr(&self) -> u16 {
        self.words[7]
    }

    pub fn set_flash_read_end_cluster_addr(&mut self, addr: u16) {
        self.words[7] = addr;
    }

    pub fn licence_key(&self) -> u16 {
        self.words[31]
    }

    pub fn security_key(&self) -> u16 {
        self.words[31]
    }

    pub fn set_licence_key(&mut self, key: u16) {
        self.words[31] = key;
    }

    pub fn smims_version_raw(&self) -> u16 {
        self.words[32]
    }

    pub fn smims_major_version(&self) -> u8 {
        (self.words[32] >> 8) as u8
    }

    pub fn smims_sub_version(&self) -> u8 {
        ((self.words[32] >> 4) & 0x000f) as u8
    }

    pub fn smims_patch_version(&self) -> u8 {
        (self.words[32] & 0x000f) as u8
    }

    pub fn fifo_size(&self) -> u16 {
        self.words[33]
    }

    pub fn flash_total_block(&self) -> u16 {
        self.words[34]
    }

    pub fn flash_block_size(&self) -> u16 {
        self.words[35]
    }

    pub fn flash_cluster_size(&self) -> u16 {
        self.words[36]
    }

    pub fn vericomm_ability(&self) -> bool {
        self.has_state_flag(0x0001)
    }

    pub fn veri_instrument_ability(&self) -> bool {
        self.has_state_flag(0x0002)
    }

    pub fn veri_link_ability(&self) -> bool {
        self.has_state_flag(0x0004)
    }

    pub fn veri_soc_ability(&self) -> bool {
        self.has_state_flag(0x0008)
    }

    pub fn vericomm_pro_ability(&self) -> bool {
        self.has_state_flag(0x0010)
    }

    pub fn veri_sdk_ability(&self) -> bool {
        self.has_state_flag(0x0100)
    }

    pub fn is_programmed(&self) -> bool {
        self.words[48] & 0x0001 != 0
    }

    pub fn is_pcb_connected(&self) -> bool {
        self.words[48] & 0x0100 == 0
    }

    pub fn vericomm_clock_continues(&self) -> bool {
        self.words[49] & 0x0001 == 0
    }

    fn has_state_flag(&self, mask: u16) -> bool {
        self.words[37] & mask != 0
    }
}
