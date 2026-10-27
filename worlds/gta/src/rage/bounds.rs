//! Collision bounds (`phBound`), as ybn files and drawables store them: every bound's header raw, composites with
//! their children's transforms and flags, geometry with its vertices (quantised, read as positions in the bound's
//! space), polygons (triangles, spheres, capsules, boxes, cylinders) and per-polygon materials. The layout is the one
//! community tools document; the census checks it against the install.
//!
//! Layout (offsets from the bound): resource file header (16), kind 0x10, sphere radius 0x14, box max 0x20, margin
//! 0x2C, box min 0x30, box centre 0x40, material index 0x4C, sphere centre 0x50, volume 0x6C. Geometry (kinds 4 and
//! 8): shrunk vertices 0x78 (count 0x84), polygons 0x88, quantum 0x90, centre 0xA0, vertices 0xB0, vertex colours
//! 0xB8, vertex count 0xD0, polygon count 0xD4, materials 0xF0, polygon material indices 0x118, material count 0x120;
//! a BVH's tree at 0x130 (not read). Composite (kind 10): children 0x70, their transforms 0x78 (4 columns of a vec3 and
//! a word, 64 bytes), bounding boxes 0x88, flags 0x90 and 0x98 (8 bytes each), child count 0xA0.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use crate::rage::le;
use crate::rage::le::LE;
use crate::rage::resource::Resource;
use crate::{Result, invalid};
use primase_foundations::bytes::View;

fn err(at: u64, detail: impl ToString) -> crate::Error {
    invalid("phBound", at, detail)
}

/// The fields every bound has, as stored.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct BoundHeader {
    /// 0 sphere, 1 capsule, 3 box, 4 geometry, 8 geometry with a BVH, 10 composite, 12 disc, 13 cylinder, 15 cloth.
    pub kind: u8,
    pub sphere_radius: f32,
    pub sphere_centre: [f32; 3],
    pub box_min: [f32; 3],
    pub box_max: [f32; 3],
    pub box_centre: [f32; 3],
    pub margin: f32,
    /// The material of a primitive bound (sphere, capsule, box, disc, cylinder): its first byte.
    pub material: [u8; 4],
    pub volume: f32,
}

/// One polygon of a geometry bound; vertex indices into [`Geometry::vertices`].
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Polygon {
    /// Vertex indices (flags in the stored high bits taken off), neighbouring polygons per edge (`-1`: none, as
    /// stored), area.
    Triangle {
        vertices: [u16; 3],
        flags: [u8; 3],
        edges: [i16; 3],
        area: f32,
    },
    Sphere {
        centre: u16,
        radius: f32,
    },
    Capsule {
        ends: [u16; 2],
        radius: f32,
    },
    /// Four vertices: the corners of a box (three diagonally opposite pairs' ends).
    Box {
        corners: [i16; 4],
    },
    Cylinder {
        ends: [u16; 2],
        radius: f32,
    },
}

/// A geometry bound's contents.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Geometry {
    /// Positions in the bound's space: stored as 16-bit integers times `quantum` plus `centre`.
    pub vertices: Vec<[f32; 3]>,
    pub quantum: [f32; 3],
    pub centre: [f32; 3],
    pub polygons: Vec<Polygon>,
    /// Material records (two words each: the material type in the low byte of the first, procedural id, room, ped
    /// density, flags, colour index as stored).
    pub materials: Vec<[u32; 2]>,
    /// Per polygon, the index of its material in `materials`.
    pub polygon_materials: Vec<u8>,
    /// Per vertex, a colour index (a byte quadruple as stored) when the bound has vertex colours.
    pub vertex_colours: Option<Vec<[u8; 4]>>,
}

/// One child of a composite: its bound and its transform (columns x, y, z, translation, in the composite's space)
/// with the flag words stored beside it.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Child {
    pub bound: Box<Bound>,
    pub transform: [[f32; 3]; 4],
    pub flags: [u32; 2],
    pub flags_2: [u32; 2],
}

/// A bound: its header and its contents.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Bound {
    pub header: BoundHeader,
    pub contents: Contents,
}

