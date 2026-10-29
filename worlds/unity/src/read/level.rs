//! A Unity scene into the world: one entity per mesh renderer (its world [`Transform`], an [`Instance`] of a
//! prototype, its GameObject path as [`Name`], its [`Shadows`], the `unity.*` details), Unity's frame converted. The
//! renderer's shadow casting mode is the cast (off, on, shadows only as only; two-sided as on, counted) and
//! `m_ReceiveShadows` the receive. A prototype is a
//! mesh, or the slice of a combined mesh a static-batched renderer draws, with its geometry as a [`Mesh`] and that mesh
//! as its [`Source`] (`unity mesh <file>/<path id>`, with `submeshes <first>+<count>` for a slice); one entity
//! per material (`unity.material`) and per texture it uses (`unity.texture`, and `core.texture` when decoded).
//!
//! Materials follow Unity's rule: material i draws submesh i. Renderers of one prototype that use other materials are
//! its variants ([`MaterialVariants`], [`Instance::variant`]). Submeshes beyond a renderer's materials are not drawn
//! and materials beyond its submeshes draw the last one again (an extra pass): neither is reproduced, both counted.
//!
//! Each camera is an entity placed by its Transform with what it draws (`unity.camera`: culling mask, target texture).
//!
//! State is data. Each GameObject stored inactive and each component stored disabled on the way to a renderer,
//! collider, light, camera or terrain is a Bool [`StateAxis`] (default off; class `active`, key `unity.active.<file>.<path
//! id>`, or `enabled`, `unity.enabled.<file>.<path id>`), and the entity's [`Condition`] is one clause: each of them
//! on. Flags stored on are constants, not axes. `resolve-state` with the defaults keeps what Unity draws as stored.
//!
//! A LODGroup whose levels are one renderer each, every coarser one at LOD 0's world matrix (up to rounding: within 4
//! f32 steps, counted) with LOD 0's state,
//! shadows and layer and not static-batched, becomes one placement (LOD 0's) of a prototype with [`Lods`]: the coarser
//! levels' meshes and materials are its forms (`detail` the level, no default: Unity picks by screen size), so a
//! prototype is a mesh with the chain of its coarser forms. Any other LODGroup keeps every level as placements, each
//! with a literal on the group's `unity.lod.<file>.<path id>` axis (class `lod`, its levels, no default: Unity picks by
//! screen size), so a program states which level draws (`resolve-state{assign-class=lod:lod0}` for the finest). A
//! disabled LODGroup switches nothing off: its levels draw together, as in Unity.
//!
//! Every renderer is placed as stored. One whose material builds its geometry in the vertex shader from component
//! data ([`procedural`]) draws a placeholder mesh; it carries its GameObject's MonoBehaviours as bytes
//! ([`UnityBehaviours`]), which hold the shape. A mesh with no submesh that has indices draws nothing: its prototype
//! has no mesh. Leaving such renderers out is a pass's ([`crate::drawn`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use primase_core::skin::SUM_TOLERANCE;
use primase_core::state::{Clause, Condition, Domain, Literal, Scope, StateAxis};
use primase_core::{
    Binding, BindingId, ClipBinding, ClipTarget, Deformation, Entity, Influences, Instance, Joint, JointId,
    LodForm, Lods, MaterialVariants, Mesh, MorphId, MorphWeights, Name, Rest, RigInstance, ShadowCast,
    Shadows, Skeleton, Skin, SkinInstance, Slot, Source, Transform, WeightRule, World,
};

use crate::animator::{UnityAnimator, UnityAvatar};
use crate::clipdata::{RigBone, lower_clip};
use crate::components::{
    ColliderMesh, Stored, UNITY, UnityBehaviours, UnityCamera, UnityCollider, UnityColliderShape, UnityLight,
    UnityMaterial, UnityMesh, UnityObject, UnityPhysicMaterial, UnityRenderSettings, UnityRenderer,
    UnityShadowQuality, UnityTerrain, UnityTerrainCollider, UnityTerrainLayer, UnityTexEnv, UnityTexture,
    UnityTree, UnityTreeInstance,
};
use crate::invalid;
use crate::read::mesh::{leading, primitives, primitives_and_morphs};
use crate::read::texture::texture;
use unity_formats::Result;
use unity_formats::clip::{ClipData, controller_clips, path_hash};
use unity_formats::material::{ShaderData, UnityMaterialData};
use unity_formats::mesh::{MeshSkin, UnityMeshData};
use unity_formats::pipeline::UnityUrpSettings;
use unity_formats::player::{ObjRef, Player, QualityLevel, SkinWeights};
use unity_formats::program::colour_properties;
use unity_formats::scene::{
    Animator, IDENTITY, LodGroup, Matrix, OffFlag, Renderer, RendererSkin, Scene, SceneJoint, Skipped,
};
use unity_formats::scene::{ColliderComponent, ColliderShape};
use unity_formats::shader::procedural;
use unity_formats::terrain::{TerrainData, TerrainLayerData};
use unity_formats::texture::UnityTexture as UnityTextureData;
use unity_formats::value::Value;

/// A prototype's or form's provenance: the Unity mesh it was read from, and the slice a static-batched renderer draws.
pub(crate) fn mesh_source(mesh: &ObjRef, submeshes: Option<(u32, u32)>) -> Source {
    Source(match submeshes {
        Some((first, count)) => format!(
            "unity mesh {}/{} submeshes {first}+{count}",
            mesh.file, mesh.path_id
        ),
        None => format!("unity mesh {}/{}", mesh.file, mesh.path_id),
    })
}

fn unity_renderer(r: &Renderer) -> UnityRenderer {
    UnityRenderer {
        renderer: UnityObject::from(&r.renderer),
        layer: r.layer,
        materials: r
            .materials
            .iter()
            .map(|m| m.as_ref().map(UnityObject::from))
            .collect(),
        submeshes: r.submeshes,
        cast_shadows: r.cast_shadows,
        receive_shadows: r.receive_shadows,
    }
}

/// A terrain tree's matrix: T(terrain position + position x size) Ry(rotation) S(width, height, width).
fn tree_matrix(
    t: &unity_formats::scene::TerrainComponent,
    tree: &unity_formats::terrain::TreeInstance,
    size: [f64; 3],
) -> Matrix {
    let (s, c) = f64::from(tree.rotation).sin_cos();
    let (wd, ht) = (f64::from(tree.width_scale), f64::from(tree.height_scale));
    let mut m = IDENTITY;
    for k in 0..3 {
        m[k][3] = t.world[k][3] + f64::from(tree.position[k]) * size[k];
    }
    m[0][0] = c * wd;
    m[0][2] = s * wd;
    m[1][1] = ht;
    m[2][0] = -s * wd;
    m[2][2] = c * wd;
    m
}

/// A Unity world matrix as a transform in Unity's frame (each entry rounded once to f32).
pub fn transform_of(m: &Matrix) -> Transform {
    let mut cols = [[0.0f32; 3]; 4];
    for (j, c) in cols.iter_mut().enumerate() {
        for (i, x) in c.iter_mut().enumerate() {
            *x = m[i][j] as f32;
        }
    }
    Transform { cols }
}

/// What reading a level did. Serialisable ([`LevelReport::to_json`]): the counts are maps and numbers, the notices
/// keyed by what they are.
#[derive(Clone, Debug, Default, Serialize)]
pub struct LevelReport {
    pub renderers: usize,
    pub prototypes: usize,
    /// Primitives of all prototypes (one per drawn submesh).
    pub primitives: usize,
    pub materials: usize,
    pub textures: usize,
    pub terrains: usize,
    pub lights: usize,
    /// Cameras read, whatever their state (`unity.camera`).
    pub cameras: usize,
    /// Skinned renderers read with their skin (`core.skin_instance`), the skeletons made for their bones (every
    /// Transform from a bone up to its scene root) and those skeletons' joints.
    pub skinned_renderers: usize,
    pub skeletons: usize,
    pub joints: usize,
    /// Animators with a controller that animate a skeleton read, the clips made for them (a clip per skeleton it
    /// moves), their bindings to rigs and the joint channels in them.
    pub animators: usize,
    pub clips: usize,
    pub clip_bindings: usize,
    pub clip_channels: usize,
    /// Blend shape targets of the meshes read (`core.morphs`), the renderers that set weights of them
    /// (`core.morph_weights`) and the morph channels of the clips (counted in `clip_channels` too).
    pub morph_targets: usize,
    pub morph_renderers: usize,
    pub morph_channels: usize,
    /// Avatars and AnimatorControllers read typed (`unity.avatar`, `unity.animator_controller`), each once, for the
    /// Animators of the rigs read (`unity.animator` on the rig).
    pub avatars: usize,
    pub controllers: usize,
    /// Clips lowered with a humanoid avatar (their muscle curves evaluated as bone rotations and their root curves as
    /// the hips' place, [`crate::humanoid`]), and the largest angle between the muscles' rotation and the lowered channel's between keys.
    pub humanoid_clips: usize,
    pub humanoid_worst_between_keys_degrees: f64,
    /// The largest distance a lowered hips translation is off the body position between keys, as a fraction of the
    /// avatar's human scale.
    pub humanoid_worst_hips_between_keys: f64,
    /// RenderSettings names a sun (`unity.render_settings` holds it when it is a light read).
    pub sun_named: bool,
    /// Colliders read: those that collide, and those that do not by reason (inactive or disabled, trigger,
    /// moved by physics).
    pub colliders: usize,
    pub colliders_not_colliding: BTreeMap<&'static str, u64>,
    /// Colliders placed by terrain trees.
    pub tree_colliders: usize,
    /// Renderers placed by terrain trees (their prefab's renderers, per tree).
    pub tree_renderers: usize,
    /// State axes made, by class (`active`, `enabled`, `lod`).
    pub axes: BTreeMap<&'static str, u64>,
    /// Entities with a state condition, by kind (`renderers`, `tree renderers`, `colliders`, `lights`, `terrains`).
    pub conditioned: BTreeMap<&'static str, u64>,
    /// LODGroups by how they are read.
    pub lod_groups: BTreeMap<&'static str, u64>,
    /// Forms made of coarser LOD levels.
    pub lod_forms: usize,
    /// Textures whose pixels were not decoded, by reason (`crunched texture format 28`, ...).
    pub textures_not_decoded: BTreeMap<String, u64>,
    /// Unity drawing rules and references not reproduced, by what: counts of renderer submeshes, materials or
    /// properties.
    pub not_reproduced: BTreeMap<String, u64>,
    /// What was read differently from how it is stored, or not read, because core holds it otherwise or has no slot
    /// for it, by what: quads and line strips converted, components core has no slot for, non-finite vertex values
    /// ([`crate::mesh::MeshNotices`]).
    pub notices: BTreeMap<String, u64>,
    /// Renderers not read, by why (`no MeshFilter`, ...).
    pub skipped: Skipped,
    /// Terrains not read, by why (`no terrain data`: a Terrain naming no TerrainData draws nothing).
    pub terrains_skipped: Skipped,
}

/// The blend shapes an Animator's character has, for what a clip made with them depends on: each by the hash of its
/// renderer's path below the Animator and of its name, the geometry holding it and its id.
type MorphKey = Vec<((u32, u32), Entity, MorphId)>;

/// A renderer whose mesh has blend shapes, for the clips that drive them: the placement drawing it, the prototype
/// holding the morph set, and each shape by the CRC-32 of its name (what a clip's curve names it by).
struct MorphRenderer {
    occurrence: Entity,
    geometry: Entity,
    shapes: Vec<(u32, MorphId)>,
}

/// A prototype being built: its entity, the submesh of each primitive, the material lists of its variants, and
/// whether its forms are made.
struct Proto {
    entity: Entity,
    drawn: Vec<u32>,
    variants: Vec<Vec<Option<Entity>>>,
    forms_made: bool,
}

/// A coarser LOD level folded into a prototype: its mesh and materials.
type Form = (ObjRef, Vec<Option<ObjRef>>);

/// A prototype's key: its mesh, its static-batch slice, the chain of its coarser forms, whether it is drawn skinned.
type ProtoKey = (ObjRef, Option<(u32, u32)>, Vec<Form>, bool);

/// How a skinned renderer is skinned: the skeleton and rig its bones are joints of, and the joint of each bone slot.
#[derive(Clone, Debug)]
struct Skinning {
    skeleton: Entity,
    rig: Entity,
    joints: Vec<JointId>,
}

/// The skin of a prototype being built: its mesh and, per binding, the skeleton and the joint of each bone slot.
struct SkinBuild {
    mesh: ObjRef,
    bindings: Vec<(Entity, Vec<JointId>)>,
}

/// What [`build_rigs`] made: per renderer its rig, each joint's Transform with its skeleton, rig and joint, and the
/// numbers of skeletons and joints.
struct Rigs {
    renderers: Vec<Option<RigOf>>,
    of_joint: BTreeMap<i64, (Entity, Entity, JointId)>,
    skeletons: usize,
    joints: usize,
}

