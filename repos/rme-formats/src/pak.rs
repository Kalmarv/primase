//! Big-endian PAK v4. Names retain their original bytes (no path normalization).
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "reader I/O: open the caller's archive"
)]
use crate::cstr;
use crate::{Result, invalid};
use primase_foundations::bytes::{Endian::Big, Field, View};
use std::{borrow::Cow, ops::Range, path::Path};

#[derive(Debug)]
pub struct Entry {
    pub name: Vec<u8>,
    pub hash: u32,
    /// Section index; a marker belongs to the section it opens.
    pub section: Option<usize>,
    pub attributes: u32,
    /// Runtime allocation alignment, not file-offset alignment.
    pub alignment: u32,
    pub offset: usize,
    pub size: usize,
    /// Zero for stored entries and markers; otherwise the encoded CSTR length.
    pub compressed_size: usize,
}

#[derive(Debug)]
pub struct Section {
    pub name: Vec<u8>,
    /// Includes this section's marker entry.
    pub entries: Range<usize>,
}

/// Owns archive bytes; metadata is checked at construction and payloads decode on request.
#[derive(Debug)]
pub struct Pak {
    bytes: Vec<u8>,
    entries: Vec<Entry>,
    sections: Vec<Section>,
}

impl Pak {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let v = View::new(&bytes);
        if v.slice(0, 4).field("PAK magic")? != b"PAK\0" {
            return Err(invalid(0, "expected PAK"));
        }
        if v.u16(4, Big).field("PAK endian tag")? != 0xfeeb {
            return Err(invalid(4, "PAK endian tag"));
        }
        if v.u16(6, Big).field("PAK version")? != 4 {
            return Err(invalid(6, "unsupported PAK version"));
        }
        let count = v.u32(8, Big).field("entry count")? as usize;
        let strsize = v.u32(12, Big).field("string table size")? as usize;
        let strstart = count
            .checked_mul(32)
            .and_then(|n| n.checked_add(16))
            .ok_or_else(|| invalid(8, "entry count overflow"))?;
        let strings = v.slice(strstart, strsize).field("string table")?;
        let mut pos = strstart + strsize;
        let mut entries = Vec::new();
        let mut sections: Vec<Section> = Vec::new();
        for i in 0..count {
            let at = 16 + i * 32;
            let hash = v.u32(at, Big).field("entry hash")?;
            let name_off = v.u32(at + 4, Big).field("name offset")? as usize;
            let tail = strings
                .get(name_off..)
                .ok_or_else(|| invalid(at + 4, "name offset outside string table"))?;
            let len = tail
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| invalid(strstart + name_off, "unterminated name"))?;
            let name = &tail[..len];
            if primase_foundations::hash::sdbm(name) != hash {
                return Err(invalid(at, "name hash mismatch"));
            }
            let attributes = v.u32(at + 8, Big).field("attributes")?;
            if !matches!(attributes, 0..=2) {
                return Err(invalid(at + 8, "unknown entry attributes"));
            }
            let alignment = v.u32(at + 12, Big).field("alignment")?;
            if !matches!(alignment, 0 | 16 | 64 | 128) {
                return Err(invalid(at + 12, "unknown alignment"));
            }
            let offset = v.u32(at + 16, Big).field("payload offset")? as usize;
            let size = v.u32(at + 20, Big).field("payload size")? as usize;
            let compressed_size = v.u32(at + 24, Big).field("compressed size")? as usize;
            if v.u32(at + 28, Big).field("runtime offset")? != 0 {
                return Err(invalid(at + 28, "nonzero on-disc runtime offset"));
            }
            if offset != pos {
                return Err(invalid(at + 16, "payload gap or overlap"));
            }
            let stored_size = if attributes == 2 { compressed_size } else { size };
            v.slice(offset, stored_size).field("entry payload")?;
            if (attributes == 2) != (compressed_size != 0) {
                return Err(invalid(
                    at + 24,
                    "compressed attribute and compressed size disagree",
                ));
            }
            if attributes == 1 {
                if size != 0 || alignment != 0 {
                    return Err(invalid(at + 20, "section marker has payload or alignment"));
                }
                if let Some(section) = sections.last_mut() {
                    section.entries.end = i;
                }
                sections.push(Section {
                    name: name.to_vec(),
                    entries: i..count,
                });
            }
            entries.push(Entry {
                name: name.to_vec(),
                hash,
                section: sections.len().checked_sub(1),
                attributes,
                alignment,
                offset,
                size,
                compressed_size,
            });
            pos += stored_size;
        }
        if pos != bytes.len() {
            return Err(invalid(pos, "trailing bytes after PAK payloads"));
        }
        Ok(Self {
            bytes,
            entries,
            sections,
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Raw encoded bytes (a marker has an empty payload).
    pub fn encoded(&self, index: usize) -> Result<&[u8]> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| invalid(8, "entry index out of bounds"))?;
        let stored_size = if e.attributes == 2 {
            e.compressed_size
        } else {
            e.size
        };
        Ok(&self.bytes[e.offset..e.offset + stored_size])
    }

    /// Stored entries borrow the archive; compressed entries allocate only when requested.
    /// Compression errors identify the entry's absolute payload offset and the stream-relative error.
    pub fn payload(&self, index: usize) -> Result<Cow<'_, [u8]>> {
        let bytes = self.encoded(index)?;
        let e = &self.entries[index];
        if e.attributes == 2 {
            Ok(Cow::Owned(
                cstr::decode(bytes, e.size)
                    .map_err(|err| invalid(e.offset, err))?
                    .bytes,
            ))
        } else {
            Ok(Cow::Borrowed(bytes))
        }
    }
}
