//! What every anvil body reader shares over [`primase_bytes::Cursor`]: the byte order (little-endian throughout), the
//! grammar's fixed values, counts and embedded references, and one error for a body that doesn't read
//! ([`BodyError`]), each reader's own error naming its record ("grid", "skeleton", ...) in front of it.
//!
//! The numbers themselves are the cursor's (`c.u32(LE).field("what")?`): a failed read names its field and its exact
//! offset ([`primase_bytes::FieldError`]).

use primase_bytes::{Cursor, Endian, Field, FieldError};

use crate::read::reference::{self, Reference};

/// Every anvil format is little-endian.
pub(crate) const LE: Endian = Endian::Little;

/// Why a body doesn't read, in the grammar's terms; a reader's error puts its record's name in front.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// A field runs past the body, or its records' size overflows.
    #[error("body: {0}")]
    Short(#[from] FieldError),
    /// A value the grammar fixes, or bounds, is another.
    #[error("{what}: {found:#x} at {at}, not {read}")]
    Value {
        what: &'static str,
        at: usize,
        found: u64,
        read: &'static str,
    },
    #[error("reference at {at}: {source}")]
    Reference {
        at: usize,
        #[source]
        source: reference::Error,
    },
    /// Bytes left after what the grammar reads.
    #[error("body: {0} bytes after its end")]
    Trailing(usize),
}

/// A reader's error from a failed field read, through its [`BodyError`] variant.
macro_rules! body_error {
    ($error:ty, $variant:path) => {
        impl From<primase_bytes::FieldError> for $error {
            fn from(e: primase_bytes::FieldError) -> Self {
                $variant(crate::read::fields::BodyError::from(e))
            }
        }
    };
}
pub(crate) use body_error;

/// A value the grammar fixes, `width` bytes (1, 2, 4 or 8): refused unless it is `want`.
pub(crate) fn expect(c: &mut Cursor, what: &'static str, want: u64, width: usize) -> Result<(), BodyError> {
    let at = c.position();
    let found = match width {
        1 => c.u8().map(u64::from),
        2 => c.u16(LE).map(u64::from),
        4 => c.u32(LE).map(u64::from),
        _ => c.u64(LE),
    }
    .field(what)?;
    check(what, at, want, found)
}

/// A value read at `at` that the grammar fixes to `want`: refused otherwise.
pub(crate) fn check(what: &'static str, at: usize, want: u64, found: u64) -> Result<(), BodyError> {
    if found == want {
        Ok(())
    } else {
        Err(BodyError::Value {
            what,
            at,
            found,
            read: "the grammar's value",
        })
    }
}

/// An embedded object's header: a u64 id and its u32 type, which must be `kind`.
pub(crate) fn object(c: &mut Cursor, what: &'static str, kind: u32) -> Result<u64, BodyError> {
    let id = c.u64(LE).field(what)?;
    expect(c, what, u64::from(kind), 4)?;
    Ok(id)
}

/// A u32 count of records of `each` bytes (at least one byte each) that the rest of the body holds.
pub(crate) fn count(c: &mut Cursor, what: &'static str, each: usize) -> Result<usize, BodyError> {
    Ok(c.count(LE, each.max(1)).field(what)?)
}

/// `N` f32s in a row (a vector, a matrix's 16 column by column).
pub(crate) fn f32s<const N: usize>(c: &mut Cursor, what: &'static str) -> Result<[f32; N], FieldError> {
    let mut out = [0.0f32; N];
    for x in &mut out {
        *x = c.f32(LE).field(what)?;
    }
    Ok(out)
}

/// A stored reference ([`reference::parse`]).
pub(crate) fn reference(c: &mut Cursor) -> Result<Option<Reference>, BodyError> {
    let at = c.position();
    reference::parse(c.array::<{ reference::LENGTH }>().field("reference")?)
        .map_err(|source| BodyError::Reference { at, source })
}

/// The body read to its last byte.
pub(crate) fn end(c: &Cursor) -> Result<(), BodyError> {
    match c.remaining() {
        0 => Ok(()),
        left => Err(BodyError::Trailing(left)),
    }
}