/// What [`build_rigs`] made for one skinned renderer: the skeleton and rig its bones are joints of, and the joint of
/// each bone slot.
type RigOf = (Entity, Entity, Vec<JointId>);

/// Transform `t` and every Transform above it, up to its scene root, reading the joint data into `joints`.
fn chain_of(scene: &Scene, joints: &mut BTreeMap<i64, SceneJoint>, t: i64) -> Result<Vec<i64>> {
    let mut chain = Vec::new();
    let mut at = Some(t);
    while let Some(t) = at {
        if chain.len() > 100_000 {
            return Err(invalid(format!("Transform {t}: parents loop")));
        }
        chain.push(t);
        if let std::collections::btree_map::Entry::Vacant(slot) = joints.entry(t) {
            slot.insert(
                scene
                    .joint(t)?
                    .ok_or_else(|| invalid(format!("Transform {t} is not of its file")))?,
            );
        }
        at = joints[&t].father;
    }
    Ok(chain)
}

/// The Animator each of the Transforms `ts` belongs to, as its GameObject's Transform: the nearest Animator at or above
/// it that has a controller (the one [`build_rigs`] extends a rig up to, whose clips name the Transforms between), else
/// the nearest Animator at all; a Transform under no Animator has none. An Animator nested in another's hierarchy so
/// owns its own rig, whatever the order of the file's object table: PEAK's portrait `Scout` inside the passport UI's
/// Animator, the seated scouts inside the helicopter's. In PEAK's level 207 no clip of an outer Animator names a
/// Transform under a nested Animator but one UI button (its 45 outer Animators' tracks against their nested
/// Transforms' path hashes).
fn owners(
    scene: &Scene,
    animators: &[Animator],
    ts: impl Iterator<Item = i64>,
) -> Result<BTreeMap<i64, i64>> {
    let all: BTreeSet<i64> = animators
        .iter()
        .filter_map(|a| scene.transform_of(a.game_object))
        .collect();
    let with_controller: BTreeSet<i64> = animators
        .iter()
        .filter(|a| a.controller.is_some())
        .filter_map(|a| scene.transform_of(a.game_object))
        .collect();
    let mut joints = BTreeMap::new();
    let mut out = BTreeMap::new();
    for t in ts {
        let chain = chain_of(scene, &mut joints, t)?;
        let owner = chain
            .iter()
            .find(|x| with_controller.contains(x))
            .or_else(|| chain.iter().find(|x| all.contains(x)));
        if let Some(&owner) = owner {
            out.insert(t, owner);
        }
    }
    Ok(out)
}

/// The skeletons of skinned renderers' bones. A renderer's joints are its bones and its root bone with the Transforms
/// between them and their lowest common ancestor (`None` for a renderer whose bones hang from different scene roots),
/// and on up to the nearest Animator's GameObject, whose clips name them all (`animators`: the Transforms of the
/// Animators that have a controller); renderers whose joints share a Transform share a rig, so a character's renderers
/// are one rig and characters under one parent are not. Rigs of the same joints (names, parents, rests) share one
/// skeleton definition. Joints are in path id order (`JointId` their position), each with its local
/// position, rotation and scale as stored, in the world frame; the rig sits where its top joint's parent does (the
/// identity for a scene root; relative to the root Transform `root` of a prefab read). One entry per renderer, and the
/// number of skeletons and joints made.
fn build_rigs(
    w: &mut World,
    scene: &Scene,
    skins: &[&RendererSkin],
    animators: &BTreeSet<i64>,
    root: Option<i64>,
) -> Result<Rigs> {
    let mut joints: BTreeMap<i64, SceneJoint> = BTreeMap::new();
    // each renderer's joint set
    let mut sets: Vec<Option<BTreeSet<i64>>> = Vec::with_capacity(skins.len());
    for skin in skins {
        let mut wanted: Vec<i64> = skin.bones.iter().flatten().copied().collect();
        wanted.extend(skin.root_bone);
        wanted.sort_unstable();
        wanted.dedup();
        let chains = wanted
            .iter()
            .map(|&t| chain_of(scene, &mut joints, t))
            .collect::<Result<Vec<_>>>()?;
        let others: Vec<BTreeSet<i64>> = chains[1..].iter().map(|c| c.iter().copied().collect()).collect();
        let lca = chains[0]
            .iter()
            .copied()
            .find(|t| others.iter().all(|o| o.contains(t)));
        sets.push(match lca {
            None => None,
            Some(lca) => {
                let mut set: BTreeSet<i64> = chains
                    .iter()
                    .flat_map(|c| {
                        let end = c
                            .iter()
                            .position(|t| *t == lca)
                            .expect("the common ancestor is in every chain");
                        c[..=end].iter().copied()
                    })
                    .collect();
                // up to the nearest Animator at or above it: its clips name every Transform on the way
                let above = chain_of(scene, &mut joints, lca)?;
                if let Some(end) = above.iter().position(|t| animators.contains(t)) {
                    set.extend(above[..=end].iter().copied());
                }
                Some(set)
            }
        });
    }
    // renderers sharing a joint are one skeleton: union-find over the renderers
    let mut group: Vec<usize> = (0..skins.len()).collect();
    fn find(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    let mut first_user: BTreeMap<i64, usize> = BTreeMap::new();
    for (i, set) in sets.iter().enumerate() {
        for &t in set.iter().flatten() {
            let other = *first_user.entry(t).or_insert(i);
            let (a, b) = (find(&mut group, i), find(&mut group, other));
            group[a.max(b)] = a.min(b);
        }
    }
    let mut members: BTreeMap<usize, BTreeSet<i64>> = BTreeMap::new();
    for (i, set) in sets.iter().enumerate() {
        if let Some(set) = set {
            let g = find(&mut group, i);
            members.entry(g).or_default().extend(set.iter().copied());
        }
    }
    let mut made: BTreeMap<usize, (Entity, Entity, BTreeMap<i64, JointId>)> = BTreeMap::new();
    let mut shared: BTreeMap<String, Entity> = BTreeMap::new();
    let mut joint_count = 0;
    for (g, set) in &members {
        let id: BTreeMap<i64, JointId> = set
            .iter()
            .enumerate()
            .map(|(k, &t)| (t, JointId(k as u32)))
            .collect();
        // the top joint: the one whose father is outside the skeleton (the sets are paths up to common ancestors, so
        // sharing a joint makes one tree)
        let tops: Vec<i64> = set
            .iter()
            .copied()
            .filter(|t| joints[t].father.is_none_or(|f| !set.contains(&f)))
            .collect();
        let [top] = tops[..] else {
            return Err(invalid(format!(
                "skinned renderers' joints form {} trees, not one skeleton",
                tops.len()
            )));
        };
        let list: Vec<Joint> = set
            .iter()
            .map(|t| {
                let j = &joints[t];
                Joint {
                    id: id[t],
                    name: j.name.clone(),
                    parent: j.father.filter(|f| set.contains(f)).map(|f| id[&f]),
                    rest: Rest::Trs {
                        translation: j.translation.map(|x| x as f32),
                        rotation: j.rotation.map(|x| x as f32),
                        scale: j.scale.map(|x| x as f32),
                    }
                    .converted(&UNITY),
                }
            })
            .collect();
        // the rig sits where its top joint's parent does: in the file, or relative to the prefab's root GameObject
        let at = match (joints[&top].father, root) {
            (Some(parent), Some(root)) => scene
                .relative_to(parent, root)
                .map_or(Transform::IDENTITY, |m| UNITY.transform(&transform_of(&m))),
            (Some(parent), None) => UNITY.transform(&transform_of(&scene.world_of(parent)?)),
            (None, _) => Transform::IDENTITY,
        };
        let skeleton = match shared.get(&format!("{list:?}")) {
            Some(&skeleton) => skeleton,
            None => {
                joint_count += list.len();
                let key = format!("{list:?}");
                let skeleton = w.spawn();
                w.insert(skeleton, Name(format!("skeleton {}", joints[&top].name)));
                w.insert(skeleton, Skeleton { joints: list });
                shared.insert(key, skeleton);
                skeleton
            }
        };
        let rig = w.spawn();
        w.insert(rig, Name(format!("rig {}", joints[&top].name)));
        w.insert(rig, RigInstance { skeleton });
        w.insert(rig, at);
        made.insert(*g, (skeleton, rig, id));
    }
    let mut out = Vec::with_capacity(skins.len());
    for (i, skin) in skins.iter().enumerate() {
        out.push(match sets[i] {
            Some(_) => {
                let (skeleton, rig, id) = &made[&find(&mut group, i)];
                Some((
                    *skeleton,
                    *rig,
                    skin.bones.iter().flatten().map(|t| id[t]).collect(),
                ))
            }
            None => None,
        });
    }
    let of_joint = made
        .values()
        .flat_map(|(skeleton, rig, id)| id.iter().map(move |(t, joint)| (*t, (*skeleton, *rig, *joint))))
        .collect();
    Ok(Rigs {
        renderers: out,
        of_joint,
        skeletons: shared.len(),
        joints: joint_count,
    })
}

/// A rig's skeleton, its joints by the hash of their path below the Animator's GameObject, and, for a humanoid avatar,
/// by the hash of their path below the avatar's root.
type Rig = (Entity, BTreeMap<u32, JointId>, BTreeMap<u32, (JointId, [f64; 3])>);

/// What a renderer is in its scene's LODGroups.
#[derive(Clone, Debug, Default)]
struct Role {
    /// LOD 0 of a folded group: the coarser levels, finest first.
    forms: Vec<Form>,
    /// A coarser level of a folded group (a form, not a placement).
    folded: bool,
    /// In a level above 0 of some group and in no level 0.
    coarser_only: bool,
    /// Groups kept as placements: the group, the mask of the levels it is in, the group's level count.
    lod: Vec<(ObjRef, u64, usize)>,
}

/// Whether two world matrices are the same placement up to rounding: every entry within 4 f32 steps of its
/// magnitude (at least 1). Sibling Transforms that compose to one pose differ by such rounding.
fn close(a: &Matrix, b: &Matrix) -> bool {
    let step = 4.0 * f64::from(f32::EPSILON);
    (0..4).all(|i| (0..4).all(|j| (a[i][j] - b[i][j]).abs() <= step * b[i][j].abs().max(1.0)))
}

/// The roles of `renderers` (one scene's) in `groups` (see the module documentation), and the groups counted.
fn lod_roles(
    renderers: &[Renderer],
    groups: &[LodGroup],
    report: &mut LevelReport,
) -> BTreeMap<ObjRef, Role> {
    let by_pid: BTreeMap<i64, &Renderer> = renderers.iter().map(|r| (r.renderer.path_id, r)).collect();
    let mut memberships: BTreeMap<i64, usize> = BTreeMap::new();
    let (mut in_zero, mut in_rest) = (
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    );
    for g in groups {
        for (i, level) in g.levels.iter().enumerate() {
            for &pid in level {
                *memberships.entry(pid).or_default() += 1;
                if i == 0 {
                    in_zero.insert(pid);
                } else {
                    in_rest.insert(pid);
                }
            }
        }
    }
    let mut roles: BTreeMap<ObjRef, Role> = BTreeMap::new();
    fn role<'a>(
        roles: &'a mut BTreeMap<ObjRef, Role>,
        by_pid: &BTreeMap<i64, &Renderer>,
        pid: i64,
    ) -> Option<&'a mut Role> {
        by_pid
            .get(&pid)
            .map(|r| roles.entry(r.renderer.clone()).or_default())
    }
    for pid in in_rest.difference(&in_zero) {
        if let Some(r) = role(&mut roles, &by_pid, *pid) {
            r.coarser_only = true;
        }
    }
    let mut count = |what: &'static str| *report.lod_groups.entry(what).or_default() += 1;
    for g in groups {
        if g.levels.len() < 2 {
            count("one level (nothing to choose)");
            continue;
        }
        if !g.enabled {
            count("disabled (every level draws)");
            continue;
        }
        let singles: Option<Vec<&Renderer>> = g
            .levels
            .iter()
            .map(|l| match l[..] {
                [pid] if memberships.get(&pid) == Some(&1) => by_pid.get(&pid).copied(),
                _ => None,
            })
            .collect();
        let fits = singles.as_ref().is_some_and(|rs| {
            let r0 = rs[0];
            r0.submeshes.is_none()
                && rs.iter().all(|r| r.skin.is_none())
                && rs[1..].iter().all(|r| {
                    close(&r.world, &r0.world)
                        && r.submeshes.is_none()
                        && r.off == r0.off
                        && r.cast_shadows == r0.cast_shadows
                        && r.receive_shadows == r0.receive_shadows
                        && r.layer == r0.layer
                })
        });
        match singles {
            Some(rs) if fits => {
                count("folded into core.lods");
                if rs[1..].iter().any(|r| r.world != rs[0].world) {
                    count(
                        "folded with a coarser level's matrix within 4 f32 steps of LOD 0's (LOD 0's taken)",
                    );
                }
                let forms = rs[1..]
                    .iter()
                    .map(|r| (r.mesh.clone(), r.materials.clone()))
                    .collect();
                if let Some(r) = role(&mut roles, &by_pid, rs[0].renderer.path_id) {
                    r.forms = forms;
                }
                for r in &rs[1..] {
                    if let Some(r) = role(&mut roles, &by_pid, r.renderer.path_id) {
                        r.folded = true;
                    }
                }
            }
            _ => {
                count("kept as placements under a unity.lod axis");
                for (i, level) in g.levels.iter().enumerate() {
                    for &pid in level {
                        if let Some(r) = role(&mut roles, &by_pid, pid) {
                            match r.lod.iter_mut().find(|(group, ..)| *group == g.group) {
                                Some(entry) => entry.1 |= 1 << i,
                                None => r.lod.push((g.group.clone(), 1 << i, g.levels.len())),
                            }
                        }
                    }
                }
            }
        }
    }
    roles
}

