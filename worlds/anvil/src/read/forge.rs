//! A forge archive's index: a `.forge` file of version 27 opened, its headers and its one FileSet's index and name
//! tables read as stored, and every entry's payload located (offset and size in the file; payloads are not read or
//! decoded: their compression is not established).
//!
//! Layout (little-endian; offsets in bytes):
//! - file header: magic `scimitar\0` (9), version 0x09 (i32, 27), header size 0x0D (u64), then an i64 and an i32
//!   (observed 16 and 1);
//! - DataHeader1 (44) at the header size: entry count, four i32, an i64, `MaxFilesForThisIndex`, an i32 (observed 1:
//!   one FileSet), the offset of DataHeader2 (i64);
//! - DataHeader2 (48) there: index count, an i32 (observed 2), the index table's offset (i64), the next FileSet's
//!   offset (i64, -1: none), index start and end (i32), the name table's offset (i64), an i64 (observed: the name
//!   table's offset plus `MaxFilesForThisIndex` name records);
//! - index records (20): payload offset (i64), id (u64), payload size (i32);
//! - name records (192): size (i32), id (u64), an i32, the type id (u32: CRC-32 of the type's name), two i32, three
//!   i32, a timestamp (i32), the name (128 bytes from 44, NUL-padded), five i32.
//!
//! Index record `i` and name record `i` describe the same entry: the tables correspond by position (their sizes are
//! equal row for row). Their ids are different fields, both kept; the tables are never joined by id.
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

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The file's first bytes.
pub const MAGIC: [u8; 9] = *b"scimitar\0";
/// The one version read.
pub const VERSION: i32 = 27;
/// An index record's size.
pub const INDEX_RECORD: u64 = 20;
/// A name record's size.
pub const NAME_RECORD: u64 = 192;

const FILE_HEADER: u64 = 0x21;
const DATA_HEADER_1: u64 = 44;
const DATA_HEADER_2: u64 = 48;

