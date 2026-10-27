//! A forge archive's index: a `.forge` file of version 26 or 27 opened, its headers and its one FileSet's index and
//! name tables read as stored, and every entry's payload located (offset and size in the file; payloads are not read
//! or decoded here: [`crate::data`] decodes their blocks).
//!
//! Layout (little-endian; offsets in bytes; version 26 where it differs, as Shaun White Skateboarding's PC forges
//! store it, AnvilToolkit's version-25 reader being the third-party reading of the same layout):
//! - file header: magic `scimitar\0` (9), version 0x09 (i32, 26 or 27), header size 0x0D (u64), then an i64 and an
//!   i32 (observed 16 and 1);
//! - DataHeader1 (44; 40 in version 26) at the header size: entry count, four i32 (three in version 26), an i64,
//!   `MaxFilesForThisIndex`, an i32 (observed 1: one FileSet), the offset of DataHeader2 (i64);
//! - DataHeader2 (48) there: index count, an i32 (observed 2), the index table's offset (i64), the next FileSet's
//!   offset (i64, -1: none), index start and end (i32), the name table's offset (i64), an i64 (observed: the name
//!   table's offset plus `MaxFilesForThisIndex` name records);
//! - index records (20; 16 in version 26): payload offset (i64), id (u64; u32 in version 26), payload size (i32);
//! - name records (192; 188 in version 26): size (i32), id (u64), an i32, the type id (u32: CRC-32 of the type's name;
//!   0 on every version-26 record seen), two i32, three i32, a timestamp (i32), the name (128 bytes from 44,
//!   NUL-padded), then an i32, the meta-file key (u64; u32 in version 26) and two i32.
//!
//! Index record `i` and name record `i` describe the same entry: the tables correspond by position (their sizes are
//! equal row for row). Their ids are different fields, both kept; the tables are never joined by id.
//!
//! A version-26 entry starts with a 440-byte FILEDATA header before the bytes its size counts: `FILEDATA` (8), the
//! entry's name NUL-padded to 383 bytes, then its index id (u32), its size (i32), the name record's id (u64) and 33
//! bytes more (observed: eight i32, the timestamp the seventh of them, then a zero byte). [`Forge::payload`] checks the
//! magic, the name, the ids and the size against the tables and returns the bytes after it.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: opens the forge file the caller names"
)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::fields::LE;
use primase_foundations::bytes::{Cursor, Field, FieldError};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The file's first bytes.
pub const MAGIC: [u8; 9] = *b"scimitar\0";
/// The versions read.
pub const VERSIONS: [i32; 2] = [26, 27];
/// A version-26 entry's header's first bytes.
pub const FILEDATA_MAGIC: [u8; 8] = *b"FILEDATA";
/// A version-26 entry's header's size, before the bytes its size counts.
pub const FILEDATA_HEADER: u64 = 440;

const FILE_HEADER: u64 = 0x21;
const DATA_HEADER_2: u64 = 48;
/// Where a FILEDATA header's index id is: after the magic and the name.
const FILEDATA_IDS: usize = 8 + 383;

/// A version's record sizes: DataHeader1, an index record, a name record.
fn sizes(version: i32) -> (u64, u64, u64) {
    if version == 26 {
        (40, 16, 188)
    } else {
        (44, 20, 192)
    }
}

/// Why a forge can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading the forge: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a forge: magic {0:02x?}, not \"scimitar\\0\"")]
    Magic([u8; 9]),
    #[error("forge version {0}: only versions 26 and 27 are read")]
    Version(i32),
    #[error("entry {index}: its FILEDATA header {why}")]
    FileData { index: usize, why: String },
    #[error("{what} at {offset}, {length} bytes: outside the file's {file} bytes")]
    OutOfFile {
        what: &'static str,
        offset: i64,
        length: u64,
        file: u64,
    },
    #[error("{what} {value}: {why}")]
    Count {
        what: &'static str,
        value: i64,
        why: String,
    },
    #[error("a second FileSet at {0}: only a forge with one is read")]
    FileSets(i64),
    #[error("entry {index}: payload offset {offset}, size {size}: {why}")]
    Entry {
        index: usize,
        offset: i64,
        size: i32,
        why: &'static str,
    },
    #[error(
        "entry {index}: index size {index_size}, name record size {name_size}: the tables do not correspond"
    )]
    Sizes {
        index: usize,
        index_size: i32,
        name_size: i32,
    },
    #[error("entry {0}: its name is not UTF-8")]
    Name(usize),
    #[error("entries {first} and {second} share the index id {id:#x}")]
    DuplicateId { id: u64, first: usize, second: usize },
    #[error("{0}")]
    Short(#[from] FieldError),
}

