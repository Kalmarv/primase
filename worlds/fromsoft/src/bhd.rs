//! BHD5 archive indexes. The games share the header, buckets and the hash and AES tables; each lays out its 40-byte
//! file record its own way ([`RecordLayout`]). All integer fields are checked before use.
use crate::{
    Result,
    bytes::{View, coverage},
    invalid,
};
use aes::{
    Aes128,
    cipher::{Array, BlockCipherDecrypt, KeyInit},
};
use num_bigint::BigUint;
use primase_foundations::parallel::Threads;
/// Half-open byte range. (-1,-1) in an archive is an explicitly disabled range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub start: i64,
    pub end: i64,
}
#[derive(Debug, Clone)]
pub struct AesKey {
    pub key: [u8; 16],
    pub ranges: Vec<Range>,
}
#[derive(Debug, Clone)]
pub struct HashInfo {
    pub digest: [u8; 32],
    pub ranges: Vec<Range>,
}
/// How a game's archive paths hash to the keys its BHD5 tables store. Both run over the UTF-16 units of the lowercase
/// path, backslashes replaced by slashes and a leading slash added, with wrapping arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathHash {
    /// Base 37 in a u32 (Sekiro; [`path_hash`]), zero-extended.
    Base37,
    /// Base 133 in a u64 (Elden Ring; [`path_hash64`]).
    Base133,
}
impl PathHash {
    pub fn hash(self, path: &str) -> u64 {
        match self {
            Self::Base37 => u64::from(path_hash(path)),
            Self::Base133 => path_hash64(path),
        }
    }
}
/// The UTF-16 units of `path` as the hashes read it: lowercase, slashes, a leading slash.
fn hashed_units(path: &str) -> impl Iterator<Item = u16> {
    let path = crate::names::fold(path).replace('\\', "/");
    let lead = (!path.starts_with('/')).then_some(u16::from(b'/'));
    lead.into_iter().chain(path.encode_utf16().collect::<Vec<_>>())
}
/// An archive path's hash as Sekiro's BHD5 tables key files: base 37, wrapping u32. Game dictionary paths are ASCII.
pub fn path_hash(path: &str) -> u32 {
    hashed_units(path).fold(0u32, |h, c| h.wrapping_mul(37).wrapping_add(u32::from(c)))
}
/// An archive path's hash as Elden Ring's BHD5 tables key files: base 133, wrapping u64.
pub fn path_hash64(path: &str) -> u64 {
    hashed_units(path).fold(0u64, |h, c| h.wrapping_mul(133).wrapping_add(u64::from(c)))
}
/// Where a game's BHD5 file record keeps its fields, as byte offsets into the 40-byte record. The path hash and the
/// logical size are 4 or 8 bytes wide, the padded size 4, the offset and the table offsets 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordLayout {
    /// Path hash offset and width in bytes.
    pub hash: (usize, usize),
    pub padded_size: usize,
    /// Logical (unpadded) size offset and width in bytes.
    pub logical_size: (usize, usize),
    pub offset: usize,
    pub sha: usize,
    pub aes: usize,
}
impl RecordLayout {
    /// Sekiro's (and Dark Souls III's): u32 hash, u32 padded size, u64 offset, SHA and AES table offsets, u64 logical
    /// size.
    pub const SEKIRO: Self = Self {
        hash: (0, 4),
        padded_size: 4,
        logical_size: (32, 8),
        offset: 8,
        sha: 16,
        aes: 24,
    };
    /// Elden Ring's: u64 hash, u32 padded and u32 logical size, u64 offset, SHA and AES table offsets.
    pub const ELDEN_RING: Self = Self {
        hash: (0, 8),
        padded_size: 8,
        logical_size: (12, 4),
        offset: 16,
        sha: 24,
        aes: 32,
    };
}
/// A 4- or 8-byte unsigned field.
fn sized(v: &View<'_>, (o, width): (usize, usize)) -> Result<u64> {
    match width {
        4 => Ok(u64::from(v.u32(o)?)),
        8 => v.u64(o),
        _ => Err(invalid("BHD5", o, "unsupported field width")),
    }
}
#[derive(Debug, Clone)]
pub struct Entry {
    /// The archive path hash, in the width the game's records store it (Sekiro's u32 zero-extended).
    pub hash: u64,
    pub offset: u64,
    pub padded_size: usize,
    pub logical_size: usize,
    pub aes: Option<AesKey>,
    pub sha: Option<HashInfo>,
}
#[derive(Debug, Clone)]
pub struct Header {
    pub salt: Vec<u8>,
    pub entries: Vec<Entry>,
    pub bucket_count: usize,
    pub rsa_padding: usize,
}
/// Limbs of a 2048-bit number, least significant first.
const LIMBS: usize = 32;
type Limbs = [u64; LIMBS];

