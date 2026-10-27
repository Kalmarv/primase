//! Archive key material supplied by the caller, checked against SHA-1 digests the caller also supplies.
//!
//! GTA V Enhanced keeps its archives under the NG cipher, whose keys and tables are not present in the Enhanced
//! executable in a form a digest search finds; the caller passes them in. The Legacy executable does hold them, and
//! [`Keys::from_legacy_executable`] finds them there by the digests. This crate holds neither the key material nor
//! the digests: [`KeyDigests`] is what the caller says the material must hash to.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the key files the caller names"
)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::path::Path;

use sha1::{Digest, Sha1};

use crate::{Error, Result};

/// One NG key: 17 round keys of four little-endian u32.
pub type NgKey = [u32; 68];
/// One NG decrypt round: 16 lookup tables of 256 u32.
pub type NgRound = [[u32; 256]; 16];

/// SHA-1 digests of the key material, which [`Keys`] checks every part against before using it. They are checksums,
/// not keys: they cannot decrypt anything.
#[derive(Clone, Copy, Debug)]
pub struct KeyDigests<'a> {
    /// The 101 NG keys' digests, in key order.
    pub ng_keys: &'a [[u8; 20]],
    /// The NG decrypt tables' digests: 17 rounds of 16 tables, round-major (272).
    pub ng_decrypt_tables: &'a [[u8; 20]],
    /// The digest of the 256-byte name hash lookup table.
    pub hash_lut: [u8; 20],
    /// The digest of the 32-byte AES key.
    pub aes_key: [u8; 20],
}

impl KeyDigests<'_> {
    fn check_counts(&self) -> Result<()> {
        if self.ng_keys.len() != NG_KEY_COUNT {
            return Err(Error::Keys(format!(
                "{} NG key digests, expected {NG_KEY_COUNT}",
                self.ng_keys.len()
            )));
        }
        if self.ng_decrypt_tables.len() != NG_ROUNDS * 16 {
            return Err(Error::Keys(format!(
                "{} NG decrypt table digests, expected {}",
                self.ng_decrypt_tables.len(),
                NG_ROUNDS * 16
            )));
        }
        Ok(())
    }
}

/// The key material the archive ciphers need.
pub struct Keys {
    pub(crate) ng_keys: Vec<NgKey>,
    pub(crate) ng_tables: Vec<NgRound>,
    pub(crate) hash_lut: [u8; 256],
    pub(crate) aes_key: Option<[u8; 32]>,
}

impl std::fmt::Debug for Keys {
    // never print key material
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys")
            .field("ng_keys", &self.ng_keys.len())
            .field("aes_key", &self.aes_key.is_some())
            .finish()
    }
}

/// File names of the community layout (one folder holding the four files).
pub const NG_KEY_FILE: &str = "gtav_ng_key.dat";
pub const NG_TABLES_FILE: &str = "gtav_ng_decrypt_tables.dat";
pub const HASH_LUT_FILE: &str = "gtav_hash_lut.dat";
pub const AES_KEY_FILE: &str = "gtav_aes_key.dat";

const NG_KEY_BYTES: usize = 68 * 4;
const NG_TABLE_BYTES: usize = 256 * 4;
const NG_KEY_COUNT: usize = 101;
const NG_ROUNDS: usize = 17;

fn sha1(bytes: &[u8]) -> [u8; 20] {
    Sha1::digest(bytes).into()
}

fn words(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c))
}

impl Keys {
    /// From the four parts: 101 NG keys of 272 bytes, 17 x 16 decrypt tables of 1024 bytes, the 256-byte name
    /// lookup table, and optionally the 32-byte AES key (archives under AES only; Enhanced has none).
    /// With `digests`, every part must match its digest; with `None` the parts are taken as given.
    pub fn from_parts(
        ng_keys: &[u8],
        ng_tables: &[u8],
        hash_lut: &[u8],
        aes_key: Option<&[u8]>,
        digests: Option<&KeyDigests>,
    ) -> Result<Self> {
        if let Some(d) = digests {
            d.check_counts()?;
        }
        if ng_keys.len() != NG_KEY_COUNT * NG_KEY_BYTES {
            return Err(Error::Keys(format!(
                "NG keys: {} bytes, expected {}",
                ng_keys.len(),
                NG_KEY_COUNT * NG_KEY_BYTES
            )));
        }
        if ng_tables.len() != NG_ROUNDS * 16 * NG_TABLE_BYTES {
            return Err(Error::Keys(format!(
                "NG decrypt tables: {} bytes, expected {}",
                ng_tables.len(),
                NG_ROUNDS * 16 * NG_TABLE_BYTES
            )));
        }
        let mut keys = Vec::with_capacity(NG_KEY_COUNT);
        for (i, k) in ng_keys.as_chunks::<NG_KEY_BYTES>().0.iter().enumerate() {
            if let Some(d) = digests
                && d.ng_keys.get(i).is_some_and(|want| sha1(k) != *want)
            {
                return Err(Error::Keys(format!("NG key {i} does not match its digest")));
            }
            let mut key = [0u32; 68];
            for (d, s) in key.iter_mut().zip(words(k)) {
                *d = s;
            }
            keys.push(key);
        }
        let mut tables = Vec::with_capacity(NG_ROUNDS);
        for (r, round) in ng_tables
            .as_chunks::<{ 16 * NG_TABLE_BYTES }>()
            .0
            .iter()
            .enumerate()
        {
            let mut t = [[0u32; 256]; 16];
            for (j, (table, out)) in round
                .as_chunks::<NG_TABLE_BYTES>()
                .0
                .iter()
                .zip(t.iter_mut())
                .enumerate()
            {
                if let Some(d) = digests
                    && d.ng_decrypt_tables
                        .get(r.saturating_mul(16).saturating_add(j))
                        .is_some_and(|want| sha1(table) != *want)
                {
                    return Err(Error::Keys(format!(
                        "NG decrypt table {r}/{j} does not match its digest"
                    )));
                }
                for (d, s) in out.iter_mut().zip(words(table)) {
                    *d = s;
                }
            }
            tables.push(t);
        }
        let hash_lut: [u8; 256] = hash_lut.try_into().map_err(|_| {
            Error::Keys(format!(
                "hash lookup table: {} bytes, expected 256",
                hash_lut.len()
            ))
        })?;
        if let Some(d) = digests
            && sha1(&hash_lut) != d.hash_lut
        {
            return Err(Error::Keys("hash lookup table does not match its digest".into()));
        }
        let aes_key = match aes_key {
            None => None,
            Some(a) => {
                let a: [u8; 32] = a
                    .try_into()
                    .map_err(|_| Error::Keys(format!("AES key: {} bytes, expected 32", a.len())))?;
                if let Some(d) = digests
                    && sha1(&a) != d.aes_key
                {
                    return Err(Error::Keys("AES key does not match its digest".into()));
                }
                Some(a)
            }
        };
        Ok(Keys {
            ng_keys: keys,
            ng_tables: tables,
            hash_lut,
            aes_key,
        })
    }