impl LevelReport {
    /// The report as JSON: each field by its name, the maps as objects keyed by reason, the way a consumer lists them.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("a report of numbers and strings serialises")
    }
}

/// The state axes of stored "off" flags (a GameObject stored inactive, a component stored disabled): one Bool axis per
/// flag (`unity.active.<file>.<GameObject>` or `unity.enabled.<file>.<component>`, default off, global), found by its
/// key. The level read makes them; a reader that adds objects to the same world later ([`OffAxes::of`]) conditions
/// them on the same axes, so one state choice holds for all of them.
#[derive(Clone, Debug, Default)]
pub struct OffAxes {
    by_key: BTreeMap<String, Entity>,
}

impl OffAxes {
    /// The off-flag axes `w` already has (a level read's).
    pub fn of(w: &World) -> OffAxes {
        OffAxes {
            by_key: w
                .iter::<StateAxis>()
                .filter(|(_, a)| a.key.starts_with("unity.active.") || a.key.starts_with("unity.enabled."))
                .map(|(e, a)| (a.key.clone(), e))
                .collect(),
        }
    }

    /// The literal "on" of off flag `flag` of `scene`, and the axis's class when this call made it; `path` names the
    /// object the flag is on (the axis's source).
    pub fn literal(
        &mut self,
        w: &mut World,
        scene: &Scene,
        flag: OffFlag,
        path: &str,
    ) -> (Literal, Option<&'static str>) {
        let (class, pid) = match flag {
            OffFlag::Inactive(go) => ("active", go),
            OffFlag::Disabled(c) => ("enabled", c),
        };
        let key = format!("unity.{class}.{}.{pid}", scene.file);
        if let Some(&axis) = self.by_key.get(&key) {
            return (Literal { axis, values: 0b10 }, None);
        }
        let source = match flag {
            OffFlag::Inactive(go) => format!(
                "{} (GameObject stored inactive)",
                scene.transform_of(go).map(|t| scene.path(t)).unwrap_or_default()
            ),
            OffFlag::Disabled(c) => format!("{path} (component {c} stored disabled)"),
        };
        let axis = w.spawn();
        w.insert(
            axis,
            StateAxis {
                key: key.clone(),
                domain: Domain::Bool,
                default: Some(0),
                scope: Scope::Global,
                class: class.into(),
                source,
            },
        );
        self.by_key.insert(key, axis);
        (Literal { axis, values: 0b10 }, Some(class))
    }

    /// The condition of an object with off flags `off` (each "on"), `None` without flags.
    pub fn condition(
        &mut self,
        w: &mut World,
        scene: &Scene,
        off: &[OffFlag],
        path: &str,
    ) -> Option<Condition> {
        let literals: Vec<Literal> = off.iter().map(|&f| self.literal(w, scene, f, path).0).collect();
        (!literals.is_empty()).then(|| Condition(vec![Clause(literals)]))
    }
}

pub(crate) struct Builder<'a> {
    player: &'a Player,
    textures: BTreeMap<ObjRef, Entity>,
    materials: BTreeMap<ObjRef, Entity>,
    layers: BTreeMap<ObjRef, Entity>,
    collider_meshes: BTreeMap<ObjRef, std::sync::Arc<ColliderMesh>>,
    physic_materials: BTreeMap<ObjRef, UnityPhysicMaterial>,
    /// Each shader read: its name and render state, and its colour properties declared `[HDR]`.
    shaders: BTreeMap<ObjRef, (ShaderData, Vec<String>)>,
    /// The off flags' state axes, and LOD axes by group (and tree instance).
    axes: OffAxes,
    lod_axes: BTreeMap<(ObjRef, Option<(Entity, usize)>), Entity>,
    /// The entities of the avatars and controllers read, `None` for one that could not be, and the humanoid avatars' data
    /// the clips are lowered with.
    avatars: BTreeMap<ObjRef, Option<Entity>>,
    humanoids: BTreeMap<ObjRef, UnityAvatar>,
    controller_entities: BTreeMap<ObjRef, Option<Entity>>,
    pub(crate) report: LevelReport,
}

impl<'a> Builder<'a> {
    /// A builder reading from `player`, with nothing read yet.
    pub(crate) fn new(player: &'a Player, report: LevelReport) -> Self {
        Builder {
            player,
            textures: BTreeMap::new(),
            materials: BTreeMap::new(),
            layers: BTreeMap::new(),
            collider_meshes: BTreeMap::new(),
            physic_materials: BTreeMap::new(),
            shaders: BTreeMap::new(),
            axes: OffAxes::default(),
            lod_axes: BTreeMap::new(),
            avatars: BTreeMap::new(),
            humanoids: BTreeMap::new(),
            controller_entities: BTreeMap::new(),
            report,
        }
    }
}

