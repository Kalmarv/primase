//! A forge entry's payload: its compressed data blocks read, checked and decoded, and the resource they hold parsed
//! as far as its header (type, name, class and extension); the resource's body is kept as stored, not decoded.
//!
//! A data block (little-endian; offsets from the block's start):
//! - magic 0x1004FA9957FBAA33 (u64);
//! - the compression info, 11 bytes: version (u16), algorithm (u8), two u16 (observed 0 and 32772), block count
//!   (u32);
//! - for its one block: decoded size (u32), stored size (u32), checksum (u32: Adler-32 started from 0 over the
//!   stored bytes);
//! - the stored bytes.
//!
//! Read here: version 2, algorithm 5 (zstd: established on two Mesh resources, Riders Republic build 25394987) with
//! one block; a block whose stored size equals its decoded size is stored raw. An entry is two consecutive blocks: a
//! metadata block (16 bytes decoded, kept as stored) and the resource's. Anything else is refused, naming what it
//! found. A forge's `GlobalMetaFile` entry has no block magic: it is not data blocks, and is refused as such.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_compress::{Decompressor, Zstd};

/// A data block's first bytes.
pub const BLOCK_MAGIC: u64 = 0x1004_FA99_57FB_AA33;
/// The compression version read.
pub const VERSION: u16 = 2;
/// The one algorithm read (zstd).
pub const ALGORITHM_ZSTD: u8 = 5;
/// The checksum's starting value.
pub const CHECKSUM_INIT: u32 = 0;

/// Why a payload can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("block at {at}: no block magic (found {found:016x}): not a data block")]
    Magic { at: usize, found: u64 },
    #[error("block at {at}: compression version {version}: only version 2 is read")]
    Version { at: usize, version: u16 },
    #[error("block at {at}: compression algorithm {algorithm}: only algorithm 5 (zstd) is read")]
    Algorithm { at: usize, algorithm: u8 },
    #[error("block at {at}: {count} blocks: only one is read")]
    Blocks { at: usize, count: u32 },
    #[error("block at {at}: stored size {stored}, decoded size {decoded}: {why}")]
    Size {
        at: usize,
        stored: u32,
        decoded: u32,
        why: &'static str,
    },
    #[error(
        "block at {at}: checksum {stored:08x}, computed {computed:08x} (Adler-32 from 0 over the stored bytes)"
    )]
    Checksum { at: usize, stored: u32, computed: u32 },
    #[error("block at {at}: {source}")]
    Decode {
        at: usize,
        #[source]
        source: primase_compress::Error,
    },
    #[error("{what}: shorter than its layout ({have} of {need} bytes)")]
    Short {
        what: &'static str,
        have: usize,
        need: usize,
    },
    #[error("the entry has {0} bytes after its two blocks")]
    Trailing(usize),
    #[error("resource: {0}")]
    Resource(String),
}

/// One data block as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub version: u16,
    pub algorithm: u8,
    /// The two u16 after the algorithm (observed 0 and 32772).
    pub unknown: [u16; 2],
    pub decoded_size: u32,
    pub stored_size: u32,
    pub checksum: u32,
    /// The decoded bytes (the stored bytes where they are stored raw).
    pub data: Vec<u8>,
}

/// An entry's two blocks: the metadata (16 bytes, as stored) and the resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub metadata: Block,
    pub resource: Block,
}

/// A resource as its block holds it: the header parsed, the body as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// CRC-32 of the type's name.
    pub type_id: u32,
    pub name: String,
    /// The byte after the name (observed 0).
    pub file_header: u8,
    pub class_id: u64,
    /// Observed equal to the type id.
    pub extension: u32,
    /// The rest of the payload, as stored.
    pub body: Vec<u8>,
}

fn take<'a>(bytes: &'a [u8], at: usize, n: usize, what: &'static str) -> Result<&'a [u8], Error> {
    let end = at.checked_add(n).ok_or(Error::Short {
        what,
        have: bytes.len(),
        need: usize::MAX,
    })?;
    bytes.get(at..end).ok_or(Error::Short {
        what,
        have: bytes.len(),
        need: end,
    })
}
fn u16_at(b: &[u8], at: usize, what: &'static str) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(take(b, at, 2, what)?.try_into().map_err(
        |_| Error::Short {
            what,
            have: 0,
            need: 2,
        },
    )?))
}
fn u32_at(b: &[u8], at: usize, what: &'static str) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(take(b, at, 4, what)?.try_into().map_err(
        |_| Error::Short {
            what,
            have: 0,
            need: 4,
        },
    )?))
}
fn u64_at(b: &[u8], at: usize, what: &'static str) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(take(b, at, 8, what)?.try_into().map_err(
        |_| Error::Short {
            what,
            have: 0,
            need: 8,
        },
    )?))
}

