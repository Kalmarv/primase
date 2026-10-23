//! Hash framing used by the map writer.
/// Extend skate-writer FNV64 state by UTF-16 units followed by the unit 0xFF.
/// Initial state is 0xcbf29ce484222325. This is NOT FNV over UTF-8 bytes.
pub fn skate_fnv_string(state: u64, s: &str) -> u64 {
    s.encode_utf16()
        .map(u64::from)
        .chain([255])
        .fold(state, |h, c| (h ^ c).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_units_and_sentinel_match_reference() {
        // The reference writer's hash of "A😀é" followed by the u64 0x12345678 is 0xd8cb5178e4163d1d (the byte-wise FNV step
        // that took the word is gone, nothing used it); the string alone, worked from the definition (FNV-1a 64 over the
        // UTF-16 units, then 0xFF) in Python and matched to that value, is:
        assert_eq!(skate_fnv_string(0xcbf29ce484222325, "A😀é"), 0x6f14e7ce6a6b3a3d);
    }
}
