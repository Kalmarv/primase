//! skate.'s PhysicsResource (resource type 0x41759364): the collision a PhysicsAsset or EcsPhysicsAsset names. Read
//! and written faithfully ([`read`], [`write`]), and a triangle soup encoded as the triangle-mesh shapes ReSkate Studio
//! writes ([`weld`], [`mesh_shape`]).
//!
//! Layout (little endian; every offset is self-relative, the field's own position plus its value):
//!
//! ```text
//! 0x00 u32 version (2)   0x04 u32 magic 0x69AF7015   0x08 u32 (Studio 1, the game's own 0)   0x0C u32 parts
//! 0x10 u32 B (24-byte entries after the parts)   0x14 u32 0   0x18 u32 shapes   0x1C u32 D (44-byte entries)   0x20 u32 0
//! 0x24..0x38 five offsets: body (always 0x38), shape table, D table, part table, end of the part table
//! body: 0x4C bytes in Studio's files (copied from its template), none in the game's own
//! shape (84 bytes): f32x3 min, f32x3 max, u16 vertices, u16 corners, u16 faces, u16 tree nodes, u32 0,
//!   12 offsets: [0] vertices f32x3, [1] vertex -> corner offsets (vertices + 1 u16), [2] corner ids by vertex,
//!   [3] corner vertices u16, [4] corner edge value f32, [5] concave bits, [6] flat bits, [7] face offsets
//!   (faces + 1 u16), [8], [9] unused (0), [10] face planes (8 bytes per face, below), [11] the bounding volume tree
//!   (faces - 1 u64 nodes)
//! part (72 bytes): f32x4 rotation, f32x3 position, f32 1.0, u32, u32 shape kind, u32x3 ids, u32, u32 shape index,
//!   u32, u32 packed material, u32 flags (0x800)
//! then the arrays, by kind across all shapes in shape order: vertices, vertex offsets, corner ids, corner vertices;
//! 4-aligned before the first f32 array, then per shape its edge values, concave bits and flat bits; face offsets; and
//! 4-aligned face planes; where some shape has a tree, 8-aligned trees and 8 zero bytes; and where some shape has planes,
//! zero bytes to at least 56 past the last plane.
//! ```
//!
//! Face planes: the game's convex shapes (kind 3, and the boxes of kind 2) may carry them, one per face: its normal
//! octahedrally encoded as two snorm16 values, and its offset as f32. The zeros after them look like padding for a
//! reader that loads past the last plane; zero in every resource seen.
//!
//! Shapes of kind 5 are triangle meshes with edge data and a tree; the game's kind 3 shapes are convex polyhedra
//! (polygon faces, no edge data, no tree, zero bounds). The kind lives in the part, not the shape. Every fact here is
//! read from ReSkate Studio 20261012's writer (`reskate_cli`, unpacked: FUN_140160250, FUN_14015c790 and the functions
//! named below) and checked against Studio's output and the game's own resources (`tests/physics.rs`).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{Rd, Result, bad, capacity};

const W: &str = "physics resource";
pub const RTYPE: u32 = 0x4175_9364;
pub const MAGIC: u32 = 0x69AF_7015;
/// The shape table's offset field and the first byte of the body.
const BODY: usize = 0x38;
const SHAPE: usize = 0x54;
const PART: usize = 0x48;
/// The shape kind of a triangle mesh with edge data and a tree (Studio's only kind).
pub const KIND_MESH: u32 = 5;
/// The shape kind of a convex polyhedron (the game's own small parts).
pub const KIND_CONVEX: u32 = 3;
/// The shape kind of a box with its face planes (the game's own; [`Shape::planes`]).
pub const KIND_BOX: u32 = 2;
/// The most a shape's u16 fields hold: its vertices, and its corners (three per triangle).
pub const MAX_U16: usize = 0xFFFF;

/// A PhysicsResource: its header words, the body block, its shapes and its parts.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicsResource {
    pub version: u32,
    /// The word at 0x08: 1 in Studio's files, 0 in the game's own; meaning unknown.
    pub word08: u32,
    /// The block at 0x38: 0x4C bytes in some (ReSkate Studio writes its template's), empty in others.
    pub body: Vec<u8>,
    pub shapes: Vec<Shape>,
    pub parts: Vec<Part>,
}

/// A part's shape index when it names no shape of the resource.
pub const NO_SHAPE: u32 = u32::MAX;
/// The flags Studio and the game write into every part.
pub const PART_FLAGS: u32 = 0x800;

/// One part: a shape placed with its rotation and position, with its kind, ids and packed physics material.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub rotation: [f32; 4],
    pub position: [f32; 3],
    /// The f32 after the position (1.0 in every file seen).
    pub w: f32,
    pub word20: u32,
    pub kind: u32,
    pub ids: [u32; 3],
    pub word34: u32,
    pub shape: u32,
    pub word3c: u32,
    /// The packed physics material: `(material slot << 6) | flags` (ReSkate's `material_grid.h`).
    pub material: u32,
    pub flags: u32,
}

