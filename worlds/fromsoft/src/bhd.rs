//! BHD5 index dialect used by Sekiro. All integer fields are checked before use.
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
#[derive(Debug, Clone)]
pub struct Entry {
    pub hash: u32,
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
    pub fn read(data: &[u8]) -> Result<Self> {
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
                let hash = v.u32(p)?;
                let padded_size = v.u32(p + 4)? as usize;
                let offset = v.u64(p + 8)?;
                let sh = v.size(p + 16)?;
                let ae = v.size(p + 24)?;
                let logical_size = v.size(p + 32)?;
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
