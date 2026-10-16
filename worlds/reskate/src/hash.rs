//! Hash framing used by the map writer.
/// Extend skate-writer FNV64 state by UTF-16 units followed by the unit 0xFF.
/// Initial state is 0xcbf29ce484222325. This is NOT FNV over UTF-8 bytes.
pub fn skate_fnv_string(state: u64, s: &str) -> u64 {
    s.encode_utf16()
        .map(u64::from)
        .chain([255])
        .fold(state, |h, c| (h ^ c).wrapping_mul(0x100000001b3))
}
/// Extend skate-writer FNV64 state with eight little-endian bytes of a u64.
pub fn skate_fnv_u64(state: u64, value: u64) -> u64 {
    value
        .to_le_bytes()
        .iter()
        .fold(state, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_units_sentinel_and_word_match_reference() {
        assert_eq!(
            skate_fnv_u64(skate_fnv_string(0xcbf29ce484222325, "A😀é"), 0x12345678),
            0xd8cb5178e4163d1d
        );
    }
}