impl Builder<'_> {
    /// The literals of off flags of `scene`'s (each "on"), their axes made the first time (counted); `path` names the
    /// object the flags are on.
    fn literals(&mut self, w: &mut World, scene: &Scene, off: &[OffFlag], path: &str) -> Vec<Literal> {
        off.iter()
            .map(|&flag| {
                let (literal, made) = self.axes.literal(w, scene, flag, path);
                if let Some(class) = made {
                    *self.report.axes.entry(class).or_default() += 1;
                }
                literal
            })
            .collect()
    }

    /// The literal of a LODGroup kept as placements: its axis (per tree instance for a tree's prefab), its levels.
    fn lod_literal(
        &mut self,
        w: &mut World,
        group: &ObjRef,
        mask: u64,
        levels: usize,
        tree: Option<(Entity, usize)>,
    ) -> Literal {
        let key = (group.clone(), tree);
        let axis = match self.lod_axes.get(&key) {
            Some(&e) => e,
            None => {
                let e = w.spawn();
                let at = tree.map_or(String::new(), |(t, i)| format!(".tree{}.{i}", t.index()));
                w.insert(
                    e,
                    StateAxis {
                        key: format!("unity.lod.{}.{}{at}", group.file, group.path_id),
                        domain: Domain::Enum((0..levels).map(|i| format!("lod{i}")).collect()),
                        // Unity picks a level by screen size: the source states none (a program's choice,
                        // resolve-state's assign-class lod)
                        default: None,
                        scope: Scope::Global,
                        class: "lod".into(),
                        source: format!("LODGroup {}/{}{at}", group.file, group.path_id),
                    },
                );
                *self.report.axes.entry("lod").or_default() += 1;
                self.lod_axes.insert(key, e);
                e
            }
        };
        Literal { axis, values: mask }
    }

    /// Put `literals` on `e` as its condition (none when empty), counted as `kind`.
    fn condition(&mut self, w: &mut World, e: Entity, literals: Vec<Literal>, kind: &'static str) {
        if !literals.is_empty() {
            w.insert(e, Condition(vec![Clause(literals)]));
            *self.report.conditioned.entry(kind).or_default() += 1;
        }
    }

    /// Count what reading a mesh could not put into core as stored ([`LevelReport::notices`]).
    pub(crate) fn note_mesh(&mut self, notices: crate::read::mesh::MeshNotices) {
        for (what, n) in notices {
            *self.report.notices.entry(what.to_owned()).or_default() += n;
        }
    }

    /// The clips of every Animator of `scene` that has a controller and animates joints of the skeletons in `joint_of`
    /// (a Transform's skeleton, rig and joint): each clip lowered for each skeleton its Animator holds (a joint's path
    /// below the Animator's GameObject is its CRC-32, as a clip names its targets) as a `core.clip`, and bound to the
    /// rig by a `core.clip_binding`. A joint or blend shape renderer is its nearest Animator's (see [`owners`]), not
    /// that of every Animator above it. What a clip drives that is no joint, and clips that cannot be read, are counted
    /// in the notices.
    fn bind_clips(
        &mut self,
        w: &mut World,
        scene: &Scene,
        joint_of: &BTreeMap<i64, (Entity, Entity, JointId)>,
        morph_renderers: &BTreeMap<i64, MorphRenderer>,
        root: Option<i64>,
    ) -> Result<()> {
        let mut data: BTreeMap<ObjRef, Option<ClipData>> = BTreeMap::new();
        let mut made: BTreeMap<(ObjRef, Entity, Option<ObjRef>, MorphKey), Option<Entity>> = BTreeMap::new();
        let mut bound: BTreeSet<(Entity, Entity)> = BTreeSet::new();
        let mut controllers: BTreeMap<ObjRef, Vec<ObjRef>> = BTreeMap::new();
        let animators = scene.animators_under(self.player, root)?;
        let owner = owners(
            scene,
            &animators,
            joint_of.keys().chain(morph_renderers.keys()).copied(),
        )?;
        for animator in animators {
            let Some(tp) = scene.transform_of(animator.game_object) else {
                continue;
            };
            let owns = |t: i64| owner.get(&t) == Some(&tp);
            // the joints this Animator owns, by skeleton, each by the hash of its path below the Animator's GameObject
            let mut rigs: BTreeMap<Entity, Rig> = BTreeMap::new();
            // by rig: the rig's skeleton (rigs of one skeleton share its clips) and its joints by path hash
            for (&t, &(skeleton, rig, joint)) in joint_of {
                let Some(path) = scene.path_below(t, tp).filter(|_| owns(t)) else {
                    continue;
                };
                let hashes = &mut rigs
                    .entry(rig)
                    .or_insert_with(|| (skeleton, BTreeMap::new(), BTreeMap::new()))
                    .1;
                if hashes.insert(path_hash(&path), joint).is_some() {
                    self.notice(
                        "joints of an Animator's rig sharing a path (the clips drive the last)",
                        1,
                    );
                }
            }
            if rigs.is_empty() {
                self.notice("Animators that animate no skinned skeleton (not bound)", 1);
                continue;
            }
            // the Animator's avatar and controller as typed data, on entities of their own, named by the rigs' Animator
            let avatar = match &animator.avatar {
                Some(a) => self.typed_avatar(w, a)?,
                None => None,
            };
            let humanoid = animator
                .avatar
                .as_ref()
                .and_then(|a| self.humanoids.get(a))
                .cloned();
            if let Some(human) = humanoid.as_ref().and_then(|a| a.human.as_ref()) {
                // a humanoid avatar names its bones from its own root, which is the Animator's GameObject or a
                // descendant of it (a model under a wrapper): the root under which most of the avatar's bone paths
                // are the rig's joints, the shallowest on a tie
                let wanted: BTreeSet<u32> = human.skeleton.ids.iter().copied().collect();
                for (&rig, (_, _, bones)) in &mut rigs {
                    let joints: Vec<(i64, JointId)> = joint_of
                        .iter()
                        .filter(|(_, (_, r, _))| *r == rig)
                        .map(|(&t, &(_, _, j))| (t, j))
                        .collect();
                    let mut candidates: BTreeSet<i64> = BTreeSet::new();
                    for &(t, _) in &joints {
                        let mut at = Some(t);
                        while let Some(x) = at {
                            candidates.insert(x);
                            if x == tp {
                                break;
                            }
                            at = scene.joint(x)?.and_then(|j| j.father);
                        }
                    }
                    let mut best: Option<(usize, usize, i64)> = None;
                    for &d in &candidates {
                        let depth = scene
                            .path_below(d, tp)
                            .map_or(usize::MAX, |p| p.matches('/').count());
                        let score = joints
                            .iter()
                            .filter_map(|&(t, _)| scene.path_below(t, d))
                            .filter(|p| wanted.contains(&path_hash(p)))
                            .count();
                        if score > 0 && best.is_none_or(|(s, de, _)| score > s || (score == s && depth < de))
                        {
                            best = Some((score, depth, d));
                        }
                    }
                    if let Some((_, depth, d)) = best {
                        if d != tp {
                            self.notice(
                                "humanoid avatars rooted at a descendant of the Animator's GameObject (found by its bone paths)",
                                1,
                            );
                        }
                        let _ = depth;
                        for &(t, joint) in &joints {
                            if let Some(path) = scene.path_below(t, d)
                                && let Some(scene_joint) = scene.joint(t)?
                            {
                                bones.insert(path_hash(&path), (joint, scene_joint.translation));
                            }
                        }
                    }
                }
            }
            // the blend shapes this Animator owns, by the hash of their renderer's path and of their name
            let mut morphs: BTreeMap<(u32, u32), (Entity, Entity, MorphId)> = BTreeMap::new();
            for (&t, m) in morph_renderers {
                let Some(path) = scene.path_below(t, tp).filter(|_| owns(t)) else {
                    continue;
                };
                for &(hash, id) in &m.shapes {
                    morphs.insert((path_hash(&path), hash), (m.occurrence, m.geometry, id));
                }
            }
            let morph_key: MorphKey = morphs.iter().map(|(&k, &(_, g, id))| (k, g, id)).collect();
            let controller_entity = match &animator.controller {
                Some(c) if self.player.class_name(c) == Some("AnimatorController") => {
                    self.typed_controller(w, c)?
                }
                _ => None,
            };
            for (&rig, (_, hashes, _)) in &rigs {
                if !w.has::<UnityAnimator>(rig) {
                    w.insert(
                        rig,
                        UnityAnimator {
                            animator: UnityObject::from(&animator.animator),
                            enabled: animator.enabled,
                            apply_root_motion: animator.apply_root_motion,
                            controller: controller_entity,
                            avatar,
                            joints: hashes.iter().map(|(&hash, &joint)| (hash, joint)).collect(),
                        },
                    );
                }
            }
            let Some(controller) = &animator.controller else {
                self.notice(
                    "Animators without a controller (the clips they play are chosen at run time by scripts; not bound)",
                    1,
                );
                continue;
            };
            self.report.animators += 1;
            if self.player.class_name(controller) != Some("AnimatorController") {
                self.notice(
                    "Animators whose controller is not an AnimatorController (clips not read)",
                    1,
                );
                continue;
            }
            if !controllers.contains_key(controller) {
                controllers.insert(controller.clone(), controller_clips(self.player, controller)?);
            }
            for clip in &controllers[controller] {
                if !data.contains_key(clip) {
                    let read = match ClipData::read(self.player, clip) {
                        Ok(d) => Some(d),
                        Err(unity_formats::Error::Unsupported(why)) => {
                            self.notice(&format!("animation clips not read: {why}"), 1);
                            None
                        }
                        Err(e) => return Err(e),
                    };
                    data.insert(clip.clone(), read);
                }
                let Some(d) = &data[clip] else { continue };
                for (&rig, (skeleton, hashes, bones)) in &rigs {
                    let skeleton = *skeleton;
                    let key = (
                        clip.clone(),
                        skeleton,
                        animator.avatar.clone().filter(|_| humanoid.is_some()),
                        morph_key.clone(),
                    );
                    let entity = match made.get(&key) {
                        Some(e) => *e,
                        None => {
                            let bone_joint =
                                |hash: u32| bones.get(&hash).map(|(j, rest)| (skeleton, *j, *rest));
                            let (lowered, report) = lower_clip(
                                d,
                                &|hash| hashes.get(&hash).map(|j| (skeleton, *j)),
                                &|path, shape| morphs.get(&(path, shape)).map(|&(_, g, id)| (g, id)),
                                humanoid
                                    .as_ref()
                                    .map(|a| (a, &bone_joint as &dyn Fn(u32) -> Option<RigBone>)),
                            )?;
                            if let Some(h) = &report.humanoid {
                                self.report.humanoid_clips += 1;
                                self.report.humanoid_worst_between_keys_degrees = self
                                    .report
                                    .humanoid_worst_between_keys_degrees
                                    .max(h.worst_between_keys_degrees);
                                self.report.humanoid_worst_hips_between_keys = self
                                    .report
                                    .humanoid_worst_hips_between_keys
                                    .max(h.worst_hips_between_keys);
                                for (what, n) in [
                                    (
                                        "humanoid clips whose hips are not placed (the avatar lacks what places them)",
                                        usize::from(!h.hips),
                                    ),
                                    (
                                        "humanoid clips whose hips rotation is a fit of the editor's body frame, not exact (0.05 degrees rms on spine poses, 0.25 rms and up to 3.2 on random poses, up to 9.6 with every muscle near its limit; the centre of mass is exact)",
                                        usize::from(h.hips),
                                    ),
                                    (
                                        "humanoid clip RootT or RootQ curves missing (the body keeps the avatar's rest position, or no turn, in them)",
                                        if h.hips { h.missing_root_curves } else { 0 },
                                    ),
                                    (
                                        "humanoid clip IK goal curves not applied (used only by a layer with IK)",
                                        h.goal_curves,
                                    ),
                                    (
                                        "humanoid clip finger muscle curves not applied (no avatar read here has fingers to check the formula on)",
                                        h.finger_curves,
                                    ),
                                    (
                                        "humanoid clips lacking a body muscle curve (the muscle taken as 0)",
                                        h.missing_muscles,
                                    ),
                                ] {
                                    self.notice(what, n as u64);
                                }
                                for bone in &h.unresolved_bones {
                                    self.notice(
                                        &format!(
                                            "humanoid bones with no joint in the rig, left unrotated: {bone}"
                                        ),
                                        1,
                                    );
                                }
                            }
                            for (kind, n) in &report.unresolved {
                                self.notice(
                                    &format!("animation clip {kind} curves of Transforms that are no joint of the rig"),
                                    *n as u64,
                                );
                            }
                            for (kind, n) in &report.not_lowered {
                                self.notice(&format!("animation clip curves not lowered: {kind}"), *n as u64);
                            }
                            self.notice(
                                "animation clip blend shape curves whose renderer or shape the Animator's character lacks",
                                report.unresolved_morphs as u64,
                            );
                            self.report.morph_channels += report.morph_channels;
                            self.notice(
                                "animation clip rotation keys renormalised to unit length",
                                report.renormalised_keys as u64,
                            );
                            let entity = if report.channels == 0 {
                                self.notice("animation clips that drive no joint of a rig (not made)", 1);
                                None
                            } else {
                                let e = w.spawn();
                                self.report.clips += 1;
                                self.report.clip_channels += report.channels;
                                w.insert(e, Name(d.name.clone()));
                                w.insert(e, Source(format!("unity clip {}/{}", clip.file, clip.path_id)));
                                w.insert(e, UnityObject::from(clip));
                                w.insert(e, lowered);
                                Some(e)
                            };
                            made.insert(key, entity);
                            entity
                        }
                    };
                    if let Some(clip_entity) = entity
                        && bound.insert((clip_entity, rig))
                    {
                        // the occurrences of the blend shapes the clip drives
                        let driven: BTreeSet<Entity> = w
                            .get::<primase_core::Clip>(clip_entity)
                            .iter()
                            .flat_map(|c| &c.channels)
                            .filter_map(|c| match c.target {
                                ClipTarget::Morph { geometry, .. } => Some(geometry),
                                _ => None,
                            })
                            .collect();
                        let meshes: Vec<Entity> = morphs
                            .values()
                            .filter(|(_, g, _)| driven.contains(g))
                            .map(|(o, _, _)| *o)
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect();
                        let e = w.spawn();
                        w.insert(e, Name(format!("{} on {}", d.name, animator.path)));
                        w.insert(
                            e,
                            ClipBinding {
                                clip: clip_entity,
                                rig: Some(rig),
                                meshes,
                            },
                        );
                        self.report.clip_bindings += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// The avatar `r` as an entity with its typed data, read once; `None` (counted) for one that cannot be read.
    fn typed_avatar(&mut self, w: &mut World, r: &ObjRef) -> Result<Option<Entity>> {
        if let Some(e) = self.avatars.get(r) {
            return Ok(*e);
        }
        let entity = match unity_formats::animator::UnityAvatar::read(self.player, r).map(Stored) {
            Ok(avatar) => {
                let e = w.spawn();
                w.insert(e, Name(avatar.name.clone()));
                w.insert(e, Source(format!("unity avatar {}/{}", r.file, r.path_id)));
                if avatar.human.is_some() {
                    self.humanoids.insert(r.clone(), avatar.clone());
                }
                w.insert(e, avatar);
                self.report.avatars += 1;
                Some(e)
            }
            Err(unity_formats::Error::Unsupported(why)) => {
                self.notice(&format!("avatars not read: {why}"), 1);
                None
            }
            Err(e) => return Err(e),
        };
        self.avatars.insert(r.clone(), entity);
        Ok(entity)
    }

    /// The AnimatorController `r` as an entity with its typed data, read once.
    fn typed_controller(&mut self, w: &mut World, r: &ObjRef) -> Result<Option<Entity>> {
        if let Some(e) = self.controller_entities.get(r) {
            return Ok(*e);
        }
        let entity = match unity_formats::animator::UnityAnimatorController::read(self.player, r).map(Stored)
        {
            Ok(controller) => {
                let e = w.spawn();
                w.insert(e, Name(controller.name.clone()));
                w.insert(
                    e,
                    Source(format!("unity animator controller {}/{}", r.file, r.path_id)),
                );
                w.insert(e, controller);
                self.report.controllers += 1;
                Some(e)
            }
            Err(unity_formats::Error::Unsupported(why)) => {
                self.notice(&format!("animator controllers not read: {why}"), 1);
                None
            }
            Err(e) => return Err(e),
        };
        self.controller_entities.insert(r.clone(), entity);
        Ok(entity)
    }

    /// Count what was read differently from how it is stored ([`LevelReport::notices`]).
    fn notice(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self.report.notices.entry(what.to_owned()).or_default() += n;
        }
    }

    fn note(&mut self, what: &str, n: u64) {
        if n > 0 {
            *self.report.not_reproduced.entry(what.into()).or_default() += n;
        }
    }

    pub(crate) fn texture(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.textures.get(r) {
            return Ok(e);
        }
        let t = UnityTextureData::read(self.player, r)?;
        let e = w.spawn();
        w.insert(e, Name(t.name.clone()));
        let not_decoded = match texture(&t) {
            Ok(pixels) => {
                w.insert(e, pixels);
                None
            }
            Err(unity_formats::Error::Unsupported(why)) => {
                let why = why.rsplit(": ").next().unwrap_or(&why).to_owned();
                *self.report.textures_not_decoded.entry(why.clone()).or_default() += 1;
                Some(why)
            }
            Err(err) => return Err(err),
        };
        w.insert(
            e,
            UnityTexture {
                texture: UnityObject::from(r),
                format: t.format,
                srgb: t.srgb,
                usage: t.usage,
                wrap: t.wrap,
                filter: t.filter,
                mip_bias: t.mip_bias,
                aniso: t.aniso,
                not_decoded,
            },
        );
        self.textures.insert(r.clone(), e);
        self.report.textures += 1;
        Ok(e)
    }

    /// A texture a terrain names: `None` (counted) when it is not a Texture2D.
    fn texture_ref(&mut self, w: &mut World, r: &Option<ObjRef>) -> Result<Option<Entity>> {
        match r {
            Some(t) if self.player.class_name(t) == Some("Texture2D") => Ok(Some(self.texture(w, t)?)),
            Some(_) => {
                self.note("terrain textures that are not a Texture2D", 1);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn layer(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.layers.get(r) {
            return Ok(e);
        }
        let l = TerrainLayerData::read(self.player, r)?;
        let layer = UnityTerrainLayer {
            layer: UnityObject::from(r),
            diffuse: self.texture_ref(w, &l.diffuse)?,
            normal: self.texture_ref(w, &l.normal)?,
            mask: self.texture_ref(w, &l.mask)?,
            tile_size: l.tile_size,
            tile_offset: l.tile_offset,
            specular: l.specular,
            metallic: l.metallic,
            smoothness: l.smoothness,
            normal_scale: l.normal_scale,
            diffuse_remap: l.diffuse_remap,
            mask_remap: l.mask_remap,
        };
        let e = w.spawn();
        w.insert(e, Name(l.name));
        w.insert(e, layer);
        self.layers.insert(r.clone(), e);
        Ok(e)
    }

    /// The renderers terrain `t` places with its trees: per tree instance, its prototype prefab's renderers, each at
    /// T(terrain position + position x size) Ry(rotation) S(width, height, width) times its matrix relative to the
    /// prefab's root; the root's own transform is not applied (counted when it is not the identity). The prefab
    /// renderers' LODGroup roles go into `roles`.
    fn trees(
        &mut self,
        t: &unity_formats::scene::TerrainComponent,
        td: &TerrainData,
        prefabs: &mut BTreeMap<String, Scene>,
        roles: &mut BTreeMap<ObjRef, Role>,
    ) -> Result<Vec<(usize, Renderer)>> {
        let mut per_prototype = Vec::with_capacity(td.tree_prototypes.len());
        for p in &td.tree_prototypes {
            let Some(p) = p else {
                per_prototype.push(None);
                continue;
            };
            if !prefabs.contains_key(&p.file) {
                prefabs.insert(p.file.clone(), Scene::read(self.player, &p.file)?);
            }
            let prefab = &prefabs[&p.file];
            let root = prefab.transform_of(p.path_id).ok_or_else(|| {
                crate::invalid(format!("tree prefab {}/{}: no transform", p.file, p.path_id))
            })?;
            let root_identity = prefab.local_of(root) == Some(IDENTITY);
            self.note(
                "tree prefab roots with a transform (not applied)",
                u64::from(!root_identity),
            );
            let (rs, _) = prefab.renderers_under(self.player, Some(p.path_id))?;
            if !rs.iter().any(|r| roles.contains_key(&r.renderer)) {
                let groups = prefab.lod_groups_under(self.player, Some(p.path_id))?;
                roles.extend(lod_roles(&rs, &groups, &mut self.report));
            }
            per_prototype.push(Some((prefab.path(root), rs)));
        }
        let size = td.size();
        let name = t.path.rsplit('/').next().unwrap_or(&t.path);
        let mut out = Vec::new();
        for (i, tree) in td.trees.iter().enumerate() {
            let Some(Some((prefab_path, rs))) = per_prototype.get(tree.prototype) else {
                self.note("trees whose prototype has no prefab", 1);
                continue;
            };
            let m = tree_matrix(t, tree, size);
            for r in rs {
                let mut r = r.clone();
                // `<terrain>/tree<i>:<prefab>/<renderer's path in the prefab, its root included>`
                r.path = format!("{name}/tree{i}:{prefab_path}/{}", r.path);
                r.world = unity_formats::scene::mul(&m, &r.world);
                out.push((i, r));
            }
        }
        Ok(out)
    }

    fn physic_material(&mut self, r: &ObjRef) -> Result<UnityPhysicMaterial> {
        if let Some(p) = self.physic_materials.get(r) {
            return Ok(p.clone());
        }
        let v = self.player.decode(r)?;
        let p = read_physic_material(&v, &format!("{}:{}", r.file, r.path_id))?;
        self.physic_materials.insert(r.clone(), p.clone());
        Ok(p)
    }

    /// A mesh collider's source geometry: every triangle submesh of the mesh, as stored.
    fn collider_mesh(&mut self, r: &ObjRef) -> Result<std::sync::Arc<ColliderMesh>> {
        if let Some(m) = self.collider_meshes.get(r) {
            return Ok(m.clone());
        }
        let data = UnityMeshData::read(self.player, r)?;
        let mut m = ColliderMesh::default();
        if data.channels.first().is_some_and(Option::is_some) {
            m.positions = leading(&data, 0)?;
        }
        for s in &data.submeshes {
            if s.topology != 0 {
                self.note("mesh collider submeshes that are not triangles", 1);
                continue;
            }
            let first = (s.first_byte / data.index_size) as usize;
            let idx = data
                .indices
                .get(first..first + s.index_count as usize)
                .ok_or_else(|| crate::invalid(format!("{}: collider indices past the buffer", data.name)))?;
            m.triangles.extend(
                idx.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|t| t.map(|i| i + s.base_vertex)),
            );
        }
        let m = std::sync::Arc::new(m);
        self.collider_meshes.insert(r.clone(), m.clone());
        Ok(m)
    }

    /// A collider as the world's dialect.
    fn collider(&mut self, c: &ColliderComponent) -> Result<UnityCollider> {
        let f3 = |v: [f64; 3]| v.map(|x| x as f32);
        let shape = match &c.shape {
            ColliderShape::Box { center, size } => UnityColliderShape::Box {
                center: f3(*center),
                size: f3(*size),
            },
            ColliderShape::Sphere { center, radius } => UnityColliderShape::Sphere {
                center: f3(*center),
                radius: *radius as f32,
            },
            ColliderShape::Capsule {
                center,
                radius,
                height,
                direction,
            } => UnityColliderShape::Capsule {
                center: f3(*center),
                radius: *radius as f32,
                height: *height as f32,
                direction: *direction,
            },
            ColliderShape::Mesh { mesh, convex } => UnityColliderShape::Mesh {
                mesh: mesh.as_ref().map(UnityObject::from),
                convex: *convex,
                geometry: mesh.as_ref().map(|m| self.collider_mesh(m)).transpose()?,
            },
        };
        let physic_material = c
            .physic_material
            .as_ref()
            .map(|m| self.physic_material(m))
            .transpose()?;
        let why = if !c.off.is_empty() {
            Some("inactive or disabled")
        } else if c.trigger {
            Some("trigger")
        } else if c.rigidbody == Some(false) {
            Some("moved by physics (a non-kinematic Rigidbody)")
        } else {
            None
        };
        match why {
            Some(w) => *self.report.colliders_not_colliding.entry(w).or_default() += 1,
            None => self.report.colliders += 1,
        }
        Ok(UnityCollider {
            collider: UnityObject::from(&c.collider),
            layer: c.layer,
            shape,
            trigger: c.trigger,
            physic_material,
            rigidbody: c.rigidbody,
            lowered: false,
        })
    }

    /// The colliders terrain `t` places with its trees, as [`Self::trees`] places their renderers: per tree, its
    /// prefab's colliders (every one, flagged) at the tree's matrix times their matrix relative to the prefab root.
    fn tree_colliders(
        &mut self,
        t: &unity_formats::scene::TerrainComponent,
        td: &TerrainData,
        prefabs: &mut BTreeMap<String, Scene>,
    ) -> Result<Vec<(usize, ColliderComponent)>> {
        let mut per_prototype = Vec::with_capacity(td.tree_prototypes.len());
        for p in &td.tree_prototypes {
            let Some(p) = p else {
                per_prototype.push(None);
                continue;
            };
            if !prefabs.contains_key(&p.file) {
                prefabs.insert(p.file.clone(), Scene::read(self.player, &p.file)?);
            }
            let prefab = &prefabs[&p.file];
            let root = prefab.transform_of(p.path_id).ok_or_else(|| {
                crate::invalid(format!("tree prefab {}/{}: no transform", p.file, p.path_id))
            })?;
            per_prototype.push(Some((
                prefab.path(root),
                prefab.colliders_under(self.player, Some(p.path_id))?,
            )));
        }
        let size = td.size();
        let name = t.path.rsplit('/').next().unwrap_or(&t.path);
        let mut out = Vec::new();
        for (i, tree) in td.trees.iter().enumerate() {
            let Some(Some((prefab_path, cs))) = per_prototype.get(tree.prototype) else {
                continue;
            };
            let m = tree_matrix(t, tree, size);
            for c in cs {
                let mut c = c.clone();
                c.path = format!("{name}/tree{i}:{prefab_path}/{}", c.path);
                c.world = unity_formats::scene::mul(&m, &c.world);
                out.push((i, c));
            }
        }
        Ok(out)
    }

    /// Shader `r`'s name and render state, and the colour properties it declares `[HDR]`.
    fn shader(&mut self, r: &ObjRef) -> Result<(ShaderData, Vec<String>)> {
        if let Some(s) = self.shaders.get(r) {
            return Ok(s.clone());
        }
        let data = ShaderData::read(self.player, r)?;
        let hdr = colour_properties(self.player, r)?
            .into_iter()
            .filter(|p| p.hdr())
            .map(|p| p.name)
            .collect();
        let read = (data, hdr);
        self.shaders.insert(r.clone(), read.clone());
        Ok(read)
    }

    pub(crate) fn material(&mut self, w: &mut World, r: &ObjRef) -> Result<Entity> {
        if let Some(&e) = self.materials.get(r) {
            return Ok(e);
        }
        let m = UnityMaterialData::read(self.player, r)?;
        let shader = m.shader.as_ref().map(|s| self.shader(s)).transpose()?;
        let (shader, shader_state, hdr_colors) = match shader {
            Some((s, hdr)) => (Some(s.name), s.subshaders, hdr),
            None => (None, Vec::new(), Vec::new()),
        };
        let mut textures = Vec::with_capacity(m.textures.len());
        for (name, env) in &m.textures {
            // a texture property may name another class (a RenderTexture, a Cubemap): only Texture2D is read
            let texture = match &env.texture {
                Some(t) if self.player.class_name(t) == Some("Texture2D") => Some(self.texture(w, t)?),
                Some(_) => {
                    self.note("texture properties naming a texture that is not a Texture2D", 1);
                    None
                }
                None => None,
            };
            textures.push(UnityTexEnv {
                name: name.clone(),
                referenced: env.texture.is_some(),
                texture,
                scale: env.scale,
                offset: env.offset,
            });
        }
        let e = w.spawn();
        w.insert(e, Name(m.name.clone()));
        w.insert(
            e,
            UnityMaterial {
                material: UnityObject::from(r),
                shader,
                shader_state,
                textures,
                floats: m.floats,
                ints: m.ints,
                colors: m.colors,
                hdr_colors,
                keywords: m.keywords,
                tags: m.tags,
                render_queue: m.render_queue,
            },
        );
        self.materials.insert(r.clone(), e);
        self.report.materials += 1;
        Ok(e)
    }
}

/// What a level read is told besides the scene.
#[derive(Clone, Debug, Default)]
pub struct LevelOptions {
    /// The quality level (a name of [`Player::quality_levels`]) whose skin weights limit the skinned meshes' bone
    /// influences. The current level is a runtime setting, not data, so it is required exactly where the player's
    /// levels differ in what they draw of a skinned mesh; where they agree the data decides.
    pub quality: Option<String>,
}

/// The skin weights limit that applies to the mesh `mesh` with `skin`: the stated quality level's, else the one every
/// level agrees on (levels draw the same when [`MeshSkin::outcome`] is the same); an error naming the levels where
/// they differ and none is stated. A player without quality levels keeps every influence a mesh stores.
fn skin_limit(
    levels: &[QualityLevel],
    quality: Option<&str>,
    mesh: &str,
    skin: &MeshSkin,
) -> Result<SkinWeights> {
    // each level as the option takes it (its name, quoted), and what it draws of a skin beside it
    let names = || {
        levels
            .iter()
            .map(|l| format!("{:?} (skin weights {})", l.name, l.skin_weights))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(q) = quality {
        let mut named = levels.iter().filter(|l| l.name == q);
        return match (named.next(), named.next()) {
            (Some(l), None) => Ok(l.skin_weights),
            (None, _) => Err(invalid(format!(
                "quality level {q:?} is not one of the player's: {}",
                names()
            ))),
            (Some(_), Some(_)) => Err(invalid(format!("quality level {q:?} names more than one level"))),
        };
    }
    let Some(first) = levels.first() else {
        return Ok(SkinWeights::Unlimited);
    };
    let outcome = skin.outcome(first.skin_weights);
    if levels.iter().all(|l| skin.outcome(l.skin_weights) == outcome) {
        Ok(first.skin_weights)
    } else {
        Err(invalid(format!(
            "{mesh}: the quality levels differ in what they draw of its skin ({}): state the quality level",
            names()
        )))
    }
}

/// Read the scene file `level` of `player` into `w`; a skinned mesh whose skin the player's quality levels draw
/// differently needs [`read_level_with`] and a quality level.
pub fn read_level(player: &Player, level: &str, w: &mut World) -> Result<LevelReport> {
    read_level_with(player, level, &LevelOptions::default(), w)
}

/// [`read_level`] told `options`.
pub fn read_level_with(
    player: &Player,
    level: &str,
    options: &LevelOptions,
    w: &mut World,
) -> Result<LevelReport> {
    read_scene(player, level, None, options, w)
}

/// A character: the Animator `animator` (see [`Scene::characters`]) with everything below its GameObject, read into
/// `w` as a prefab: its renderers (skinned ones with their skeletons, rigs and skins) placed relative to that
/// GameObject, its colliders, and the clips of its AnimatorController bound to its rigs. The file may be a scene or
/// an assets file holding the prefab; no terrains, lights or cameras are read.
pub fn read_character(
    player: &Player,
    animator: &ObjRef,
    options: &LevelOptions,
    w: &mut World,
) -> Result<LevelReport> {
    let scene = Scene::read(player, &animator.file)?;
    let found = scene
        .animators(player)?
        .into_iter()
        .find(|a| a.animator == *animator)
        .ok_or_else(|| {
            invalid(format!(
                "{}/{} is no Animator of its file",
                animator.file, animator.path_id
            ))
        })?;
    read_scene(player, &animator.file, Some(found.game_object), options, w)
}

/// The level `level`, or only the GameObject `root` of it and its descendants (a prefab).
fn read_scene(
    player: &Player,
    level: &str,
    root: Option<i64>,
    options: &LevelOptions,
    w: &mut World,
) -> Result<LevelReport> {
    let scene = Scene::read(player, level)?;
    let root_tp = root.and_then(|go| scene.transform_of(go));
    let (renderers, skipped) = scene.renderers_under(player, root)?;
    let mut b = Builder::new(
        player,
        LevelReport {
            skipped,
            ..LevelReport::default()
        },
    );
    let groups = scene.lod_groups_under(player, root)?;
    let mut roles = lod_roles(&renderers, &groups, &mut b.report);
    // terrains, and the renderers of their trees
    let mut trees: Vec<((Entity, usize), Renderer)> = Vec::new();
    let mut tree_colliders: Vec<((Entity, usize), ColliderComponent)> = Vec::new();
    let mut terrain_state: BTreeMap<Entity, (String, Vec<OffFlag>)> = BTreeMap::new();
    // a disabled terrain collider makes no tree colliders: its flag joins their state
    let mut terrain_collider_off: BTreeMap<Entity, Vec<OffFlag>> = BTreeMap::new();
    let mut prefabs: BTreeMap<String, Scene> = BTreeMap::new();
    let terrains = if root.is_none() {
        let (terrains, skipped) = scene.terrains(player)?;
        b.report.terrains_skipped = skipped;
        terrains
    } else {
        Vec::new()
    };
    for t in terrains {
        let td = TerrainData::read(player, &t.data)?;
        let e = w.spawn();
        if t.draw_trees {
            trees.extend(
                b.trees(&t, &td, &mut prefabs, &mut roles)?
                    .into_iter()
                    .map(|(i, r)| ((e, i), r)),
            );
        }
        if let Some(c) = t.collider.as_ref().filter(|c| c.tree_colliders) {
            if !c.enabled {
                terrain_collider_off.insert(e, vec![OffFlag::Disabled(c.collider.path_id)]);
            }
            tree_colliders.extend(
                b.tree_colliders(&t, &td, &mut prefabs)?
                    .into_iter()
                    .map(|(i, c)| ((e, i), c)),
            );
        }
        let mut layers = Vec::with_capacity(td.layers.len());
        for l in &td.layers {
            layers.push(l.as_ref().map(|l| b.layer(w, l)).transpose()?);
        }
        let mut alphamaps = Vec::with_capacity(td.alphamaps.len());
        for a in &td.alphamaps {
            alphamaps.push(b.texture_ref(w, a)?);
        }
        let material = t.material.as_ref().map(|m| b.material(w, m)).transpose()?;
        let collider = match &t.collider {
            Some(c) => Some(UnityTerrainCollider {
                collider: UnityObject::from(&c.collider),
                enabled: c.enabled,
                other_data: c.data.as_ref().filter(|d| **d != t.data).map(UnityObject::from),
                physic_material: match &c.material {
                    Some(m) => player
                        .decode(m)?
                        .get("m_Name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    None => None,
                },
                tree_colliders: c.tree_colliders,
            }),
            None => None,
        };
        // Unity places a terrain by its position only
        let mut at = IDENTITY;
        for (i, row) in at.iter_mut().enumerate().take(3) {
            row[3] = t.world[i][3];
        }
        let linear_identity = (0..3).all(|i| (0..3).all(|j| t.world[i][j] == IDENTITY[i][j]));
        b.note(
            "terrain rotations or scales ignored (Unity places terrains by position)",
            u64::from(!linear_identity),
        );
        w.insert(e, Name(t.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&at)));
        w.insert(
            e,
            UnityTerrain {
                terrain: UnityObject::from(&t.terrain),
                data: UnityObject::from(&t.data),
                resolution: td.resolution,
                scale: td.scale,
                heights: td.heights,
                holes: td.holes,
                layers,
                alphamaps,
                alphamap_resolution: td.alphamap_resolution,
                material,
                draw_heightmap: t.draw_heightmap,
                draw_trees: t.draw_trees,
                collider,
                tree_prototypes: td
                    .tree_prototypes
                    .iter()
                    .map(|p| p.as_ref().map(UnityObject::from))
                    .collect(),
                trees: td
                    .trees
                    .iter()
                    .map(|i| UnityTree {
                        position: i.position,
                        width_scale: i.width_scale,
                        height_scale: i.height_scale,
                        rotation: i.rotation,
                        color: i.color,
                        prototype: i.prototype,
                    })
                    .collect(),
            },
        );
        let literals = b.literals(w, &scene, &t.off, &t.path);
        b.condition(w, e, literals, "terrains");
        terrain_state.insert(e, (t.path.clone(), t.off.clone()));
        b.report.terrains += 1;
    }
    // Renderers: those Unity draws as stored first, in the scene's order and then the trees', so the entities a
    // pipeline keeps come in the order the reader always made them; then the coarser forms; then the others.
    let role_of = |r: &Renderer| roles.get(&r.renderer).cloned().unwrap_or_default();
    let placed: Vec<(Option<(Entity, usize)>, &Renderer)> = renderers
        .iter()
        .map(|r| (None, r))
        .chain(trees.iter().map(|(of, r)| (Some(*of), r)))
        .filter(|(_, r)| !role_of(r).folded)
        .collect();
    let drawn_as_stored = |tree: &Option<(Entity, usize)>, r: &Renderer| {
        r.off.is_empty()
            && !role_of(r).coarser_only
            && tree.is_none_or(|(t, _)| terrain_state.get(&t).is_none_or(|(_, off)| off.is_empty()))
    };
    let (first, rest): (Vec<_>, Vec<_>) = placed.into_iter().partition(|(tree, r)| drawn_as_stored(tree, r));
    let mut meshes: BTreeMap<ObjRef, UnityMeshData> = BTreeMap::new();
    let mut prototypes: BTreeMap<ProtoKey, Proto> = BTreeMap::new();
    // the skinned renderers whose skin is read: a mesh with blend weights, a bone for each of its bind poses, every
    // bone a joint of one skeleton; the others are drawn unskinned, counted
    let mut candidates: Vec<(&Renderer, &RendererSkin)> = Vec::new();
    let mut levels: Option<Vec<QualityLevel>> = None;
    let mut limits: BTreeMap<ObjRef, SkinWeights> = BTreeMap::new();
    for (tree, r) in first.iter().chain(rest.iter()) {
        let Some(skin) = &r.skin else { continue };
        if tree.is_some() {
            b.notice("tree renderers with bones drawn unskinned", 1);
            continue;
        }
        if !meshes.contains_key(&r.mesh) {
            meshes.insert(r.mesh.clone(), UnityMeshData::read(player, &r.mesh)?);
        }
        let data = &meshes[&r.mesh];
        let why = if let Some(mesh_skin) = &data.skin {
            if mesh_skin.bind_poses.len() != skin.bones.len() {
                Some("skinned renderers whose bones and bind poses differ in number drawn unskinned")
            } else if skin.bones.is_empty() {
                Some("skinned renderers without bones drawn unskinned")
            } else if skin.bones.iter().any(Option::is_none) {
                Some("skinned renderers with a null bone drawn unskinned")
            } else {
                None
            }
        } else {
            Some("skinned renderers whose mesh has no blend weights drawn unskinned")
        };
        match why {
            Some(why) => b.notice(why, 1),
            None => {
                if !limits.contains_key(&r.mesh) {
                    if levels.is_none() {
                        levels = Some(player.quality_levels()?);
                    }
                    let mesh_skin = data.skin.as_ref().expect("a candidate's mesh has a skin");
                    let limit = skin_limit(
                        levels.as_deref().unwrap_or_default(),
                        options.quality.as_deref(),
                        &data.name,
                        mesh_skin,
                    )?;
                    limits.insert(r.mesh.clone(), limit);
                }
                candidates.push((*r, skin));
            }
        }
    }
    let mut morph_renderers: BTreeMap<i64, MorphRenderer> = BTreeMap::new();
    let mut skinning: BTreeMap<ObjRef, Skinning> = BTreeMap::new();
    let mut joint_of: BTreeMap<i64, (Entity, Entity, JointId)> = BTreeMap::new();
    if !candidates.is_empty() {
        let skins: Vec<&RendererSkin> = candidates.iter().map(|(_, s)| *s).collect();
        let animators: BTreeSet<i64> = scene
            .animators_under(player, root)?
            .iter()
            .filter(|a| a.controller.is_some())
            .filter_map(|a| scene.transform_of(a.game_object))
            .collect();
        let made = build_rigs(w, &scene, &skins, &animators, root_tp)?;
        b.report.skeletons = made.skeletons;
        b.report.joints = made.joints;
        joint_of = made.of_joint;
        for ((r, _), rig) in candidates.iter().zip(made.renderers) {
            match rig {
                Some((skeleton, rig, joints)) => {
                    skinning.insert(
                        r.renderer.clone(),
                        Skinning {
                            skeleton,
                            rig,
                            joints,
                        },
                    );
                }
                None => b.notice(
                    "skinned renderers whose bones hang from different roots drawn unskinned",
                    1,
                ),
            }
        }
    }
    let mut skin_builds: BTreeMap<Entity, SkinBuild> = BTreeMap::new();
    for batch in [first, rest] {
        for (tree, r) in batch {
            let role = role_of(r);
            let mut materials = Vec::with_capacity(r.materials.len());
            for m in &r.materials {
                materials.push(match m {
                    Some(m) => Some(b.material(w, m)?),
                    None => None,
                });
            }
            // a procedural renderer's shape is in its GameObject's MonoBehaviours
            let procedural_shape = materials
                .iter()
                .flatten()
                .filter_map(|&m| w.get::<UnityMaterial>(m).and_then(|u| u.shader.as_deref()))
                .any(procedural);
            let behaviours = if procedural_shape && tree.is_none() {
                scene.behaviours_of(player, r.game_object)?
            } else {
                Vec::new()
            };
            if !meshes.contains_key(&r.mesh) {
                meshes.insert(r.mesh.clone(), UnityMeshData::read(player, &r.mesh)?);
            }
            let data = &meshes[&r.mesh];
            let (first, count) = match r.submeshes {
                Some((first, count)) => (first, count as usize),
                None => (0, data.submeshes.len()),
            };
            let skinning_of = skinning.get(&r.renderer);
            let key = (
                r.mesh.clone(),
                r.submeshes,
                role.forms.clone(),
                skinning_of.is_some(),
            );
            if !prototypes.contains_key(&key) {
                let range = first as usize..first as usize + count;
                let (read, morphs, notices) =
                    primitives_and_morphs(data, range, skinning_of.is_some().then(|| limits[&r.mesh]))?;
                b.note_mesh(notices);
                let (drawn, primitives): (Vec<u32>, Vec<_>) =
                    read.into_iter().map(|(k, p)| (k as u32, p)).unzip();
                let e = w.spawn();
                // a mesh with no submesh that has indices draws nothing: no core mesh
                if !primitives.is_empty() {
                    b.report.primitives += primitives.len();
                    w.insert(e, Mesh::new(primitives));
                    if let Some(morphs) = morphs {
                        b.report.morph_targets += morphs.targets.len();
                        w.insert(e, morphs);
                    }
                }
                w.insert(
                    e,
                    UnityMesh {
                        mesh: UnityObject::from(&r.mesh),
                        submeshes: r.submeshes,
                        drawn: drawn.clone(),
                    },
                );
                w.insert(e, mesh_source(&r.mesh, r.submeshes));
                prototypes.insert(
                    key.clone(),
                    Proto {
                        entity: e,
                        drawn,
                        variants: Vec::new(),
                        forms_made: key.2.is_empty(),
                    },
                );
            }
            let proto = prototypes.get_mut(&key).expect("inserted");
            let variant = match proto.variants.iter().position(|v| *v == materials) {
                Some(v) => v,
                None => {
                    proto.variants.push(materials.clone());
                    proto.variants.len() - 1
                }
            };
            let (entity, unmaterialed) = (
                proto.entity,
                proto
                    .drawn
                    .iter()
                    .filter(|&&k| (k - first) as usize >= materials.len())
                    .count() as u64,
            );
            b.note(
                "renderer submeshes without a material (not drawn by Unity)",
                unmaterialed,
            );
            b.note(
                "renderer materials beyond the submeshes (an extra pass over the last)",
                materials.len().saturating_sub(count) as u64,
            );
            let e = w.spawn();
            w.insert(e, UNITY.transform(&transform_of(&r.world)));
            w.insert(
                e,
                Instance {
                    prototype: entity,
                    variant: variant as u32,
                },
            );
            w.insert(e, Name(r.path.clone()));
            w.insert(e, unity_renderer(r));
            if let (Some(shapes), true) = (&data.shapes, w.has::<primase_core::Morphs>(entity)) {
                // the weights the renderer sets, then where its shapes can be found by the clips
                let weights: Vec<(MorphId, f32)> = r
                    .skin
                    .iter()
                    .flat_map(|s| s.blend_weights.iter().enumerate())
                    .filter(|(c, v)| *c < shapes.channels.len() && **v != 0.0)
                    .map(|(c, v)| (MorphId(c as u32), *v))
                    .collect();
                if r.skin
                    .as_ref()
                    .is_some_and(|s| s.blend_weights.len() > shapes.channels.len())
                {
                    b.notice("renderers with more blend shape weights than their mesh has shapes (the extra left out)", 1);
                }
                if !weights.is_empty() {
                    b.report.morph_renderers += 1;
                    w.insert(e, MorphWeights(weights));
                }
                if let Some(t) = scene.transform_of(r.game_object) {
                    morph_renderers.insert(
                        t,
                        MorphRenderer {
                            occurrence: e,
                            geometry: entity,
                            shapes: shapes
                                .channels
                                .iter()
                                .enumerate()
                                .map(|(c, ch)| (ch.name_hash, MorphId(c as u32)))
                                .collect(),
                        },
                    );
                }
            }
            if let Some(sk) = skinning_of {
                let build = skin_builds.entry(entity).or_insert_with(|| SkinBuild {
                    mesh: r.mesh.clone(),
                    bindings: Vec::new(),
                });
                let id = match build
                    .bindings
                    .iter()
                    .position(|(skeleton, joints)| *skeleton == sk.skeleton && *joints == sk.joints)
                {
                    Some(id) => id,
                    None => {
                        build.bindings.push((sk.skeleton, sk.joints.clone()));
                        build.bindings.len() - 1
                    }
                };
                w.insert(
                    e,
                    SkinInstance {
                        rig: sk.rig,
                        binding: BindingId(id as u32),
                    },
                );
                b.report.skinned_renderers += 1;
            }
            let cast = match r.cast_shadows {
                0 => Some(ShadowCast::Off),
                1 => Some(ShadowCast::On),
                2 => {
                    b.note("two-sided shadow casting written as casting", 1);
                    Some(ShadowCast::On)
                }
                3 => Some(ShadowCast::Only),
                _ => {
                    b.note("renderers with an unknown shadow casting mode, cast unknown", 1);
                    None
                }
            };
            w.insert(
                e,
                Shadows {
                    cast,
                    receive: r.receive_shadows,
                },
            );
            // state: the terrain's for a tree, then the renderer's own (its scene: the level or the tree's prefab),
            // then the LODGroups kept as placements
            let mut literals = Vec::new();
            if let Some((terrain, _)) = tree
                && let Some((path, off)) = terrain_state.get(&terrain)
            {
                literals.extend(b.literals(w, &scene, off, path));
            }
            let own = prefabs
                .get(&r.renderer.file)
                .filter(|_| tree.is_some())
                .unwrap_or(&scene);
            literals.extend(b.literals(w, own, &r.off, &r.path));
            for (group, mask, levels) in &role.lod {
                literals.push(b.lod_literal(w, group, *mask, *levels, tree));
            }
            b.condition(
                w,
                e,
                literals,
                if tree.is_some() {
                    "tree renderers"
                } else {
                    "renderers"
                },
            );
            if let Some((terrain, tree)) = tree {
                w.insert(e, UnityTreeInstance { terrain, tree });
            }
            if !behaviours.is_empty() {
                w.insert(e, UnityBehaviours(behaviours));
            }
        }
        // the coarser forms of the prototypes made so far
        for (key, p) in prototypes.iter_mut().filter(|(_, p)| !p.forms_made) {
            p.forms_made = true;
            let mut forms = Vec::with_capacity(key.2.len());
            for (level, (mesh, mats)) in key.2.iter().enumerate() {
                if !meshes.contains_key(mesh) {
                    meshes.insert(mesh.clone(), UnityMeshData::read(player, mesh)?);
                }
                let data = &meshes[mesh];
                let (read, notices) = primitives(data, 0..data.submeshes.len())?;
                b.note_mesh(notices);
                let (drawn, mut primitives): (Vec<u32>, Vec<_>) =
                    read.into_iter().map(|(k, p)| (k as u32, p)).unzip();
                // material i draws submesh i
                for (prim, &k) in primitives.iter_mut().zip(&drawn) {
                    prim.material = match mats.get(k as usize).cloned().flatten() {
                        Some(m) => Some(b.material(w, &m)?),
                        None => None,
                    };
                }
                let f = w.spawn();
                if !primitives.is_empty() {
                    w.insert(f, Mesh::new(primitives));
                }
                w.insert(
                    f,
                    UnityMesh {
                        mesh: UnityObject::from(mesh),
                        submeshes: None,
                        drawn,
                    },
                );
                w.insert(f, mesh_source(mesh, None));
                forms.push(LodForm {
                    detail: u8::try_from(level + 1).unwrap_or(u8::MAX),
                    form: f,
                });
            }
            if w.has::<Mesh>(p.entity) {
                b.report.lod_forms += forms.len();
                w.insert(
                    p.entity,
                    Lods {
                        detail: 0,
                        forms,
                        default: None,
                    },
                );
            } else {
                // core.lods needs a mesh on the prototype: the forms stay unused
                b.note("LOD 0 meshes that draw nothing (coarser forms not attached)", 1);
                for f in forms {
                    w.despawn(f.form);
                }
            }
        }
    }
    // each primitive's material from variant 0, the other variants' where they differ
    for (key, p) in &prototypes {
        let first = key.1.map_or(0, |s| s.0);
        let material = |v: &[Option<Entity>], k: u32| v.get((k - first) as usize).copied().flatten();
        let mut overrides: Vec<BTreeMap<u32, Entity>> = vec![BTreeMap::new(); p.drawn.len()];
        let mut unset = 0;
        for (i, &k) in p.drawn.iter().enumerate() {
            let base = material(&p.variants[0], k);
            for (v, list) in p.variants.iter().enumerate().skip(1) {
                match material(list, k) {
                    Some(m) if Some(m) != base => {
                        overrides[i].insert(v as u32, m);
                    }
                    None if base.is_some() => unset += 1,
                    _ => {}
                }
            }
        }
        b.note(
            "variant primitives without a material where variant 0 has one (drawn with variant 0's)",
            unset,
        );
        let Some(mesh) = w.get_mut::<Mesh>(p.entity) else {
            continue;
        };
        for (mut prim, &k) in mesh.primitives_mut().zip(&p.drawn) {
            *prim.material_mut() = material(&p.variants[0], k);
        }
        if overrides.iter().any(|o| !o.is_empty()) {
            w.insert(p.entity, MaterialVariants(overrides));
        }
    }
    // each skinned prototype's skin: the influences its streams carry, a binding per skeleton and bone list
    for (proto, build) in &skin_builds {
        let Some(mesh_skin) = meshes[&build.mesh].skin.as_ref() else {
            continue;
        };
        let primitives = w.get::<Mesh>(*proto).map_or(0, |m| m.primitives().len());
        let limit = limits[&build.mesh];
        let sets = mesh_skin.sets(limit);
        let (weights, ..) = mesh_skin.padded(limit);
        let sums_to_one = (0..weights[0].len()).all(|v| {
            let sum: f32 = weights.iter().flat_map(|set| set[v]).sum();
            (sum - 1.0).abs() <= SUM_TOLERANCE
        });
        b.notice(
            "skinned meshes whose weights do not all sum to 1 (used as stored)",
            u64::from(!sums_to_one),
        );
        let bindings = build
            .bindings
            .iter()
            .enumerate()
            .map(|(id, (skeleton, joints))| {
                let palette: Vec<Slot> = joints
                    .iter()
                    .zip(&mesh_skin.bind_poses)
                    .map(|(&joint, pose)| Slot {
                        joint,
                        inverse_bind: UNITY.transform(&transform_of(pose)),
                    })
                    .collect();
                Binding {
                    id: BindingId(id as u32),
                    skeleton: *skeleton,
                    palettes: vec![Some(palette); primitives],
                    deformation: Deformation::Linear,
                }
            })
            .collect();
        w.insert(
            *proto,
            Skin {
                influences: vec![Some(Influences::Streams { sets: sets as u8 }); primitives],
                weights: if sums_to_one {
                    WeightRule::SumsToOne
                } else {
                    WeightRule::AsStored
                },
                bindings,
            },
        );
    }
    // the clips of the Animators that animate the skeletons made: a clip lowered for each skeleton, bound to its rig
    if !joint_of.is_empty() {
        b.bind_clips(w, &scene, &joint_of, &morph_renderers, root)?;
    }
    // colliders: the scene's, then the terrain trees'
    for c in scene.colliders_under(player, root)? {
        let u = b.collider(&c)?;
        let e = w.spawn();
        w.insert(e, Name(c.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&c.world)));
        w.insert(e, u);
        let literals = b.literals(w, &scene, &c.off, &c.path);
        b.condition(w, e, literals, "colliders");
    }
    for ((terrain, tree), c) in &tree_colliders {
        let u = b.collider(c)?;
        let e = w.spawn();
        w.insert(e, Name(c.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&c.world)));
        w.insert(e, u);
        w.insert(
            e,
            UnityTreeInstance {
                terrain: *terrain,
                tree: *tree,
            },
        );
        // the terrain's state and its collider's, then the prefab collider's own
        let mut literals = Vec::new();
        if let Some((path, off)) = terrain_state.get(terrain) {
            literals.extend(b.literals(w, &scene, off, path));
            let collider_off = terrain_collider_off.get(terrain).map_or(&[][..], Vec::as_slice);
            literals.extend(b.literals(w, &scene, collider_off, path));
        }
        let prefab = prefabs.get(&c.collider.file).unwrap_or(&scene);
        literals.extend(b.literals(w, prefab, &c.off, &c.path));
        b.condition(w, e, literals, "colliders");
        b.report.tree_colliders += 1;
    }
    // a prefab has no lights, cameras or render settings of its own
    let lights = if root.is_none() {
        scene.lights(player)?
    } else {
        Vec::new()
    };
    let settings = if root.is_none() {
        scene.render_settings(player)?
    } else {
        None
    };
    let sun = settings.as_ref().and_then(|s| s.sun.clone());
    let mut sun_entity = None;
    for l in &lights {
        let e = w.spawn();
        if sun.as_ref() == Some(&l.light) {
            sun_entity = Some(e);
        }
        w.insert(e, Name(l.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&l.world)));
        w.insert(
            e,
            UnityLight {
                light: UnityObject::from(&l.light),
                kind: l.kind,
                color: l.color,
                intensity: l.intensity,
                range: l.range,
                spot_angle: l.spot_angle,
                inner_spot_angle: l.inner_spot_angle,
                shadows: l.shadows,
                color_temperature: l.color_temperature,
                bounce_intensity: l.bounce_intensity,
                light_unit: l.light_unit,
                lux_at_distance: l.lux_at_distance,
                area_size: l.area_size,
                render_mode: l.render_mode,
                baked: l.baked,
                culling_mask: l.culling_mask,
                cookie: l.cookie.as_ref().map(UnityObject::from),
            },
        );
        let literals = b.literals(w, &scene, &l.off, &l.path);
        b.condition(w, e, literals, "lights");
    }
    b.report.lights = lights.len();
    let cameras = if root.is_none() {
        scene.cameras(player)?
    } else {
        Vec::new()
    };
    for c in &cameras {
        let e = w.spawn();
        w.insert(e, Name(c.path.clone()));
        w.insert(e, UNITY.transform(&transform_of(&c.world)));
        w.insert(
            e,
            UnityCamera {
                camera: UnityObject::from(&c.camera),
                culling_mask: c.culling_mask,
                target_texture: c.target_texture.as_ref().map(UnityObject::from),
            },
        );
        let literals = b.literals(w, &scene, &c.off, &c.path);
        b.condition(w, e, literals, "cameras");
    }
    b.report.cameras = cameras.len();
    b.report.sun_named = sun.is_some();
    // the pipeline asset's settings belong to the scene's rendering, not to a prefab's
    let urp = if root.is_none() {
        urp_settings(player, options.quality.as_deref()).unwrap_or_else(|why| {
            b.notice(
                &format!("Universal Render Pipeline asset settings not read: {why}"),
                1,
            );
            None
        })
    } else {
        None
    };
    let shadow_quality = if root.is_none() {
        shadow_quality(player, options.quality.as_deref()).unwrap_or_else(|why| {
            b.notice(&format!("quality level shadow settings not read: {why}"), 1);
            None
        })
    } else {
        None
    };
    w.insert_resource(UnityRenderSettings {
        sun: sun_entity,
        pipeline: player.render_pipeline()?,
        fog: settings.as_ref().and_then(|s| s.fog.clone()),
        ambient: settings.and_then(|s| s.ambient),
        urp,
        shadow_quality,
    });
    b.report.renderers = renderers.iter().filter(|r| !role_of(r).folded).count();
    b.report.tree_renderers = trees.iter().filter(|(_, r)| !role_of(r).folded).count();
    b.report.prototypes = prototypes.len();
    Ok(b.report)
}

/// The shadow settings of the quality level `quality`, or of every level where they agree and none is named.
fn shadow_quality(player: &Player, quality: Option<&str>) -> Result<Option<UnityShadowQuality>> {
    let levels = player.quality_levels()?;
    let chosen: Vec<&QualityLevel> = match quality {
        Some(name) => vec![
            levels
                .iter()
                .find(|l| l.name == name)
                .ok_or_else(|| invalid(format!("no quality level named {name}")))?,
        ],
        None => levels.iter().collect(),
    };
    let of = |l: &QualityLevel| UnityShadowQuality {
        shadows: l.shadows,
        distance: l.shadow_distance,
        cascades: l.shadow_cascades,
    };
    let Some(first) = chosen.first().map(|l| of(l)) else {
        return Ok(None);
    };
    Ok(chosen.iter().all(|l| of(l) == first).then_some(first))
}

/// The settings of the Universal Render Pipeline asset the quality level `quality` draws with, where it is one.
fn urp_settings(player: &Player, quality: Option<&str>) -> Result<Option<UnityUrpSettings>> {
    let Some(asset) = player.urp_asset(quality)? else {
        return Ok(None);
    };
    let (_, bytes) = player.behaviour(&asset)?;
    unity_formats::pipeline::read_urp_settings(&bytes).map(Some)
}

/// The five fields of a PhysicMaterial, by the two names a type tree gives them: Unity 6 and its kin prefix them
/// (`m_DynamicFriction`), 2019.4 does not (`dynamicFriction`).
const PHYSIC_FIELDS: [(&str, &str); 5] = [
    ("m_DynamicFriction", "dynamicFriction"),
    ("m_StaticFriction", "staticFriction"),
    ("m_Bounciness", "bounciness"),
    ("m_FrictionCombine", "frictionCombine"),
    ("m_BounceCombine", "bounceCombine"),
];

/// A PhysicMaterial object (`object` names it for errors). Every field is required: `m_Name` and the five of
/// [`PHYSIC_FIELDS`], all under one family of names. A material with both families, or a partial family, or a field of
/// another type, is an error naming the object and the fields (never read as frictionless).
fn read_physic_material(v: &Value, object: &str) -> Result<UnityPhysicMaterial> {
    let fail = |what: String| crate::invalid(format!("PhysicMaterial {object}: {what}"));
    let present = |names: Vec<&'static str>| -> Vec<&'static str> {
        names.into_iter().filter(|k| v.get(k).is_some()).collect()
    };
    let prefixed = present(PHYSIC_FIELDS.iter().map(|f| f.0).collect());
    let plain = present(PHYSIC_FIELDS.iter().map(|f| f.1).collect());
    let family: Vec<&str> = match (prefixed.len(), plain.len()) {
        (5, 0) => PHYSIC_FIELDS.iter().map(|f| f.0).collect(),
        (0, 5) => PHYSIC_FIELDS.iter().map(|f| f.1).collect(),
        (p, q) => {
            return Err(fail(format!(
                "not one complete family of fields: has {prefixed:?} ({p} of 5 m_-prefixed) and {plain:?} ({q} of 5 unprefixed)"
            )));
        }
    };
    let f = |k: &str| {
        v.get(k)
            .and_then(Value::as_f32)
            .ok_or_else(|| fail(format!("{k} is not a number")))
    };
    let i = |k: &str| {
        v.get(k)
            .and_then(Value::as_i64)
            .ok_or_else(|| fail(format!("{k} is not an integer")))
    };
    Ok(UnityPhysicMaterial {
        name: v
            .get("m_Name")
            .and_then(Value::as_str)
            .ok_or_else(|| fail("no string m_Name".into()))?
            .to_owned(),
        dynamic_friction: f(family[0])?,
        static_friction: f(family[1])?,
        bounciness: f(family[2])?,
        friction_combine: i(family[3])?,
        bounce_combine: i(family[4])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_formats::mesh::Blend;

    /// A quality level the player lacks is refused listing the levels as the option takes them: each name quoted,
    /// what it draws of a skin beside it.
    #[test]
    fn an_unknown_quality_level_lists_the_names_the_option_takes() {
        let level = |name: &str, skin_weights| QualityLevel {
            name: name.into(),
            skin_weights,
            shadows: 0,
            shadow_distance: 0.,
            shadow_cascades: 0,
        };
        let levels = [
            level("PC High", SkinWeights::Unlimited),
            level("PC Low", SkinWeights::Bones(2)),
        ];
        let skin = MeshSkin {
            channel: Blend::default(),
            unlimited: None,
            bind_poses: Vec::new(),
        };
        let error = skin_limit(&levels, Some("PC High (unlimited)"), "body", &skin)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(
                r#"is not one of the player's: "PC High" (skin weights unlimited), "PC Low" (skin weights 2 bones)"#
            ),
            "{error}"
        );
        assert_eq!(
            skin_limit(&levels, Some("PC Low"), "body", &skin).unwrap(),
            SkinWeights::Bones(2)
        );
    }

    fn fields(names: &[&str]) -> Value {
        let value = |k: &str| match k {
            "m_Name" => Value::String("Grippy".into()),
            "m_FrictionCombine" | "frictionCombine" => Value::Int(1),
            "m_BounceCombine" | "bounceCombine" => Value::Int(2),
            "m_DynamicFriction" | "dynamicFriction" => Value::Float(0.25),
            "m_StaticFriction" | "staticFriction" => Value::Float(0.5),
            _ => Value::Float(0.75),
        };
        Value::Struct(names.iter().map(|k| ((*k).into(), value(k))).collect())
    }

    const PREFIXED: [&str; 6] = [
        "m_Name",
        "m_DynamicFriction",
        "m_StaticFriction",
        "m_Bounciness",
        "m_FrictionCombine",
        "m_BounceCombine",
    ];
    const PLAIN: [&str; 6] = [
        "m_Name",
        "dynamicFriction",
        "staticFriction",
        "bounciness",
        "frictionCombine",
        "bounceCombine",
    ];

    /// A material with every field under the `m_` names (Unity 6) or under the plain ones (2019.4) reads the same.
    #[test]
    fn physic_materials_read_under_either_family_of_names() {
        let want = UnityPhysicMaterial {
            name: "Grippy".into(),
            dynamic_friction: 0.25,
            static_friction: 0.5,
            bounciness: 0.75,
            friction_combine: 1,
            bounce_combine: 2,
        };
        assert_eq!(read_physic_material(&fields(&PREFIXED), "f:1").unwrap(), want);
        assert_eq!(read_physic_material(&fields(&PLAIN), "f:1").unwrap(), want);
    }

    /// Both families, a partial family, a missing name or a field of another type is an error naming the object.
    #[test]
    fn physic_materials_refuse_mixed_partial_and_mistyped_fields() {
        let both: Vec<&str> = PREFIXED.iter().chain(&PLAIN[1..]).copied().collect();
        let mixed = [
            "m_Name",
            "m_DynamicFriction",
            "staticFriction",
            "bounciness",
            "m_FrictionCombine",
            "bounceCombine",
        ];
        for (names, why) in [
            (both, "both families"),
            (mixed.to_vec(), "a mix"),
            (PREFIXED[..5].to_vec(), "a partial prefixed family"),
            (PLAIN[1..3].to_vec(), "a partial plain family"),
            (vec!["m_Name"], "no field"),
        ] {
            let e = read_physic_material(&fields(&names), "level1:42")
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("level1:42") && e.contains("not one complete family"),
                "{why}: {e}"
            );
        }
        let e = read_physic_material(&fields(&PLAIN[1..]), "level1:42")
            .unwrap_err()
            .to_string();
        assert!(e.contains("level1:42") && e.contains("m_Name"), "{e}");
        let mut v = fields(&PLAIN);
        let Value::Struct(f) = &mut v else { unreachable!() };
        f.iter_mut().find(|(k, _)| &**k == "bounciness").unwrap().1 = Value::String("x".into());
        let e = read_physic_material(&v, "level1:42").unwrap_err().to_string();
        assert!(
            e.contains("level1:42") && e.contains("bounciness is not a number"),
            "{e}"
        );
    }

    fn obj(pid: i64) -> ObjRef {
        ObjRef {
            file: "level1".into(),
            path_id: pid,
        }
    }

    fn renderer(pid: i64, mesh: i64) -> Renderer {
        Renderer {
            renderer: obj(pid),
            game_object: pid + 1000,
            path: format!("r{pid}"),
            layer: 0,
            mesh: obj(mesh),
            materials: vec![Some(obj(mesh + 500))],
            world: IDENTITY,
            submeshes: None,
            cast_shadows: 1,
            receive_shadows: Some(true),
            off: Vec::new(),
            skin: None,
        }
    }

    fn group(pid: i64, levels: Vec<Vec<i64>>) -> LodGroup {
        LodGroup {
            group: obj(pid),
            path: format!("g{pid}"),
            enabled: true,
            off: Vec::new(),
            levels,
        }
    }

    /// The report as JSON: counts as numbers, every map keyed by its reason, nothing dropped.
    #[test]
    fn the_report_serialises_with_its_maps_keyed_by_reason() {
        let mut report = LevelReport {
            renderers: 3,
            sun_named: true,
            ..LevelReport::default()
        };
        report.notices.insert("quads converted".into(), 2);
        report.colliders_not_colliding.insert("trigger", 5);
        report.skipped.insert("no mesh", 1);
        let json = report.to_json();
        assert_eq!(json["renderers"], 3);
        assert_eq!(json["sun_named"], true);
        assert_eq!(json["notices"]["quads converted"], 2);
        assert_eq!(json["colliders_not_colliding"]["trigger"], 5);
        assert_eq!(json["skipped"]["no mesh"], 1);
        // every field of the struct is there
        assert_eq!(json.as_object().map(serde_json::Map::len), Some(37));
    }

    #[test]
    fn lod_groups_fold_into_forms_only_when_every_level_matches_lod_0() {
        let mut report = LevelReport::default();
        // a fitting group: one renderer per level, same matrix, state, shadows and layer
        let rs = vec![renderer(1, 10), renderer(2, 11), renderer(3, 12)];
        let roles = lod_roles(&rs, &[group(100, vec![vec![1], vec![2], vec![3]])], &mut report);
        assert_eq!(roles[&obj(1)].forms.len(), 2);
        assert_eq!(roles[&obj(1)].forms[0].0, obj(11));
        assert!(roles[&obj(2)].folded && roles[&obj(3)].folded);
        assert!(roles[&obj(2)].coarser_only && !roles[&obj(1)].coarser_only);
        assert_eq!(report.lod_groups["folded into core.lods"], 1);

        // each mismatch keeps the levels as placements under the group's axis
        let mismatches: [fn(&mut Renderer); 6] = [
            |r| r.world[0][3] = 1.0,
            |r| r.off = vec![OffFlag::Inactive(7)],
            |r| r.cast_shadows = 0,
            |r| r.receive_shadows = Some(false),
            |r| r.layer = 3,
            |r| r.submeshes = Some((0, 1)),
        ];
        for m in mismatches {
            let mut report = LevelReport::default();
            let mut rs = vec![renderer(1, 10), renderer(2, 11)];
            m(&mut rs[1]);
            let roles = lod_roles(&rs, &[group(100, vec![vec![1], vec![2]])], &mut report);
            assert!(roles[&obj(1)].forms.is_empty() && !roles[&obj(2)].folded);
            assert_eq!(roles[&obj(1)].lod, vec![(obj(100), 0b01, 2)]);
            assert_eq!(roles[&obj(2)].lod, vec![(obj(100), 0b10, 2)]);
            assert_eq!(report.lod_groups["kept as placements under a unity.lod axis"], 1);
        }

        // two renderers in a level, or a renderer in two levels: the axis, its mask the levels it is in
        let mut report = LevelReport::default();
        let rs = vec![renderer(1, 10), renderer(2, 11), renderer(3, 12)];
        let roles = lod_roles(&rs, &[group(100, vec![vec![1, 3], vec![2, 3]])], &mut report);
        assert_eq!(roles[&obj(3)].lod, vec![(obj(100), 0b11, 2)]);
        assert!(!roles[&obj(3)].coarser_only);

        // a disabled group and a one-level group choose nothing
        let mut report = LevelReport::default();
        let mut off = group(100, vec![vec![1], vec![2]]);
        off.enabled = false;
        let roles = lod_roles(
            &[renderer(1, 10), renderer(2, 11), renderer(3, 12)],
            &[off, group(101, vec![vec![3]])],
            &mut report,
        );
        assert!(
            roles
                .values()
                .all(|r| r.forms.is_empty() && !r.folded && r.lod.is_empty())
        );
        assert_eq!(report.lod_groups["disabled (every level draws)"], 1);
        assert_eq!(report.lod_groups["one level (nothing to choose)"], 1);
    }
}
