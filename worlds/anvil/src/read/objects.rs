//! The objects of a version-26 forge entry (Shaun White Skateboarding's PC layout): its metadata block is a directory
//! of the objects its resource block holds, and the resource block is those objects back to back.
//!
//! Layout (little-endian):
//! - the metadata block: the object count (u16), then per object its id (u32) and its size in the resource block
//!   (u32), and nothing after;
//! - the resource block: the objects in the directory's order, each `size` bytes: its class (u32: the CRC-32 of the
//!   class's name), its body's size (u32), its name's length (u32), the name, a header byte (0, or 1 on Animation
//!   objects, as Riders Republic's resources have it), the body, then the bytes left of the directory's size (the
//!   object's trailer, kept as stored; its meaning is not read here).
//!
//! The reader is strict: the directory must fill its block exactly, every object must lie inside its directory size,
//! the name must be its stated length followed by its header byte, and the objects must fill the resource block
//! exactly.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::ops::Range;

use primase_bytes::{Field, FieldError, View};

use crate::read::fields::LE;

/// Why an entry's objects can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Short(#[from] FieldError),
    #[error("the directory of {count} objects takes {expected} bytes; its block holds {actual}")]
    Directory {
        count: usize,
        expected: usize,
        actual: usize,
    },
    #[error("object {index} (id {id:#x}) at {at}: {why}")]
    Object {
        index: usize,
        id: u32,
        at: usize,
        why: String,
    },
    #[error("the objects end at {end}; the resource block holds {actual} bytes")]
    Trailing { end: usize, actual: usize },
}

/// One object of an entry, as stored: where its parts lie in the resource block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredObject {
    /// The directory's id of it.
    pub id: u32,
    /// CRC-32 of its class's name.
    pub class: u32,
    pub name: String,
    /// The byte after the name (0, or 1 on Animation objects).
    pub header: u8,
    /// The whole object in the resource block (the directory's size of it).
    pub span: Range<usize>,
    /// Its body.
    pub body: Range<usize>,
    /// The bytes after the body, to the directory's size (as stored).
    pub trailer: Range<usize>,
}

/// The objects of an entry whose decoded metadata block is `metadata` and resource block `resource` (the module
/// documentation).
pub fn read_objects(metadata: &[u8], resource: &[u8]) -> Result<Vec<StoredObject>, Error> {
    let v = View::new(metadata);
    let count = usize::from(v.u16(0, LE).field("the object count")?);
    let expected = count
        .checked_mul(8)
        .and_then(|n| n.checked_add(2))
        .unwrap_or(usize::MAX);
    if expected != metadata.len() {
        return Err(Error::Directory {
            count,
            expected,
            actual: metadata.len(),
        });
    }
    let r = View::new(resource);
    let mut objects = Vec::with_capacity(count);
    let mut at = 0usize;
    for index in 0..count {
        let row = index.saturating_mul(8).saturating_add(2);
        let id = v.u32(row, LE).field("an object id")?;
        let size = v.u32(row.saturating_add(4), LE).field("an object size")?;
        let fail = |at: usize, why: String| Error::Object { index, id, at, why };
        let size = usize::try_from(size).map_err(|_| fail(at, "size past this machine".into()))?;
        let end = at
            .checked_add(size)
            .filter(|&e| e <= resource.len())
            .ok_or_else(|| {
                fail(
                    at,
                    format!("{size} bytes past the resource block's {}", resource.len()),
                )
            })?;
        let class = r.u32(at, LE).field("an object's class")?;
        let body_len = r.u32(at.saturating_add(4), LE).field("an object's body size")?;
        let name_len = r.u32(at.saturating_add(8), LE).field("an object's name length")?;
        let name_at = at.saturating_add(12);
        let name_len =
            usize::try_from(name_len).map_err(|_| fail(at, "name length past this machine".into()))?;
        let name_end = name_at
            .checked_add(name_len)
            .filter(|&e| e < end)
            .ok_or_else(|| fail(at, format!("a name of {name_len} bytes past the object's {size}")))?;
        let name = r.slice(name_at, name_len).field("an object's name")?;
        let header = r.u8(name_end).field("an object's header byte")?;
        let name =
            String::from_utf8(name.to_vec()).map_err(|_| fail(name_at, "the name is not UTF-8".into()))?;
        let body_at = name_end.saturating_add(1);
        let body_len =
            usize::try_from(body_len).map_err(|_| fail(at, "body size past this machine".into()))?;
        let body_end = body_at
            .checked_add(body_len)
            .filter(|&e| e <= end)
            .ok_or_else(|| {
                fail(
                    body_at,
                    format!("a body of {body_len} bytes past the object's {size}"),
                )
            })?;
        objects.push(StoredObject {
            id,
            class,
            name,
            header,
            span: at..end,
            body: body_at..body_end,
            trailer: body_end..end,
        });
        at = end;
    }
    if at != resource.len() {
        return Err(Error::Trailing {
            end: at,
            actual: resource.len(),
        });
    }
    Ok(objects)
}
