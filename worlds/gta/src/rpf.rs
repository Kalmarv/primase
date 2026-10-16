//! RPF7 archives: a 16-byte header (magic, entry count, names length, cipher), 16-byte entries and a name table,
//! both under the archive's cipher; file data in 512-byte blocks. An archive may hold archives (binary entries
//! named `*.rpf`, stored uncompressed), which are read in place.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use primase_compress::{Decompressor, Deflate};

use crate::keys::Keys;
use crate::ng::{aes_decrypt, ng_decrypt};
use crate::resource::{self, PageFlags, Resource};
use crate::{Result, invalid};

/// The "RPF7" magic, little-endian.
pub const MAGIC: u32 = 0x5250_4637;
const BLOCK: u64 = 512;

/// The cipher on an archive's table of contents (and its encrypted entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encryption {
    None,
    /// A table of contents left in the clear by an editing tool.
    Open,
    Aes,
    Ng,
}

impl Encryption {
    fn from_u32(v: u32) -> Option<Self> {
        match v {
            0x4E45_504F => Some(Encryption::None),
            0x4F50_454E => Some(Encryption::Open),
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

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u24_at(b: &[u8], o: usize) -> u32 {
    u32::from(b[o]) | u32::from(b[o + 1]) << 8 | u32::from(b[o + 2]) << 16
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
        let e = &self.entries[index];
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
        let start = self.start + u64::from(block) * BLOCK;
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
        if u32_at(&h, 0) != MAGIC {
            return Err(invalid(
                "RPF7",
                start,
                format!("{path}: magic {:#010x}", u32_at(&h, 0)),
            ));
        }
        let count = u32_at(&h, 4) as usize;
        let names_len = u32_at(&h, 8) as usize;
        let encryption = Encryption::from_u32(u32_at(&h, 12)).ok_or_else(|| {
            invalid(
                "RPF7",
                start + 12,
                format!("{path}: cipher {:#010x}", u32_at(&h, 12)),
            )
        })?;
        let toc_len = count
            .checked_mul(16)
            .and_then(|n| n.checked_add(names_len))
            .ok_or_else(|| invalid("RPF7", start, "size overflow"))?;
        if 16 + toc_len as u64 > size {
            return Err(invalid(
                "RPF7",
                start,
                format!("{path}: table of contents runs past the archive"),
            ));
        }
        let mut entries_data = vec![0u8; count * 16];
        let mut names = vec![0u8; names_len];
        f.read_exact(&mut entries_data)?;
        f.read_exact(&mut names)?;
        let size32 = u32::try_from(size).map_err(|_| invalid("RPF7", start, "archive larger than 4 GiB"))?;
        match encryption {
            Encryption::None | Encryption::Open => {}
            Encryption::Aes => {
                aes_decrypt(keys, &mut entries_data)?;
                aes_decrypt(keys, &mut names)?;
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
            String::from_utf8(rest[..end].to_vec())
                .map_err(|_| invalid("RPF7", start, format!("{path}: name at {o} is not UTF-8")))
        };
        let mut kinds = Vec::with_capacity(count);
        let mut raw_names = Vec::with_capacity(count);
        for i in 0..count {
            let e = &entries_data[i * 16..i * 16 + 16];
            let second = u32_at(e, 4);
            let (name_off, kind) = if second == 0x7FFF_FF00 {
                (
                    u32_at(e, 0) as usize,
                    EntryKind::Directory {
                        first: u32_at(e, 8),
                        count: u32_at(e, 12),
                    },
                )
            } else if second & 0x8000_0000 == 0 {
                let enc = u32_at(e, 12);
                if enc > 1 {
                    return Err(invalid(
                        "RPF7",
                        start,
                        format!("{path}: entry {i} cipher flag {enc}"),
                    ));
                }
                (
                    usize::from(u16::from_le_bytes([e[0], e[1]])),
                    EntryKind::Binary {
                        block: u24_at(e, 5),
                        stored: u24_at(e, 2),
                        size: u32_at(e, 8),
                        encrypted: enc == 1,
                    },
                )
            } else {
                let mut stored = u24_at(e, 2);
                let block = u24_at(e, 5) & 0x7F_FFFF;
                if stored == 0xFF_FFFF {
                    // a stored size of 2^24 - 1 or more: the real size is spread over the resource header's bytes
                    let mut rh = [0u8; 16];
                    f.seek(SeekFrom::Start(start + u64::from(block) * BLOCK))?;
                    f.read_exact(&mut rh)?;
                    stored = u32::from(rh[7])
                        | u32::from(rh[14]) << 8
                        | u32::from(rh[5]) << 16
                        | u32::from(rh[2]) << 24;
                }
                (
                    usize::from(u16::from_le_bytes([e[0], e[1]])),
                    EntryKind::Resource {
                        block,
                        stored,
                        system: PageFlags(u32_at(e, 8)),
                        graphics: PageFlags(u32_at(e, 12)),
                    },
                )
            };
            raw_names.push(name_at(name_off)?);
            kinds.push(kind);
        }
        // the directory tree: every entry but the root is the child of exactly one directory
        let mut parent: Vec<Option<usize>> = vec![None; count];
        let mut order = vec![0usize];
        let mut paths = vec![String::new(); count];
        paths[0] = path.clone();
        let mut i = 0;
        while i < order.len() {
            let d = order[i];
            i += 1;
            let EntryKind::Directory { first, count: n } = kinds[d] else {
                continue;
            };
            let (first, n) = (first as usize, n as usize);
            if first.checked_add(n).is_none_or(|end| end > count) || first == 0 {
                return Err(invalid(
                    "RPF7",
                    start,
                    format!("{path}: directory {d} children {first}+{n} out of range"),
                ));
            }
            for c in first..first + n {
                if parent[c].is_some() || c == 0 {
                    return Err(invalid(
                        "RPF7",
                        start,
                        format!("{path}: entry {c} has two parents"),
                    ));
                }
                parent[c] = Some(d);
                paths[c] = format!("{}\\{}", paths[d], raw_names[c].to_lowercase());
                order.push(c);
            }
        }
        if let Some(orphan) = (1..count).find(|&c| parent[c].is_none()) {
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
            if u64::from(block) * BLOCK + u64::from(stored) > size {
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

    fn read_at(&self, f: &mut File, block: u32, len: u32) -> Result<Vec<u8>> {
        let mut data = vec![0u8; len as usize];
        f.seek(SeekFrom::Start(self.start + u64::from(block) * BLOCK))?;
        f.read_exact(&mut data)?;
        Ok(data)
    }

    /// A binary entry's contents: decrypted and inflated as its entry says, exactly its recorded size.
    pub fn binary(&self, f: &mut File, index: usize, keys: &Keys) -> Result<Vec<u8>> {
        let e = &self.entries[index];
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
                Encryption::Aes => aes_decrypt(keys, &mut data)?,
                // archives left in the clear by tools still NG-encrypt their entries
                _ => ng_decrypt(keys, &e.name, size, &mut data)?,
            }
        }
        if stored == 0 {
            return Ok(data);
        }
        Ok(Deflate.decompress(&data, size as usize)?)
    }

    /// A resource entry inflated by the entry's page flags (the 16 bytes before the body are no header here).
    pub fn resource(&self, f: &mut File, index: usize, keys: &Keys) -> Result<Resource> {
        let e = &self.entries[index];
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
        // only scripts are encrypted; nothing marks them but the extension
        if e.name.to_lowercase().ends_with(".ysc") {
            match self.encryption {
                Encryption::Aes => aes_decrypt(keys, &mut data[16..])?,
                _ => ng_decrypt(keys, &e.name, stored, &mut data[16..])?,
            }
        }
        resource::inflate(&data[16..], system, graphics)
    }
}
