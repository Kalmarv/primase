//! A forge entry's payload: its compressed data blocks read, checked and decoded, and the resource they hold parsed
//! as far as its header (type, name, class and extension); the resource's body is kept as stored, not decoded.
//!
//! A data block (little-endian; offsets from the block's start):
//! - magic 0x1004FA9957FBAA33 (u64);
//! - the compression info, 11 bytes: version (u16), algorithm (u8), two u16 (observed 0 and 32772), chunk count
//!   (u32); in version 1, 9 bytes: the chunk count is a u16 (the two u16 observed 32768 and 32768);
//! - every chunk's decoded size and stored size (u32 each; u16 each in version 1), in order;
//! - then for each chunk in order: its checksum (u32: Adler-32 started from 0 over its stored bytes) and its stored
//!   bytes.
//!
//! Read here: version 2, algorithm 5 (zstd: established on Mesh resources and a three-chunk CompiledMip, Riders
//! Republic build 25394987), and version 1, algorithm 2 (LZO2A: every chunk of Shaun White Skateboarding's PC forges,
//! algorithm 2 as AnvilToolkit maps it for the Assassin's Creed 2 era); a chunk whose stored size equals its decoded
//! size is stored raw. The block's data is its
//! chunks' decoded bytes in order. An entry is two consecutive blocks: a metadata block (16 bytes decoded, kept as
//! stored) and the resource's. Anything else is refused, naming what it
//! found. A forge's `GlobalMetaFile` entry has no block magic: it is not data blocks, and is refused as such.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_foundations::bytes::{Field, FieldError, View};
use primase_foundations::compress::{Decompressor, Lzo2a, Zstd};

use crate::fields::LE;

/// A data block's first bytes.
pub const BLOCK_MAGIC: u64 = 0x1004_FA99_57FB_AA33;
/// The compression version read with zstd.
pub const VERSION: u16 = 2;
/// The compression version read with LZO2A.
pub const VERSION_1: u16 = 1;
/// Version 2's algorithm read (zstd).
pub const ALGORITHM_ZSTD: u8 = 5;
/// Version 1's algorithm read (LZO2A).
pub const ALGORITHM_LZO2A: u8 = 2;
/// The checksum's starting value.
pub const CHECKSUM_INIT: u32 = 0;

/// Why a payload can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("block at {at}: no block magic (found {found:016x}): not a data block")]
    Magic { at: usize, found: u64 },
    #[error("block at {at}: compression version {version}: only versions 1 and 2 are read")]
    Version { at: usize, version: u16 },
    #[error(
        "block at {at}: compression version {version}, algorithm {algorithm}: only 2 with 5 (zstd) and 1 with 2 (LZO2A) are read"
    )]
    Algorithm { at: usize, version: u16, algorithm: u8 },
    #[error("block at {at}: {count} chunks: {why}")]
    Chunks {
        at: usize,
        count: u32,
        why: &'static str,
    },
    #[error("block at {at}, chunk {chunk}: stored size {stored}, decoded size {decoded}: {why}")]
    Size {
        at: usize,
        chunk: usize,
        stored: u32,
        decoded: u32,
        why: &'static str,
    },
    #[error(
        "block at {at}, chunk {chunk}: checksum {stored:08x}, computed {computed:08x} (Adler-32 from 0 over the stored bytes)"
    )]
    Checksum {
        at: usize,
        chunk: usize,
        stored: u32,
        computed: u32,
    },
    #[error("block at {at}: {source}")]
    Decode {
        at: usize,
        #[source]
        source: primase_foundations::compress::Error,
    },
    #[error("{0}")]
    Short(#[from] FieldError),
    #[error("{0}: sizes past the address space")]
    Overflow(&'static str),
    #[error("the entry has {0} bytes after its two blocks")]
    Trailing(usize),
    #[error("resource: {0}")]
    Resource(String),
}

/// One chunk of a block as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub decoded_size: u32,
    pub stored_size: u32,
    pub checksum: u32,
}

/// One data block as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub version: u16,
    pub algorithm: u8,
    /// The two u16 after the algorithm (observed 0 and 32772).
    pub unknown: [u16; 2],
    pub chunks: Vec<Chunk>,
    /// The chunks' decoded bytes in order (the stored bytes where a chunk is stored raw).
    pub data: Vec<u8>,
}

impl Block {
    /// The decoded size of all its chunks.
    pub fn decoded_size(&self) -> usize {
        self.data.len()
    }
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
    /// The byte after the name: 0 (1,292,246 resources of the six forges' census) or 1 (9,091: every Animation, the
    /// one AnimTrackBuiltinTracksTable and three of 0x8DEEB8AD), which a table follows ([`Resource::header_rows`]).
    pub file_header: u8,
    /// With file header 1: the table between it and the payload, as stored: the bytes `02 00 00`, the row count (u32)
    /// and that many rows of 12 bytes. Their meaning isn't read (on an Animation: pairs of a hash and a count, the
    /// registry's object-type hashes among them). Empty with file header 0.
    pub header_rows: Vec<[u8; 12]>,
    pub class_id: u64,
    /// Observed equal to the type id.
    pub extension: u32,
    /// The rest of the payload, as stored.
    pub body: Vec<u8>,
}

