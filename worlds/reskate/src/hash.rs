//! Hash framing used by the map writer.
/// Extend skate-writer FNV64 state by UTF-16 units followed by the unit 0xFF.
/// Initial state is 0xcbf29ce484222325. This is NOT FNV over UTF-8 bytes.
pub fn skate_fnv_string(state: u64, s: &str) -> u64 {
    s.encode_utf16()
        .map(u64::from)
        .chain([255])
        .fold(state, |h, c| (h ^ c).wrapping_mul(0x100000001b3))
}

/// A prototype's key when its description gives none: FNV-1a 32 (seed 0x811C9DC5, multiplier 0x01000193) over the id's
/// UTF-16LE bytes (each code unit's low byte, then its high byte), without a terminator, as the reference scene writer
/// computes it.
pub fn prototype_key(id: &str) -> u32 {
    id.encode_utf16()
        .flat_map(u16::to_le_bytes)
        .fold(0x811c_9dc5u32, |h, c| {
            (h ^ u32::from(c)).wrapping_mul(0x0100_0193)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values from the reference scene writer's C# method.
    #[test]
    fn a_prototype_key_is_fnv1a_over_utf16le() {
        assert_eq!(prototype_key("A😀é"), 0x5ed8_3406);
        assert_eq!(prototype_key("hello"), 0xc345_7ef7);
    }
    #[test]
    fn utf16_units_and_sentinel_match_reference() {
        // The reference writer's hash of "A😀é" followed by the u64 0x12345678 is 0xd8cb5178e4163d1d (the byte-wise FNV step
        // that took the word is gone, nothing used it); the string alone, worked from the definition (FNV-1a 64 over the
        // UTF-16 units, then 0xFF) in Python and matched to that value, is:
        assert_eq!(skate_fnv_string(0xcbf29ce484222325, "A😀é"), 0x6f14e7ce6a6b3a3d);
    }
}