/// What a bound holds beyond its header.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub enum Contents {
    /// A sphere, capsule, box, disc or cylinder: its shape is the header's.
    Primitive,
    Geometry(Geometry),
    Composite(Vec<Child>),
    /// A kind this reader does not read (cloth), kept as its header.
    Other,
}

/// The bound at the root of a ybn.
pub fn read_ybn(r: &Resource) -> Result<Bound> {
    read(r, 0x5000_0000, 0)
}

/// The bound at `p`.
pub fn read(r: &Resource, p: u64, depth: usize) -> Result<Bound> {
    if depth > 8 {
        return Err(err(p, "composites nest deeper than 8"));
    }
    r.charge(0x70, p)?;
    let h = r.block::<0x70>(p)?;
    let header = BoundHeader {
        kind: h[0x10],
        sphere_radius: View::new(h).f32(0x14, LE)?,
        box_max: le::vec3_of(&View::new(h).array::<12>(0x20)?),
        margin: View::new(h).f32(0x2C, LE)?,
        box_min: le::vec3_of(&View::new(h).array::<12>(0x30)?),
        box_centre: le::vec3_of(&View::new(h).array::<12>(0x40)?),
        material: [h[0x4C], h[0x4D], h[0x4E], h[0x4F]],
        sphere_centre: le::vec3_of(&View::new(h).array::<12>(0x50)?),
        volume: View::new(h).f32(0x6C, LE)?,
    };
    let contents = match header.kind {
        0 | 1 | 3 | 12 | 13 => Contents::Primitive,
        4 | 8 => Contents::Geometry(geometry(r, p)?),
        10 => Contents::Composite(composite(r, p, depth)?),
        15 => Contents::Other,
        k => return Err(err(p, format!("bound kind {k}"))),
    };
    Ok(Bound { header, contents })
}

fn geometry(r: &Resource, p: u64) -> Result<Geometry> {
    let g = r.block::<0x130>(p)?;
    let (n_vertices, n_polygons) = (
        View::new(g).u32(0xD0, LE)? as usize,
        View::new(g).u32(0xD4, LE)? as usize,
    );
    // what it copies: vertices (6 bytes, 4 more of colour), polygons (16, one material byte), materials (8 each)
    r.charge(
        n_vertices
            .saturating_mul(10)
            .saturating_add(n_polygons.saturating_mul(17))
            .saturating_add(usize::from(g[0x120]).saturating_mul(8))
            .saturating_add(0x130),
        p,
    )?;
    let quantum = le::vec3_of(&View::new(g).array::<12>(0x90)?);
    let centre = le::vec3_of(&View::new(g).array::<12>(0xA0)?);
    let vertices = if n_vertices == 0 {
        Vec::new()
    } else {
        let v = r.records(View::new(g).u64(0xB0, LE)?, n_vertices, 6)?;
        let ([qx, qy, qz], [cx, cy, cz]) = (quantum, centre);
        v.as_chunks::<6>()
            .0
            .iter()
            .map(|c| {
                let [x, y, z] = le::u16x3_of(c).map(|x| f32::from(x as i16));
                [x * qx + cx, y * qy + cy, z * qz + cz]
            })
            .collect()
    };
    let vertex_colours = match View::new(g).u64(0xB8, LE)? {
        0 => None,
        vp if n_vertices > 0 => Some(r.records(vp, n_vertices, 4)?.as_chunks::<4>().0.to_vec()),
        _ => None,
    };
    let polygons = if n_polygons == 0 {
        Vec::new()
    } else {
        let d = r.records(View::new(g).u64(0x88, LE)?, n_polygons, 16)?;
        d.as_chunks::<16>()
            .0
            .iter()
            .enumerate()
            .map(|(i, c)| polygon(c, n_vertices).map_err(|e| err(p, format!("polygon {i}: {e}"))))
            .collect::<Result<_>>()?
    };
    let n_materials = usize::from(g[0x120]);
    let materials = match View::new(g).u64(0xF0, LE)? {
        0 => Vec::new(),
        mp => r
            .records(mp, n_materials, 8)?
            .as_chunks::<8>()
            .0
            .iter()
            .map(le::u32x2_of)
            .collect(),
    };
    let polygon_materials = match View::new(g).u64(0x118, LE)? {
        0 => vec![0; n_polygons],
        ip if n_polygons > 0 => r.at(ip, n_polygons)?.to_vec(),
        _ => Vec::new(),
    };
    if let Some(&m) = polygon_materials
        .iter()
        .find(|&&m| usize::from(m) >= materials.len().max(1))
    {
        return Err(err(p, format!("polygon material {m} of {}", materials.len())));
    }
    Ok(Geometry {
        vertices,
        quantum,
        centre,
        polygons,
        materials,
        polygon_materials,
        vertex_colours,
    })
}

