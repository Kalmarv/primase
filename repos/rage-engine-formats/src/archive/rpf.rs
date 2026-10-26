//! RPF7 archives: a 16-byte header (magic, entry count, names length, cipher), 16-byte entries and a name table,
//! both under the archive's cipher; file data in 512-byte blocks. An archive may hold archives (binary entries
//! named `*.rpf`, stored uncompressed), which are read in place.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: reads RPF archives"
)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use primase_foundations::compress::{DEFLATE_MAX_RATIO, Decompressor, Deflate};

use crate::archive::keys::Keys;
use crate::archive::ng::{aes_decrypt, ng_decrypt};
use crate::resource::{self, PageFlags, Resource};
use crate::{Result, invalid};

/// The "RPF7" magic, little-endian.
pub const MAGIC: u32 = 0x5250_4637;
const BLOCK: u64 = 512;

/// The cipher on an archive's table of contents (and its encrypted entries), by the header's cipher word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encryption {
    /// Word 0: no cipher word at all, as some tools write.
    None,
    /// Word `0x4E45504F` (the bytes "OPEN"): a table of contents left in the clear by an editing tool.
    Open,
    /// Word `0x0FFFFFF9`.
    Aes,
    /// Word `0x0FEFFFFF`.
    Ng,
}

impl Encryption {
    fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Encryption::None),
            0x4E45_504F => Some(Encryption::Open),
            0x0FFF_FFF9 => Some(Encryption::Aes),
            0x0FEF_FFFF => Some(Encryption::Ng),
            _ => None,
        }
    }
}

/// What an entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Entries `first .. first + count` are its children.
    Directory { first: u32, count: u32 },
    /// `stored` bytes at `block` (0: stored uncompressed, `size` bytes); DEFLATE when `stored` is not 0.
    Binary {
        block: u32,
        stored: u32,
        size: u32,
        encrypted: bool,
    },
    /// `stored` bytes at `block` (header included); the page flags give the inflated size.
    Resource {
        block: u32,
        stored: u32,
        system: PageFlags,
        graphics: PageFlags,
    },
}

/// One entry of the table of contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// As stored (the cipher keys by it).
    pub name: String,
    /// Lowercase, from the archive's path down, joined by `\`.
    pub path: String,
    pub kind: EntryKind,
}

/// An archive: where it lies in which file, and its table of contents.
#[derive(Clone, Debug)]
pub struct Archive {
    /// The physical file it is in.
    pub file: std::path::PathBuf,
    /// Its first byte in that file.
    pub start: u64,
    /// Its length in bytes.
    pub size: u64,
    /// Its own name (the cipher keys by it).
    pub name: String,
    /// Lowercase path: relative to the install for a top-level archive, the parent entry's path for a nested one.
    pub path: String,
    pub encryption: Encryption,
    pub entries: Vec<Entry>,
}

/// The byte offset of `block` in an archive starting at `start`.
fn block_offset(start: u64, block: u32) -> Result<u64> {
    u64::from(block)
        .checked_mul(BLOCK)
        .and_then(|o| o.checked_add(start))
        .ok_or_else(|| invalid("RPF7", start, format!("block {block} past the address space")))
}

/// One 16-byte entry record's fields: the 16-bit word at 0, the 24-bit words at 2 and 5, the 32-bit words at 0, 4,
/// 8 and 12.
struct Record {
    u16_0: u32,
    u24_2: u32,
    u24_5: u32,
    words: [u32; 4],
}

impl Record {
    fn of(e: &[u8; 16]) -> Record {
        let [b0, b1, b2, b3, b4, b5, b6, b7, ..] = *e;
        Record {
            u16_0: u32::from(u16::from_le_bytes([b0, b1])),
            u24_2: u32::from_le_bytes([b2, b3, b4, 0]),
            u24_5: u32::from_le_bytes([b5, b6, b7, 0]),
            words: crate::le::u32x4_of(e),
        }
    }
}

