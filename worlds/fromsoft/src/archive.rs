#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads the install's archives"
)]
use crate::{
    Result,
    bhd::{Entry, Header, unwrap_rsa},
    dcx::Decoder,
    invalid,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
/// An archive path's hash, as the BHD5 tables key files: FromSoft's base37, wrapping u32, over the UTF-16 units of the
/// lowercase path, backslashes replaced by slashes and a leading slash added. Game dictionary paths are ASCII.
pub fn path_hash(path: &str) -> u32 {
    let path = path.to_lowercase().replace('\\', "/");
    let mut h = if path.starts_with('/') { 0u32 } else { 47 };
    for c in path.encode_utf16() {
        h = h.wrapping_mul(37).wrapping_add(u32::from(c));
    }
    h
}

/// Caller-provided UTF-8 dictionary, one path per line; blank/comment lines ignored.
/// Hash collisions retain every distinct normalized candidate, never overwrite.
#[derive(Debug, Default, Clone)]
pub struct NameDictionary {
    pub names: BTreeMap<u32, Vec<String>>,
}
impl NameDictionary {
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?)
    }
    pub fn parse(text: &str) -> Result<Self> {
        let mut names: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for (i, line) in text.lines().enumerate() {
            let s = line.trim().trim_start_matches('\u{feff}');
            if s.is_empty() || s.starts_with('#') {
                continue;
            }
            if !s.is_ascii() {
                return Err(invalid("dictionary", i, "non-ASCII Sekiro path unsupported"));
            }
            let mut s = s.replace('\\', "/").to_ascii_lowercase();
            if !s.starts_with('/') {
                s.insert(0, '/');
            }
            let list = names.entry(path_hash(&s)).or_default();
            if !list.contains(&s) {
                list.push(s);
                list.sort();
            }
        }
        Ok(Self { names })
    }
}
#[derive(Debug)]
pub struct IndexedEntry {
    pub shard: u8,
    pub entry: Entry,
}
/// Five immutable archive indexes. BDT files are opened for each read, so reads
/// need no shared seek cursor. Names and Oodle loading remain caller-controlled.
#[derive(Debug)]
pub struct Archive {
    root: PathBuf,
    pub entries: BTreeMap<u32, IndexedEntry>,
    pub headers: Vec<Header>,
}
impl Archive {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let mut entries = BTreeMap::new();
        let mut headers = Vec::new();
        for (i, (n, e)) in crate::keys::SEKIRO.iter().enumerate() {
            let shard = (i + 1) as u8;
            let encrypted = std::fs::read(root.join(format!("Data{shard}.bhd")))?;
            let plain = unwrap_rsa(&encrypted, n, e)?;
            let header = Header::read(&plain)?;
            let size = std::fs::metadata(root.join(format!("Data{shard}.bdt")))?.len();
            for entry in &header.entries {
                if entry
                    .offset
                    .checked_add(entry.padded_size as u64)
                    .is_none_or(|end| end > size)
                {
                    return Err(invalid("BDT", entry.offset as usize, "entry beyond file"));
                }
                if entries
                    .insert(
                        entry.hash,
                        IndexedEntry {
                            shard,
                            entry: entry.clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(invalid("BHD5", 0, "cross-archive hash collision"));
                }
            }
            headers.push(header);
        }
        Ok(Self {
            root,
            entries,
            headers,
        })
    }
    /// Read decrypted stored bytes, including BDT padding. Metadata logical_size
    /// lets consumers distinguish padding without losing reference-reader parity.
    pub fn read_stored(&self, hash: u32) -> Result<Vec<u8>> {
        let e = self.entries.get(&hash).ok_or(crate::Error::Missing(hash))?;
        let mut f = std::fs::File::open(self.root.join(format!("Data{}.bdt", e.shard)))?;
        f.seek(SeekFrom::Start(e.entry.offset))?;
        let mut out = vec![0; e.entry.padded_size];
        f.read_exact(&mut out)?;
        e.entry.decrypt(&mut out)?;
        Ok(out)
    }
    /// Decode a DCX envelope when present; non-DCX bytes retain their stored padding.
    pub fn read_hash(&self, hash: u32, decoder: &Decoder<'_>) -> Result<Vec<u8>> {
        let bytes = self.read_stored(hash)?;
        decoder.decode_if_dcx(&bytes)
    }
    pub fn read(&self, path: &str, decoder: &Decoder<'_>) -> Result<Vec<u8>> {
        self.read_hash(path_hash(path), decoder)
    }
}
/// Public key parameters for one Sekiro shard (1 through 5).
pub fn public_key(shard: u8) -> Result<(&'static [u8], &'static [u8])> {
    crate::keys::SEKIRO
        .get(usize::from(shard).wrapping_sub(1))
        .copied()
        .ok_or_else(|| invalid("RSA", 0, "shard must be 1..5"))
}
