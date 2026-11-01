//! `omit-materialless-primitives`: every primitive drawn with no material, neither its own nor any placement
//! variant's (`core.material_variants`), left out of its mesh: recorded as dropped primitives (their triangles go with
//! them, as the conservation ledger counts them), the triangles also counted.
//! A mesh left with no primitive loses its `core.mesh` with its per-primitive components and its file link
//! ([`World::remove_mesh`]): the entity stays, drawing nothing. A mesh it changes is no longer its file's: the file
//! link (`core.mesh_file`), from which a load would restore the primitives left out, goes too. What it records it
//! has done: a mesh it cannot change (an opaque component) fails the pass with nothing recorded for that mesh. Running it is the program's choice of omission for a target that refuses a
//! primitive without a material; a program that would rather draw them gives them a material instead.

use primase_core::ledger::Measure;
use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::{Component, Entity, MaterialVariants, Mesh, MeshFile, Topology, World};

/// Why its drops are made.
const WHY: &str = "primitives without a material, of their own or a variant's";

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OmitMateriallessPrimitives;

/// `omit-materialless-primitives`' declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const OMIT_MATERIALLESS_PRIMITIVES: PassSpec = PassSpec {
    name: "omit-materialless-primitives",
    options: &[],
    make: |_| Ok(Box::new(OmitMateriallessPrimitives)),
};

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
        let omitted: Vec<(Entity, Vec<(usize, Omission)>)> = w
            .iter::<Mesh>()
            .filter_map(|(e, mesh)| {
                let variants = w.get::<MaterialVariants>(e);
                let bare: Vec<(usize, Omission)> = (0..mesh.primitives().len())
                    .filter(|&i| materialless(mesh, variants, i))
                    .map(|i| (i, MATERIALLESS))
                    .collect();
                (!bare.is_empty()).then_some((e, bare))
            })
            .collect();
        log.count("meshes with primitives without a material", omitted.len() as u64);
        leave_out(w, log, omitted)
    }
}

/// Why primitives are left out: the drop's reason and the count of their triangles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Omission {
    pub why: &'static str,
    pub triangles: &'static str,
}

const MATERIALLESS: Omission = Omission {
    why: WHY,
    triangles: "triangles of the primitives left out",
};

/// Leaves out the listed primitives of each mesh (index, why), recorded as dropped primitives by reason with their
/// triangles counted; a mesh left with none loses its `core.mesh` with its per-primitive components and its file link,
/// a changed one its file link. What it records it has done: a mesh it cannot change fails with nothing recorded for it.
pub(crate) fn leave_out(
    w: &mut World,
    log: &mut PassLog,
    chosen: Vec<(Entity, Vec<(usize, Omission)>)>,
) -> Result<(), String> {
    for (e, out) in chosen {
        let mesh = w.get::<Mesh>(e).ok_or_else(|| format!("{e:?}: no core.mesh"))?;
        let of = mesh.primitives().len();
        let mut by: Vec<(Omission, u64, u64)> = Vec::new();
        for &(i, why) in &out {
            let p = &mesh.primitives()[i];
            let triangles = match p.topology {
                Topology::Triangles => (p.indices.as_ref().map_or(p.vertex_count(), Vec::len) / 3) as u64,
                _ => 0,
            };
            match by.iter_mut().find(|(w, _, _)| *w == why) {
                Some((_, n, t)) => {
                    *n += 1;
                    *t += triangles;
                }
                None => by.push((why, 1, triangles)),
            }
        }
        // the change first: what is recorded is what was done
        if out.len() == of {
            for name in w.remove_mesh(e)? {
                log.record_removed(name);
            }
            log.count("meshes left with no primitive, removed", 1);
        } else {
            w.rebuild_primitives(e, |mesh| {
                let from: Vec<usize> = (0..mesh.primitives().len())
                    .filter(|i| !out.iter().any(|(j, _)| j == i))
                    .collect();
                Ok((from.iter().map(|&i| mesh.primitives()[i].clone()).collect(), from))
            })?;
            if w.remove::<MeshFile>(e).is_some() {
                log.count("file links of changed meshes removed", 1);
            }
        }
        for (why, n, triangles) in by {
            log.dropped(Measure::Primitives, why.why, n);
            log.count(why.triangles, triangles);
        }
    }
    Ok(())
}
