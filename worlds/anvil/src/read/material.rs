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
}
