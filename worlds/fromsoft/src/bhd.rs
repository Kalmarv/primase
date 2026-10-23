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
    let path = path.to_lowercase().replace('\\', "/");
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
/// Raw 2048-bit RSA public operation, 256 ciphertext bytes to 255 plaintext
/// bytes. This is the archive signature envelope, not PKCS#1 encryption.
pub fn unwrap_rsa(input: &[u8], modulus: &[u8], exponent: &[u8]) -> Result<Vec<u8>> {
    if input.is_empty() || !input.len().is_multiple_of(256) || modulus.len() != 256 {
        return Err(invalid("RSA", 0, "expected 256-byte blocks and modulus"));
    }
    let n = BigUint::from_bytes_be(modulus);
    let e = BigUint::from_bytes_be(exponent);
    if n == BigUint::from(0u8) || e == BigUint::from(0u8) {
        return Err(invalid("RSA", 0, "zero public key"));
    }
    let mut out = vec![0; input.len() / 256 * 255];
    for (i, b) in input.as_chunks::<256>().0.iter().enumerate() {
        let x = BigUint::from_bytes_be(b);
        if x >= n {
            return Err(invalid("RSA", i * 256, "ciphertext exceeds modulus"));
        }
        let p = x.modpow(&e, &n).to_bytes_be();
        if p.len() > 255 {
            return Err(invalid("RSA", i * 256, "plaintext exceeds 255 bytes"));
        }
        out[(i + 1) * 255 - p.len()..(i + 1) * 255].copy_from_slice(&p);
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