/// One 16-byte polygon; its kind is the low 3 bits of its first byte. A triangle's vertex words carry a flag in their
/// top bit (taken off); a sphere's, capsule's and cylinder's vertex indices are whole 16-bit words (as CodeWalker
/// reads them).
fn polygon(c: &[u8; 16], n_vertices: usize) -> std::result::Result<Polygon, String> {
    // the record as eight little-endian words: word k at byte 2k
    let w = le::u16x8_of(c);
    let radius = f32::from_bits(u32::from(w[2]) | u32::from(w[3]) << 16);
    let vertex = |i: u16| -> std::result::Result<u16, String> {
        if usize::from(i) >= n_vertices {
            return Err(format!("vertex {i} of {n_vertices}"));
        }
        Ok(i)
    };
    let flagged = |x: u16| vertex(x & 0x7FFF);
    Ok(match w[0] & 7 {
        0 => {
            let raw = [w[2], w[3], w[4]];
            Polygon::Triangle {
                vertices: [flagged(w[2])?, flagged(w[3])?, flagged(w[4])?],
                flags: raw.map(|x| (x >> 15) as u8),
                edges: [w[5], w[6], w[7]].map(|x| x as i16),
                area: f32::from_bits((u32::from(w[0]) | u32::from(w[1]) << 16) & !7),
            }
        }
        1 => Polygon::Sphere {
            centre: vertex(w[1])?,
            radius,
        },
        2 => Polygon::Capsule {
            ends: [vertex(w[1])?, vertex(w[4])?],
            radius,
        },
        3 => {
            let corners = [w[2], w[3], w[4], w[5]].map(|x| x as i16);
            if corners.iter().any(|&i| i < 0 || i as usize >= n_vertices) {
                return Err(format!("box corners {corners:?} of {n_vertices}"));
            }
            Polygon::Box { corners }
        }
        4 => Polygon::Cylinder {
            ends: [vertex(w[1])?, vertex(w[4])?],
            radius,
        },
        k => return Err(format!("polygon kind {k}")),
    })
}

