/// Credentials used to activate a licensed VLFD board mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Licence {
    /// Derive the per-board key from an issued customer identifier.
    CustomerId(u16),
    /// Use an already-derived key supplied by an external credential store.
    Key(u16),
}

impl Licence {
    /// Resolves this credential against the security key read from the board.
    pub const fn key_for(self, security_key: u16) -> u16 {
        match self {
            Self::CustomerId(customer_id) => generate_key(security_key, customer_id),
            Self::Key(key) => key,
        }
    }
}

const fn generate_key(security_key: u16, customer_id: u16) -> u16 {
    let mut rotated = 0u16;
    let mut nibble = 0;
    while nibble < 4 {
        let shift = nibble * 4;
        let value = (customer_id >> shift) & 0x000f;
        let amount = (security_key >> shift) & 0x0003;
        let value = ((value >> amount) | (value << (4 - amount))) & 0x000f;
        rotated |= value << shift;
        nibble += 1;
    }

    let folded = (rotated as u32) << 5;
    !(((folded >> 16) as u16) | folded as u16)
}

#[cfg(test)]
mod tests {
    use super::Licence;

    #[test]
    fn derives_the_confirmed_board_key() {
        assert_eq!(Licence::CustomerId(0xf805).key_for(0x14b8), 0xff40);
    }

    #[test]
    fn accepts_precomputed_keys_without_mutation() {
        assert_eq!(Licence::Key(0x1234).key_for(0xabcd), 0x1234);
    }
}