/// The block at `at` in `bytes`, checked and decoded, and where the next one starts. `max_decoded` bounds the decoded
/// size the block may declare.
pub fn read_block(bytes: &[u8], at: usize, max_decoded: usize) -> Result<(Block, usize), Error> {
    let magic = u64_at(bytes, at, "block magic")?;
    if magic != BLOCK_MAGIC {
        return Err(Error::Magic { at, found: magic });
    }
    let info = |off: usize| {
        at.checked_add(off).ok_or(Error::Short {
            what: "block",
            have: bytes.len(),
            need: usize::MAX,
        })
    };
    let version = u16_at(bytes, info(8)?, "compression version")?;
    if version != VERSION {
        return Err(Error::Version { at, version });
    }
    let algorithm = *take(bytes, info(10)?, 1, "compression algorithm")?
        .first()
        .ok_or(Error::Short {
            what: "compression algorithm",
            have: 0,
            need: 1,
        })?;
    if algorithm != ALGORITHM_ZSTD {
        return Err(Error::Algorithm { at, algorithm });
    }
    let unknown = [
        u16_at(bytes, info(11)?, "compression info")?,
        u16_at(bytes, info(13)?, "compression info")?,
    ];
    let count = u32_at(bytes, info(15)?, "block count")?;
    if count != 1 {
        return Err(Error::Blocks { at, count });
    }
    let decoded_size = u32_at(bytes, info(19)?, "decoded size")?;
    let stored_size = u32_at(bytes, info(23)?, "stored size")?;
    let checksum = u32_at(bytes, info(27)?, "checksum")?;
    let size = |why| Error::Size {
        at,
        stored: stored_size,
        decoded: decoded_size,
        why,
    };
    let decoded = usize::try_from(decoded_size).map_err(|_| size("decoded size past this machine"))?;
    let stored_len = usize::try_from(stored_size).map_err(|_| size("stored size past this machine"))?;
    if decoded > max_decoded {
        return Err(size("decoded size over the stated bound"));
    }
    let stored = take(bytes, info(31)?, stored_len, "stored bytes")?;
    let computed = primase_hash::adler32(CHECKSUM_INIT, stored);
    if computed != checksum {
        return Err(Error::Checksum {
            at,
            stored: checksum,
            computed,
        });
    }
    let data = if stored_len == decoded {
        stored.to_vec()
    } else {
        Zstd.decompress(stored, decoded)
            .map_err(|source| Error::Decode { at, source })?
    };
    let next = info(31)?
        .checked_add(stored_len)
        .ok_or(size("block past the entry"))?;
    Ok((
        Block {
            version,
            algorithm,
            unknown,
            decoded_size,
            stored_size,
            checksum,
            data,
        },
        next,
    ))
}

/// An entry's payload (its bytes from the index's offset, the index's size): its metadata block and its resource
/// block, which must end the entry exactly. `max_decoded` bounds each block's declared decoded size.
pub fn read_entry(payload: &[u8], max_decoded: usize) -> Result<Entry, Error> {
    let (metadata, next) = read_block(payload, 0, max_decoded)?;
    let (resource, end) = read_block(payload, next, max_decoded)?;
    let rest = payload.len().checked_sub(end).ok_or(Error::Short {
        what: "entry",
        have: payload.len(),
        need: end,
    })?;
    if rest != 0 {
        return Err(Error::Trailing(rest));
    }
    Ok(Entry { metadata, resource })
}

impl Resource {
    /// A resource block's decoded bytes: type id (u32), payload length (i32), name length (i32), the name, one byte
    /// (observed 0), then the payload (class id u64, extension u32, the body), which must end the block exactly.
    pub fn parse(data: &[u8]) -> Result<Resource, Error> {
        let bad = |m: String| Error::Resource(m);
        let type_id = u32_at(data, 0, "resource type")?;
        let payload_len = u32_at(data, 4, "resource payload length")?;
        let name_len = u32_at(data, 8, "resource name length")?;
        let name_len = usize::try_from(name_len).map_err(|_| bad("name length past this machine".into()))?;
        let name = take(data, 12, name_len, "resource name")?;
        let name = String::from_utf8(name.to_vec()).map_err(|_| bad("name is not UTF-8".into()))?;
        let header_at = 12usize
            .checked_add(name_len)
            .ok_or(bad("name length overflows".into()))?;
        let file_header = *take(data, header_at, 1, "resource file header")?
            .first()
            .ok_or(bad("no file header byte".into()))?;
        let payload_at = header_at.checked_add(1).ok_or(bad("offset overflows".into()))?;
        let payload_len =
            usize::try_from(payload_len).map_err(|_| bad("payload length past this machine".into()))?;
        let end = payload_at
            .checked_add(payload_len)
            .ok_or(bad("payload length overflows".into()))?;
        if end != data.len() {
            return Err(bad(format!(
                "header and payload end at {end}, the block holds {} bytes",
                data.len()
            )));
        }
        let class_id = u64_at(data, payload_at, "resource class id")?;
        let extension = u32_at(
            data,
            payload_at.checked_add(8).ok_or(bad("offset overflows".into()))?,
            "resource extension",
        )?;
        let body_at = payload_at.checked_add(12).ok_or(bad("offset overflows".into()))?;
        let body = data
            .get(body_at..)
            .ok_or(bad("payload shorter than its class id and extension".into()))?;
        Ok(Resource {
            type_id,
            name,
            file_header,
            class_id,
            extension,
            body: body.to_vec(),
        })
    }
}