/// The modulus' Montgomery context with 64-bit limbs, made once per key: `-n^-1 mod 2^64` and `R^2 mod n` with
/// `R = 2^2048`.
struct Montgomery {
    n: Limbs,
    n0: u64,
    r2: Limbs,
}

fn limbs_of(bytes: &[u8]) -> Limbs {
    let mut out = [0; LIMBS];
    for (limb, chunk) in out.iter_mut().zip(bytes.rchunks(8)) {
        *limb = chunk.iter().fold(0, |v, b| (v << 8) | u64::from(*b));
    }
    out
}

/// `a >= b`.
fn at_least(a: &Limbs, b: &Limbs) -> bool {
    for (x, y) in a.iter().zip(b).rev() {
        if x != y {
            return x > y;
        }
    }
    true
}

impl Montgomery {
    /// The context of the odd 256-byte modulus `n`.
    fn new(modulus: &[u8], n: &BigUint) -> Self {
        let n_limbs = limbs_of(modulus);
        // Newton's iteration for the inverse of the odd low limb modulo 2^64: each step doubles the correct bits
        let mut inverse = n_limbs[0];
        for _ in 0..6 {
            inverse = inverse.wrapping_mul(2u64.wrapping_sub(n_limbs[0].wrapping_mul(inverse)));
        }
        let r2 = (BigUint::from(1u8) << (64 * LIMBS * 2)) % n;
        Montgomery {
            n: n_limbs,
            n0: inverse.wrapping_neg(),
            r2: limbs_of(&r2.to_bytes_be()),
        }
    }

    /// `a * b / R mod n` for `a`, `b` below `n` (the CIOS method).
    fn multiply(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let mut t = [0u64; LIMBS + 2];
        for &bi in b {
            let mut carry = 0u128;
            for (tj, aj) in t.iter_mut().zip(a) {
                let s = u128::from(*tj) + u128::from(*aj) * u128::from(bi) + carry;
                *tj = s as u64;
                carry = s >> 64;
            }
            let s = u128::from(t[LIMBS]) + carry;
            t[LIMBS] = s as u64;
            t[LIMBS + 1] = (s >> 64) as u64;
            let m = t[0].wrapping_mul(self.n0);
            let s = u128::from(t[0]) + u128::from(m) * u128::from(self.n[0]);
            let mut carry = s >> 64;
            for j in 1..LIMBS {
                let s = u128::from(t[j]) + u128::from(m) * u128::from(self.n[j]) + carry;
                t[j - 1] = s as u64;
                carry = s >> 64;
            }
            let s = u128::from(t[LIMBS]) + carry;
            t[LIMBS - 1] = s as u64;
            t[LIMBS] = t[LIMBS + 1] + (s >> 64) as u64;
            t[LIMBS + 1] = 0;
        }
        let mut out = [0; LIMBS];
        out.copy_from_slice(&t[..LIMBS]);
        if t[LIMBS] != 0 || at_least(&out, &self.n) {
            let mut borrow = false;
            for (o, n) in out.iter_mut().zip(&self.n) {
                let (d, b1) = o.overflowing_sub(*n);
                let (d, b2) = d.overflowing_sub(u64::from(borrow));
                *o = d;
                borrow = b1 || b2;
            }
        }
        out
    }

