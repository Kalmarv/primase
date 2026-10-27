//! The collision shapes besides MeshShape (see [`crate::read::mesh_shape`]): SphereShape, CapsuleShape, BoxShape and
//! ReferenceListShape. Each type id is the CRC-32 of its name. Little-endian throughout; vec4 is four f32.
//!
//! - **SphereShape** (35 bytes): `01`, a u32 0, a vec4 centre, an f32 radius, a reference to its CollisionMaterial
//!   (or none).
//! - **CapsuleShape** (51 bytes): `01`, a u32 0, a vec4 end A, a vec4 end B, an f32 radius, a CollisionMaterial
//!   reference: the segment A-B swept by the radius.
//! - **BoxShape** (95 bytes): `01`, a byte (0 or 0x21, as stored), three zero bytes, a vec4 of half extents, a
//!   column-major 4x4 (the box's own transform within its owner), a CollisionMaterial reference.
//! - **ReferenceListShape**: `01`, a u32 0, a u32 count n and n references to its child shapes, a u32 n and n
//!   column-major 4x4 matrices (each child's transform within the list), a u32 n and n vec4 (as stored; (1, 1, 1, 1) on
//!   all but one), then a vec4 minimum and maximum (the children's box: stored, not checked against them).
//!
//! The grammars are the Riders Republic notes' (docs/collision.md: every entry of each type to its last byte; the
//! reference list's matrices column-major as ACExplorer's reader takes them, equal on all 3,731), consumed to the last
//! byte by the census (worlds/anvil/tests/census.rs) on every entry of the game. The notes read the vectors as
//! positions, lengths and transforms in their owner's local frame (each shape's box against the stored boxes of the
//! lists and Entities holding it); what the reference list's vec4 array and the box's byte are isn't read.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use crate::read::fields::{BodyError, LE, body_error, count, end, expect, f32s, reference};
use crate::read::reference::{self, Reference};
use primase_foundations::bytes::{Cursor, Field};

/// A record type: the CRC-32 of `SphereShape`.
pub const SPHERE_SHAPE: u32 = 0xFA3F_7A18;
/// A record type: the CRC-32 of `CapsuleShape`.
pub const CAPSULE_SHAPE: u32 = 0xB859_9052;
/// A record type: the CRC-32 of `BoxShape`.
pub const BOX_SHAPE: u32 = 0x4EC6_8E98;
/// A record type: the CRC-32 of `ReferenceListShape`.
pub const REFERENCE_LIST_SHAPE: u32 = 0x2D67_5BA2;

/// Why a shape body can't be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("shape {0}")]
    Body(#[from] BodyError),
}
body_error!(Error, Error::Body);

/// A SphereShape as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredSphere {
    pub centre: [f32; 4],
    pub radius: f32,
    pub material: Option<Reference>,
}

impl StoredSphere {
    pub fn parse(body: &[u8]) -> Result<StoredSphere, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        expect(&mut c, "word", 0, 4)?;
        let s = StoredSphere {
            centre: f32s::<4>(&mut c, "centre")?,
            radius: c.f32(LE).field("radius")?,
            material: reference(&mut c)?,
        };
        end(&c)?;
        Ok(s)
    }
}

/// A CapsuleShape as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredCapsule {
    pub a: [f32; 4],
    pub b: [f32; 4],
    pub radius: f32,
    pub material: Option<Reference>,
}

impl StoredCapsule {
    pub fn parse(body: &[u8]) -> Result<StoredCapsule, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        expect(&mut c, "word", 0, 4)?;
        let s = StoredCapsule {
            a: f32s::<4>(&mut c, "end")?,
            b: f32s::<4>(&mut c, "end")?,
            radius: c.f32(LE).field("radius")?,
            material: reference(&mut c)?,
        };
        end(&c)?;
        Ok(s)
    }
}

/// A BoxShape as stored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StoredBox {
    /// The byte after the first, as stored (0 or 0x21).
    pub byte: u8,
    pub half_extents: [f32; 4],
    /// Column-major.
    pub matrix: [f32; 16],
    pub material: Option<Reference>,
}

