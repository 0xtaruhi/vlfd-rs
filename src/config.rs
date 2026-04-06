mod word {
    pub const VERICOMM_CLOCK_HIGH_DELAY: usize = 0;
    pub const VERICOMM_CLOCK_LOW_DELAY: usize = 1;
    pub const VERICOMM_MISC: usize = 2;
    pub const MODE_AND_CHANNEL: usize = 3;
    pub const FLASH_BEGIN_BLOCK: usize = 4;
    pub const FLASH_BEGIN_CLUSTER: usize = 5;
    pub const FLASH_READ_END_BLOCK: usize = 6;
    pub const FLASH_READ_END_CLUSTER: usize = 7;
    pub const LICENCE_AND_SECURITY_KEY: usize = 31;
    pub const SMIMS_VERSION: usize = 32;
    pub const FIFO_SIZE_WORDS: usize = 33;
    pub const FLASH_TOTAL_BLOCK: usize = 34;
    pub const FLASH_BLOCK_SIZE: usize = 35;
    pub const FLASH_CLUSTER_SIZE: usize = 36;
    pub const ABILITY_FLAGS: usize = 37;
    pub const PROGRAM_STATE: usize = 48;
    pub const CLOCK_STATE: usize = 49;
}

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

    pub fn vericomm_clock_high_delay(&self) -> u16 {
        self.words[word::VERICOMM_CLOCK_HIGH_DELAY]
    }

    pub fn set_vericomm_clock_high_delay(&mut self, delay: u16) {
        self.words[word::VERICOMM_CLOCK_HIGH_DELAY] = delay;
    }

    pub fn vericomm_clock_low_delay(&self) -> u16 {
        self.words[word::VERICOMM_CLOCK_LOW_DELAY]
    }

    pub fn set_vericomm_clock_low_delay(&mut self, delay: u16) {
        self.words[word::VERICOMM_CLOCK_LOW_DELAY] = delay;
    }

    pub fn vericomm_isv(&self) -> u8 {
        ((self.words[word::VERICOMM_MISC] >> 4) & 0x000f) as u8
    }

    pub fn set_vericomm_isv(&mut self, value: u8) {
        let value = (value & 0x0f) as u16;
        let clock_check = self.words[word::VERICOMM_MISC] & 0x0001;
        self.words[word::VERICOMM_MISC] = (value << 4) | clock_check;
    }

    pub fn vericomm_clock_check_enabled(&self) -> bool {
        self.words[word::VERICOMM_MISC] & 0x0001 != 0
    }

    pub fn set_vericomm_clock_check_enabled(&mut self, enabled: bool) {
        if enabled {
            self.words[word::VERICOMM_MISC] |= 0x0001;
        } else {
            self.words[word::VERICOMM_MISC] &= !0x0001;
        }
    }

    pub fn veri_sdk_channel_selector(&self) -> u8 {
        (self.words[word::MODE_AND_CHANNEL] & 0x00ff) as u8
    }

    pub fn set_veri_sdk_channel_selector(&mut self, channel: u8) {
        self.words[word::MODE_AND_CHANNEL] =
            (self.words[word::MODE_AND_CHANNEL] & 0xff00) | channel as u16;
    }

    pub fn mode_selector(&self) -> u8 {
        (self.words[word::MODE_AND_CHANNEL] >> 8) as u8
    }

    pub fn set_mode_selector(&mut self, mode: u8) {
        self.words[word::MODE_AND_CHANNEL] =
            (self.words[word::MODE_AND_CHANNEL] & 0x00ff) | ((mode as u16) << 8);
    }

    pub fn flash_begin_block_addr(&self) -> u16 {
        self.words[word::FLASH_BEGIN_BLOCK]
    }

    pub fn set_flash_begin_block_addr(&mut self, addr: u16) {
        self.words[word::FLASH_BEGIN_BLOCK] = addr;
    }

    pub fn flash_begin_cluster_addr(&self) -> u16 {
        self.words[word::FLASH_BEGIN_CLUSTER]
    }

    pub fn set_flash_begin_cluster_addr(&mut self, addr: u16) {
        self.words[word::FLASH_BEGIN_CLUSTER] = addr;
    }

    pub fn flash_read_end_block_addr(&self) -> u16 {
        self.words[word::FLASH_READ_END_BLOCK]
    }

    pub fn set_flash_read_end_block_addr(&mut self, addr: u16) {
        self.words[word::FLASH_READ_END_BLOCK] = addr;
    }

    pub fn flash_read_end_cluster_addr(&self) -> u16 {
        self.words[word::FLASH_READ_END_CLUSTER]
    }

    pub fn set_flash_read_end_cluster_addr(&mut self, addr: u16) {
        self.words[word::FLASH_READ_END_CLUSTER] = addr;
    }

    pub fn licence_key(&self) -> u16 {
        self.words[word::LICENCE_AND_SECURITY_KEY]
    }

    /// The hardware exposes the security key through the same configuration
    /// word used for the active license key.
    pub fn security_key(&self) -> u16 {
        self.words[word::LICENCE_AND_SECURITY_KEY]
    }

    pub fn set_licence_key(&mut self, key: u16) {
        self.words[word::LICENCE_AND_SECURITY_KEY] = key;
    }

    pub fn smims_version_raw(&self) -> u16 {
        self.words[word::SMIMS_VERSION]
    }

    pub fn smims_major_version(&self) -> u8 {
        (self.words[word::SMIMS_VERSION] >> 8) as u8
    }

    pub fn smims_sub_version(&self) -> u8 {
        ((self.words[word::SMIMS_VERSION] >> 4) & 0x000f) as u8
    }

    pub fn smims_patch_version(&self) -> u8 {
        (self.words[word::SMIMS_VERSION] & 0x000f) as u8
    }

    /// Returns the FIFO capacity in 16-bit words.
    ///
    /// This matches the legacy C++ implementation, where `CFG[33]` is passed
    /// directly to `SMIMS_FIFO_Write` / `SMIMS_FIFO_Read` as a word count.
    pub fn fifo_size_words(&self) -> u16 {
        self.words[word::FIFO_SIZE_WORDS]
    }

    /// Legacy alias for [`Self::fifo_size_words`].
    pub fn fifo_size(&self) -> u16 {
        self.fifo_size_words()
    }

    pub fn flash_total_block(&self) -> u16 {
        self.words[word::FLASH_TOTAL_BLOCK]
    }

    pub fn flash_block_size(&self) -> u16 {
        self.words[word::FLASH_BLOCK_SIZE]
    }

    pub fn flash_cluster_size(&self) -> u16 {
        self.words[word::FLASH_CLUSTER_SIZE]
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
        self.words[word::PROGRAM_STATE] & 0x0001 != 0
    }

    pub fn is_pcb_connected(&self) -> bool {
        self.words[word::PROGRAM_STATE] & 0x0100 == 0
    }

    pub fn vericomm_clock_continues(&self) -> bool {
        self.words[word::CLOCK_STATE] & 0x0001 == 0
    }

    fn has_state_flag(&self, mask: u16) -> bool {
        self.words[word::ABILITY_FLAGS] & mask != 0
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn mode_and_channel_share_the_same_word_without_clobbering_each_other() {
        let mut config = Config::new();
        config.set_mode_selector(0x12);
        config.set_veri_sdk_channel_selector(0x34);
        assert_eq!(config.mode_selector(), 0x12);
        assert_eq!(config.veri_sdk_channel_selector(), 0x34);
        assert_eq!(config.words()[3], 0x1234);
    }

    #[test]
    fn fifo_size_aliases_are_word_based() {
        let mut words = [0u16; Config::WORD_COUNT];
        words[33] = 512;
        let config = Config::from_words(words);
        assert_eq!(config.fifo_size(), 512);
        assert_eq!(config.fifo_size_words(), 512);
    }

    #[test]
    fn licence_and_security_key_share_the_same_backing_word() {
        let mut config = Config::new();
        config.set_licence_key(0x55aa);
        assert_eq!(config.licence_key(), 0x55aa);
        assert_eq!(config.security_key(), 0x55aa);
    }
}