/// The file header, before DataHeader1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHeader {
    pub version: i32,
    /// Where DataHeader1 starts.
    pub header_size: u64,
    /// The i64 and i32 after the header size (observed 16 and 1).
    pub unknown_15: i64,
    pub unknown_1d: i32,
}

/// DataHeader1, at the header size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHeader1 {
    pub entries: i32,
    /// Four i32 (three in version 26).
    pub unknown_04: Vec<i32>,
    pub unknown_14: i64,
    pub max_files_for_this_index: i32,
    /// Observed 1 (one FileSet).
    pub unknown_20: i32,
    /// Where DataHeader2 starts.
    pub offset_to_data: i64,
}

/// DataHeader2: where the FileSet's tables are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataHeader2 {
    pub index_count: i32,
    /// Observed 2.
    pub unknown_04: i32,
    pub offset_to_index_table: i64,
    /// -1: no other FileSet.
    pub offset_to_next_data_section: i64,
    pub index_start: i32,
    pub index_end: i32,
    pub offset_to_name_table: i64,
    /// Observed: the name table's offset plus `MaxFilesForThisIndex` name records.
    pub unknown_28: i64,
}

/// An index record: where an entry's payload is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexRecord {
    pub offset: i64,
    /// The index table's id (not the name record's); 32 bits in version 26.
    pub id: u64,
    pub size: i32,
}

/// A name record: an entry's name and type, every field as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRecord {
    pub size: i32,
    /// The name table's id (not the index record's).
    pub id: u64,
    pub unknown_0c: i32,
    /// The entry's type id as the name table holds it (on every type read here it equals the record's own type id,
    /// itself equal to the CRC-32 of a type name: `Mesh`, `TextureMap`, ...; the names are labels).
    pub type_id: u32,
    pub unknown_14: [i32; 2],
    pub unknown_1c: i32,
    pub unknown_20: i32,
    pub unknown_24: i32,
    pub timestamp: i32,
    /// The name, up to its first NUL.
    pub name: String,
    pub unknown_ac: i32,
    /// The meta-file key (AnvilToolkit's name for it; 32 bits in version 26).
    pub meta_file_key: u64,
    pub unknown_b8: [i32; 2],
}

/// A forge's index: its headers, and its entries' index and name records in table order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forge {
    /// The file's size in bytes.
    pub file_size: u64,
    pub header: FileHeader,
    pub data_header_1: DataHeader1,
    pub data_header_2: DataHeader2,
    pub index: Vec<IndexRecord>,
    pub names: Vec<NameRecord>,
    /// Each index id's entry (index ids are unique within a forge; a repeated one is refused).
    by_id: BTreeMap<u64, usize>,
}

impl Forge {
    /// The forge at `path` (see [`Forge::read`]).
    pub fn open(path: impl AsRef<Path>) -> Result<Forge, Error> {
        Forge::read(&mut File::open(path)?)
    }

