//! A MeshShape resource's body as a run of sections, each kept as stored; nothing in it is interpreted (no geometry).
//!
//! The body (little-endian; offsets from the body's start, after the resource's class id and extension):
//! - a prefix: a u32 (observed 1), a u8 (observed 0), the section run's length (u32);
//! - the section run, from byte 9 for that length: consecutive sections, each a type (u16), the marker 0xECD7 (u16),
//!   its size (i32, its 8 header bytes included), then its bytes; the sections close the run exactly;
//! - a tail: whatever follows the run, kept as stored (not sections).
//!
//! Established on two MeshShape resources of different sizes (Riders Republic build 25394987: ten sections each, in
//! the same type order). A prefix with other values, a section without the marker, a size under its header or past
//! the run, or a run past the body is refused, naming it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// MeshShape's type id (CRC-32 of its type name).
pub const MESH_SHAPE: u32 = 0xB22B_3E61;
/// The marker in every section header.
pub const SECTION_MARKER: u16 = 0xECD7;
/// The prefix's first u32, the one value read.
pub const VERSION: u32 = 1;
/// The prefix's byte, the one value read.
pub const FLAG: u8 = 0;
/// The prefix's length: where the section run starts.
pub const PREFIX: usize = 9;
/// A section header's length (type, marker, size).
pub const SECTION_HEADER: usize = 8;

/// Why a MeshShape body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("MeshShape body: {have} bytes, shorter than its {need}-byte {what}")]
    Short {
        what: &'static str,
        have: usize,
        need: usize,
    },
    #[error("MeshShape prefix: version {0}: only version 1 is read")]
    Version(u32),
    #[error("MeshShape prefix: byte {0}: only 0 is read (another value isn't established)")]
    Flag(u8),
    #[error("MeshShape section run: {length} bytes from {PREFIX} past the body's {body} bytes")]
    Run { length: u32, body: usize },
    #[error("MeshShape section {index} at {at}: marker {marker:#06x}, not 0xECD7")]
    Marker { index: usize, at: usize, marker: u16 },
    #[error("MeshShape section {index} at {at}: size {size}: {why}")]
    Size {
        index: usize,
        at: usize,
        size: i32,
        why: &'static str,
    },
}

/// One section as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Where its header starts in the body.
    pub at: usize,
    pub kind: u16,
    /// Its size as stored (header included).
    pub size: u32,
    /// Its bytes after the header.
    pub bytes: Vec<u8>,
}

/// A MeshShape body: its prefix, its sections in order and its tail, all as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshShape {
    pub version: u32,
    pub flag: u8,
    /// The section run's length as stored.
    pub run_length: u32,
    pub sections: Vec<Section>,
    /// The bytes after the run, as stored.
    pub tail: Vec<u8>,
}

fn field<const N: usize>(body: &[u8], at: usize, what: &'static str) -> Result<[u8; N], Error> {
    let short = || Error::Short {
        what,
        have: body.len(),
        need: at.saturating_add(N),
    };
    body.get(at..at.checked_add(N).ok_or_else(short)?)
        .ok_or_else(short)?
        .try_into()
        .map_err(|_| short())
}

impl MeshShape {
    /// A MeshShape resource's body (its [`crate::read::data::Resource::body`]).
    pub fn parse(body: &[u8]) -> Result<MeshShape, Error> {
        let version = u32::from_le_bytes(field(body, 0, "prefix")?);
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let [flag] = field::<1>(body, 4, "prefix")?;
        if flag != FLAG {
            return Err(Error::Flag(flag));
        }
        let run_length = u32::from_le_bytes(field(body, 5, "prefix")?);
        let past = || Error::Run {
            length: run_length,
            body: body.len(),
        };
        let run_end = usize::try_from(run_length)
            .ok()
            .and_then(|n| PREFIX.checked_add(n))
            .filter(|&end| end <= body.len())
            .ok_or_else(past)?;
        let mut sections = Vec::new();
        let mut at = PREFIX;
        while at < run_end {
            let index = sections.len();
            let header: [u8; SECTION_HEADER] = field(body, at, "section header")?;
            let [k0, k1, m0, m1, s0, s1, s2, s3] = header;
            let kind = u16::from_le_bytes([k0, k1]);
            let marker = u16::from_le_bytes([m0, m1]);
            if marker != SECTION_MARKER {
                return Err(Error::Marker { index, at, marker });
            }
            let size = i32::from_le_bytes([s0, s1, s2, s3]);
            let bad = |why| Error::Size { index, at, size, why };
            let len = usize::try_from(size).map_err(|_| bad("negative"))?;
            if len < SECTION_HEADER {
                return Err(bad("under its 8-byte header"));
            }
            let end = at.checked_add(len).ok_or(bad("past the run"))?;
            if end > run_end {
                return Err(bad("past the run"));
            }
            let bytes = body
                .get(at.checked_add(SECTION_HEADER).ok_or(bad("past the run"))?..end)
                .ok_or(bad("past the run"))?
                .to_vec();
            sections.push(Section {
                at,
                kind,
                size: u32::try_from(size).map_err(|_| bad("negative"))?,
                bytes,
            });
            at = end;
        }
        let tail = body.get(run_end..).ok_or_else(past)?.to_vec();
        Ok(MeshShape {
            version,
            flag,
            run_length,
            sections,
            tail,
        })
    }
}