/// The block at `at` in `bytes`, checked and decoded, and where the next one starts. `max_decoded` bounds the decoded
/// size its chunks may declare in all.
pub fn read_block(bytes: &[u8], at: usize, max_decoded: usize) -> Result<(Block, usize), Error> {
    let magic = View::new(bytes).u64(at, LE).field("block magic")?;
    if magic != BLOCK_MAGIC {
        return Err(Error::Magic { at, found: magic });
    }
    // an offset past the address space is past the data: its read is refused
    let info = |off: usize| at.saturating_add(off);
    let overflow = Error::Overflow;
    let version = View::new(bytes).u16(info(8), LE).field("compression version")?;
    if version != VERSION && version != VERSION_1 {
        return Err(Error::Version { at, version });
    }
    let narrow = version == VERSION_1;
    let algorithm = View::new(bytes).u8(info(10)).field("compression algorithm")?;
    let decoder: &dyn Decompressor = match (version, algorithm) {
        (VERSION, ALGORITHM_ZSTD) => &Zstd,
        (VERSION_1, ALGORITHM_LZO2A) => &Lzo2a,
        _ => {
            return Err(Error::Algorithm {
                at,
                version,
                algorithm,
            });
        }
    };
    let unknown = [
        View::new(bytes).u16(info(11), LE).field("compression info")?,
        View::new(bytes).u16(info(13), LE).field("compression info")?,
    ];
    let count = if narrow {
        View::new(bytes).u16(info(15), LE).field("chunk count")?.into()
    } else {
        View::new(bytes).u32(info(15), LE).field("chunk count")?
    };
    // the chunk table's start and a row's size: (u16, u16) rows after a u16 count in version 1
    let (table, row): (usize, usize) = if narrow { (17, 4) } else { (19, 8) };
    let n = usize::try_from(count).map_err(|_| Error::Chunks {
        at,
        count,
        why: "past this machine",
    })?;
    if n == 0 {
        return Err(Error::Chunks {
            at,
            count,
            why: "a block holds at least one",
        });
    }
    // every chunk needs its row of sizes and 4 bytes of checksum: a count the bytes can't hold is refused before
    // allocating
    let tables = n
        .checked_mul(row.saturating_add(4))
        .ok_or(overflow("chunk table"))?;
    if tables > bytes.len().saturating_sub(at) {
        return Err(Error::Chunks {
            at,
            count,
            why: "more than the bytes can hold",
        });
    }
    let mut chunks = Vec::with_capacity(n);
    let mut total = 0usize;
    for k in 0..n {
        let pair = info(table)
            .checked_add(k.checked_mul(row).ok_or(overflow("chunk table"))?)
            .ok_or(overflow("chunk table"))?;
        let (decoded_size, stored_size) = if narrow {
            let second = pair.checked_add(2).ok_or(overflow("chunk table"))?;
            (
                u32::from(View::new(bytes).u16(pair, LE).field("chunk decoded size")?),
                u32::from(View::new(bytes).u16(second, LE).field("chunk stored size")?),
            )
        } else {
            let second = pair.checked_add(4).ok_or(overflow("chunk table"))?;
            (
                View::new(bytes).u32(pair, LE).field("chunk decoded size")?,
                View::new(bytes).u32(second, LE).field("chunk stored size")?,
            )
        };
        let decoded = usize::try_from(decoded_size).map_err(|_| Error::Size {
            at,
            chunk: k,
            stored: stored_size,
            decoded: decoded_size,
            why: "decoded size past this machine",
        })?;
        total = total.checked_add(decoded).ok_or(overflow("decoded sizes"))?;
        if total > max_decoded {
            return Err(Error::Size {
                at,
                chunk: k,
                stored: stored_size,
                decoded: decoded_size,
                why: "decoded sizes over the stated bound",
            });
        }
        chunks.push(Chunk {
            decoded_size,
            stored_size,
            checksum: 0,
        });
    }
    let mut data = Vec::new();
    data.try_reserve_exact(total)
        .map_err(|_| overflow("decoded bytes"))?;
    let mut cursor = info(table)
        .checked_add(n.checked_mul(row).ok_or(overflow("chunk table"))?)
        .ok_or(overflow("chunk table"))?;
    for (k, chunk) in chunks.iter_mut().enumerate() {
        let size = |why| Error::Size {
            at,
            chunk: k,
            stored: chunk.stored_size,
            decoded: chunk.decoded_size,
            why,
        };
        chunk.checksum = View::new(bytes).u32(cursor, LE).field("chunk checksum")?;
        let from = cursor.checked_add(4).ok_or(overflow("chunk"))?;
        let stored_len =
            usize::try_from(chunk.stored_size).map_err(|_| size("stored size past this machine"))?;
        let decoded =
            usize::try_from(chunk.decoded_size).map_err(|_| size("decoded size past this machine"))?;
        let stored = View::new(bytes)
            .slice(from, stored_len)
            .field("chunk stored bytes")?;
        let computed = primase_foundations::hash::adler32(CHECKSUM_INIT, stored);
        if computed != chunk.checksum {
            return Err(Error::Checksum {
                at,
                chunk: k,
                stored: chunk.checksum,
                computed,
            });
        }
        if stored_len == decoded {
            data.extend_from_slice(stored);
        } else {
            let out = decoder
                .decompress(stored, decoded)
                .map_err(|source| Error::Decode { at, source })?;
            data.extend_from_slice(&out);
        }
        cursor = from.checked_add(stored_len).ok_or(size("chunk past the entry"))?;
    }
    Ok((
        Block {
            version,
            algorithm,
            unknown,
            chunks,
            data,
        },
        cursor,
    ))
}

