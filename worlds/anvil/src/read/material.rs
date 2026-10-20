//! A Material's or TextureSet's body: the two references it starts with, and the rest as stored. What a reference is
//! for (its slot's role) is not read; nor is anything after the two.
//!
//! The body (little-endian, after the resource's class id and extension): a byte (1), then two 10-byte references
//! ([`crate::read::reference`]: `01`, a byte and a u64 ClassID, or `03` and nine zeros for none), then the rest. No
//! count of references is stored before them, and the rest may hold more: two are what is established.
//!
//! Established on three Materials and a TextureSet (Riders Republic build 25394987, base WhiteRoom forge, entries 26,
//! 210, 162 and 163; games/riders-republic docs/recon/material-reference-envelope.md, e244d10). There, a Material's
//! second reference resolves to a TextureSet in all three and its first to a MaterialTemplate (210's in the same forge;
//! 26's and 162's in DataPC.forge, games/riders-republic docs/recon/material-reference-targets.md, aa2337c); the
//! TextureSet's two resolve to TextureMaps. Another first byte, a reference
//! [`crate::read::reference`] refuses, or a body shorter than the two references, is refused, naming it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

pub use crate::read::mesh::MATERIAL;
use crate::read::reference::{self, Reference};

/// A record type: equals the CRC-32 of the ASCII string `TextureSet` (the name a label).
pub const TEXTURE_SET: u32 = 0xD70E_6670;
/// A record type: equals the CRC-32 of the ASCII string `TextureMap` (the name a label).
pub const TEXTURE_MAP: u32 = 0xA2B7_E917;
/// A record type: equals the CRC-32 of the ASCII string `MaterialTemplate` (the name a label).
pub const MATERIAL_TEMPLATE: u32 = 0xBCFB_3C7A;

/// Why a Material or TextureSet body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reference body: {have} bytes, shorter than its two references (21)")]
    Short { have: usize },
    #[error("reference body: byte {found} at 0, only 1 is read")]
    First { found: u8 },
    #[error("reference body: reference {index}: {source}")]
    Reference {
        index: usize,
        #[source]
        source: crate::read::reference::Error,
    },
}

/// A Material's or TextureSet's body as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredReferences {
    /// The two references, in order (`None`: the stored form referring to nothing).
    pub references: [Option<Reference>; 2],
    /// Everything after them, uninterpreted.
    pub rest: Vec<u8>,
}

impl StoredReferences {
    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredReferences, Error> {
        let have = body.len();
        let (head, rest) = body.split_first_chunk::<21>().ok_or(Error::Short { have })?;
        let (first, refs) = head.split_first().ok_or(Error::Short { have })?;
        if *first != 1 {
            return Err(Error::First { found: *first });
        }
        let (a, b) = refs.split_at(reference::LENGTH);
        let read = |index: usize, bytes: &[u8]| {
            let bytes: [u8; reference::LENGTH] = bytes.try_into().map_err(|_| Error::Short { have })?;
            reference::parse(bytes).map_err(|source| Error::Reference { index, source })
        };
        Ok(StoredReferences {
            references: [read(0, a)?, read(1, b)?],
            rest: rest.to_vec(),
        })
    }
}

/// A TextureSet's body as stored: a byte (1), 19 references ([`crate::read::reference`]), a zero byte, 8 bytes and a
/// u32, 204 bytes in all. Established by the census: every TextureSet of Riders Republic (10,027) is exactly that,
/// ending the body (RRAssetStudio reads the same). Which reference is which texture's role is not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredTextureSet {
    /// The 19 references, in stored order (`None`: the stored form referring to nothing).
    pub references: [Option<Reference>; 19],
    /// The 8 bytes after the zero byte.
    pub rest: [u8; 8],
    /// The u32 ending the body (0, 19, 20, 21 and 27 seen; what it counts is not established).
    pub last: u32,
}

