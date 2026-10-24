//! Rigid FLVER lowering. A static vertex follows one bone at an implicit weight of 1: the one its NORMAL.w names
//! when its mesh's layout stores a normal W, else its mesh's node (`NodeIndex`). A mesh with no normal W whose node
//! names no bone (-1, or past the bones: Elden Ring's files have such meshes) stays as stored, counted as an
//! approximation on the model ([`UNBOUND_NODE`]): that is Smithbox's observed rule (it gives such a mesh no
//! transform), not established from the game. Dynamic vertices are already in model bind space. Multi-influence or non-unit explicit weights requested for rigid
//! lowering are refused. Dynamic meshes are animated at runtime: their model-space bind pose is preserved and
//! counted, not submitted to this rigid-only transformation.
use crate::flver::{Mesh, Model};
use crate::pose::{ID, Matrix, StaticBones, affine_point, bone_matrices, static_bones};
pub use crate::pose::{posed_bounds, posed_positions};
use primase_core::{
    Component, Entity, Linear, Name, World,
    pass::{Pass, PassLog, PassSpec},
};

/// The approximation counted on a model with a static mesh that stores no normal W and whose node names no bone: it
/// stays as stored.
pub const UNBOUND_NODE: &str = "static meshes with no normal W whose NodeIndex names no bone, left as stored (Smithbox's observed rule, not established from the game)";

/// The FLVER models by their former path.
pub use crate::components::models::*;

/// `fromsoft-apply-rigid-skinning`: pose every `fromsoft.flver` into a `fromsoft.rigid_model` beside it (the raw
/// model stays; a later pass or the program removes what it no longer needs).
#[derive(Debug)]
pub struct ApplyRigidSkinning;
/// `fromsoft-apply-rigid-skinning`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, static meshes posed by their bones as the game draws them.
pub const FROMSOFT_APPLY_RIGID_SKINNING: PassSpec = PassSpec {
    name: "fromsoft-apply-rigid-skinning",
    options: &[],
    make: |_| Ok(Box::new(ApplyRigidSkinning)),
};

impl Pass for ApplyRigidSkinning {
    fn name(&self) -> &'static str {
        "fromsoft-apply-rigid-skinning"
    }
    /// A world with no FLVER model has no static mesh to pose: an area of collision alone, or whose only models were
    /// the distant backdrop `drop-fromsoft-far` left out.
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn requires(&self) -> &[&'static str] {
        &[SourceModel::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[RigidModel::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[RigidModel::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a static mesh with no normal W whose node names no bone is left as stored (Smithbox's observed rule)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let staged = world
            .iter::<SourceModel>()
            .map(|(e, m)| {
                lower(&m.0, Some(e), log).map(|m| (e, m)).map_err(|err| {
                    let name = world
                        .get::<Name>(e)
                        .map_or_else(|| format!("{e:?}"), |n| n.0.clone());
                    format!("{name}: {err}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (e, m) in staged {
            world.insert(e, RigidModel(m));
        }
        Ok(())
    }
}

/// The same implementation used by the declared world pass. The input is never
/// mutated; errors count refused vertices before returning any lowered geometry.
pub fn lower_model(source: &Model, log: &mut PassLog) -> Result<Model, String> {
    lower(source, None, log)
}

/// [`lower_model`], its approximations recorded on the model's entity `on` when there is one.
fn lower(source: &Model, on: Option<Entity>, log: &mut PassLog) -> Result<Model, String> {
    let mut refused = 0;
    for m in &source.meshes {
        if m.dynamic {
            continue;
        }
        for v in &m.vertices {
            let weights: Vec<_> = v.bone_weights.iter().copied().filter(|&w| w != 0.).collect();
            if !weights.is_empty() && (weights.len() != 1 || weights[0] != 1.) {
                refused += 1;
            }
        }
    }
    log.count("refused non-rigid vertices", refused);
    if refused != 0 {
        return Err(format!(
            "rigid skinning refused {refused} vertices: requires one bone with weight 1"
        ));
    }
    let worlds = bone_matrices(&source.bones)?;
    let mut output = source.clone();
    for (index, m) in output.meshes.iter_mut().enumerate() {
        if m.dynamic {
            if !m.vertices.is_empty() {
                log.count("model-space bind vertices", m.vertices.len() as u64);
            }
            continue;
        }
        let bones = static_bones(source, index);
        match bones {
            StaticBones::PerVertex => {}
            StaticBones::Node(_) => log.count("static meshes following their node (no normal W)", 1),
            StaticBones::Unbound(_) => {
                match on {
                    Some(e) => log.approximate_on(e, UNBOUND_NODE, 1),
                    None => log.approximate(UNBOUND_NODE, 1),
                }
                continue;
            }
        }
        pose_static_mesh(m, bones, &worlds, log)?;
    }
    log.count("models lowered", 1);
    Ok(output)
}

/// Pose the static mesh `m` into model space: each vertex by the matrix (`worlds`, [`bone_matrices`]) of the bone it
/// follows (`bones`, [`static_bones`]), its normal by the inverse transpose and its tangents by the 3x3 part; a vertex
/// whose NORMAL.w names no bone stays as stored (counted), and a triangle whose first vertex a mirroring bone moved has
/// its winding reversed. The rule [`lower_model`] applies and the stored-pose lowering binds a static mesh by.
pub(crate) fn pose_static_mesh(
    m: &mut Mesh,
    bones: StaticBones,
    worlds: &[Matrix],
    log: &mut PassLog,
) -> Result<(), String> {
    let mut mirrored = Vec::with_capacity(m.vertices.len());
    for v in &mut m.vertices {
        let matrix = bones.of(v).and_then(|i| worlds.get(i));
        let Some(matrix) = matrix else {
            log.count("unbound model-space vertices", 1);
            mirrored.push(false);
            continue;
        };
        if *matrix == ID {
            mirrored.push(false);
            continue;
        }
        // normals by the inverse transpose and tangents by the bone's 3x3 part, of unit length, as core turns
        // local space
        let l = Linear::of_f64([matrix[0], matrix[1], matrix[2]]);
        if l.singular() {
            return Err("singular rigid bone transform".into());
        }
        v.position = affine_point(matrix, v.position.map(f64::from)).map(|x| x as f32);
        v.normal = l.normal(v.normal).expect("not singular");
        for t in &mut v.tangents {
            *t = l.tangent(*t).expect("not singular");
        }
        if v.position
            .iter()
            .chain(&v.normal)
            .chain(v.tangents.iter().flatten())
            .any(|x| !x.is_finite())
        {
            return Err("nonfinite rigid result".into());
        }
        mirrored.push(l.mirrors());
        log.count("rigid vertices transformed", 1);
    }
    for f in &mut m.faces {
        if mirrored.iter().any(|&m| m) {
            let mut indices = f.triangulate();
            for t in indices.as_chunks_mut::<3>().0 {
                if mirrored[t[0] as usize] {
                    t.swap(1, 2);
                }
            }
            f.indices = indices;
            f.strip = false;
        }
    }
    Ok(())
}