fn composite(r: &Resource, p: u64, depth: usize) -> Result<Vec<Child>> {
    let h = r.block::<0xB0>(p)?;
    let n = usize::from(View::new(h).u16(0xA0, LE)?);
    if n == 0 {
        return Ok(Vec::new());
    }
    // each child: its pointer, transform and two flag words
    r.charge(n.saturating_mul(8 + 64 + 8 + 8), p)?;
    let children = r.records(View::new(h).u64(0x70, LE)?, n, 8)?.as_chunks::<8>().0;
    let transforms = r.records(View::new(h).u64(0x78, LE)?, n, 64)?.as_chunks::<64>().0;
    // per child its flag words, as stored, or none
    let words = |pointer: u64| -> Result<Vec<[u32; 2]>> {
        Ok(match pointer {
            0 => vec![[0, 0]; n],
            f => r
                .records(f, n, 8)?
                .as_chunks::<8>()
                .0
                .iter()
                .map(le::u32x2_of)
                .collect(),
        })
    };
    let flags = words(View::new(h).u64(0x90, LE)?)?;
    let flags_2 = words(View::new(h).u64(0x98, LE)?)?;
    let mut out = Vec::with_capacity(n);
    for (((child, t), &flags), &flags_2) in children.iter().zip(transforms).zip(&flags).zip(&flags_2) {
        let cp = u64::from_le_bytes(*child);
        if cp == 0 {
            continue;
        }
        let ([c0, c1, c2, c3], _) = t.as_chunks::<16>() else {
            return Err(err(p, "a child transform of other than four columns"));
        };
        out.push(Child {
            bound: Box::new(read(r, cp, depth.saturating_add(1))?),
            transform: [c0, c1, c2, c3].map(|c| {
                let [x, y, z, _] = le::vec4_of(c);
                [x, y, z]
            }),
            flags,
            flags_2,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod polygon_tests {
    use super::*;

    /// A 16-byte polygon record: kind in the first byte, then 16-bit words at the given offsets, a radius at 4.
    fn record(kind: u8, words: &[(usize, u16)], radius: f32) -> [u8; 16] {
        let mut c = [0u8; 16];
        c[4..8].copy_from_slice(&radius.to_le_bytes());
        for &(o, w) in words {
            c[o..o + 2].copy_from_slice(&w.to_le_bytes());
        }
        c[0] = (c[0] & !7) | kind;
        c
    }

    /// Worked by hand: a sphere whose centre word is 0x8000 is vertex 32,768 of a bound with 32,769 vertices and no
    /// vertex of a bound with one (not vertex 0); a capsule's and cylinder's ends likewise; a triangle's vertex word
    /// 0x8001 is vertex 1 with its flag set.
    #[test]
    fn only_triangle_vertex_words_carry_a_flag() {
        let sphere = record(1, &[(2, 0x8000)], 0.5);
        assert_eq!(
            polygon(&sphere, 32_769),
            Ok(Polygon::Sphere {
                centre: 0x8000,
                radius: 0.5
            })
        );
        assert!(polygon(&sphere, 1).is_err());
        for kind in [2, 4] {
            let tube = record(kind, &[(2, 0x8000), (8, 3)], 0.25);
            assert!(polygon(&tube, 4).is_err(), "kind {kind}");
            let ends = match polygon(&tube, 32_769).unwrap() {
                Polygon::Capsule { ends, .. } | Polygon::Cylinder { ends, .. } => ends,
                p => panic!("kind {kind}: {p:?}"),
            };
            assert_eq!(ends, [0x8000, 3]);
        }
        let triangle = record(0, &[(4, 0x8001), (6, 2), (8, 3)], 0.0);
        match polygon(&triangle, 4).unwrap() {
            Polygon::Triangle { vertices, flags, .. } => {
                assert_eq!(vertices, [1, 2, 3]);
                assert_eq!(flags, [1, 0, 0]);
            }
            p => panic!("{p:?}"),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "fixtures, not untrusted bytes"
)]
mod budget_tests {
    use super::*;
    use crate::rage::resource::PageFlags;

    /// A resource of 64 KiB holding nine bounds at 0x100 intervals: bound `k` is a composite of `fan` children that all
    /// name bound `k + 1`, down to a box (kind 3) at the ninth.
    fn shared_tree(fan: u16) -> Resource {
        let mut data = vec![0u8; 0x10000];
        for k in 0..9usize {
            let h = k * 0x100;
            data[h + 0x10] = if k < 8 { 10 } else { 3 };
            if k < 8 {
                data[h + 0xA0..h + 0xA2].copy_from_slice(&fan.to_le_bytes());
                let children = 0x1000 + k * 0x400;
                data[h + 0x70..h + 0x78].copy_from_slice(&(0x5000_0000u64 + children as u64).to_le_bytes());
                data[h + 0x78..h + 0x80].copy_from_slice(&(0x5000_0000u64 + 0xA000).to_le_bytes());
                for i in 0..usize::from(fan) {
                    let child = 0x5000_0000u64 + ((k + 1) * 0x100) as u64;
                    data[children + i * 8..children + i * 8 + 8].copy_from_slice(&child.to_le_bytes());
                }
            }
        }
        Resource {
            version: 0,
            system: PageFlags(7 | 1 << 27),
            graphics: PageFlags(0),
            data,
            zero_filled: 0,
            spent: Default::default(),
        }
    }

    /// Children that share their own children multiply: with 6 to a composite and eight levels, 1.7 million bounds
    /// from a resource of 64 KiB. The read is refused once its bytes pass a few times the resource's, long before.
    #[test]
    fn children_shared_past_the_resources_size_are_refused() {
        let r = shared_tree(6);
        let e = read(&r, 0x5000_0000, 0).unwrap_err().to_string();
        assert!(e.contains("more than"), "{e}");
        // a tree that is a tree (one child each) reads
        assert!(read(&shared_tree(1), 0x5000_0000, 0).is_ok());
    }
}