/// An entry's payload (its bytes from the index's offset, the index's size): its metadata block and its resource
/// block, which must end the entry exactly. `max_decoded` bounds each block's declared decoded size.
pub fn read_entry(payload: &[u8], max_decoded: usize) -> Result<Entry, Error> {
    let (metadata, next) = read_block(payload, 0, max_decoded)?;
    let (resource, end) = read_block(payload, next, max_decoded)?;
    primase_foundations::bytes::checked_span(end, 0, 1, payload.len()).field("entry")?;
    let rest = payload.len().saturating_sub(end);
    if rest != 0 {
        return Err(Error::Trailing(rest));
    }
    Ok(Entry { metadata, resource })
}

impl Resource {
    /// A resource block's decoded bytes: type id (u32), payload length (i32), name length (i32), the name, one byte
    /// (0, or 1 and the table `02 00 00`, a row count and that many 12-byte rows), then the payload (class id u64,
    /// extension u32, the body), which must end the block exactly.
    pub fn parse(data: &[u8]) -> Result<Resource, Error> {
        let bad = |m: String| Error::Resource(m);
        let type_id = View::new(data).u32(0, LE).field("resource type")?;
        let payload_len = View::new(data).u32(4, LE).field("resource payload length")?;
        let name_len = View::new(data).u32(8, LE).field("resource name length")?;
        let name_len = usize::try_from(name_len).map_err(|_| bad("name length past this machine".into()))?;
        let name = View::new(data).slice(12, name_len).field("resource name")?;
        let name = String::from_utf8(name.to_vec()).map_err(|_| bad("name is not UTF-8".into()))?;
        let header_at = 12usize
            .checked_add(name_len)
            .ok_or(bad("name length overflows".into()))?;
        let file_header = *View::new(data)
            .slice(header_at, 1)
            .field("resource file header")?
            .first()
            .ok_or(bad("no file header byte".into()))?;
        let mut payload_at = header_at.checked_add(1).ok_or(bad("offset overflows".into()))?;
        let mut header_rows = Vec::new();
        if file_header == 1 {
            if View::new(data)
                .slice(payload_at, 3)
                .field("resource header table")?
                != [2, 0, 0]
            {
                return Err(bad("file header 1 without the table's bytes 02 00 00".into()));
            }
            let count_at = payload_at.checked_add(3).ok_or(bad("offset overflows".into()))?;
            let rows = usize::try_from(
                View::new(data)
                    .u32(count_at, LE)
                    .field("resource header table rows")?,
            )
            .map_err(|_| bad("row count past this machine".into()))?;
            let mut at = count_at.checked_add(4).ok_or(bad("offset overflows".into()))?;
            for _ in 0..rows.min(data.len()) {
                let row = View::new(data).slice(at, 12).field("resource header table row")?;
                header_rows.push(<[u8; 12]>::try_from(row).map_err(|_| bad("a row of 12 bytes".into()))?);
                at = at.checked_add(12).ok_or(bad("offset overflows".into()))?;
            }
            if header_rows.len() != rows {
                return Err(bad(format!(
                    "{rows} header rows in a block of {} bytes",
                    data.len()
                )));
            }
            payload_at = at;
        }
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
        let class_id = View::new(data).u64(payload_at, LE).field("resource class id")?;
        let extension = View::new(data)
            .u32(
                payload_at.checked_add(8).ok_or(bad("offset overflows".into()))?,
                LE,
            )
            .field("resource extension")?;
        let body_at = payload_at.checked_add(12).ok_or(bad("offset overflows".into()))?;
        let body = data
            .get(body_at..)
            .ok_or(bad("payload shorter than its class id and extension".into()))?;
        Ok(Resource {
            type_id,
            name,
            file_header,
            header_rows,
            class_id,
            extension,
            body: body.to_vec(),
        })
    }
}
