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
/// Adler-32 (RFC 1950) started from `init` instead of the standard 1: the low half is the byte sum plus `init`'s low
/// half, the high half the sum of those running sums plus `init`'s high half, each modulo 65521. Some archives
/// store it started from 0.
pub fn adler32(init: u32, bytes: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (init & 0xffff, init >> 16);
    for &x in bytes {
        a = (a + u32::from(x)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
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
    /// Standard Adler-32 (start 1) of "Wikipedia" is 0x11E60398 (RFC 1950's worked example); from 0, each running
    /// sum loses the 1: a = 920 - 1, b = 4582 - 9.
    #[test]
    fn adler32_standard_and_from_zero() {
        assert_eq!(adler32(1, b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(0, b"Wikipedia"), (4573 << 16) | 919);
        assert_eq!(adler32(1, b""), 1);
        assert_eq!(adler32(0, b""), 0);
    }
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

/// MD5's per-round shift amounts and sine constants (RFC 1321 §3.4).
const MD5_SHIFT: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5,
    9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6,
    10, 15, 21, 6, 10, 15, 21,
];
const MD5_SINE: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];
/// MD5's initial state (RFC 1321 §3.3).
pub const MD5_INIT: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];

/// MD5's block function (RFC 1321 §3.4): `state` advanced by one 64-byte block. Exposed for digests that pad
/// differently (DXBC's container checksum).
pub fn md5_block(state: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (i, w) in m.iter_mut().enumerate() {
        *w = u32::from_le_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let rotated = a
            .wrapping_add(f)
            .wrapping_add(MD5_SINE[i])
            .wrapping_add(m[g])
            .rotate_left(MD5_SHIFT[i]);
        (a, b, c, d) = (d, b.wrapping_add(rotated), b, c);
    }
    for (s, x) in state.iter_mut().zip([a, b, c, d]) {
        *s = s.wrapping_add(x);
    }
}

/// The MD5 digest of `bytes` (RFC 1321).
pub fn md5(bytes: &[u8]) -> [u8; 16] {
    let mut state = MD5_INIT;
    let (blocks, rest) = bytes.as_chunks::<64>();
    for block in blocks {
        md5_block(&mut state, block);
    }
    let mut tail = [0u8; 128];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;
    let end = if rest.len() < 56 { 64 } else { 128 };
    let bits = (bytes.len() as u64).wrapping_mul(8);
    tail[end - 8..end].copy_from_slice(&bits.to_le_bytes());
    for block in tail[..end].as_chunks::<64>().0 {
        md5_block(&mut state, block);
    }
    md5_bytes(state)
}

/// An MD5 state as its 16-byte digest (each word little-endian).
pub fn md5_bytes(state: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *o = s.to_le_bytes();
    }
    out
}

#[cfg(test)]
mod md5_tests {
    use super::*;
    /// RFC 1321's test suite (§A.5).
    #[test]
    fn rfc_1321_test_suite() {
        let cases: [(&str, &str); 7] = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (input, want) in cases {
            let got: String = md5(input.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, want, "{input:?}");
        }
    }
}