/// Why a forge can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reading the forge: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a forge: magic {0:02x?}, not \"scimitar\\0\"")]
    Magic([u8; 9]),
    #[error("forge version {0}: only version 27 is read")]
    Version(i32),
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
    #[error("{0}: shorter than its layout")]
    Short(&'static str),
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
    pub unknown_04: [i32; 4],
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
    /// The index table's id (not the name record's).
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
    /// CRC-32 of the type's name (`Mesh`, `TextureMap`, ...).
    pub type_id: u32,
    pub unknown_14: [i32; 2],
    pub unknown_1c: i32,
    pub unknown_20: i32,
    pub unknown_24: i32,
    pub timestamp: i32,
    /// The name, up to its first NUL.
    pub name: String,
    pub unknown_ac: [i32; 5],
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
        let mut head = Fields::new("the file header", &head_bytes);
        let magic: [u8; 9] = head.take()?;
        if magic != MAGIC {
            return Err(Error::Magic(magic));
        }
        let version = head.i32()?;
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let header = FileHeader {
            version,
            header_size: head.u64()?,
            unknown_15: head.i64()?,
            unknown_1d: head.i32()?,
        };
        let start = i64::try_from(header.header_size).map_err(|_| Error::OutOfFile {
            what: "DataHeader1",
            offset: i64::MAX,
            length: DATA_HEADER_1,
            file: file_size,
        })?;
        let d1_bytes = at(r, file_size, "DataHeader1", start, DATA_HEADER_1)?;
        let mut f = Fields::new("DataHeader1", &d1_bytes);
        let data_header_1 = DataHeader1 {
            entries: f.i32()?,
            unknown_04: [f.i32()?, f.i32()?, f.i32()?, f.i32()?],
            unknown_14: f.i64()?,
            max_files_for_this_index: f.i32()?,
            unknown_20: f.i32()?,
            offset_to_data: f.i64()?,
        };
        let d2_bytes = at(
            r,
            file_size,
            "DataHeader2",
            data_header_1.offset_to_data,
            DATA_HEADER_2,
        )?;
        let mut f = Fields::new("DataHeader2", &d2_bytes);
        let data_header_2 = DataHeader2 {
            index_count: f.i32()?,
            unknown_04: f.i32()?,
            offset_to_index_table: f.i64()?,
            offset_to_next_data_section: f.i64()?,
            index_start: f.i32()?,
            index_end: f.i32()?,
            offset_to_name_table: f.i64()?,
            unknown_28: f.i64()?,
        };
        let count = counts(&data_header_1, &data_header_2)?;
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
            INDEX_RECORD,
        )?;
        let name_bytes = table(
            r,
            "the name table",
            data_header_2.offset_to_name_table,
            NAME_RECORD,
        )?;
        let index = index_bytes
            .as_chunks::<20>()
            .0
            .iter()
            .map(|b| {
                let mut f = Fields::new("an index record", b);
                Ok(IndexRecord {
                    offset: f.i64()?,
                    id: f.u64()?,
                    size: f.i32()?,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let mut names = Vec::with_capacity(index.len());
        for (k, (b, entry)) in name_bytes.as_chunks::<192>().0.iter().zip(&index).enumerate() {
            check_entry(k, entry, file_size)?;
            let n = name_record(k, b)?;
            if n.size != entry.size {
                return Err(Error::Sizes {
                    index: k,
                    index_size: entry.size,
                    name_size: n.size,
                });
            }
            names.push(n);
        }
        Ok(Forge {
            file_size,
            header,
            data_header_1,
            data_header_2,
            index,
            names,
        })
    }

    /// Every entry's index and name records, by position.
    pub fn entries(&self) -> impl Iterator<Item = (&IndexRecord, &NameRecord)> {
        self.index.iter().zip(&self.names)
    }
}

/// The entry count, checked against the index count, `MaxFilesForThisIndex`, the index range and the FileSet link.
fn counts(d1: &DataHeader1, d2: &DataHeader2) -> Result<usize, Error> {
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
    if d2.index_start < 0 || range != i64::from(d2.index_count) {
        return Err(Error::Count {
            what: "the index range",
            value: range,
            why: format!(
                "from {} to {}: not the index count {}",
                d2.index_start, d2.index_end, d2.index_count
            ),
        });
    }
    if d2.offset_to_next_data_section != -1 {
        return Err(Error::FileSets(d2.offset_to_next_data_section));
    }
    Ok(entries)
}

/// Entry `k`'s payload inside the file.
fn check_entry(k: usize, e: &IndexRecord, file: u64) -> Result<(), Error> {
    let fail = |why| Error::Entry {
        index: k,
        offset: e.offset,
        size: e.size,
        why,
    };
    let offset = u64::try_from(e.offset).map_err(|_| fail("negative offset"))?;
    let size = u64::try_from(e.size).map_err(|_| fail("negative size"))?;
    match offset.checked_add(size) {
        Some(end) if end <= file => Ok(()),
        _ => Err(fail("past the end of the file")),
    }
}

/// Name record `k` from its 192 bytes.
fn name_record(k: usize, b: &[u8]) -> Result<NameRecord, Error> {
    let mut f = Fields::new("a name record", b);
    let size = f.i32()?;
    let id = f.u64()?;
    let unknown_0c = f.i32()?;
    let type_id = f.u32()?;
    let unknown_14 = [f.i32()?, f.i32()?];
    let unknown_1c = f.i32()?;
    let unknown_20 = f.i32()?;
    let unknown_24 = f.i32()?;
    let timestamp = f.i32()?;
    let stored: [u8; 128] = f.take()?;
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
        unknown_ac: [f.i32()?, f.i32()?, f.i32()?, f.i32()?, f.i32()?],
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

/// Little-endian fields read in order from a record's bytes.
struct Fields<'a> {
    what: &'static str,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    fn new(what: &'static str, bytes: &'a [u8]) -> Fields<'a> {
        Fields { what, bytes, at: 0 }
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let end = self.at.checked_add(N).ok_or(Error::Short(self.what))?;
        let field = self
            .bytes
            .get(self.at..end)
            .and_then(|b| <[u8; N]>::try_from(b).ok())
            .ok_or(Error::Short(self.what))?;
        self.at = end;
        Ok(field)
    }
    fn i32(&mut self) -> Result<i32, Error> {
        self.take().map(i32::from_le_bytes)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        self.take().map(u32::from_le_bytes)
    }
    fn i64(&mut self) -> Result<i64, Error> {
        self.take().map(i64::from_le_bytes)
    }
    fn u64(&mut self) -> Result<u64, Error> {
        self.take().map(u64::from_le_bytes)
    }
}