impl Archive {
    /// A top-level archive file; `path` is its path relative to the install (any case, `/` or `\`).
    pub fn open(file: &Path, path: &str, keys: &Keys) -> Result<Self> {
        let size = std::fs::metadata(file)?.len();
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| invalid("RPF7", 0, "file name is not text"))?
            .to_string();
        let mut f = File::open(file)?;
        Self::read(
            &mut f,
            file,
            0,
            size,
            name,
            path.replace('/', "\\").to_lowercase(),
            keys,
        )
    }

    /// The archive held by entry `index` (a binary entry named `*.rpf`).
    pub fn nested(&self, f: &mut File, index: usize, keys: &Keys) -> Result<Self> {
        let e = self.entry(index)?;
        let EntryKind::Binary {
            block,
            stored,
            size,
            encrypted,
        } = e.kind
        else {
            return Err(invalid(
                "RPF7",
                self.start,
                format!("{} is not a binary entry", e.path),
            ));
        };
        if stored != 0 || encrypted {
            return Err(invalid(
                "RPF7",
                self.start,
                format!("{}: a nested archive must be stored plain", e.path),
            ));
        }
        let start = block_offset(self.start, block)?;
        Self::read(
            f,
            &self.file,
            start,
            u64::from(size),
            e.name.clone(),
            e.path.clone(),
            keys,
        )
    }

    fn read(
        f: &mut File,
        file: &Path,
        start: u64,
        size: u64,
        name: String,
        path: String,
        keys: &Keys,
    ) -> Result<Self> {
        let mut h = [0u8; 16];
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut h)?;
        let [magic, count, names_len, cipher] = crate::le::u32x4_of(&h);
        if magic != MAGIC {
            return Err(invalid("RPF7", start, format!("{path}: magic {magic:#010x}")));
        }
        let size_err = || invalid("RPF7", start, "size overflow");
        let count = usize::try_from(count).map_err(|_| size_err())?;
        let names_len = usize::try_from(names_len).map_err(|_| size_err())?;
        let encryption = Encryption::from_u32(cipher)
            .ok_or_else(|| invalid("RPF7", start, format!("{path}: cipher {cipher:#010x}")))?;
        let entries_len = count.checked_mul(16).ok_or_else(size_err)?;
        let toc_end = entries_len
            .checked_add(names_len)
            .and_then(|n| n.checked_add(16))
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(size_err)?;
        if toc_end > size {
            return Err(invalid(
                "RPF7",
                start,
                format!("{path}: table of contents runs past the archive"),
            ));
        }
        let mut entries_data = vec![0u8; entries_len];
        let mut names = vec![0u8; names_len];
        f.read_exact(&mut entries_data)?;
        f.read_exact(&mut names)?;
        let size32 = u32::try_from(size).map_err(|_| invalid("RPF7", start, "archive larger than 4 GiB"))?;
        match encryption {
            Encryption::None | Encryption::Open => {}
            Encryption::Aes => {
                aes_decrypt(keys, &path, &mut entries_data)?;
                aes_decrypt(keys, &path, &mut names)?;
            }
            Encryption::Ng => {
                ng_decrypt(keys, &name, size32, &mut entries_data)?;
                ng_decrypt(keys, &name, size32, &mut names)?;
            }
        }
        if count == 0 {
            return Err(invalid("RPF7", start, format!("{path}: no root directory")));
        }
        let name_at = |o: usize| -> Result<String> {
            let rest = names.get(o..).ok_or_else(|| {
                invalid(
                    "RPF7",
                    start,
                    format!("{path}: name offset {o} past the name table"),
                )
            })?;
            let end = rest
                .iter()
                .position(|&c| c == 0)
                .ok_or_else(|| invalid("RPF7", start, format!("{path}: name at {o} not terminated")))?;
            let (name, _) = rest.split_at(end);
            String::from_utf8(name.to_vec())
                .map_err(|_| invalid("RPF7", start, format!("{path}: name at {o} is not UTF-8")))
        };
        let mut kinds = Vec::with_capacity(count);
        let mut raw_names = Vec::with_capacity(count);
        for (i, e) in entries_data.as_chunks::<16>().0.iter().enumerate() {
            let r = Record::of(e);
            let [w0, second, w8, w12] = r.words;
            let (name_off, kind) = if second == 0x7FFF_FF00 {
                (
                    w0,
                    EntryKind::Directory {
                        first: w8,
                        count: w12,
                    },
                )
            } else if second & 0x8000_0000 == 0 {
                let enc = w12;
                if enc > 1 {
                    return Err(invalid(
                        "RPF7",
                        start,
                        format!("{path}: entry {i} cipher flag {enc}"),
                    ));
                }
                (
                    r.u16_0,
                    EntryKind::Binary {
                        block: r.u24_5,
                        stored: r.u24_2,
                        size: w8,
                        encrypted: enc == 1,
                    },
                )
            } else {
                let mut stored = r.u24_2;
                let block = r.u24_5 & 0x7F_FFFF;
                if stored == 0xFF_FFFF {
                    // a stored size of 2^24 - 1 or more: the real size is spread over the resource header's bytes
                    let mut rh = [0u8; 16];
                    f.seek(SeekFrom::Start(block_offset(start, block)?))?;
                    f.read_exact(&mut rh)?;
                    let [_, _, b2, _, _, b5, _, b7, _, _, _, _, _, _, b14, _] = rh;
                    stored = u32::from_le_bytes([b7, b14, b5, b2]);
                }
                (
                    r.u16_0,
                    EntryKind::Resource {
                        block,
                        stored,
                        system: PageFlags(w8),
                        graphics: PageFlags(w12),
                    },
                )
            };
            let name_off = usize::try_from(name_off).map_err(|_| size_err())?;
            raw_names.push(name_at(name_off)?);
            kinds.push(kind);
        }
        // the directory tree: every entry but the root is the child of exactly one directory; the entries are in
        // range by construction (count of each), so a miss is the table's error
        let bad = |what: String| invalid("RPF7", start, format!("{path}: {what}"));
        let mut parent: Vec<Option<usize>> = vec![None; count];
        let mut order = vec![0usize];
        let mut paths = vec![String::new(); count];
        if let Some(root) = paths.first_mut() {
            root.clone_from(&path);
        }
        let mut next = 0;
        while let Some(&d) = order.get(next) {
            next = next.saturating_add(1);
            let Some(&EntryKind::Directory { first, count: n }) = kinds.get(d) else {
                continue;
            };
            let (first, n) = (first as usize, n as usize);
            let end = first
                .checked_add(n)
                .filter(|&end| end <= count && first > 0)
                .ok_or_else(|| bad(format!("directory {d} children {first}+{n} out of range")))?;
            for c in first..end {
                let here = paths
                    .get(d)
                    .cloned()
                    .ok_or_else(|| bad(format!("directory {d}")))?;
                let name = raw_names.get(c).ok_or_else(|| bad(format!("entry {c}")))?;
                match parent.get_mut(c) {
                    Some(p @ None) => *p = Some(d),
                    _ => return Err(bad(format!("entry {c} has two parents"))),
                }
                if let Some(p) = paths.get_mut(c) {
                    *p = format!("{here}\\{}", name.to_lowercase());
                }
                order.push(c);
            }
        }
        if let Some(orphan) = parent
            .iter()
            .skip(1)
            .position(Option::is_none)
            .map(|c| c.saturating_add(1))
        {
            return Err(invalid(
                "RPF7",
                start,
                format!("{path}: entry {orphan} is in no directory"),
            ));
        }
        for k in &kinds {
            let (block, stored) = match *k {
                EntryKind::Binary {
                    block, stored, size, ..
                } => (block, if stored == 0 { size } else { stored }),
                EntryKind::Resource { block, stored, .. } => (block, stored),
                EntryKind::Directory { .. } => continue,
            };
            if u64::from(block)
                .checked_mul(BLOCK)
                .and_then(|o| o.checked_add(u64::from(stored)))
                .is_none_or(|end| end > size)
            {
                return Err(invalid(
                    "RPF7",
                    start,
                    format!("{path}: entry data runs past the archive"),
                ));
            }
        }
        let entries = raw_names
            .into_iter()
            .zip(paths)
            .zip(kinds)
            .map(|((name, path), kind)| Entry { name, path, kind })
            .collect();
        Ok(Archive {
            file: file.to_path_buf(),
            start,
            size,
            name,
            path,
            encryption,
            entries,
        })
    }

    /// Entry `index`.
    fn entry(&self, index: usize) -> Result<&Entry> {
        self.entries.get(index).ok_or_else(|| {
            invalid(
                "RPF7",
                self.start,
                format!("entry {index} of {}", self.entries.len()),
            )
        })
    }

    fn read_at(&self, f: &mut File, block: u32, len: u32) -> Result<Vec<u8>> {
        let len =
            usize::try_from(len).map_err(|_| invalid("RPF7", self.start, "entry larger than memory"))?;
        let mut data = vec![0u8; len];
        f.seek(SeekFrom::Start(block_offset(self.start, block)?))?;
        f.read_exact(&mut data)?;
        Ok(data)
    }

    /// A binary entry's contents: decrypted and inflated as its entry says, exactly its recorded size.
    pub fn binary(&self, f: &mut File, index: usize, keys: &Keys) -> Result<Vec<u8>> {
        match self.stored(f, index, keys)? {
            (data, None) => Ok(data),
            (data, Some(size)) => Ok(Deflate.decompress(&data, size)?),
        }
    }

    /// A binary entry's bytes as the archive stores them, decrypted: a compressed entry's raw deflate stream with the
    /// size it inflates to, or an uncompressed entry's contents with `None`.
    pub fn stored(&self, f: &mut File, index: usize, keys: &Keys) -> Result<(Vec<u8>, Option<usize>)> {
        let e = self.entry(index)?;
        let EntryKind::Binary {
            block,
            stored,
            size,
            encrypted,
        } = e.kind
        else {
            return Err(invalid(
                "RPF7",
                self.start,
                format!("{} is not a binary entry", e.path),
            ));
        };
        let mut data = self.read_at(f, block, if stored == 0 { size } else { stored })?;
        if encrypted {
            match self.encryption {
                Encryption::Aes => aes_decrypt(keys, &self.path, &mut data)?,
                // archives left in the clear by tools still NG-encrypt their entries
                _ => ng_decrypt(keys, &e.name, size, &mut data)?,
            }
        }
        if stored == 0 {
            return Ok((data, None));
        }
        let size =
            usize::try_from(size).map_err(|_| invalid("RPF7", self.start, "entry larger than memory"))?;
        // the stated size is the entry's word: a stream of `data.len()` bytes cannot inflate to more than this
        if size > data.len().saturating_mul(DEFLATE_MAX_RATIO) {
            return Err(invalid(
                "RPF7",
                self.start,
                format!(
                    "{}: {size} bytes from a compressed entry of {} bytes, which inflates to at most {}",
                    e.path,
                    data.len(),
                    data.len().saturating_mul(DEFLATE_MAX_RATIO)
                ),
            ));
        }
        Ok((data, Some(size)))
    }

    /// A resource entry inflated by the entry's page flags (the 16 bytes before the body are no header here).
    pub fn resource(&self, f: &mut File, index: usize, keys: &Keys) -> Result<Resource> {
        let e = self.entry(index)?;
        let EntryKind::Resource {
            block,
            stored,
            system,
            graphics,
        } = e.kind
        else {
            return Err(invalid(
                "RPF7",
                self.start,
                format!("{} is not a resource entry", e.path),
            ));
        };
        if stored < 16 {
            return Err(invalid(
                "RPF7",
                self.start,
                format!("{}: resource shorter than 16 bytes", e.path),
            ));
        }
        let mut data = self.read_at(f, block, stored)?;
        let short = || {
            invalid(
                "RPF7",
                self.start,
                format!("{}: resource shorter than 16 bytes", e.path),
            )
        };
        let body = data.get_mut(16..).ok_or_else(short)?;
        // only scripts are encrypted; nothing marks them but the extension
        if e.name.to_lowercase().ends_with(".ysc") {
            match self.encryption {
                Encryption::Aes => aes_decrypt(keys, &self.path, body)?,
                _ => ng_decrypt(keys, &e.name, stored, body)?,
            }
        }
        resource::inflate(body, system, graphics)
    }
}