    /// From a folder in the community layout: `gtav_ng_key.dat`, `gtav_ng_decrypt_tables.dat`,
    /// `gtav_hash_lut.dat` and, if present, `gtav_aes_key.dat`.
    pub fn from_dir(dir: &Path, digests: Option<&KeyDigests>) -> Result<Self> {
        let read =
            |name: &str| std::fs::read(dir.join(name)).map_err(|e| Error::Keys(format!("{name}: {e}")));
        let aes = dir.join(AES_KEY_FILE);
        let aes = if aes.exists() {
            Some(read(AES_KEY_FILE)?)
        } else {
            None
        };
        Self::from_parts(
            &read(NG_KEY_FILE)?,
            &read(NG_TABLES_FILE)?,
            &read(HASH_LUT_FILE)?,
            aes.as_deref(),
            digests,
        )
    }

    /// Found in the Legacy executable (GTA5.exe) by the digests (8-byte aligned windows). The Enhanced executable
    /// does not hold the NG material in this form; this fails naming what is missing.
    pub fn from_legacy_executable(exe: &[u8], digests: &KeyDigests) -> Result<Self> {
        digests.check_counts()?;
        let find = |len: usize, wanted: &[[u8; 20]]| -> Vec<Option<Vec<u8>>> {
            let mut found: Vec<Option<Vec<u8>>> = vec![None; wanted.len()];
            for window in exe.windows(len).step_by(8) {
                if found.iter().all(Option::is_some) {
                    break;
                }
                let h = sha1(window);
                for (slot, w) in found.iter_mut().zip(wanted) {
                    if slot.is_none() && h == *w {
                        *slot = Some(window.to_vec());
                    }
                }
            }
            found
        };
        let collect = |what: &str, parts: Vec<Option<Vec<u8>>>| -> Result<Vec<u8>> {
            let missing = parts.iter().filter(|p| p.is_none()).count();
            if missing > 0 {
                return Err(Error::Keys(format!(
                    "{missing} of {} {what} not found in the executable",
                    parts.len()
                )));
            }
            Ok(parts.into_iter().flatten().flatten().collect())
        };
        let lut = collect("hash lookup tables", find(256, &[digests.hash_lut]))?;
        let aes = find(32, &[digests.aes_key]).pop().flatten();
        let keys = collect("NG keys", find(NG_KEY_BYTES, digests.ng_keys))?;
        let tables = collect(
            "NG decrypt tables",
            find(NG_TABLE_BYTES, digests.ng_decrypt_tables),
        )?;
        Self::from_parts(&keys, &tables, &lut, aes.as_deref(), Some(digests))
    }

    /// No key material: reads archives without a cipher only (an NG or AES archive fails with a message).
    pub fn none() -> Self {
        Keys {
            ng_keys: Vec::new(),
            ng_tables: Vec::new(),
            hash_lut: [0; 256],
            aes_key: None,
        }
    }

    /// The archive cipher's name hash: the lookup table maps every character before mixing.
    pub fn name_hash(&self, name: &str) -> u32 {
        let mut h: u32 = 0;
        for c in name.bytes() {
            // a byte indexes every one of the table's 256 entries
            let mapped = self.hash_lut.get(usize::from(c)).copied().unwrap_or_default();
            let t = 1025u32.wrapping_mul(u32::from(mapped).wrapping_add(h));
            h = (t >> 6) ^ t;
        }
        let n = 9u32.wrapping_mul(h);
        32769u32.wrapping_mul((n >> 11) ^ n)
    }

    /// The NG key of an archive or entry: chosen by its name's hash and its length; an error without NG key material.
    pub fn ng_key(&self, name: &str, length: u32) -> Result<&NgKey> {
        let i = self.name_hash(name).wrapping_add(length).wrapping_add(61) % 101;
        self.ng_keys
            .get(usize::try_from(i).unwrap_or(usize::MAX))
            .ok_or_else(|| Error::Keys("an NG archive needs the NG key material".into()))
    }
}