impl StoredBox {
    pub fn parse(body: &[u8]) -> Result<StoredBox, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        let [byte] = c.array::<1>().field("second byte")?;
        for _ in 0..3 {
            expect(&mut c, "zero byte", 0, 1)?;
        }
        let s = StoredBox {
            byte,
            half_extents: f32s::<4>(&mut c, "half extents")?,
            matrix: f32s::<16>(&mut c, "matrix")?,
            material: reference(&mut c)?,
        };
        end(&c)?;
        Ok(s)
    }
}

/// A ReferenceListShape as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredReferenceList {
    pub children: Vec<Reference>,
    /// Each child's transform, column-major.
    pub matrices: Vec<[f32; 16]>,
    /// The vec4 array, as stored.
    pub vectors: Vec<[f32; 4]>,
    pub min: [f32; 4],
    pub max: [f32; 4],
}

impl StoredReferenceList {
    pub fn parse(body: &[u8]) -> Result<StoredReferenceList, Error> {
        let mut c = Cursor::new(body);
        expect(&mut c, "first byte", 1, 1)?;
        expect(&mut c, "word", 0, 4)?;
        let n = count(&mut c, "child count", reference::LENGTH)?;
        let mut children = Vec::with_capacity(n);
        for _ in 0..n {
            let at = c.position();
            children.push(reference(&mut c)?.ok_or(BodyError::Value {
                what: "child reference",
                at,
                found: 3,
                read: "a reference (tag 1)",
            })?);
        }
        let held = |c: &mut Cursor<'_>, what, each| -> Result<(), Error> {
            let at = c.position();
            let m = count(c, what, each)?;
            if m == n {
                Ok(())
            } else {
                Err(BodyError::Value {
                    what,
                    at,
                    found: u64::try_from(m).unwrap_or(u64::MAX),
                    read: "the child count",
                }
                .into())
            }
        };
        held(&mut c, "matrix count", 64)?;
        let matrices = (0..n)
            .map(|_| f32s::<16>(&mut c, "matrix"))
            .collect::<Result<_, _>>()?;
        held(&mut c, "vector count", 16)?;
        let vectors = (0..n)
            .map(|_| f32s::<4>(&mut c, "vector"))
            .collect::<Result<_, _>>()?;
        let min = f32s::<4>(&mut c, "minimum")?;
        let max = f32s::<4>(&mut c, "maximum")?;
        end(&c)?;
        Ok(StoredReferenceList {
            children,
            matrices,
            vectors,
            min,
            max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn a_sphere_and_a_list_read_and_a_long_one_is_refused() {
        let mut b = vec![1, 0, 0, 0, 0];
        b.extend(f(&[1.0, 2.0, 3.0, 0.0, 0.5]));
        b.extend([1, 1, 0x1D, 0x31, 0xF7, 0x74, 0, 0, 0, 0]);
        let s = StoredSphere::parse(&b).expect("sphere");
        assert_eq!(
            (s.centre, s.radius, s.material.map(|m| m.id)),
            ([1.0, 2.0, 3.0, 0.0], 0.5, Some(0x74F7_311D))
        );
        b.push(0);
        assert!(matches!(
            StoredSphere::parse(&b),
            Err(Error::Body(BodyError::Trailing(1)))
        ));
        // one child at the identity
        let mut l = vec![
            1, 0, 0, 0, 0, 1, 0, 0, 0, 1, 1, 7, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
        ];
        let identity = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        l.extend(f(&identity));
        l.extend([1, 0, 0, 0]);
        l.extend(f(&[1.0; 4]));
        l.extend(f(&[-1.0, -1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0]));
        let r = StoredReferenceList::parse(&l).expect("list");
        assert_eq!(
            (r.children.len(), r.matrices.first(), r.max),
            (1, Some(&identity), [1.0; 4])
        );
        assert_eq!(l.len(), 49 + 90);
    }
}
