/// One logical VeriComm sample as exposed by VLFD-compatible fixtures.
///
/// Lanes `0..16` live in word 0, lanes `16..32` in word 1, and so on.
/// The type makes the fixture's four-word sample boundary explicit while the
/// lower-level slice transfer APIs remain available for bulk streaming.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct VeriCommFrame([u16; Self::WORDS]);

impl VeriCommFrame {
    pub const WORDS: usize = 4;
    pub const LANES: usize = Self::WORDS * u16::BITS as usize;
    pub const ZERO: Self = Self([0; Self::WORDS]);

    pub const fn from_words(words: [u16; Self::WORDS]) -> Self {
        Self(words)
    }

    pub const fn words(&self) -> &[u16; Self::WORDS] {
        &self.0
    }

    pub fn words_mut(&mut self) -> &mut [u16; Self::WORDS] {
        &mut self.0
    }

    pub const fn into_words(self) -> [u16; Self::WORDS] {
        self.0
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self([
            bits as u16,
            (bits >> 16) as u16,
            (bits >> 32) as u16,
            (bits >> 48) as u16,
        ])
    }

    pub const fn bits(self) -> u64 {
        self.0[0] as u64
            | ((self.0[1] as u64) << 16)
            | ((self.0[2] as u64) << 32)
            | ((self.0[3] as u64) << 48)
    }

    pub const fn lane(self, lane: usize) -> Option<bool> {
        if lane >= Self::LANES {
            return None;
        }
        let word = lane / u16::BITS as usize;
        let bit = lane % u16::BITS as usize;
        Some(self.0[word] & (1 << bit) != 0)
    }

    pub fn set_lane(&mut self, lane: usize, high: bool) -> bool {
        if lane >= Self::LANES {
            return false;
        }
        let word = lane / u16::BITS as usize;
        let mask = 1 << (lane % u16::BITS as usize);
        if high {
            self.0[word] |= mask;
        } else {
            self.0[word] &= !mask;
        }
        true
    }
}

impl From<[u16; VeriCommFrame::WORDS]> for VeriCommFrame {
    fn from(words: [u16; VeriCommFrame::WORDS]) -> Self {
        Self::from_words(words)
    }
}

impl From<VeriCommFrame> for [u16; VeriCommFrame::WORDS] {
    fn from(frame: VeriCommFrame) -> Self {
        frame.into_words()
    }
}

#[cfg(test)]
mod tests {
    use super::VeriCommFrame;

    #[test]
    fn bits_and_words_have_an_explicit_lane_order() {
        let frame = VeriCommFrame::from_bits(0x7654_3210_fedc_ba98);
        assert_eq!(frame.words(), &[0xba98, 0xfedc, 0x3210, 0x7654]);
        assert_eq!(frame.bits(), 0x7654_3210_fedc_ba98);
    }

    #[test]
    fn lane_access_crosses_word_boundaries() {
        let mut frame = VeriCommFrame::ZERO;
        for lane in [0, 15, 16, 53, 63] {
            assert!(frame.set_lane(lane, true));
            assert_eq!(frame.lane(lane), Some(true));
        }
        assert!(!frame.set_lane(64, true));
        assert_eq!(frame.lane(64), None);
        assert_eq!(frame.words(), &[0x8001, 0x0001, 0x0000, 0x8020]);
    }
}
