//! `omit-materialless-primitives`: every primitive drawn with no material, neither its own nor any placement
//! variant's (`core.material_variants`), left out of its mesh: recorded as dropped primitives (their triangles go with
//! them, as the conservation ledger counts them), the triangles also counted.
//! A mesh left with no primitive loses its `core.mesh` with its per-primitive components and its file link
//! ([`World::remove_mesh`]): the entity stays, drawing nothing. A mesh it changes is no longer its file's: the file
//! link (`core.mesh_file`), from which a load would restore the primitives left out, goes too. What it records it
//! has done: a mesh it cannot change (an opaque component) fails the pass with nothing recorded for that mesh. Running it is the program's choice of omission for a target that refuses a
//! primitive without a material; a program that would rather draw them gives them a material instead.

use primase_core::ledger::Measure;
use primase_core::pass::{Pass, PassLog};
use primase_core::{Component, Entity, MaterialVariants, Mesh, MeshFile, Topology, World};

/// Why its drops are made.
const WHY: &str = "primitives without a material, of their own or a variant's";

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OmitMateriallessPrimitives;

/// Primitive `i` of `mesh` has no material, of its own or for any variant.
fn materialless(mesh: &Mesh, variants: Option<&MaterialVariants>, i: usize) -> bool {
    mesh.primitives()[i].material.is_none() && variants.and_then(|v| v.0.get(i)).is_none_or(|v| v.is_empty())
}

impl Pass for OmitMateriallessPrimitives {
    fn name(&self) -> &'static str {
        "omit-materialless-primitives"
    }
    // a world without meshes has nothing to omit
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    /// A changed mesh's file link.
    fn removes(&self) -> &[&'static str] {
        &[MeshFile::NAME]
    }
    /// A mesh left with no primitive goes with its per-primitive components, whichever dialects they are.
    fn removes_by_log(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let omitted: Vec<(Entity, Vec<usize>, usize)> = w
            .iter::<Mesh>()
            .filter_map(|(e, mesh)| {
                let variants = w.get::<MaterialVariants>(e);
                let bare: Vec<usize> = (0..mesh.primitives().len())
                    .filter(|&i| materialless(mesh, variants, i))
                    .collect();
                (!bare.is_empty()).then_some((e, bare, mesh.primitives().len()))
            })
            .collect();
        log.count("meshes with primitives without a material", omitted.len() as u64);
        for (e, bare, of) in omitted {
            let mesh = w.get::<Mesh>(e).expect("listed");
            let triangles: u64 = bare
                .iter()
                .map(|&i| {
                    let p = &mesh.primitives()[i];
                    match p.topology {
                        Topology::Triangles => {
                            (p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3) as u64
                        }
                        _ => 0,
                    }
                })
                .sum();
            // the change first: what is recorded is what was done
            if bare.len() == of {
                for name in w.remove_mesh(e)? {
                    log.record_removed(name);
                }
                log.count("meshes left with no primitive, removed", 1);
            } else {
                w.rebuild_primitives(e, |mesh| {
                    let from: Vec<usize> = (0..mesh.primitives().len())
                        .filter(|i| !bare.contains(i))
                        .collect();
                    Ok((from.iter().map(|&i| mesh.primitives()[i].clone()).collect(), from))
                })?;
                if w.remove::<MeshFile>(e).is_some() {
                    log.count("file links of changed meshes removed", 1);
                }
            }
            log.dropped(Measure::Primitives, WHY, bare.len() as u64);
            log.count("triangles of the primitives left out", triangles);
        }
        Ok(())
    }
}
