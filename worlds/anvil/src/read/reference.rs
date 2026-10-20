//! A 10-byte reference to another resource, as LODSelector levels and the start of Material and TextureSet bodies
//! store it: `01`, a byte, and the u64 ClassID it refers to; or `03` and nine zero bytes, referring to nothing. Another
//! first byte, or a `03` followed by anything but zeros, is refused.
//!
//! Established on two LODSelectors (both forms) and three Materials and a TextureSet (the `01` form) as fixtures, and
//! by the census of every LODSelector, Material and TextureSet in Riders Republic (worlds/anvil/tests/census.rs). The
//! byte after `01` is kept as stored (1 on every entry read; what it selects is not established).
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// A reference's bytes.
pub const LENGTH: usize = 10;

/// Why a reference can't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("reference tag {0}: only 1 and 3 are read")]
    Tag(u8),
    #[error("reference tagged 3 (none) with nonzero bytes after it")]
    NonZero,
}

/// A reference to a resource (tag `01`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    /// The byte after the tag, as stored.
    pub mode: u8,
    /// The ClassID referred to.
    pub id: u64,
}

/// The reference these bytes hold: `Some` for `01`, `None` for `03`.
pub fn parse(bytes: [u8; LENGTH]) -> Result<Option<Reference>, Error> {
    let [tag, mode, i0, i1, i2, i3, i4, i5, i6, i7] = bytes;
    match tag {
        1 => Ok(Some(Reference {
            mode,
            id: u64::from_le_bytes([i0, i1, i2, i3, i4, i5, i6, i7]),
        })),
        3 if [mode, i0, i1, i2, i3, i4, i5, i6, i7] == [0; 9] => Ok(None),
        3 => Err(Error::NonZero),
        tag => Err(Error::Tag(tag)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_forms_read_and_others_are_refused() {
        let mut b = [1, 1, 0x21, 0xC2, 0x0D, 0x97, 0xD3, 0x01, 0, 0];
        assert_eq!(
            parse(b),
            Ok(Some(Reference {
                mode: 1,
                id: 0x01D3_970D_C221
            }))
        );
        assert_eq!(parse([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]), Ok(None));
        assert_eq!(parse([3, 0, 0, 0, 0, 0, 0, 0, 0, 1]), Err(Error::NonZero));
        b[0] = 2;
        assert_eq!(parse(b), Err(Error::Tag(2)));
    }
}