/// Why a TextureSet body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum TextureSetError {
    #[error("TextureSet body: {0} bytes, not 204")]
    Length(usize),
    #[error("TextureSet body: byte {found} at {at}, only {read} is read")]
    Byte { at: usize, found: u8, read: u8 },
    #[error("TextureSet reference {index}: {source}")]
    Reference {
        index: usize,
        #[source]
        source: crate::read::reference::Error,
    },
}

impl StoredTextureSet {
    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredTextureSet, TextureSetError> {
        let body: &[u8; 204] = body.try_into().map_err(|_| TextureSetError::Length(body.len()))?;
        let (first, rest) = body
            .split_first_chunk::<1>()
            .ok_or(TextureSetError::Length(204))?;
        if first[0] != 1 {
            return Err(TextureSetError::Byte {
                at: 0,
                found: first[0],
                read: 1,
            });
        }
        let (refs, tail) = rest.split_at(19 * reference::LENGTH);
        let mut references = [None; 19];
        for (index, (slot, bytes)) in references
            .iter_mut()
            .zip(refs.as_chunks::<{ reference::LENGTH }>().0)
            .enumerate()
        {
            *slot =
                reference::parse(*bytes).map_err(|source| TextureSetError::Reference { index, source })?;
        }
        let [zero, r0, r1, r2, r3, r4, r5, r6, r7, l0, l1, l2, l3] = tail else {
            return Err(TextureSetError::Length(204));
        };
        if *zero != 0 {
            return Err(TextureSetError::Byte {
                at: 191,
                found: *zero,
                read: 0,
            });
        }
        Ok(StoredTextureSet {
            references,
            rest: [*r0, *r1, *r2, *r3, *r4, *r5, *r6, *r7],
            last: u32::from_le_bytes([*l0, *l1, *l2, *l3]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(first: u8, refs: [[u8; 10]; 2], rest: &[u8]) -> Vec<u8> {
        let mut b = vec![first];
        b.extend(refs[0]);
        b.extend(refs[1]);
        b.extend(rest);
        b
    }

    const TO_7: [u8; 10] = [1, 1, 7, 0, 0, 0, 0, 0, 0, 0];
    const NONE: [u8; 10] = [3, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    #[test]
    fn a_body_reads_its_two_references_and_keeps_the_rest() {
        let r = StoredReferences::parse(&body(1, [TO_7, NONE], &[3, 0, 9])).expect("read");
        assert_eq!(r.references, [Some(Reference { mode: 1, id: 7 }), None]);
        assert_eq!(r.rest, [3, 0, 9]);
    }

    #[test]
    fn another_first_byte_a_bad_reference_or_a_short_body_is_refused() {
        assert!(matches!(
            StoredReferences::parse(&body(0, [TO_7, TO_7], &[])),
            Err(Error::First { found: 0 })
        ));
        let mut bad = NONE;
        bad[9] = 1;
        assert!(matches!(
            StoredReferences::parse(&body(1, [TO_7, bad], &[])),
            Err(Error::Reference { index: 1, .. })
        ));
        let b = body(1, [TO_7, TO_7], &[]);
        assert!(matches!(
            StoredReferences::parse(b.get(..20).expect("20")),
            Err(Error::Short { have: 20 })
        ));
    }

    #[test]
    fn a_texture_set_reads_its_19_references() {
        let mut b = vec![1];
        for k in 0..19u8 {
            b.extend(if k % 2 == 0 {
                [1, 1, k, 0, 0, 0, 0, 0, 0, 0]
            } else {
                NONE
            });
        }
        b.push(0);
        b.extend([0; 8]);
        b.extend(19u32.to_le_bytes());
        let t = StoredTextureSet::parse(&b).expect("read");
        assert_eq!(t.references[2], Some(Reference { mode: 1, id: 2 }));
        assert_eq!(t.references[1], None);
        assert_eq!(t.last, 19);
        let mut short = b.clone();
        short.pop();
        assert!(matches!(
            StoredTextureSet::parse(&short),
            Err(TextureSetError::Length(203))
        ));
        *b.get_mut(191).expect("204 bytes") = 1;
        assert!(matches!(
            StoredTextureSet::parse(&b),
            Err(TextureSetError::Byte { at: 191, .. })
        ));
    }
}