    /// `x^e mod n` for `x` below `n`, `e` nonzero, by left-to-right square and multiply.
    fn power(&self, x: &Limbs, exponent: &BigUint) -> Limbs {
        let base = self.multiply(x, &self.r2);
        let mut result = base;
        for bit in (0..exponent.bits().saturating_sub(1)).rev() {
            result = self.multiply(&result, &result);
            if exponent.bit(bit) {
                result = self.multiply(&result, &base);
            }
        }
        let mut one = [0; LIMBS];
        one[0] = 1;
        self.multiply(&result, &one)
    }
}

/// Raw 2048-bit RSA public operation, 256 ciphertext bytes to 255 plaintext
/// bytes. This is the archive signature envelope, not PKCS#1 encryption. The modulus' Montgomery context is made
/// once for all the blocks; an even modulus (no key has one) is computed by `BigUint::modpow`.
pub fn unwrap_rsa(input: &[u8], modulus: &[u8], exponent: &[u8]) -> Result<Vec<u8>> {
    unwrap_rsa_with(Threads::ONE, input, modulus, exponent)
}

/// The blocks a thread unwraps at a time: enough that a block's work (about 50 us) outweighs handing it out.
const BLOCKS_PER_TASK: usize = 64;

/// [`unwrap_rsa`] with the blocks unwrapped on `threads`, in groups ([`BLOCKS_PER_TASK`]) joined in order: the same
/// bytes on any count, and the first failing block's error, as one thread meets it.
pub fn unwrap_rsa_with(threads: Threads, input: &[u8], modulus: &[u8], exponent: &[u8]) -> Result<Vec<u8>> {
    if input.is_empty() || !input.len().is_multiple_of(256) || modulus.len() != 256 {
        return Err(invalid("RSA", 0, "expected 256-byte blocks and modulus"));
    }
    let n = BigUint::from_bytes_be(modulus);
    let e = BigUint::from_bytes_be(exponent);
    if n == BigUint::from(0u8) || e == BigUint::from(0u8) {
        return Err(invalid("RSA", 0, "zero public key"));
    }
    let context = n.bit(0).then(|| Montgomery::new(modulus, &n));
    // block `i` into its 255-byte slot of `out`
    let unwrap = |i: usize, b: &[u8; 256], slot: &mut [u8]| -> Result<()> {
        let p = match &context {
            Some(context) => {
                let x = limbs_of(b);
                if at_least(&x, &context.n) {
                    return Err(invalid("RSA", i * 256, "ciphertext exceeds modulus"));
                }
                let power = context.power(&x, &e);
                let bytes: Vec<u8> = power.iter().rev().flat_map(|l| l.to_be_bytes()).collect();
                let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
                bytes[first..].to_vec()
            }
            None => {
                let x = BigUint::from_bytes_be(b);
                if x >= n {
                    return Err(invalid("RSA", i * 256, "ciphertext exceeds modulus"));
                }
                x.modpow(&e, &n).to_bytes_be()
            }
        };
        if p.len() > 255 {
            return Err(invalid("RSA", i * 256, "plaintext exceeds 255 bytes"));
        }
        slot[255 - p.len()..].copy_from_slice(&p);
        Ok(())
    };
    let blocks = input.as_chunks::<256>().0;
    let groups: Vec<(usize, &[[u8; 256]])> = blocks
        .chunks(BLOCKS_PER_TASK)
        .enumerate()
        .map(|(g, blocks)| (g * BLOCKS_PER_TASK, blocks))
        .collect();
    let done = primase_foundations::parallel::map(threads, &groups, |_, &(first, blocks)| {
        let mut out = vec![0; blocks.len() * 255];
        for (k, (b, slot)) in blocks.iter().zip(out.as_chunks_mut::<255>().0).enumerate() {
            unwrap(first + k, b, slot)?;
        }
        Ok::<_, crate::Error>(out)
    })
    .map_err(|p| {
        invalid(
            "RSA",
            groups[p.item].0 * 256,
            format!("unwrapping: {}", p.message),
        )
    })?;
    let mut out = Vec::with_capacity(blocks.len() * 255);
    for group in done {
        out.extend(group?);
    }
    Ok(out)
}
fn ranges(v: &View<'_>, o: usize, size: usize, encrypted: bool) -> Result<Vec<Range>> {
    let n = v.u32(o)? as usize;
    v.slice(
        o + 4,
        n.checked_mul(16)
            .ok_or_else(|| invalid("BHD5", o, "range count overflow"))?,
    )?;
    let mut out = Vec::new();
    let mut previous = 0;
    for i in 0..n {
        let p = o + 4 + i * 16;
        let start = v.u64(p)? as i64;
        let end = v.u64(p + 8)? as i64;
        if (start, end) != (-1, -1)
            && (start < 0
                || end < start
                || end as u64 > size as u64
                || start < previous
                || (encrypted
                    && (!(start as usize).is_multiple_of(16) || !(end as usize).is_multiple_of(16))))
        {
            return Err(invalid("BHD5", p, "invalid or overlapping range"));
        }
        if start >= 0 {
            previous = end;
        }
        out.push(Range { start, end });
    }
    Ok(out)
}
impl Header {
    /// Read an RSA-unwrapped index whose file records are laid out as `record` says.
    pub fn read(data: &[u8], record: &RecordLayout) -> Result<Self> {
        let mut v = View {
            data,
            format: "BHD5",
            be: false,
        };
        v.expect(0, b"BHD5")?;
        v.be = match v.u8(4)? {
            0 => true,
            255 => false,
            _ => return Err(invalid("BHD5", 4, "endian flag")),
        };
        v.boolean(5)?;
        v.expect(6, &[0, 0])?;
        if v.u32(8)? != 1 {
            return Err(invalid("BHD5", 8, "version"));
        }
        let end = v.u32(12)? as usize;
        v.slice(0, end)?;
        if data.len() - end >= 255 || data[end..].iter().any(|&b| b != 0) {
            return Err(invalid("BHD5", end, "invalid RSA padding"));
        }
        let count = v.u32(16)? as usize;
        let table = v.u32(20)? as usize;
        let saltlen = v.u32(24)? as usize;
        let salt = v.slice(28, saltlen)?.to_vec();
        let mut spans = vec![
            (0, 28 + saltlen),
            (
                table,
                count
                    .checked_mul(8)
                    .ok_or_else(|| invalid("BHD5", 16, "bucket overflow"))?,
            ),
        ];
        v.slice(table, count * 8)?;
        let mut entries = Vec::new();
        let mut hashes = std::collections::BTreeSet::new();
        for bucket in 0..count {
            let n = v.u32(table + bucket * 8)? as usize;
            let ptr = v.u32(table + bucket * 8 + 4)? as usize;
            let bytes = n
                .checked_mul(40)
                .ok_or_else(|| invalid("BHD5", ptr, "entry overflow"))?;
            v.slice(ptr, bytes)?;
            if n > 0 {
                spans.push((ptr, bytes));
            }
            for i in 0..n {
                let p = ptr + i * 40;
                let hash = sized(&v, (p + record.hash.0, record.hash.1))?;
                let padded_size = v.u32(p + record.padded_size)? as usize;
                let offset = v.u64(p + record.offset)?;
                let sh = v.size(p + record.sha)?;
                let ae = v.size(p + record.aes)?;
                let logical_size =
                    usize::try_from(sized(&v, (p + record.logical_size.0, record.logical_size.1))?)
                        .map_err(|_| invalid("BHD5", p, "logical size overflow"))?;
                if logical_size > padded_size || !hashes.insert(hash) {
                    return Err(invalid("BHD5", p, "size mismatch or duplicate hash"));
                }
                let sha = if sh == 0 {
                    None
                } else {
                    let digest = v
                        .slice(sh, 32)?
                        .try_into()
                        .map_err(|_| invalid("BHD5", sh, "hash"))?;
                    let ranges = ranges(&v, sh + 32, padded_size, false)?;
                    spans.push((sh, 36 + ranges.len() * 16));
                    Some(HashInfo { digest, ranges })
                };
                let aes = if ae == 0 {
                    None
                } else {
                    let key = v
                        .slice(ae, 16)?
                        .try_into()
                        .map_err(|_| invalid("BHD5", ae, "key"))?;
                    let ranges = ranges(&v, ae + 16, padded_size, true)?;
                    spans.push((ae, 20 + ranges.len() * 16));
                    Some(AesKey { key, ranges })
                };
                entries.push(Entry {
                    hash,
                    offset,
                    padded_size,
                    logical_size,
                    aes,
                    sha,
                });
            }
        }
        coverage(&v, spans, end, 0)?;
        Ok(Self {
            salt,
            entries,
            bucket_count: count,
            rsa_padding: data.len() - end,
        })
    }
}
impl Entry {
    /// Decrypt exactly the listed AES-128-ECB ranges, leaving other bytes and
    /// archive padding untouched. The input must be the complete padded entry.
    pub fn decrypt(&self, bytes: &mut [u8]) -> Result<()> {
        if bytes.len() != self.padded_size {
            return Err(invalid("BDT", 0, "padded size mismatch"));
        }
        if let Some(aes) = &self.aes {
            let cipher = Aes128::new(&Array::from(aes.key));
            for range in &aes.ranges {
                if range.start == -1 && range.end == -1 {
                    continue;
                }
                let start =
                    usize::try_from(range.start).map_err(|_| invalid("BDT", 0, "negative AES range"))?;
                let end = usize::try_from(range.end).map_err(|_| invalid("BDT", 0, "negative AES range"))?;
                if !start.is_multiple_of(16) || !end.is_multiple_of(16) {
                    return Err(invalid("BDT", start, "unaligned AES range"));
                }
                let part = bytes
                    .get_mut(start..end)
                    .ok_or_else(|| invalid("BDT", start, "AES range out of bounds"))?;
                for block in part.as_chunks_mut::<16>().0 {
                    let mut b = Array::from(*block);
                    cipher.decrypt_block(&mut b);
                    block.copy_from_slice(&b);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The square-and-multiply of the `num-bigint` crate, the reference the Montgomery product is held to.
    fn reference(input: &[u8], modulus: &[u8], exponent: &[u8]) -> Vec<u8> {
        let (n, e) = (BigUint::from_bytes_be(modulus), BigUint::from_bytes_be(exponent));
        let mut out = vec![0; input.len() / 256 * 255];
        for (i, b) in input.as_chunks::<256>().0.iter().enumerate() {
            let p = BigUint::from_bytes_be(b).modpow(&e, &n).to_bytes_be();
            out[(i + 1) * 255 - p.len()..(i + 1) * 255].copy_from_slice(&p);
        }
        out
    }

    /// 256-byte blocks below `modulus`: 0, 1, 2, n - 1 and `random` of a xorshift stream reduced modulo `n`.
    fn blocks(modulus: &[u8], random: usize) -> Vec<u8> {
        let n = BigUint::from_bytes_be(modulus);
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut values = vec![
            BigUint::from(0u8),
            BigUint::from(1u8),
            BigUint::from(2u8),
            &n - 1u8,
        ];
        for _ in 0..random {
            let bytes: Vec<u8> = (0..256).map(|_| next() as u8).collect();
            values.push(BigUint::from_bytes_be(&bytes) % &n);
        }
        values
            .iter()
            .flat_map(|v| {
                let b = v.to_bytes_be();
                let mut block = vec![0; 256 - b.len()];
                block.extend(b);
                block
            })
            .collect()
    }

    /// Every published Sekiro key, with its exponent and others (1, 3, 65537, a wide one), on the edge blocks and
    /// random ones: `x^e mod n` is `BigUint::modpow`'s, bit for bit.
    #[test]
    fn the_montgomery_power_is_biguint_modpow() {
        let wide: Vec<u8> = (1..=32).collect();
        for (key, (modulus, exponent)) in crate::keys::SEKIRO.iter().enumerate() {
            let n = BigUint::from_bytes_be(modulus);
            let context = Montgomery::new(modulus, &n);
            for block in blocks(modulus, 24).chunks(256) {
                let x = BigUint::from_bytes_be(block);
                for e in [*exponent, &[1][..], &[3], &[1, 0, 1], &wide] {
                    let e = BigUint::from_bytes_be(e);
                    let got: Vec<u8> = context
                        .power(&limbs_of(block), &e)
                        .iter()
                        .rev()
                        .flat_map(|l| l.to_be_bytes())
                        .collect();
                    assert_eq!(
                        BigUint::from_bytes_be(&got),
                        x.modpow(&e, &n),
                        "key {key}, exponent {e}"
                    );
                }
            }
        }
    }

    /// `unwrap_rsa` end to end where the plaintext fits (blocks of 60 bytes, exponents 1 and 3: below 2^1440): the bytes are the
    /// reference's for an odd modulus (Montgomery) and an even one (`BigUint::modpow`); a block at or above the modulus
    /// is refused on both paths, and so is a plaintext of the full 256 bytes.
    #[test]
    fn unwrap_rsa_matches_the_reference_and_refuses_the_same_blocks() {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut small = Vec::new();
        for _ in 0..16 {
            let mut block = vec![0u8; 196];
            block.extend((0..60).map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            }));
            small.extend(block);
        }
        for odd in [0xffu8, 0xfe] {
            let mut modulus = [0xffu8; 256];
            modulus[255] = odd;
            for e in [&[1u8][..], &[3]] {
                assert_eq!(
                    unwrap_rsa(&small, &modulus, e).unwrap(),
                    reference(&small, &modulus, e),
                    "modulus ending {odd:x}, exponent {e:?}"
                );
            }
            assert!(unwrap_rsa(&modulus, &modulus, &[3]).is_err(), "n itself");
            let mut above = modulus;
            above[0] = 0xff;
            assert!(unwrap_rsa(&above, &modulus, &[1]).is_err());
            // 2 to the 2040 + 1 is below n and its plaintext (e = 1) is 256 bytes
            let mut full = [0u8; 256];
            full[0] = 1;
            full[255] = 1;
            assert!(unwrap_rsa(&full, &modulus, &[1]).is_err(), "a 256-byte plaintext");
        }
    }

    /// 200 blocks (three groups and a part) unwrap to the same bytes on one thread and on four, and the error of
    /// two failing blocks is the first one's, with its offset, on both.
    #[test]
    fn the_blocks_unwrap_the_same_on_any_thread_count() {
        let (modulus, exponent) = crate::keys::SEKIRO[0];
        let mut state = 0x1234_5678_9abc_def1u64;
        let mut input = Vec::new();
        for _ in 0..200 {
            let mut block = vec![0u8; 196];
            block.extend((0..60).map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            }));
            input.extend(block);
        }
        // exponent 3 keeps a 60-byte block's plaintext in 255 bytes, whatever the key's own exponent
        let one = unwrap_rsa_with(Threads::ONE, &input, modulus, &[3]).unwrap();
        let four = unwrap_rsa_with(Threads::new(4).unwrap(), &input, modulus, &[3]).unwrap();
        assert_eq!(one, four);
        assert_eq!(one, reference(&input, modulus, &[3]));
        let _ = exponent;
        for at in [150usize, 70] {
            input[at * 256..(at + 1) * 256].copy_from_slice(&[0xff; 256]);
        }
        for threads in [Threads::ONE, Threads::new(4).unwrap()] {
            let e = unwrap_rsa_with(threads, &input, modulus, &[3]).unwrap_err();
            assert!(
                matches!(&e, crate::Error::Invalid { offset, detail, .. } if *offset == 70 * 256 && detail.contains("exceeds modulus")),
                "{e:?}"
            );
        }
    }
}
