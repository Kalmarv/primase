//! `drop-duplicate-triangles`: a triangle a primitive already holds, with the same corners in the same order, left out
//! of it.
//!
//! **Which triangles.** In each triangle-list primitive, a triangle whose three corners equal an earlier triangle's
//! corners, bit for bit in every vertex stream (position, normal, tangent, texture coordinates, colour, joints and
//! weights, shader input), read from the same corner or any rotation of the three (a rotation keeps the winding). The
//! earlier triangle stays. The two draw the same pixels with the same material (a primitive has one material, and one
//! per placement variant, [`primase_core::MaterialVariants`], which every triangle of it shares), so the later one adds
//! nothing the first does not draw, bar blending: a blended surface drawn twice is drawn denser, and a mesh that
//! relies on it loses that. Corners that differ in any stream (a second texture set, a normal) are different
//! triangles; so is the same triangle wound the other way, which faces the other way. Triangles of different
//! primitives are not compared, even of one material.
//!
//! **What changes.** A primitive with a duplicate keeps its vertices and loses the duplicate's indices, in order (unused
//! vertices are harmless to every writer, as in `drop-covered-surfaces`); morph targets and other per-vertex components
//! stay as they are. A changed mesh's file link goes. Collision is not touched.
//!
//! Reported: the duplicates left out, as a drop of triangles.

use std::collections::{HashMap, HashSet};

use primase_core::ledger::Measure;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, Mesh, MeshFile, Primitive, Stream, Topology, World};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropDuplicateTriangles;

/// `drop-duplicate-triangles`' declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const DROP_DUPLICATE_TRIANGLES: PassSpec = PassSpec {
    name: "drop-duplicate-triangles",
    options: &[],
    make: |_| Ok(Box::new(DropDuplicateTriangles)),
};

/// The indices each changed primitive keeps, by primitive, by mesh.
type Kept = Vec<(Entity, Vec<(usize, Vec<u32>)>)>;

const WHY: &str =
    "triangles identical to an earlier triangle of the same primitive (all vertex streams, same winding)";

/// The indices of `p` with every repeat of an earlier triangle left out; `None` when it has none.
fn without_repeats(p: &Primitive) -> Option<Vec<u32>> {
    if p.topology != Topology::Triangles {
        return None;
    }
    let n = p.vertex_count();
    let indices: Vec<u32> = match &p.indices {
        Some(ix) => ix.clone(),
        None => (0..u32::try_from(n).ok()?).collect(),
    };
    // vertices that hold the same bits in every stream are one corner
    let mut class: HashMap<Vec<u32>, u32> = HashMap::new();
    let corner: Vec<u32> = (0..n)
        .map(|v| {
            let mut bits: Vec<u32> = Vec::new();
            for s in p.streams.values() {
                match s {
                    Stream::F32x2(a) => bits.extend(a[v].iter().map(|f| f.to_bits())),
                    Stream::F32x3(a) => bits.extend(a[v].iter().map(|f| f.to_bits())),
                    Stream::F32x4(a) => bits.extend(a[v].iter().map(|f| f.to_bits())),
                    Stream::U8x4(a) => bits.extend(a[v].iter().map(|&b| u32::from(b))),
                    Stream::U16x4(a) => bits.extend(a[v].iter().map(|&b| u32::from(b))),
                }
            }
            let next = class.len() as u32;
            *class.entry(bits).or_insert(next)
        })
        .collect();
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    let mut kept: Vec<u32> = Vec::with_capacity(indices.len());
    for t in indices.as_chunks::<3>().0 {
        let c = t.map(|i| corner[i as usize]);
        // the least of the three rotations, so they are one key
        let key = [c, [c[1], c[2], c[0]], [c[2], c[0], c[1]]]
            .into_iter()
            .min()
            .unwrap_or(c);
        if seen.insert(key) {
            kept.extend_from_slice(t);
        }
    }
    (kept.len() != indices.len()).then_some(kept)
}

impl Pass for DropDuplicateTriangles {
    fn name(&self) -> &'static str {
        "drop-duplicate-triangles"
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    /// A changed mesh's file link.
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let changes: Kept = w
            .iter::<Mesh>()
            .filter_map(|(e, mesh)| {
                let found: Vec<(usize, Vec<u32>)> = mesh
                    .primitives()
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| without_repeats(p).map(|kept| (i, kept)))
                    .collect();
                (!found.is_empty()).then_some((e, found))
            })
            .collect();
        let mut dropped = 0u64;
        for (e, found) in changes {
            let mesh = w
                .get_mut::<Mesh>(e)
                .ok_or_else(|| format!("{e:?}: no core.mesh"))?;
            for (i, kept) in found {
                let mut p = mesh
                    .primitive_mut(i)
                    .ok_or_else(|| format!("{e:?}: primitive {i} vanished"))?;
                let before = p.indices.as_ref().map_or(p.vertex_count(), Vec::len);
                dropped += ((before - kept.len()) / 3) as u64;
                *p.indices_mut() = Some(kept);
            }
            if w.remove::<MeshFile>(e).is_some() {
                log.count("file links of changed meshes removed", 1);
            }
            log.count("meshes with duplicate triangles", 1);
        }
        log.dropped(Measure::Triangles, WHY, dropped);
        Ok(())
    }
}