    /// The forge in `r`: its headers and tables read and checked, nothing else (the payloads stay unread). An error,
    /// naming the field, for another magic or version, a header or table outside the file, an entry count that
    /// disagrees with the index count or exceeds `MaxFilesForThisIndex`, an index range that is not the index count,
    /// a second FileSet, an entry whose payload offset or size is negative or past the file, a row whose index and
    /// name record sizes differ, and a name that is not UTF-8.
    pub fn read<R: Read + Seek>(r: &mut R) -> Result<Forge, Error> {
        let file_size = r.seek(SeekFrom::End(0))?;
        let head_bytes = at(r, file_size, "the file header", 0, FILE_HEADER)?;
        let mut head = Cursor::new(&head_bytes);
        let magic: [u8; 9] = head.array().field("the file header")?;
        if magic != MAGIC {
            return Err(Error::Magic(magic));
        }
        let version = head.i32(LE).field("the file header")?;
        if !VERSIONS.contains(&version) {
            return Err(Error::Version(version));
        }
        let (data_header_1_size, index_record, name_record_size) = sizes(version);
        let header = FileHeader {
            version,
            header_size: head.u64(LE).field("the file header")?,
            unknown_15: head.i64(LE).field("the file header")?,
            unknown_1d: head.i32(LE).field("the file header")?,
        };
        let start = i64::try_from(header.header_size).map_err(|_| Error::OutOfFile {
            what: "DataHeader1",
            offset: i64::MAX,
            length: data_header_1_size,
            file: file_size,
        })?;
        let d1_bytes = at(r, file_size, "DataHeader1", start, data_header_1_size)?;
        let mut f = Cursor::new(&d1_bytes);
        let data_header_1 = DataHeader1 {
            entries: f.i32(LE).field("DataHeader1")?,
            unknown_04: (0..if version == 26 { 3 } else { 4 })
                .map(|_| f.i32(LE).field("DataHeader1"))
                .collect::<Result<_, _>>()?,
            unknown_14: f.i64(LE).field("DataHeader1")?,
            max_files_for_this_index: f.i32(LE).field("DataHeader1")?,
            unknown_20: f.i32(LE).field("DataHeader1")?,
            offset_to_data: f.i64(LE).field("DataHeader1")?,
        };
        let d2_bytes = at(
            r,
            file_size,
            "DataHeader2",
            data_header_1.offset_to_data,
            DATA_HEADER_2,
        )?;
        let mut f = Cursor::new(&d2_bytes);
        let data_header_2 = DataHeader2 {
            index_count: f.i32(LE).field("DataHeader2")?,
            unknown_04: f.i32(LE).field("DataHeader2")?,
            offset_to_index_table: f.i64(LE).field("DataHeader2")?,
            offset_to_next_data_section: f.i64(LE).field("DataHeader2")?,
            index_start: f.i32(LE).field("DataHeader2")?,
            index_end: f.i32(LE).field("DataHeader2")?,
            offset_to_name_table: f.i64(LE).field("DataHeader2")?,
            unknown_28: f.i64(LE).field("DataHeader2")?,
        };
        let count = counts(version, &data_header_1, &data_header_2)?;
        let table = |r: &mut R, what: &'static str, offset: i64, record: u64| {
            let length = u64::try_from(count)
                .ok()
                .and_then(|n| n.checked_mul(record))
                .ok_or(Error::OutOfFile {
                    what,
                    offset,
                    length: u64::MAX,
                    file: file_size,
                })?;
            at(r, file_size, what, offset, length)
        };
        let index_bytes = table(
            r,
            "the index table",
            data_header_2.offset_to_index_table,
            index_record,
        )?;
        let name_bytes = table(
            r,
            "the name table",
            data_header_2.offset_to_name_table,
            name_record_size,
        )?;
        let wide = version != 26;
        let index = index_bytes
            .chunks_exact(usize::try_from(index_record).unwrap_or(usize::MAX))
            .map(|b| {
                let mut f = Cursor::new(b);
                Ok(IndexRecord {
                    offset: f.i64(LE).field("an index record")?,
                    id: if wide {
                        f.u64(LE).field("an index record")?
                    } else {
                        f.u32(LE).field("an index record")?.into()
                    },
                    size: f.i32(LE).field("an index record")?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let header_bytes = if wide { 0 } else { FILEDATA_HEADER };
        let mut names = Vec::with_capacity(index.len());
        let records = name_bytes.chunks_exact(usize::try_from(name_record_size).unwrap_or(usize::MAX));
        for (k, (b, entry)) in records.zip(&index).enumerate() {
            check_entry(k, entry, header_bytes, file_size)?;
            let n = name_record(k, b, wide)?;
            if n.size != entry.size {
                return Err(Error::Sizes {
                    index: k,
                    index_size: entry.size,
                    name_size: n.size,
                });
            }
            names.push(n);
        }
        let mut by_id = BTreeMap::new();
        for (k, e) in index.iter().enumerate() {
            if let Some(first) = by_id.insert(e.id, k) {
                return Err(Error::DuplicateId {
                    id: e.id,
                    first,
                    second: k,
                });
            }
        }
        Ok(Forge {
            file_size,
            header,
            data_header_1,
            data_header_2,
            index,
            names,
            by_id,
        })
    }

    /// The entry whose index record has id `id`. A resource's ClassID equals its entry's index id, and resources
    /// refer to one another by that 64-bit id: this resolves such a reference within this forge (not across forges).
    pub fn find(&self, id: u64) -> Option<usize> {
        self.by_id.get(&id).copied()
    }

    /// Every entry's index and name records, by position.
    pub fn entries(&self) -> impl Iterator<Item = (&IndexRecord, &NameRecord)> {
        self.index.iter().zip(&self.names)
    }

    /// The forge's version (one of [`VERSIONS`]).
    pub fn version(&self) -> i32 {
        self.header.version
    }

    /// Entry `k`'s payload bytes as stored (its index record's size of them, at its offset; in version 26 after its
    /// FILEDATA header, which is checked against the entry's records), from the forge `r` this index was read from;
    /// [`crate::data::read_entry`] reads its blocks.
    pub fn payload<R: Read + Seek>(&self, r: &mut R, k: usize) -> Result<Vec<u8>, Error> {
        let (Some(e), Some(n)) = (self.index.get(k), self.names.get(k)) else {
            return Err(Error::Count {
                what: "the entry",
                value: i64::try_from(k).unwrap_or(i64::MAX),
                why: format!("past the {} entries", self.index.len()),
            });
        };
        let file = r.seek(SeekFrom::End(0))?;
        let size = u64::try_from(e.size).map_err(|_| Error::Entry {
            index: k,
            offset: e.offset,
            size: e.size,
            why: "negative size",
        })?;
        if self.version() != 26 {
            return at(r, file, "the entry's payload", e.offset, size);
        }
        let length = FILEDATA_HEADER.saturating_add(size);
        let mut bytes = at(r, file, "the entry's payload", e.offset, length)?;
        check_filedata(k, &bytes, e, n)?;
        Ok(bytes.split_off(usize::try_from(FILEDATA_HEADER).unwrap_or(usize::MAX)))
    }
}

/// A version-26 entry's FILEDATA header (the module documentation), held to its index and name records.
fn check_filedata(k: usize, bytes: &[u8], e: &IndexRecord, n: &NameRecord) -> Result<(), Error> {
    let fail = |why: String| Error::FileData { index: k, why };
    let magic = bytes.get(..8).unwrap_or_default();
    if magic != FILEDATA_MAGIC {
        return Err(fail(format!("starts {magic:02x?}, not FILEDATA")));
    }
    let stored = bytes.get(8..FILEDATA_IDS).unwrap_or_default();
    let end = stored.iter().position(|&c| c == 0).unwrap_or(stored.len());
    let (name, padding) = stored.split_at(end);
    if name != n.name.as_bytes() || padding.iter().any(|&c| c != 0) {
        return Err(fail(format!(
            "names {:?}, not the name record's {:?}",
            String::from_utf8_lossy(name),
            n.name
        )));
    }
    let mut f = Cursor::new(bytes.get(FILEDATA_IDS..).unwrap_or_default());
    let id = f.u32(LE).field("a FILEDATA header")?;
    let size = f.i32(LE).field("a FILEDATA header")?;
    let name_id = f.u64(LE).field("a FILEDATA header")?;
    if u64::from(id) != e.id || size != e.size || name_id != n.id {
        return Err(fail(format!(
            "holds index id {id:#x}, size {size} and name id {name_id:#x}; the records hold {:#x}, {} and {:#x}",
            e.id, e.size, n.id
        )));
    }
    Ok(())
}

/// The entry count, checked against the index count, `MaxFilesForThisIndex`, the index range and the FileSet link. The
/// index range is the entries' (`index_end` the count) in version 27, the table's room in version 26 (`index_end` the
/// last of `MaxFilesForThisIndex` records, on all 33 forges of the one version-26 game read).
fn counts(version: i32, d1: &DataHeader1, d2: &DataHeader2) -> Result<usize, Error> {
    let count = |what, value: i32| {
        usize::try_from(value).map_err(|_| Error::Count {
            what,
            value: value.into(),
            why: "negative".into(),
        })
    };
    let entries = count("the entry count", d1.entries)?;
    if d2.index_count != d1.entries {
        return Err(Error::Count {
            what: "the index count",
            value: d2.index_count.into(),
            why: format!("not the entry count {}", d1.entries),
        });
    }
    if d1.entries > d1.max_files_for_this_index {
        return Err(Error::Count {
            what: "the entry count",
            value: d1.entries.into(),
            why: format!("over MaxFilesForThisIndex {}", d1.max_files_for_this_index),
        });
    }
    // two i32 apart: no i64 overflow
    let range = i64::from(d2.index_end).saturating_sub(i64::from(d2.index_start));
    let (expected, of) = if version == 26 {
        (
            i64::from(d1.max_files_for_this_index).saturating_sub(1),
            "MaxFilesForThisIndex - 1",
        )
    } else {
        (i64::from(d2.index_count), "the index count")
    };
    if d2.index_start < 0 || range != expected {
        return Err(Error::Count {
            what: "the index range",
            value: range,
            why: format!("from {} to {}: not {of} {expected}", d2.index_start, d2.index_end),
        });
    }
    if d2.offset_to_next_data_section != -1 {
        return Err(Error::FileSets(d2.offset_to_next_data_section));
    }
    Ok(entries)
}

/// Entry `k`'s payload (after a header of `header` bytes) inside the file.
fn check_entry(k: usize, e: &IndexRecord, header: u64, file: u64) -> Result<(), Error> {
    let fail = |why| Error::Entry {
        index: k,
        offset: e.offset,
        size: e.size,
        why,
    };
    let offset = u64::try_from(e.offset).map_err(|_| fail("negative offset"))?;
    let size = u64::try_from(e.size).map_err(|_| fail("negative size"))?;
    match offset.checked_add(size).and_then(|end| end.checked_add(header)) {
        Some(end) if end <= file => Ok(()),
        _ => Err(fail("past the end of the file")),
    }
}

/// Name record `k` from its bytes (192, or 188 when not `wide`: version 26).
fn name_record(k: usize, b: &[u8], wide: bool) -> Result<NameRecord, Error> {
    let mut f = Cursor::new(b);
    let size = f.i32(LE).field("a name record")?;
    let id = f.u64(LE).field("a name record")?;
    let unknown_0c = f.i32(LE).field("a name record")?;
    let type_id = f.u32(LE).field("a name record")?;
    let unknown_14 = [
        f.i32(LE).field("a name record")?,
        f.i32(LE).field("a name record")?,
    ];
    let unknown_1c = f.i32(LE).field("a name record")?;
    let unknown_20 = f.i32(LE).field("a name record")?;
    let unknown_24 = f.i32(LE).field("a name record")?;
    let timestamp = f.i32(LE).field("a name record")?;
    let stored: [u8; 128] = f.array().field("a name record")?;
    let end = stored.iter().position(|&c| c == 0).unwrap_or(stored.len());
    let name = stored
        .get(..end)
        .and_then(|n| std::str::from_utf8(n).ok())
        .ok_or(Error::Name(k))?
        .to_string();
    Ok(NameRecord {
        size,
        id,
        unknown_0c,
        type_id,
        unknown_14,
        unknown_1c,
        unknown_20,
        unknown_24,
        timestamp,
        name,
        unknown_ac: f.i32(LE).field("a name record")?,
        meta_file_key: if wide {
            f.u64(LE).field("a name record")?
        } else {
            f.u32(LE).field("a name record")?.into()
        },
        unknown_b8: [
            f.i32(LE).field("a name record")?,
            f.i32(LE).field("a name record")?,
        ],
    })
}

/// `length` bytes at `offset`, which must lie inside the file.
fn at<R: Read + Seek>(
    r: &mut R,
    file: u64,
    what: &'static str,
    offset: i64,
    length: u64,
) -> Result<Vec<u8>, Error> {
    let outside = || Error::OutOfFile {
        what,
        offset,
        length,
        file,
    };
    let start = u64::try_from(offset).map_err(|_| outside())?;
    match start.checked_add(length) {
        Some(end) if end <= file => {}
        _ => return Err(outside()),
    }
    let mut buf = vec![0; usize::try_from(length).map_err(|_| outside())?];
    r.seek(SeekFrom::Start(start))?;
    r.read_exact(&mut buf)?;
    Ok(buf)
}
