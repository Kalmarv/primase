//! A Material's or TextureSet's body: the two references it starts with, and the rest as stored. What a reference is
//! for (its slot's role) is not read; nor is anything after the two.
//!
//! The body (little-endian, after the resource's class id and extension): a byte (1), then two references, each two
//! bytes (1, 1) and a u64 ClassID, then the rest. No count of references is stored before them, and the rest may hold
//! more: two are what is established.
//!
//! Established on three Materials and a TextureSet (Riders Republic build 25394987, base WhiteRoom forge, entries 26,
//! 210, 162 and 163; games/riders-republic docs/recon/material-reference-envelope.md, e244d10). There, a Material's
//! second reference resolves to a TextureSet in all three and its first to a MaterialTemplate in one (the other two
//! resolve to nothing in that forge); the TextureSet's two resolve to TextureMaps. Another first byte or tag, or a body
//! shorter than the two references, is refused, naming it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

pub use crate::read::mesh::MATERIAL;

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
    #[error("reference body: byte {found} at {at}, only 1 is read")]
    Byte { at: usize, found: u8 },
}

/// A Material's or TextureSet's body as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredReferences {
    /// The two ClassIDs, in order.
    pub references: [u64; 2],
    /// Everything after them, uninterpreted.
    pub rest: Vec<u8>,
}

impl StoredReferences {
    /// The body parsed.
    pub fn parse(body: &[u8]) -> Result<StoredReferences, Error> {
        let have = body.len();
        let (head, rest) = body.split_first_chunk::<21>().ok_or(Error::Short { have })?;
        let [flag, a0, a1, first @ .., b0, b1, s0, s1, s2, s3, s4, s5, s6, s7] = head;
        for (at, found) in [(0, *flag), (1, *a0), (2, *a1), (11, *b0), (12, *b1)] {
            if found != 1 {
                return Err(Error::Byte { at, found });
            }
        }
        Ok(StoredReferences {
            references: [
                u64::from_le_bytes(*first),
                u64::from_le_bytes([*s0, *s1, *s2, *s3, *s4, *s5, *s6, *s7]),
            ],
            rest: rest.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(bytes: [u8; 5], ids: [u64; 2], rest: &[u8]) -> Vec<u8> {
        let mut b = vec![bytes[0], bytes[1], bytes[2]];
        b.extend(ids[0].to_le_bytes());
        b.extend([bytes[3], bytes[4]]);
        b.extend(ids[1].to_le_bytes());
        b.extend(rest);
        b
    }

    #[test]
    fn a_body_reads_its_two_references_and_keeps_the_rest() {
        let r = StoredReferences::parse(&body([1; 5], [0x01D3_970D_C221, 7], &[3, 0, 9])).expect("read");
        assert_eq!(
            r,
            StoredReferences {
                references: [0x01D3_970D_C221, 7],
                rest: vec![3, 0, 9]
            }
        );
        assert!(StoredReferences::parse(&body([1; 5], [1, 2], &[])).is_ok());
    }

    #[test]
    fn another_byte_or_a_short_body_is_refused() {
        for (k, at) in [(0, 0), (1, 1), (2, 2), (3, 11), (4, 12)] {
            let mut bytes = [1; 5];
            *bytes.get_mut(k).expect("five") = 0;
            assert!(matches!(
                StoredReferences::parse(&body(bytes, [1, 2], &[])),
                Err(Error::Byte { at: a, found: 0 }) if a == at
            ));
        }
        let b = body([1; 5], [1, 2], &[]);
        assert!(matches!(
            StoredReferences::parse(b.get(..20).expect("20")),
            Err(Error::Short { have: 20 })
        ));
    }
}
