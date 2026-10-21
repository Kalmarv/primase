//! A `SkeletonAsset` (the render skeleton a costume's MeshSet is drawn with,
//! `characters/maincharacters/common/animbase/animbase_default_renderskeleton`, 386 bones) as a `core.skeleton`, with
//! the asset's other facts in `skate.skeleton_asset`.
//!
//! Each bone is joint `i` (its index: MeshSet palettes name bones by index), named by `BoneNames[i]`, its parent
//! `Hierarchy[i]` (-1: a root), its rest `LocalPose[i]` as stored (`Rest::Affine`). The asset also stores each bone's
//! model pose and its inverse (`ModelPose`, `InverseModelPose`: the inverse binds a MeshSet's skin uses), its name
//! hash, and the server skeleton and gameplay bone maps: kept as read in [`SkateSkeletonAsset`]. Nothing is derived
//! or checked against anything else here; [`check_poses`] states how the stored poses relate.

use primase_core::{
    Component, Entity, Joint, JointId, JsonError, Name, Rest, Skeleton, Source, Transform, World,
};
use primase_frostbite::ebx::{Document, Value};
use serde::Serialize;
use serde_json::Value as Json;

use super::linear_transform;

/// `skate.skeleton_asset`: a `SkeletonAsset`'s facts beside its bones (see the module documentation), one entry a
/// bone where the asset has one a bone.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateSkeletonAsset {
    /// The asset's `Name`.
    pub name: String,
    pub bone_name_hashes: Vec<u32>,
    /// `ModelPose`: each bone's rest in the skeleton's space.
    pub model_pose: Vec<Transform>,
    /// `InverseModelPose`: its inverse, the inverse bind a MeshSet's skin uses.
    pub inverse_model_pose: Vec<Transform>,
    pub server_hierarchy: Vec<i64>,
    pub server_skeleton_to_skeleton: Vec<i64>,
    pub skeleton_to_server_skeleton: Vec<i64>,
    /// `GameplayBonesToSkeleton`: (gameplay bone hash, bone index).
    pub gameplay_bones: Vec<(i64, i64)>,
    /// `GameplayBonesToServerSkeleton`: (gameplay bone hash, server bone index).
    pub gameplay_bones_server: Vec<(i64, i64)>,
}

impl Component for SkateSkeletonAsset {
    const NAME: &'static str = "skate.skeleton_asset";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// The name and counts; the poses are bulk.
    fn to_json(&self) -> Result<Option<Json>, JsonError> {
        Ok(Some(serde_json::json!({
            "name": self.name,
            "bones": self.bone_name_hashes.len(),
            "server_bones": self.server_hierarchy.len(),
            "gameplay_bones": self.gameplay_bones.len(),
        })))
    }
    /// One entry a bone of the entity's `core.skeleton` in every per-bone list.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let n = w
            .get::<Skeleton>(e)
            .ok_or("skate.skeleton_asset without a core.skeleton")?
            .joints
            .len();
        for (what, len) in [
            ("name hashes", self.bone_name_hashes.len()),
            ("model poses", self.model_pose.len()),
            ("inverse model poses", self.inverse_model_pose.len()),
            (
                "skeleton-to-server entries",
                self.skeleton_to_server_skeleton.len(),
            ),
        ] {
            if len != n {
                return Err(format!("{len} {what} for {n} bones"));
            }
        }
        Ok(())
    }
}

fn array<'a>(o: &'a primase_frostbite::ebx::Object, field: &str) -> Result<&'a [Value], String> {
    match o.field(field) {
        Some(Value::Array(a, _)) => Ok(a),
        _ => Err(format!("SkeletonAsset has no array {field}")),
    }
}

fn int(v: &Value, what: &str) -> Result<i64, String> {
    match *v {
        Value::Int(i) => Ok(i),
        Value::UInt(u) => i64::try_from(u).map_err(|_| format!("{what} {u} is out of range")),
        _ => Err(format!("{what} is not an integer")),
    }
}

fn ints(o: &primase_frostbite::ebx::Object, field: &str) -> Result<Vec<i64>, String> {
    array(o, field)?
        .iter()
        .enumerate()
        .map(|(i, v)| int(v, &format!("{field}[{i}]")))
        .collect()
}

fn poses(o: &primase_frostbite::ebx::Object, field: &str) -> Result<Vec<Transform>, String> {
    array(o, field)?
        .iter()
        .enumerate()
        .map(|(i, v)| linear_transform(v, &format!("{field}[{i}]")))
        .collect()
}