impl Part {
    /// A triangle-mesh part of `shape` at `position`, unrotated, with `ids` and the usual flags.
    pub fn mesh(shape: u32, position: [f32; 3], ids: [u32; 3], material: u32) -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            position,
            w: 1.0,
            word20: 0,
            kind: KIND_MESH,
            ids,
            word34: 0,
            shape,
            word3c: 0,
            material,
            flags: PART_FLAGS,
        }
    }
}

/// What a corner's edge (from its vertex to the next corner's of its face) says about the face across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// The cosine of half the angle between the two faces' normals on a convex edge, 1.0 otherwise.
    pub value: f32,
    pub concave: bool,
    pub flat: bool,
}

impl Edge {
    pub const OPEN: Edge = Edge {
        value: 1.0,
        concave: false,
        flat: false,
    };
}

/// A shape: its local bounds, vertices and polygon faces, the edge data of every corner and its tree. The vertex ->
/// corner index is derived from the faces (read checks it, write builds it).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub vertices: Vec<[f32; 3]>,
    /// Face `f`'s corners are `corners[faces[f] as usize..faces[f + 1] as usize]`; `faces` starts at 0.
    pub faces: Vec<u16>,
    pub corners: Vec<u16>,
    /// One per corner, or none (a convex shape).
    pub edges: Vec<Edge>,
    /// The tree's nodes in heap order (faces - 1 of them), or none.
    pub tree: Vec<u64>,
    /// One plane per face, or none.
    pub planes: Vec<Plane>,
}

/// A face's plane as stored: its normal octahedrally encoded (two snorm16 values) and its offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    pub normal: [i16; 2],
    pub offset: f32,
}

impl Shape {
    pub fn face_count(&self) -> usize {
        self.faces.len().saturating_sub(1)
    }

    /// The vertex -> corner index: `offsets` (one per vertex and one more) into `ids`, the corners of each vertex in
    /// corner order.
    pub fn corner_index(&self) -> (Vec<u16>, Vec<u16>) {
        let mut offsets = vec![0u16; self.vertices.len() + 1];
        for &v in &self.corners {
            if let Some(o) = offsets.get_mut(v as usize + 1) {
                *o += 1;
            }
        }
        for i in 0..self.vertices.len() {
            offsets[i + 1] += offsets[i];
        }
        let mut next: Vec<u16> = offsets[..self.vertices.len()].to_vec();
        let mut ids = vec![0u16; self.corners.len()];
        for (c, &v) in self.corners.iter().enumerate() {
            if let Some(n) = next.get_mut(v as usize) {
                ids[*n as usize] = c as u16;
                *n += 1;
            }
        }
        (offsets, ids)
    }
}

fn rel(r: &Rd, at: usize) -> Result<usize> {
    let v = r.u32le(at)? as usize;
    Ok(if v == 0 { 0 } else { at + v })
}

fn f32s3(r: &Rd, at: usize) -> Result<[f32; 3]> {
    Ok([
        f32::from_bits(r.u32le(at)?),
        f32::from_bits(r.u32le(at + 4)?),
        f32::from_bits(r.u32le(at + 8)?),
    ])
}

fn u16s(r: &Rd, at: usize, n: usize) -> Result<Vec<u16>> {
    let b = r.bytes(at, 2 * n)?;
    Ok(b.as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect())
}

fn bit(r: &Rd, at: usize, i: usize) -> Result<bool> {
    Ok(r.u32le(at + 4 * (i / 32))? >> (i % 32) & 1 == 1)
}

