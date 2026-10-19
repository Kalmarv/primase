//! Exact game name hashes. Byte functions never normalize their inputs.
#![deny(clippy::disallowed_methods, clippy::disallowed_types)]

/// IEEE CRC-32: reflected polynomial 0xEDB88320, initial/final xor 0xFFFFFFFF.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut h = !0u32;
    for &b in bytes {
        h ^= u32::from(b);
        for _ in 0..8 {
            h = (h >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(h & 1));
        }
    }
    !h
}
/// Jenkins one-at-a-time over bytes, with ASCII uppercase folded to lowercase,
/// wrapping u32 at every step (CodeWalker JenkHash ASCII path convention).
pub fn joaat(bytes: &[u8]) -> u32 {
    let mut h = 0u32;
    for b in bytes {
        h = h.wrapping_add(u32::from(b.to_ascii_lowercase()));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}
/// Jenkins one-at-a-time over bytes as given (no case folding), wrapping u32 at every step: the hash of the
/// structure and field names in RAGE's META descriptions (`CMapData`, `archetypeName`).
pub fn joaat_exact(bytes: &[u8]) -> u32 {
    let mut h = 0u32;
    for &b in bytes {
        h = h.wrapping_add(u32::from(b));
        h = h.wrapping_add(h << 10);
        h ^= h >> 6;
    }
    h = h.wrapping_add(h << 3);
    h ^= h >> 11;
    h.wrapping_add(h << 15)
}
/// FNV-1a u32 over UTF-16LE bytes, seed 0x811C9DC5,
/// multiplier 0x01000193; scene prototype key, without a terminator.
pub fn fnv1a32_utf16le(s: &str) -> u32 {
    s.encode_utf16()
        .flat_map(u16::to_le_bytes)
        .fold(0x811c9dc5u32, |h, c| (h ^ u32::from(c)).wrapping_mul(0x01000193))
}
/// Standard FNV-1 64 over bytes, offset basis 0xcbf29ce484222325, prime 0x100000001b3.
pub fn fnv1_64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| {
        h.wrapping_mul(0x100000001b3) ^ u64::from(b)
    })
}
/// Standard FNV-1a 64 over bytes with the same basis and prime as [`fnv1_64`].
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
/// Extend skate-writer Hash64 state by one word: Murmur3 constants, rotl31 for
/// the word and rotl27 for the state, then state*5+0x52DCE729 (wrapping u64).
/// Default initial state is 0x9E3779B97F4A7C15; finalize with [`content_finish`].
pub fn content_word(state: u64, word: u64) -> u64 {
    let k = word
        .wrapping_mul(0x87c37b91114253d5)
        .rotate_left(31)
        .wrapping_mul(0x4cf5ad432745937f);
    (state ^ k)
        .rotate_left(27)
        .wrapping_mul(5)
        .wrapping_add(0x52dce729)
}
/// Extend Hash64 with byte length, then little-endian 8-byte words, then one
/// zero-padded tail word if needed. Does not finalize; calls can be composed.
pub fn content_bytes(state: u64, bytes: &[u8]) -> u64 {
    let mut h = content_word(state, bytes.len() as u64);
    for part in bytes.chunks(8) {
        let mut word = [0; 8];
        word[..part.len()].copy_from_slice(part);
        h = content_word(h, u64::from_le_bytes(word));
    }
    h
}
/// Extend Hash64 with UTF-16 length then one word per UTF-16 code unit.
pub fn content_string(state: u64, s: &str) -> u64 {
    s.encode_utf16()
        .fold(content_word(state, s.encode_utf16().count() as u64), |h, c| {
            content_word(h, u64::from(c))
        })
}
/// Hash64 final avalanche: xor-shift33, multiply FF51AFD7ED558CCD, xor-shift33,
/// multiply C4CEB9FE1A85EC53, xor-shift33, wrapping u64.
pub fn content_finish(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^ (h >> 33)
}
#[cfg(test)]
mod tests {
    use super::*;
    /// Known values from independent reference C# name/content hash methods,
    /// Python TOC hashing, documented CRC32 parameter IDs and GTA model IDs.
    #[test]
    fn reference_values() {
        assert_eq!(prototype_key("A😀é"), 0x5ed83406);
        assert_eq!(prototype_key("hello"), 0xc3457ef7);
        assert_eq!(crc32(b"NormalTiling"), 0x730f2086);
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(
            content_finish(content_word(
                content_string(0x9e3779b97f4a7c15, "A😀é"),
                0x12345678
            )),
            0x30c66d82292e7540
        );
        assert_eq!(fnv1_64(b"hello"), 0x7b495389bdbdd4c7);
        assert_eq!(fnv1a_64(b"hello"), 0xa430d84680aabd0b);
        assert_eq!(joaat(b"adder"), 0xb779a091);
        assert_eq!(joaat(b"ADDER"), 0xb779a091);
        assert_eq!(joaat_exact(b"adder"), 0xb779a091);
        // a structure name as GTA V's META descriptions store it
        assert_eq!(joaat_exact(b"CMapData"), 0xd3593fa6);
        assert_ne!(joaat_exact(b"CMapData"), joaat(b"CMapData"));
    }
}

/// Scene prototype key, exactly the reference scene writer over UTF-16LE bytes.
pub fn prototype_key(s: &str) -> u32 {
    fnv1a32_utf16le(s)
}

#[cfg(test)]
mod byte_tests {
    use super::*;
    #[test]
    fn common_cs_addbytes_oracle() {
        for (n, want) in [
            (0, 0xf955b5881a882d0d),
            (1, 0x769470f3b86d3938),
            (7, 0x0fdce44cf6ec2477),
            (8, 0x3ae6b5b09d70917c),
            (9, 0x7218e61756085f9b),
            (17, 0x96351c1b05cad37d),
        ] {
            let b: Vec<u8> = (0..n).collect();
            assert_eq!(content_finish(content_bytes(0x9e3779b97f4a7c15, &b)), want);
        }
    }
}