fn gameplay(o: &primase_frostbite::ebx::Object, field: &str) -> Result<Vec<(i64, i64)>, String> {
    array(o, field)?
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let Value::Object(g) = v else {
                return Err(format!("{field}[{i}] is not an object"));
            };
            let get = |k: &str| {
                g.field(k)
                    .ok_or_else(|| format!("{field}[{i}] has no {k}"))
                    .and_then(|x| int(x, &format!("{field}[{i}].{k}")))
            };
            Ok((get("GameplayBone")?, get("BoneIndex")?))
        })
        .collect()
}

/// Read the `SkeletonAsset` document `doc` into `world` as one entity (its `core.skeleton`, `core.name` the asset's
/// name, `core.source` `source`, and `skate.skeleton_asset`).
pub fn read_skeleton(doc: &Document, source: &str, world: &mut World) -> Result<Entity, String> {
    if doc.root_type != "SkeletonAsset" {
        return Err(format!(
            "the document's root is a {}, not a SkeletonAsset",
            doc.root_type
        ));
    }
    let o = doc
        .instances
        .first()
        .and_then(|i| i.object.as_ref())
        .ok_or("the SkeletonAsset instance is not read")?;
    let name = match o.field("Name") {
        Some(Value::Str(s)) => String::from_utf8_lossy(s).into_owned(),
        _ => return Err("SkeletonAsset has no Name".into()),
    };
    let names: Vec<String> = array(o, "BoneNames")?
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::Str(s) => Ok(String::from_utf8_lossy(s).into_owned()),
            _ => Err(format!("BoneNames[{i}] is not a string")),
        })
        .collect::<Result<_, String>>()?;
    let hierarchy = ints(o, "Hierarchy")?;
    let local = poses(o, "LocalPose")?;
    let n = names.len();
    if hierarchy.len() != n || local.len() != n {
        return Err(format!(
            "{n} bone names, {} parents, {} local poses",
            hierarchy.len(),
            local.len()
        ));
    }
    let joints = (0..n)
        .map(|i| {
            let parent = match hierarchy[i] {
                -1 => None,
                p if (0..n as i64).contains(&p) && p != i as i64 => Some(JointId(p as u32)),
                p => return Err(format!("bone {i}'s parent {p} is not a bone")),
            };
            Ok(Joint {
                id: JointId(i as u32),
                name: names[i].clone(),
                parent,
                rest: Rest::Affine(local[i]),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let asset = SkateSkeletonAsset {
        name: name.clone(),
        bone_name_hashes: ints(o, "BoneNameHashes")?
            .into_iter()
            .map(|h| u32::try_from(h).map_err(|_| format!("bone name hash {h} is not 32-bit")))
            .collect::<Result<_, _>>()?,
        model_pose: poses(o, "ModelPose")?,
        inverse_model_pose: poses(o, "InverseModelPose")?,
        server_hierarchy: ints(o, "ServerHierarchy")?,
        server_skeleton_to_skeleton: ints(o, "ServerSkeletonToSkeletonMap")?,
        skeleton_to_server_skeleton: ints(o, "SkeletonToServerSkeletonMap")?,
        gameplay_bones: gameplay(o, "GameplayBonesToSkeleton")?,
        gameplay_bones_server: gameplay(o, "GameplayBonesToServerSkeleton")?,
    };
    let e = world.spawn();
    world.insert(e, Skeleton { joints });
    world.insert(e, asset);
    world.insert(e, Name(name));
    world.insert(e, Source(source.to_owned()));
    Ok(e)
}

/// How far, at most, the stored poses are from these relations, over every bone: the model pose is the parent's model
/// pose composed with the local pose (a root's is its local pose), and the inverse model pose composed with the model
/// pose is the identity. (Largest entry difference of each, composed in f64.)
pub fn check_poses(skeleton: &Skeleton, asset: &SkateSkeletonAsset) -> (f64, f64) {
    use primase_animation::pose::{affine, compose};
    let diff = |a: &[[f64; 3]; 4], b: &[[f64; 3]; 4]| {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max)
    };
    let identity = affine(&Transform::IDENTITY);
    let (mut chain, mut inverse) = (0.0f64, 0.0f64);
    for (i, j) in skeleton.joints.iter().enumerate() {
        let local = affine(&j.rest.matrix());
        let model = match j.parent {
            Some(p) => compose(&affine(&asset.model_pose[p.0 as usize]), &local),
            None => local,
        };
        let stored = affine(&asset.model_pose[i]);
        chain = chain.max(diff(&model, &stored));
        inverse = inverse.max(diff(
            &compose(&affine(&asset.inverse_model_pose[i]), &stored),
            &identity,
        ));
    }
    (chain, inverse)
}
