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
use crate::le;
use crate::resource::Resource;
use crate::{Result, invalid};

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
    let h = r.at(p, 0x70)?;
    let header = BoundHeader {
        kind: h[0x10],
        sphere_radius: le::f32(h, 0x14),
        box_max: le::vec3(h, 0x20),
        margin: le::f32(h, 0x2C),
        box_min: le::vec3(h, 0x30),
        box_centre: le::vec3(h, 0x40),
        material: [h[0x4C], h[0x4D], h[0x4E], h[0x4F]],
        sphere_centre: le::vec3(h, 0x50),
        volume: le::f32(h, 0x6C),
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
    let g = r.at(p, 0x130)?;
    let (n_vertices, n_polygons) = (le::u32(g, 0xD0) as usize, le::u32(g, 0xD4) as usize);
    let quantum = le::vec3(g, 0x90);
    let centre = le::vec3(g, 0xA0);
    let vertices = if n_vertices == 0 {
        Vec::new()
    } else {
        let v = r.at(le::u64(g, 0xB0), 6 * n_vertices)?;
        v.chunks(6)
            .map(|c| {
                let q = [0, 2, 4].map(|o| f32::from(le::u16(c, o) as i16));
                std::array::from_fn(|i| q[i] * quantum[i] + centre[i])
            })
            .collect()
    };
    let vertex_colours = match le::u64(g, 0xB8) {
        0 => None,
        vp if n_vertices > 0 => Some(
            r.at(vp, 4 * n_vertices)?
                .chunks(4)
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect(),
        ),
        _ => None,
    };
    let polygons = if n_polygons == 0 {
        Vec::new()
    } else {
        let d = r.at(le::u64(g, 0x88), 16 * n_polygons)?;
        d.chunks(16)
            .enumerate()
            .map(|(i, c)| polygon(c, n_vertices).map_err(|e| err(p, format!("polygon {i}: {e}"))))
            .collect::<Result<_>>()?
    };
    let n_materials = usize::from(g[0x120]);
    let materials = match le::u64(g, 0xF0) {
        0 => Vec::new(),
        mp => r
            .at(mp, 8 * n_materials)?
            .chunks(8)
            .map(|c| [le::u32(c, 0), le::u32(c, 4)])
            .collect(),
    };
    let polygon_materials = match le::u64(g, 0x118) {
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

/// One 16-byte polygon; its kind is the low 3 bits of its first byte.
fn polygon(c: &[u8], n_vertices: usize) -> std::result::Result<Polygon, String> {
    let v = |o: usize| -> std::result::Result<u16, String> {
        let i = le::u16(c, o) & 0x7FFF;
        if usize::from(i) >= n_vertices {
            return Err(format!("vertex {i} of {n_vertices}"));
        }
        Ok(i)
    };
    Ok(match c[0] & 7 {
        0 => {
            let raw = [4, 6, 8].map(|o| le::u16(c, o));
            Polygon::Triangle {
                vertices: [v(4)?, v(6)?, v(8)?],
                flags: raw.map(|x| (x >> 15) as u8),
                edges: [10, 12, 14].map(|o| le::u16(c, o) as i16),
                area: f32::from_bits(le::u32(c, 0) & !7),
            }
        }
        1 => Polygon::Sphere {
            centre: v(2)?,
            radius: le::f32(c, 4),
        },
        2 => Polygon::Capsule {
            ends: [v(2)?, v(8)?],
            radius: le::f32(c, 4),
        },
        3 => {
            let corners = [4, 6, 8, 10].map(|o| le::u16(c, o) as i16);
            if corners.iter().any(|&i| i < 0 || i as usize >= n_vertices) {
                return Err(format!("box corners {corners:?} of {n_vertices}"));
            }
            Polygon::Box { corners }
        }
        4 => Polygon::Cylinder {
            ends: [v(2)?, v(8)?],
            radius: le::f32(c, 4),
        },
        k => return Err(format!("polygon kind {k}")),
    })
}

fn composite(r: &Resource, p: u64, depth: usize) -> Result<Vec<Child>> {
    let h = r.at(p, 0xB0)?;
    let n = usize::from(le::u16(h, 0xA0));
    if n == 0 {
        return Ok(Vec::new());
    }
    let children = r.at(le::u64(h, 0x70), 8 * n)?;
    let transforms = r.at(le::u64(h, 0x78), 64 * n)?;
    let flags = match le::u64(h, 0x90) {
        0 => None,
        f => Some(r.at(f, 8 * n)?),
    };
    let flags_2 = match le::u64(h, 0x98) {
        0 => None,
        f => Some(r.at(f, 8 * n)?),
    };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let cp = le::u64(children, 8 * i);
        if cp == 0 {
            continue;
        }
        let t = &transforms[64 * i..64 * i + 64];
        let word = |f: Option<&[u8]>| f.map_or([0, 0], |f| [le::u32(f, 8 * i), le::u32(f, 8 * i + 4)]);
        out.push(Child {
            bound: Box::new(read(r, cp, depth + 1)?),
            transform: [0, 16, 32, 48].map(|o| le::vec3(t, o)),
            flags: word(flags),
            flags_2: word(flags_2),
        });
    }
    Ok(out)
}
