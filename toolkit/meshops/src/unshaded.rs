//! `omit-unshaded-primitives`: every primitive none of whose materials says how it is shaded, left out of its mesh. A
//! material is shaded when it holds `pbr.material` or `core.layered`, what writers draw; a reader's own material that
//! no lowering turned into either (a shader family with no rule yet) draws nothing a writer can take. A primitive is
//! left out when it has no material at all ([`crate::OmitMateriallessPrimitives`]'s case) or when each of its own
//! material and its variants' (`core.material_variants`) is unshaded; the drops are recorded by which, their triangles
//! counted, as are the unshaded materials. A primitive drawn with a shaded material by some placements and an unshaded
//! one by others is refused: leaving out its unshaded variants would draw those placements with its own material
//! instead (`specialize-material-variants` gives each variant a prototype first). A mesh left with no primitive loses
//! its `core.mesh` with its per-primitive components and its file link, a changed one its file link, as
//! `omit-materialless-primitives` does. Running it is the program's choice of omission for a target that refuses an
//! unshaded primitive; the lowering that would shade it is the other.

use std::collections::BTreeSet;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::pbr::PbrMaterial;
use primase_core::{Component, Entity, LayeredMaterial, MaterialVariants, Mesh, MeshFile, World};

use crate::materialless::{Omission, leave_out};

/// See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OmitUnshadedPrimitives;

/// `omit-unshaded-primitives`' declaration ([`primase_core::pass::PassRegistry::add`]): no options.
pub const OMIT_UNSHADED_PRIMITIVES: PassSpec = PassSpec {
    name: "omit-unshaded-primitives",
    options: &[],
    make: |_| Ok(Box::new(OmitUnshadedPrimitives)),
};

const NO_MATERIAL: Omission = Omission {
    why: "primitives without a material, of their own or a variant's",
    triangles: "triangles of the primitives without a material left out",
};

const UNSHADED: Omission = Omission {
    why: "primitives whose materials have no shading (pbr.material or core.layered)",
    triangles: "triangles of the primitives with unshaded materials left out",
};

fn shaded(w: &World, m: Entity) -> bool {
    w.has::<PbrMaterial>(m) || w.has::<LayeredMaterial>(m)
}

impl Pass for OmitUnshadedPrimitives {
    fn name(&self) -> &'static str {
        "omit-unshaded-primitives"
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
        let mut chosen: Vec<(Entity, Vec<(usize, Omission)>)> = Vec::new();
        let mut unshaded: BTreeSet<Entity> = BTreeSet::new();
        for (e, mesh) in w.iter::<Mesh>() {
            let variants = w.get::<MaterialVariants>(e);
            let mut out = Vec::new();
            for (i, p) in mesh.primitives().iter().enumerate() {
                let drawn: Vec<Entity> = p
                    .material
                    .into_iter()
                    .chain(
                        variants
                            .and_then(|v| v.0.get(i))
                            .into_iter()
                            .flat_map(|v| v.values().copied()),
                    )
                    .collect();
                if drawn.is_empty() {
                    out.push((i, NO_MATERIAL));
                    continue;
                }
                let bare: Vec<Entity> = drawn.iter().copied().filter(|&m| !shaded(w, m)).collect();
                if bare.len() == drawn.len() {
                    unshaded.extend(bare);
                    out.push((i, UNSHADED));
                } else if !bare.is_empty() {
                    return Err(format!(
                        "{e:?} primitive {i}: drawn with shaded and unshaded materials ({bare:?} unshaded); \
                         specialize-material-variants first"
                    ));
                }
            }
            if !out.is_empty() {
                chosen.push((e, out));
            }
        }
        log.count("meshes with unshaded primitives", chosen.len() as u64);
        log.count(
            "unshaded materials the primitives left out drew",
            unshaded.len() as u64,
        );
        leave_out(w, log, chosen)
    }
}