/// The resource read: its header, body, shapes and parts. Refused: another magic or version, B or D entries, any of a
/// shape's unused offsets set, and a vertex -> corner index that disagrees with the faces (the writer derives it).
pub fn read(d: &[u8]) -> Result<PhysicsResource> {
    let r = Rd::new(d, W);
    let version = r.u32le(0)?;
    if r.u32le(4)? != MAGIC || version & !2 != 0 {
        return Err(bad(W, 0, "not a version 0 or 2 PhysicsResource"));
    }
    let (word08, nparts, nb, nshapes, nd) = (
        r.u32le(8)?,
        r.u32le(0xC)? as usize,
        r.u32le(0x10)?,
        r.u32le(0x18)? as usize,
        r.u32le(0x1C)?,
    );
    if nb != 0 || nd != 0 || r.u32le(0x14)? != 0 || r.u32le(0x20)? != 0 {
        return Err(bad(W, 0x10, format!("{nb} B and {nd} D entries (none read yet)")));
    }
    let p: Vec<usize> = (0..5).map(|k| rel(&r, 0x24 + 4 * k)).collect::<Result<_>>()?;
    if p[0] != BODY
        || p[1] < BODY
        || p[2] != p[1] + SHAPE * nshapes
        || p[3] != p[2]
        || p[4] != p[3] + PART * nparts
        || p[4] > d.len()
    {
        return Err(bad(W, 0x24, "table offsets do not match the counts"));
    }
    let body = r.bytes(BODY, p[1] - BODY)?.to_vec();
    let mut shapes = Vec::with_capacity(capacity(W, nshapes, SHAPE, d.len())?);
    for i in 0..nshapes {
        let s = p[1] + SHAPE * i;
        let nv = r.u16le(s + 0x18)? as usize;
        let nc = r.u16le(s + 0x1A)? as usize;
        let slot = |k: usize| rel(&r, s + 0x24 + 4 * k);
        let sl: Vec<usize> = (0..12).map(slot).collect::<Result<_>>()?;
        if sl[8] != 0 || sl[9] != 0 || r.u32le(s + 0x20)? != 0 {
            return Err(bad(W, s, "a shape slot read nowhere yet is set"));
        }
        let vertices = (0..nv)
            .map(|v| f32s3(&r, sl[0] + 12 * v))
            .collect::<Result<Vec<_>>>()?;
        let corners = u16s(&r, sl[3], nc)?;
        // faces in the low half, tree nodes in the high half
        let packed = r.u32le(s + 0x1C)?;
        let (nf, nodes) = ((packed & 0xFFFF) as usize, (packed >> 16) as usize);
        let faces = u16s(&r, sl[7], nf + 1)?;
        if faces.first() != Some(&0)
            || faces.last().map(|&l| l as usize) != Some(nc)
            || faces.windows(2).any(|w| w[1] < w[0] + 3)
        {
            return Err(bad(
                W,
                sl[7],
                "face offsets do not run from 0 to the corners in steps of at least 3",
            ));
        }
        if corners.iter().any(|&v| v as usize >= nv) {
            return Err(bad(W, sl[3], "a corner names a vertex past the vertices"));
        }
        let edges = if sl[4] != 0 {
            (0..nc)
                .map(|c| {
                    Ok(Edge {
                        value: f32::from_bits(r.u32le(sl[4] + 4 * c)?),
                        concave: bit(&r, sl[5], c)?,
                        flat: bit(&r, sl[6], c)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        if (sl[11] != 0) != (nodes != 0) || (nodes != 0 && nodes + 1 != nf) {
            return Err(bad(W, s + 0x1C, format!("{nodes} tree nodes for {nf} faces")));
        }
        let tree = if sl[11] != 0 {
            (0..nodes)
                .map(|k| r.u64le(sl[11] + 8 * k))
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let planes = if sl[10] != 0 {
            (0..nf)
                .map(|f| {
                    let o = sl[10] + 8 * f;
                    Ok(Plane {
                        normal: [r.u16le(o)? as i16, r.u16le(o + 2)? as i16],
                        offset: f32::from_bits(r.u32le(o + 4)?),
                    })
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let shape = Shape {
            min: f32s3(&r, s)?,
            max: f32s3(&r, s + 12)?,
            vertices,
            faces,
            corners,
            edges,
            tree,
            planes,
        };
        let (offsets, ids) = shape.corner_index();
        if u16s(&r, sl[1], nv + 1)? != offsets || u16s(&r, sl[2], nc)? != ids {
            return Err(bad(
                W,
                sl[1],
                "the vertex -> corner index disagrees with the faces",
            ));
        }
        shapes.push(shape);
    }
    let mut parts = Vec::with_capacity(nparts);
    for i in 0..nparts {
        let o = p[3] + PART * i;
        let f = |k: usize| -> Result<f32> { Ok(f32::from_bits(r.u32le(o + k)?)) };
        parts.push(Part {
            rotation: [f(0)?, f(4)?, f(8)?, f(0xC)?],
            position: [f(0x10)?, f(0x14)?, f(0x18)?],
            w: f(0x1C)?,
            word20: r.u32le(o + 0x20)?,
            kind: r.u32le(o + 0x24)?,
            ids: [r.u32le(o + 0x28)?, r.u32le(o + 0x2C)?, r.u32le(o + 0x30)?],
            word34: r.u32le(o + 0x34)?,
            shape: r.u32le(o + 0x38)?,
            word3c: r.u32le(o + 0x3C)?,
            material: r.u32le(o + 0x40)?,
            flags: r.u32le(o + 0x44)?,
        });
    }
    Ok(PhysicsResource {
        version,
        word08,
        body,
        shapes,
        parts,
    })
}

fn put_u32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The resource written in the layout of the module documentation (the order Studio's writer and the game's own
/// resources use). Refused: a shape past the u16 limits, edge data for some corners only, a tree of the wrong size, a
/// part naming a missing shape.
pub fn write(p: &PhysicsResource) -> Result<Vec<u8>> {
    for (i, s) in p.shapes.iter().enumerate() {
        let nf = s.face_count();
        if s.vertices.len() > MAX_U16 || s.corners.len() > MAX_U16 || nf == 0 {
            return Err(bad(W, i, "a shape holds no face or more than u16 storage"));
        }
        if !(s.edges.is_empty() || s.edges.len() == s.corners.len()) {
            return Err(bad(W, i, "edge data for some corners only"));
        }
        if !(s.tree.is_empty() || s.tree.len() == nf - 1) {
            return Err(bad(W, i, format!("{} tree nodes for {nf} faces", s.tree.len())));
        }
        if !(s.planes.is_empty() || s.planes.len() == nf) {
            return Err(bad(W, i, format!("{} planes for {nf} faces", s.planes.len())));
        }
    }
    if let Some(part) = p
        .parts
        .iter()
        .find(|q| q.shape != NO_SHAPE && q.shape as usize >= p.shapes.len())
    {
        return Err(bad(
            W,
            part.shape as usize,
            "a part names a shape past the shapes",
        ));
    }
    let shape_at = BODY + p.body.len();
    let part_at = shape_at + SHAPE * p.shapes.len();
    let end = part_at + PART * p.parts.len();
    let mut out = vec![0u8; end];
    put_u32(&mut out, 0, p.version);
    put_u32(&mut out, 4, MAGIC);
    put_u32(&mut out, 8, p.word08);
    put_u32(&mut out, 0xC, p.parts.len() as u32);
    put_u32(&mut out, 0x18, p.shapes.len() as u32);
    for (k, target) in [BODY, shape_at, part_at, part_at, end].into_iter().enumerate() {
        let at = 0x24 + 4 * k;
        put_u32(&mut out, at, (target - at) as u32);
    }
    out[BODY..shape_at].copy_from_slice(&p.body);
    for (i, s) in p.shapes.iter().enumerate() {
        let o = shape_at + SHAPE * i;
        for (k, v) in s.min.iter().chain(&s.max).enumerate() {
            put_u32(&mut out, o + 4 * k, v.to_bits());
        }
        out[o + 0x18..o + 0x1A].copy_from_slice(&(s.vertices.len() as u16).to_le_bytes());
        out[o + 0x1A..o + 0x1C].copy_from_slice(&(s.corners.len() as u16).to_le_bytes());
        put_u32(
            &mut out,
            o + 0x1C,
            s.face_count() as u32 | (s.tree.len() as u32) << 16,
        );
    }
    for (i, q) in p.parts.iter().enumerate() {
        let o = part_at + PART * i;
        for (k, v) in q.rotation.iter().chain(&q.position).chain([&q.w]).enumerate() {
            put_u32(&mut out, o + 4 * k, v.to_bits());
        }
        for (at, v) in [
            (0x20, q.word20),
            (0x24, q.kind),
            (0x28, q.ids[0]),
            (0x2C, q.ids[1]),
            (0x30, q.ids[2]),
            (0x34, q.word34),
            (0x38, q.shape),
            (0x3C, q.word3c),
            (0x40, q.material),
            (0x44, q.flags),
        ] {
            put_u32(&mut out, o + at, v);
        }
    }
    let slot = |i: usize, k: usize| shape_at + SHAPE * i + 0x24 + 4 * k;
    let place = |out: &mut Vec<u8>, field: usize, data: &[u8]| {
        let at = out.len() - field;
        put_u32(out, field, at as u32);
        out.extend_from_slice(data);
    };
    let le16 = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let index: Vec<(Vec<u16>, Vec<u16>)> = p.shapes.iter().map(Shape::corner_index).collect();
    for (i, s) in p.shapes.iter().enumerate() {
        let v: Vec<u8> = s
            .vertices
            .iter()
            .flatten()
            .flat_map(|x| x.to_bits().to_le_bytes())
            .collect();
        place(&mut out, slot(i, 0), &v);
    }
    for (i, (offsets, _)) in index.iter().enumerate() {
        place(&mut out, slot(i, 1), &le16(offsets));
    }
    for (i, (_, ids)) in index.iter().enumerate() {
        place(&mut out, slot(i, 2), &le16(ids));
    }
    for (i, s) in p.shapes.iter().enumerate() {
        place(&mut out, slot(i, 3), &le16(&s.corners));
    }
    if p.shapes.iter().any(|s| !s.edges.is_empty()) {
        out.resize(crate::align(out.len(), 4), 0);
    }
    let bits = |flags: Vec<bool>| {
        let mut w = vec![0u32; flags.len().div_ceil(32)];
        for (i, f) in flags.into_iter().enumerate() {
            if f {
                w[i / 32] |= 1 << (i % 32);
            }
        }
        w.into_iter().flat_map(u32::to_le_bytes).collect::<Vec<u8>>()
    };
    for (i, s) in p.shapes.iter().enumerate() {
        if s.edges.is_empty() {
            continue;
        }
        let values: Vec<u8> = s
            .edges
            .iter()
            .flat_map(|e| e.value.to_bits().to_le_bytes())
            .collect();
        place(&mut out, slot(i, 4), &values);
        place(
            &mut out,
            slot(i, 5),
            &bits(s.edges.iter().map(|e| e.concave).collect()),
        );
        place(
            &mut out,
            slot(i, 6),
            &bits(s.edges.iter().map(|e| e.flat).collect()),
        );
    }
    for (i, s) in p.shapes.iter().enumerate() {
        place(&mut out, slot(i, 7), &le16(&s.faces));
    }
    let planes = p.shapes.iter().any(|s| !s.planes.is_empty());
    if planes {
        out.resize(crate::align(out.len(), 4), 0);
    }
    for (i, s) in p.shapes.iter().enumerate() {
        if s.planes.is_empty() {
            continue;
        }
        let bytes: Vec<u8> = s
            .planes
            .iter()
            .flat_map(|q| {
                let [a, b] = q.normal.map(i16::to_le_bytes);
                let o = q.offset.to_bits().to_le_bytes();
                [a[0], a[1], b[0], b[1], o[0], o[1], o[2], o[3]]
            })
            .collect();
        place(&mut out, slot(i, 10), &bytes);
    }
    let planes_end = out.len();
    if p.shapes.iter().any(|s| !s.tree.is_empty()) {
        out.resize(crate::align(out.len(), 8), 0);
        for (i, s) in p.shapes.iter().enumerate() {
            if s.tree.is_empty() {
                continue;
            }
            let nodes: Vec<u8> = s.tree.iter().flat_map(|n| n.to_le_bytes()).collect();
            place(&mut out, slot(i, 11), &nodes);
        }
        out.extend_from_slice(&[0u8; 8]);
    }
    // the resource runs at least 56 bytes past the planes
    if planes && out.len() < planes_end + 56 {
        out.resize(planes_end + 56, 0);
    }
    Ok(out)
}

// ---- encoding a triangle soup as Studio does ----------------------------------------------------------------------

/// A triangle soup welded as Studio welds one collision object (FUN_140161790): vertices of exactly equal position
/// (-0.0 taken as 0.0) are one, numbered by first use and keeping the first use's value; a triangle with two corners on
/// one vertex or no area is dropped; each triangle starts at its smallest vertex (its winding kept); a triangle equal to
/// an earlier one (same corners in the same order) is dropped. `source[t]` is the input triangle each kept one came
/// from.
#[derive(Clone, Debug, PartialEq)]
pub struct Welded {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub source: Vec<usize>,
    /// Input triangles dropped: degenerate (two corners welded together, or no area) and repeated.
    pub degenerate: usize,
    pub repeated: usize,
}

/// `triangles` of `positions` welded (see [`Welded`]). An index past the positions or a position that is not finite is
/// an error.
pub fn weld(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<Welded> {
    let mut by_pos: HashMap<[u32; 3], u32> = HashMap::new();
    let mut w = Welded {
        positions: Vec::new(),
        triangles: Vec::new(),
        source: Vec::new(),
        degenerate: 0,
        repeated: 0,
    };
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    for (t, tri) in triangles.iter().enumerate() {
        let mut ids = [0u32; 3];
        for (k, &i) in tri.iter().enumerate() {
            let p = *positions
                .get(i as usize)
                .ok_or_else(|| bad(W, t, "collision index is outside the vertex buffer"))?;
            if p.iter().any(|c| !c.is_finite()) {
                return Err(bad(W, t, "collision vertex is not finite"));
            }
            // -0.0 and 0.0 are one position
            let key = p.map(|c| if c == 0.0 { 0 } else { c.to_bits() });
            let next = w.positions.len() as u32;
            ids[k] = *by_pos.entry(key).or_insert_with(|| {
                w.positions.push(p);
                next
            });
        }
        if ids[0] == ids[1] || ids[1] == ids[2] || ids[0] == ids[2] || area2(&w.positions, ids) <= 0.0 {
            w.degenerate += 1;
            continue;
        }
        let m = (0..3).min_by_key(|&k| ids[k]).unwrap_or(0);
        let ids = [ids[m], ids[(m + 1) % 3], ids[(m + 2) % 3]];
        if !seen.insert(ids) {
            w.repeated += 1;
            continue;
        }
        w.triangles.push(ids);
        w.source.push(t);
    }
    Ok(w)
}

/// The length of the cross product of a triangle's edges, in f64 (Studio's area test).
fn area2(p: &[[f32; 3]], t: [u32; 3]) -> f64 {
    let a = p[t[0] as usize].map(f64::from);
    let b = p[t[1] as usize].map(f64::from);
    let c = p[t[2] as usize].map(f64::from);
    let (u, v) = (
        [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
        [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
    );
    let x = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()
}

/// One shape of welded triangles (a batch: one material's triangles of a welded object, at most [`MAX_U16`] corners),
/// encoded as Studio's FUN_140158c20 and FUN_14015c790 do: the vertices recentred on the middle of their bounds (that
/// middle is the part's position, returned), triangles with no area after recentring dropped, the vertices the rest
/// use renumbered in first-use order, every corner's edge data ([`edges`]), and the tree ([`tree`]) with the faces in
/// its leaf order. `None` when no triangle is left.
pub fn mesh_shape(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Result<Option<(Shape, [f32; 3])>> {
    if triangles.len() * 3 > MAX_U16 {
        return Err(bad(
            W,
            triangles.len(),
            "physics mesh face batch is outside native uint16 storage",
        ));
    }
    let mut used: Vec<u32> = Vec::new();
    let mut seen = HashSet::new();
    for t in triangles {
        for &v in t {
            if positions.get(v as usize).is_none() {
                return Err(bad(W, v as usize, "collision index is outside the vertex buffer"));
            }
            if seen.insert(v) {
                used.push(v);
            }
        }
    }
    if used.is_empty() {
        return Ok(None);
    }
    // the middle of the used vertices' bounds, in f64 then f32
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for &v in &used {
        for a in 0..3 {
            let c = f64::from(positions[v as usize][a]);
            if c < lo[a] {
                lo[a] = c;
            }
            if hi[a] < c {
                hi[a] = c;
            }
        }
    }
    let centre: [f32; 3] = std::array::from_fn(|a| ((lo[a] + hi[a]) * 0.5) as f32);
    if centre.iter().any(|c| !c.is_finite()) {
        return Err(bad(W, 0, "collision value is outside finite float32 range"));
    }
    let local = |v: u32| -> [f32; 3] { std::array::from_fn(|a| positions[v as usize][a] - centre[a]) };
    let mut remap: BTreeMap<u32, u16> = BTreeMap::new();
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut corners: Vec<u16> = Vec::new();
    for t in triangles {
        let p = t.map(local);
        let lp: Vec<[f32; 3]> = p.to_vec();
        if area2(&lp, [0, 1, 2]) <= 0.0 {
            continue;
        }
        for &v in t {
            let next = vertices.len() as u16;
            let id = *remap.entry(v).or_insert_with(|| {
                vertices.push(local(v));
                next
            });
            corners.push(id);
        }
    }
    if corners.is_empty() {
        return Ok(None);
    }
    if vertices.len() > MAX_U16 {
        return Err(bad(
            W,
            vertices.len(),
            "collision geometry exceeds native uint16 storage",
        ));
    }
    let faces: Vec<u16> = (0..=corners.len() / 3).map(|f| (3 * f) as u16).collect();
    let bounds = bounds_of(vertices.iter().copied());
    let edge = edges(&vertices, &faces, &corners);
    let nf = faces.len() - 1;
    let (tree, order) = if nf > 1 {
        let recs: Vec<FaceRecord> = (0..nf)
            .map(|f| {
                let ps: Vec<[f32; 3]> = corners[faces[f] as usize..faces[f + 1] as usize]
                    .iter()
                    .map(|&v| vertices[v as usize])
                    .collect();
                let n = ps.len() as f64;
                FaceRecord {
                    bounds: bounds_of(ps.iter().copied()),
                    centroid: std::array::from_fn(|a| ps.iter().map(|p| f64::from(p[a])).sum::<f64>() / n),
                    index: f as u32,
                }
            })
            .collect();
        tree(recs, bounds)?
    } else {
        (Vec::new(), vec![0u32])
    };
    let mut out = Shape {
        min: [bounds[0], bounds[1], bounds[2]],
        max: [bounds[3], bounds[4], bounds[5]],
        vertices,
        faces: vec![0],
        corners: Vec::with_capacity(corners.len()),
        edges: Vec::with_capacity(corners.len()),
        tree,
        planes: Vec::new(),
    };
    for f in order {
        let f = f as usize;
        for c in faces[f] as usize..faces[f + 1] as usize {
            out.corners.push(corners[c]);
            out.edges.push(edge[c]);
        }
        out.faces.push(out.corners.len() as u16);
    }
    Ok(Some((out, centre)))
}

/// min x, y, z, max x, y, z of the points; a tie keeps the earlier value (Studio's FUN_140158550 and FUN_140158400
/// compare `<=` towards the kept one, which only matters for -0.0 against 0.0).
fn bounds_of(points: impl Iterator<Item = [f32; 3]>) -> [f32; 6] {
    let mut b: Option<[f32; 6]> = None;
    for p in points {
        b = Some(match b {
            None => [p[0], p[1], p[2], p[0], p[1], p[2]],
            Some(mut b) => {
                for a in 0..3 {
                    if p[a] < b[a] {
                        b[a] = p[a];
                    }
                    if b[3 + a] < p[a] {
                        b[3 + a] = p[a];
                    }
                }
                b
            }
        });
    }
    b.unwrap_or([0.0; 6])
}

/// Every corner's [`Edge`] (FUN_14015dcf0): a corner's edge runs from its vertex to the next corner's. When exactly
/// one corner holds that directed edge and exactly one holds the opposite one, the two faces' normals (Newell's,
/// normalised; neither may be zero) give `d`, their dot product clamped to [-1, 1]: the edge is flat when `d >=
/// cos(0.1 deg)`; else, when the other face's centroid lies on or above this face's plane (through the corner's
/// vertex), it is concave if `d > 0`; else (convex) its value is the cosine of half the angle, `cos(acos(d) / 2)`.
/// Every other edge is [`Edge::OPEN`].
pub fn edges(vertices: &[[f32; 3]], faces: &[u16], corners: &[u16]) -> Vec<Edge> {
    let nf = faces.len().saturating_sub(1);
    let nc = corners.len();
    let mut face_of = vec![0usize; nc];
    let mut next = vec![0usize; nc];
    let mut normals = Vec::with_capacity(nf);
    let mut centroids = Vec::with_capacity(nf);
    for f in 0..nf {
        let (a, b) = (faces[f] as usize, faces[f + 1] as usize);
        let mut s = [0f64; 3];
        for k in a..b {
            face_of[k] = f;
            next[k] = if k + 1 < b { k + 1 } else { a };
            for (x, v) in s.iter_mut().zip(vertices[corners[k] as usize]) {
                *x += f64::from(v);
            }
        }
        let n = (b - a) as f64;
        centroids.push(s.map(|x| x / n));
        normals.push(newell(vertices, &corners[a..b]));
    }
    let mut holders: HashMap<(u16, u16), (usize, usize)> = HashMap::new();
    for c in 0..nc {
        let e = holders.entry((corners[c], corners[next[c]])).or_insert((c, 0));
        e.1 += 1;
    }
    let cos_flat = 0.001_745_329_251_994_329_6f64.cos();
    (0..nc)
        .map(|c| {
            let (a, b) = (corners[c], corners[next[c]]);
            if holders.get(&(a, b)).map(|h| h.1) != Some(1) {
                return Edge::OPEN;
            }
            let Some(&(o, 1)) = holders.get(&(b, a)) else {
                return Edge::OPEN;
            };
            let (n1, n2) = (normals[face_of[c]], normals[face_of[o]]);
            if dot(n1, n1).sqrt() == 0.0 || dot(n2, n2).sqrt() == 0.0 {
                return Edge::OPEN;
            }
            let d = dot(n1, n2).clamp(-1.0, 1.0);
            let flat = cos_flat <= d;
            let va = vertices[a as usize].map(f64::from);
            let c2 = centroids[face_of[o]];
            let above = (c2[1] - va[1]) * n1[1] + (c2[0] - va[0]) * n1[0] + (c2[2] - va[2]) * n1[2] >= 0.0;
            if flat {
                Edge {
                    value: 1.0,
                    concave: false,
                    flat: true,
                }
            } else if above {
                Edge {
                    value: 1.0,
                    concave: d > 0.0,
                    flat: false,
                }
            } else {
                Edge {
                    value: (d.acos() * 0.5).cos() as f32,
                    concave: false,
                    flat: false,
                }
            }
        })
        .collect()
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A polygon's unit normal by Newell's method in f64 (FUN_14015ef10), zero when it has no area.
fn newell(vertices: &[[f32; 3]], poly: &[u16]) -> [f64; 3] {
    let n = poly.len();
    let (mut x, mut y, mut z) = (0f64, 0f64, 0f64);
    for i in 0..n {
        let a = vertices[poly[i] as usize].map(f64::from);
        let b = vertices[poly[(i + 1) % n] as usize].map(f64::from);
        z += (a[0] - b[0]) * (b[1] + a[1]);
        x += (a[1] - b[1]) * (b[2] + a[2]);
        y += (a[2] - b[2]) * (b[0] + a[0]);
    }
    let l = (x * x + y * y + z * z).sqrt();
    if l <= 0.0 { [0.0; 3] } else { [x / l, y / l, z / l] }
}

// ---- the tree (FUN_140158700, FUN_14015c4b0, FUN_14015b6b0, FUN_14015f0f0) ------------------------------------------

#[derive(Clone, Copy, Debug)]
struct FaceRecord {
    bounds: [f32; 6],
    centroid: [f64; 3],
    index: u32,
}

/// A total order of floats by `<` and `==` (as the C++ comparators: -0.0 equals 0.0; no NaN reaches here).
fn order_f64(a: f64, b: f64) -> std::cmp::Ordering {
    if a < b {
        std::cmp::Ordering::Less
    } else if b < a {
        std::cmp::Ordering::Greater
    } else {
        std::cmp::Ordering::Equal
    }
}

fn union(recs: &[FaceRecord]) -> [f32; 6] {
    let mut b = recs[0].bounds;
    for r in &recs[1..] {
        for a in 0..3 {
            if r.bounds[a] < b[a] {
                b[a] = r.bounds[a];
            }
            if b[3 + a] < r.bounds[3 + a] {
                b[3 + a] = r.bounds[3 + a];
            }
        }
    }
    b
}

/// How many leaves the left child of heap node `i` holds in a complete binary tree whose `n` leaves are nodes
/// `n - 1 ..= 2n - 2`.
fn left_leaves(i: usize, n: usize) -> usize {
    let (first, last) = (n - 1, 2 * n - 2);
    let (mut lo, mut width, mut count) = (2 * i + 1, 1usize, 0usize);
    while lo <= last {
        let (a, b) = (lo.max(first), (lo + width - 1).min(last));
        if a <= b {
            count += b - a + 1;
        }
        lo = lo * 2 + 1;
        width *= 2;
    }
    count
}

/// The tree over the faces: node `i`'s children are `2i + 1` and `2i + 2`; a node splits its faces sorted by centroid
/// on the axis their centroids spread most along (ties: by bounds' min x, then index), the left taking as many as its
/// subtree has leaves; when the right half then starts lower in x than the left, the halves swap places (the last
/// faces become the left), and if that still holds the faces are sorted by min x (then index). Each node holds both
/// children's boxes quantised in its own box ([`node`]); a child's box is what its parent's node decodes to. Returns
/// the nodes and the faces in leaf order.
fn tree(recs: Vec<FaceRecord>, root: [f32; 6]) -> Result<(Vec<u64>, Vec<u32>)> {
    let n = recs.len();
    let mut nodes = vec![0u64; n - 1];
    let mut order = vec![0u32; n];
    let mut stack = vec![(0usize, recs, root)];
    while let Some((i, mut faces, parent)) = stack.pop() {
        if i >= n - 1 {
            order[i - (n - 1)] = faces[0].index;
            continue;
        }
        let nl = left_leaves(i, n);
        let spread: [f64; 3] = std::array::from_fn(|a| {
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            for f in &faces {
                lo = lo.min(f.centroid[a]);
                hi = hi.max(f.centroid[a]);
            }
            hi - lo
        });
        let mut axis = usize::from(spread[0] < spread[1]);
        if spread[axis] < spread[2] {
            axis = 2;
        }
        faces.sort_by(|a, b| {
            order_f64(a.centroid[axis], b.centroid[axis])
                .then(order_f64(f64::from(a.bounds[0]), f64::from(b.bounds[0])))
                .then(a.index.cmp(&b.index))
        });
        let (mut lb, mut rb) = (union(&faces[..nl]), union(&faces[nl..]));
        if rb[0] < lb[0] {
            let k = faces.len() - nl;
            faces.rotate_left(k);
            (lb, rb) = (union(&faces[..nl]), union(&faces[nl..]));
            if rb[0] < lb[0] {
                faces.sort_by(|a, b| {
                    order_f64(f64::from(a.bounds[0]), f64::from(b.bounds[0])).then(a.index.cmp(&b.index))
                });
                (lb, rb) = (union(&faces[..nl]), union(&faces[nl..]));
            }
        }
        let w = node(parent, lb, rb)?;
        nodes[i] = w;
        let (dl, dr) = decode_node(parent, w);
        let right = faces.split_off(nl);
        stack.push((2 * i + 2, right, dr));
        stack.push((2 * i + 1, faces, dl));
    }
    Ok((nodes, order))
}

/// `v`'s plane in `bins` steps of `lo..hi`, from `lo` (a min plane, rounded down) or from `hi` (a max plane): the
/// largest step whose plane does not cut into the box it bounds (FUN_14015f0f0).
fn quantise(lo: f32, hi: f32, v: f32, bins: u32, from_max: bool) -> u32 {
    let step = ((f64::from(hi) - f64::from(lo)) as f32) / bins as f32;
    if step <= 0.0 {
        return 0;
    }
    let d = if from_max {
        f64::from(hi) - f64::from(v)
    } else {
        f64::from(v) - f64::from(lo)
    };
    let mut q = ((d / f64::from(step)).floor() as i64).clamp(0, i64::from(bins) - 1) as u32;
    let plane = |k: u32| {
        let s = step * k as f32;
        if from_max { hi - s } else { lo + s }
    };
    let cuts = |x: f32| if from_max { x < v } else { v < x };
    while q != 0 && cuts(plane(q)) {
        q -= 1;
    }
    while q < bins - 1 && !cuts(plane(q + 1)) {
        q += 1;
    }
    q
}

/// A node: the inner min planes (the larger of the two children's mins, 10 bits each of x, y, z) with which child
/// owns the low y and z (bits 30, 31; the left always owns the low x), and the inner max planes (10, 9 and 10 bits)
/// with which child owns each max (bits 61-63), all quantised in the parent's box (FUN_14015c4b0).
fn node(parent: [f32; 6], left: [f32; 6], right: [f32; 6]) -> Result<u64> {
    if left[0] > right[0] {
        return Err(bad(W, 0, "packed collision tree has an invalid low-X owner"));
    }
    let mut w = 0u64;
    for a in 0..3 {
        let inner = if right[a] <= left[a] { left[a] } else { right[a] };
        w |= u64::from(quantise(parent[a], parent[3 + a], inner, 1024, false)) << (10 * a);
    }
    if left[1] > right[1] {
        w |= 1 << 30;
    }
    if left[2] > right[2] {
        w |= 1 << 31;
    }
    let shifts = [32, 42, 51];
    for a in 0..3 {
        let inner = if left[3 + a] <= right[3 + a] {
            left[3 + a]
        } else {
            right[3 + a]
        };
        let bins = if a == 1 { 512 } else { 1024 };
        w |= u64::from(quantise(parent[a], parent[3 + a], inner, bins, true)) << shifts[a];
        if right[3 + a] > left[3 + a] {
            w |= 1 << (61 + a);
        }
    }
    Ok(w)
}

/// The two child boxes a node gives in its parent's box (FUN_14015b6b0), each min x, y, z, max x, y, z: the left
/// keeps the parent's planes and the right takes the inner ones, each owner bit swapping its side. The root's box is
/// its shape's bounds.
pub fn decode_node(parent: [f32; 6], w: u64) -> ([f32; 6], [f32; 6]) {
    let q = [w & 0x3FF, (w >> 10) & 0x3FF, (w >> 20) & 0x3FF];
    let p = [(w >> 32) & 0x3FF, (w >> 42) & 0x1FF, (w >> 51) & 0x3FF];
    let mut left = parent;
    let mut right = [0f32; 6];
    for a in 0..3 {
        let (lo, hi) = (parent[a], parent[3 + a]);
        let ext = f64::from(hi - lo);
        let smin = (ext * 0.000_976_562_5) as f32;
        let smax = (ext / if a == 1 { 512.0 } else { 1024.0 }) as f32;
        right[a] = q[a] as f32 * smin + lo;
        right[3 + a] = hi - p[a] as f32 * smax;
    }
    for (a, b) in [(1, 30), (2, 31), (3, 61), (4, 62), (5, 63)] {
        if w >> b & 1 == 1 {
            std::mem::swap(&mut left[a], &mut right[a]);
        }
    }
    (left, right)
}
