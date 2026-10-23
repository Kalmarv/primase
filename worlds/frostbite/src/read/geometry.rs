//! A MeshSet entity's vertices and triangles read into core meshes ([`Source::read_geometry`]): one mesh for each of its
//! levels of detail, a primitive for each section in the stored order.

use primase_core::{Entity, LodForm, Lods, Mesh, Name, Primitive, Topology, World};

use super::folder::Source;
use super::listing::Bundle;
use crate::components::{FrostbiteMeshSet, FrostbiteVertexElements};
use crate::{Error, Result, geometry};

impl Source<'_> {
    /// The geometry of `meshset` (an entity [`Source::read_entry`] made from a MeshSet of `bundle`, whose LODs' geometry
    /// chunks the bundle or its TOC holds): level 0 as the entity's own `core.mesh` and every other level as an entity
    /// of its own holding that level's mesh, listed in the entity's `core.lods` (named for the entity and `#lod<n>`).
    /// A primitive is a section: its position, texture coordinate and colour streams as stored, its triangles, and the
    /// section's material (the `frostbite.material` of its key where [`Source::resolve_materials`] made one); the
    /// elements [`crate::geometry`] does not decode are in each level's `frostbite.vertex_elements`. Everything is read
    /// before anything is added to `w`; a chunk the bundle and its TOC do not hold, a section whose data does not fit
    /// its chunk, or an element the decoder does not know is an error naming it.
    pub fn read_geometry(&self, bundle: &Bundle, meshset: Entity, w: &mut World) -> Result<Vec<Entity>> {
        let set = w
            .get::<FrostbiteMeshSet>(meshset)
            .ok_or_else(|| Error::Unsupported("not a MeshSet entity".to_owned()))?
            .clone();
        let name = w.get::<Name>(meshset).map(|n| n.0.clone()).unwrap_or_default();
        let mut levels: Vec<(Mesh, FrostbiteVertexElements)> = Vec::with_capacity(set.meshset.lods.len());
        for (l, lod) in set.meshset.lods.iter().enumerate() {
            let (file, size) = bundle.chunk(self, &lod.chunk)?;
            let chunk = self.payload(file, size)?;
            // the vertex buffer, the index buffer, and padding to a multiple of 16 bytes (the padding is not read)
            let used = lod.vertex_bytes as usize + lod.index_bytes as usize;
            if chunk.len() != used.next_multiple_of(16) {
                return Err(Error::Unsupported(format!(
                    "{name}: LOD {l}: a chunk of {} bytes for {} vertex and {} index bytes",
                    chunk.len(),
                    lod.vertex_bytes,
                    lod.index_bytes
                )));
            }
            let (mut primitives, mut elements) = (Vec::new(), Vec::new());
            for (s, section) in lod.sections.iter().enumerate() {
                let d = geometry::decode(&chunk, lod, section)
                    .map_err(|e| Error::Unsupported(format!("{name}: LOD {l} section {s}: {e}")))?;
                primitives.push(Primitive {
                    topology: Topology::Triangles,
                    streams: d.streams,
                    indices: Some(d.indices),
                    material: set.materials.get(&section.key).copied(),
                });
                elements.push(d.raw);
            }
            levels.push((
                Mesh::new(primitives),
                FrostbiteVertexElements { primitives: elements },
            ));
        }
        let (mut made, mut forms) = (Vec::new(), Vec::new());
        for (l, (mesh, elements)) in levels.into_iter().enumerate() {
            if l == 0 {
                w.insert(meshset, mesh);
                w.insert(meshset, elements);
                made.push(meshset);
                continue;
            }
            let e = w.spawn();
            w.insert(e, Name(format!("{name}#lod{l}")));
            w.insert(e, mesh);
            w.insert(e, elements);
            forms.push(LodForm {
                detail: u8::try_from(l).map_err(|_| Error::Unsupported(format!("{name}: {l} levels")))?,
                form: e,
            });
            made.push(e);
        }
        if !forms.is_empty() {
            w.insert(
                meshset,
                Lods {
                    detail: 0,
                    forms,
                    default: None,
                },
            );
        }
        Ok(made)
    }
}
