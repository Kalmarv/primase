//! skate.'s animation rig as the game stores it: a `RigAsset` (`Animation/Dingo/AnimBase_Default_Rig`, in
//! `Win32/globals.toc`'s bundle `win32/configurations/gameconfigurations/delmargameconfiguration`), the documents it
//! imports from that bundle, and what they state:
//! - its skeleton, an `AntSkeletonAsset`: each joint's name (an offset into the asset's `NameTable` of characters,
//!   ended by a zero), name hash, parent (-1: a root) and type;
//! - its degrees of freedom (DOFs): `DofIds`, cut into one run a `DofSets` entry by `DofSetIdIndices`; each set a
//!   `DofSetAsset` whose `LayoutData` (u64 words, little-endian bytes) holds a 32-byte header (u32 4: where its
//!   strings start) and one 32-byte entry a DOF, in the run's order (u32 3: its name's offset from the strings,
//!   ended by a zero). A DOF's name names what it drives: `<joint>.q` a joint's rotation, `<joint>.t` its
//!   translation; others are the rig's scalars (kept by name);
//! - its default pose, the `DefaultPoseAsset` it points at: values by DOF id (`FloatValues` / `FloatDofIds`,
//!   `Vector3Values`, `Vector4Values`, `IntValues`, each with its ids).
//!
//! Read as stored: nothing is derived here, and nothing is checked against the render skeleton (a different
//! skeleton: [`super::skeleton`]).

use std::collections::BTreeMap;

use frostbite_formats::ebx::{Document, Object, Pointer, Value};
use primase_core::{Component, Entity, Joint, JointId, JsonError, Name, Rest, Skeleton, World};
use serde::Serialize;
use serde_json::Value as Json;

/// One joint of the animation skeleton.
#[derive(Clone, Debug, PartialEq)]
pub struct AntJoint {
    pub name: String,
    pub name_hash: u32,
    /// -1 for a root.
    pub parent: i64,
    pub kind: i64,
}

/// A default pose value.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum DefaultValue {
    Float(f32),
    Vector3([f32; 3]),
    Vector4([f32; 4]),
    Int(i64),
}

/// The rig as stored (see the module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimRig {
    pub name: String,
    pub skeleton_name: String,
    pub joints: Vec<AntJoint>,
    /// DOF id -> its name.
    pub dofs: BTreeMap<u32, String>,
    /// DOF id -> its default value.
    pub defaults: BTreeMap<u32, DefaultValue>,
}

fn object<'a>(doc: &'a Document, i: usize, what: &str) -> Result<&'a Object, String> {
    doc.instances
        .get(i)
        .and_then(|x| x.object.as_ref())
        .ok_or_else(|| format!("{what}: instance {i} is not read"))
}

fn array<'a>(o: &'a Object, field: &str, what: &str) -> Result<&'a [Value], String> {
    match o.field(field) {
        Some(Value::Array(a, _)) => Ok(a),
        _ => Err(format!("{what} has no array {field}")),
    }
}

fn int(v: &Value, what: &str) -> Result<i64, String> {
    match *v {
        Value::Int(i) => Ok(i),
        Value::UInt(u) => i64::try_from(u).map_err(|_| format!("{what}: {u} is out of range")),
        _ => Err(format!("{what} is not an integer")),
    }
}

fn u32_of(v: &Value, what: &str) -> Result<u32, String> {
    u32::try_from(int(v, what)?).map_err(|_| format!("{what} is not a u32"))
}

fn float(v: &Value, what: &str) -> Result<f32, String> {
    match *v {
        Value::Float(x) => Ok(x as f32),
        _ => Err(format!("{what} is not a float")),
    }
}

fn components<const N: usize>(v: &Value, what: &str) -> Result<[f32; N], String> {
    let Value::Object(o) = v else {
        return Err(format!("{what} is not a vector"));
    };
    let keys = ["x", "y", "z", "w"];
    let mut out = [0.0; N];
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = float(
            o.field(keys[k])
                .ok_or_else(|| format!("{what} has no {}", keys[k]))?,
            what,
        )?;
    }
    Ok(out)
}

fn text(v: &Value, what: &str) -> Result<String, String> {
    match v {
        Value::Str(s) => Ok(String::from_utf8_lossy(s).into_owned()),
        _ => Err(format!("{what} is not a string")),
    }
}

/// Resolves an external pointer `v` of document `doc` to the document it names (`what` names it in errors): the
/// reader's I/O ([`crate::read::game`]), so this module reads documents only.
pub type Imports<'a> = dyn Fn(&Document, &Value, &str) -> Result<Document, String> + 'a;

/// The names of a `DofSetAsset`'s DOFs, in order.
fn dof_names(set: &Document, what: &str) -> Result<Vec<String>, String> {
    let o = object(set, 0, what)?;
    let bytes: Vec<u8> = array(o, "LayoutData", what)?
        .iter()
        .enumerate()
        .map(|(i, w)| match w {
            // the words are raw bytes; the game's field type is a signed 64-bit integer
            Value::Int(x) => Ok(x.to_le_bytes()),
            Value::UInt(u) => Ok(u.to_le_bytes()),
            _ => Err(format!("{what}: LayoutData[{i}] is not a 64-bit integer")),
        })
        .collect::<Result<Vec<_>, String>>()?
        .concat();
    let u = |at: usize| -> Result<usize, String> {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(|| format!("{what}: LayoutData ends before byte {at}"))
    };
    let strings = u(16)?;
    if strings < 32 || (strings - 32) % 32 != 0 || strings > bytes.len() {
        return Err(format!("{what}: strings at {strings} in {} bytes", bytes.len()));
    }
    (0..(strings - 32) / 32)
        .map(|e| {
            let at = strings + u(32 + e * 32 + 12)?;
            let end = bytes
                .get(at..)
                .and_then(|r| r.iter().position(|&c| c == 0))
                .ok_or_else(|| format!("{what}: DOF {e}'s name runs past the layout"))?;
            Ok(String::from_utf8_lossy(&bytes[at..at + end]).into_owned())
        })
        .collect()
}

/// The rig the `RigAsset` document `doc` states (see the module documentation), the documents it imports resolved by
/// `imported`.
pub fn rig_from(doc: &Document, imported: &Imports<'_>) -> Result<AnimRig, String> {
    if doc.root_type != "RigAsset" {
        return Err(format!("the document is a {}, not a RigAsset", doc.root_type));
    }
    let rig = object(doc, 0, "RigAsset")?;
    let name = text(rig.field("Name").ok_or("RigAsset has no Name")?, "RigAsset.Name")?;

    // the skeleton
    let skel_doc = imported(
        doc,
        rig.field("SkeletonAsset")
            .ok_or("RigAsset has no SkeletonAsset")?,
        "RigAsset.SkeletonAsset",
    )?;
    let skel = object(&skel_doc, 0, "AntSkeletonAsset")?;
    let skeleton_name = text(
        skel.field("Name").ok_or("AntSkeletonAsset has no Name")?,
        "AntSkeletonAsset.Name",
    )?;
    let table: Vec<u8> = array(skel, "NameTable", "AntSkeletonAsset")?
        .iter()
        .enumerate()
        .map(|(i, c)| u8::try_from(int(c, "NameTable")?).map_err(|_| format!("NameTable[{i}] is not a byte")))
        .collect::<Result<_, String>>()?;
    let joints = array(skel, "Joints", "AntSkeletonAsset")?
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let what = format!("Joints[{i}]");
            let Value::Object(j) = j else {
                return Err(format!("{what} is not an object"));
            };
            let get = |k: &str| j.field(k).ok_or_else(|| format!("{what} has no {k}"));
            let at = int(get("Name")?, &what)? as usize;
            let end = table
                .get(at..)
                .and_then(|r| r.iter().position(|&c| c == 0))
                .ok_or_else(|| format!("{what}: its name runs past the NameTable"))?;
            Ok(AntJoint {
                name: String::from_utf8_lossy(&table[at..at + end]).into_owned(),
                name_hash: u32_of(get("NameHash")?, &what)?,
                parent: int(get("Parent")?, &what)?,
                kind: int(get("Type")?, &what)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    for (i, j) in joints.iter().enumerate() {
        if j.parent != -1 && !(0..joints.len() as i64).contains(&j.parent) {
            return Err(format!("joint {i}'s parent {} is not a joint", j.parent));
        }
    }

    // the DOFs
    let ids: Vec<u32> = array(rig, "DofIds", "RigAsset")?
        .iter()
        .map(|v| u32_of(v, "DofIds"))
        .collect::<Result<_, _>>()?;
    let starts: Vec<usize> = array(rig, "DofSetIdIndices", "RigAsset")?
        .iter()
        .map(|v| int(v, "DofSetIdIndices").map(|x| x as usize))
        .collect::<Result<_, _>>()?;
    let sets = array(rig, "DofSets", "RigAsset")?;
    if sets.len() != starts.len() {
        return Err(format!("{} DOF sets, {} start indices", sets.len(), starts.len()));
    }
    let mut dofs = BTreeMap::new();
    for (i, set) in sets.iter().enumerate() {
        let what = format!("RigAsset.DofSets[{i}]");
        let names = dof_names(&imported(doc, set, &what)?, &what)?;
        let end = starts.get(i + 1).copied().unwrap_or(ids.len());
        let run = ids
            .get(starts[i]..end)
            .ok_or_else(|| format!("{what}: ids {}..{end} of {}", starts[i], ids.len()))?;
        if run.len() != names.len() {
            return Err(format!("{what}: {} ids for {} DOFs", run.len(), names.len()));
        }
        for (&id, name) in run.iter().zip(names) {
            if let Some(other) = dofs.insert(id, name.clone())
                && other != name
            {
                return Err(format!("DOF id {id} names both {other} and {name}"));
            }
        }
    }

    // the default pose
    let Some(Value::Pointer(Pointer::Internal(k))) = rig.field("DefaultPoseAsset") else {
        return Err("RigAsset's DefaultPoseAsset is not in its document".into());
    };
    let pose = object(doc, *k, "DefaultPoseAsset")?;
    let mut defaults = BTreeMap::new();
    let mut take = |values: &str, ids: &str, f: &dyn Fn(&Value, &str) -> Result<DefaultValue, String>| {
        let (v, d) = (
            array(pose, values, "DefaultPoseAsset")?,
            array(pose, ids, "DefaultPoseAsset")?,
        );
        if v.len() != d.len() {
            return Err(format!("{} {values} for {} {ids}", v.len(), d.len()));
        }
        for (x, id) in v.iter().zip(d) {
            let id = u32_of(id, ids)?;
            if defaults.insert(id, f(x, values)?).is_some() {
                return Err(format!("DOF id {id} has two default values"));
            }
        }
        Ok::<(), String>(())
    };
    take("FloatValues", "FloatDofIds", &|x, w| {
        float(x, w).map(DefaultValue::Float)
    })?;
    take("Vector3Values", "Vector3DofIds", &|x, w| {
        components::<3>(x, w).map(DefaultValue::Vector3)
    })?;
    take("Vector4Values", "Vector4DofIds", &|x, w| {
        components::<4>(x, w).map(DefaultValue::Vector4)
    })?;
    take("IntValues", "IntDofIds", &|x, w| int(x, w).map(DefaultValue::Int))?;

    Ok(AnimRig {
        name,
        skeleton_name,
        joints,
        dofs,
        defaults,
    })
}

/// What a DOF drives, by its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum DofTarget {
    /// `<joint>.q`, `<joint>.t`, `<joint>.s`: a joint's local rotation, translation or scale.
    Rotation(JointId),
    Translation(JointId),
    Scale(JointId),
    /// Any other name: one of the rig's other DOFs (its scalars, and values of things that are not joints, as IK
    /// effectors' positions and rotations), its meaning the game's.
    Other(String),
}

/// One DOF of the rig.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateDof {
    pub id: u32,
    pub name: String,
    pub target: DofTarget,
}

/// `skate.anim_rig`, on the entity holding the animation skeleton (`core.skeleton`): the rig's name, its DOFs (what
/// each drives) and the default values of those that drive no joint (the joints' defaults are the skeleton's rests),
/// by DOF id.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkateAnimRig {
    pub name: String,
    pub dofs: Vec<SkateDof>,
    pub other_defaults: Vec<(u32, DefaultValue)>,
}

impl SkateAnimRig {
    /// The DOF with id `id`.
    pub fn dof(&self, id: u32) -> Option<&SkateDof> {
        self.dofs
            .binary_search_by_key(&id, |d| d.id)
            .ok()
            .map(|i| &self.dofs[i])
    }
}

impl Component for SkateAnimRig {
    const NAME: &'static str = "skate.anim_rig";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// The name and counts.
    fn to_json(&self) -> Result<Option<Json>, JsonError> {
        Ok(Some(serde_json::json!({
            "name": self.name,
            "dofs": self.dofs.len(),
            "other_defaults": self.other_defaults.len(),
        })))
    }
    /// DOFs in id order, each once; each joint target a joint of the entity's skeleton.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let skeleton = w
            .get::<Skeleton>(e)
            .ok_or("skate.anim_rig without a core.skeleton")?;
        if self.dofs.windows(2).any(|p| p[0].id >= p[1].id) {
            return Err("DOFs not in increasing id order".into());
        }
        for d in &self.dofs {
            if let DofTarget::Rotation(j) | DofTarget::Translation(j) | DofTarget::Scale(j) = d.target
                && skeleton.index_of(j).is_none()
            {
                return Err(format!("DOF {} drives joint {j:?}, not in the skeleton", d.name));
            }
        }
        Ok(())
    }
}

/// `rig` into `world` as one entity: its skeleton as `core.skeleton` (joint `i` the `AntSkeletonAsset`'s joint `i`,
/// named by it, its parent its parent, its rest the default pose's `<joint>.t`, `.q` and `.s` as `Rest::Trs`; a joint
/// without all three defaults is refused), `skate.anim_rig`, `core.name` the rig's and `core.source` `source`.
pub fn insert_rig(rig: &AnimRig, source: &str, world: &mut World) -> Result<Entity, String> {
    let by_name: BTreeMap<&str, u32> = rig.dofs.iter().map(|(id, n)| (n.as_str(), *id)).collect();
    let joint_of: BTreeMap<&str, JointId> = rig
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| (j.name.as_str(), JointId(i as u32)))
        .collect();
    if joint_of.len() != rig.joints.len() {
        return Err("two joints share a name: DOF names would not name one joint".into());
    }
    let default = |joint: &str, k: &str| {
        by_name
            .get(format!("{joint}.{k}").as_str())
            .and_then(|id| rig.defaults.get(id))
            .ok_or_else(|| format!("joint {joint} has no default {k}"))
    };
    let joints = rig
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| {
            let (
                DefaultValue::Vector3(translation),
                DefaultValue::Vector4(rotation),
                DefaultValue::Vector3(scale),
            ) = (
                default(&j.name, "t")?,
                default(&j.name, "q")?,
                default(&j.name, "s")?,
            )
            else {
                return Err(format!(
                    "joint {}'s defaults are not a vector, a quaternion and a vector",
                    j.name
                ));
            };
            Ok(Joint {
                id: JointId(i as u32),
                name: j.name.clone(),
                parent: (j.parent >= 0).then_some(JointId(j.parent as u32)),
                rest: Rest::Trs {
                    translation: *translation,
                    rotation: *rotation,
                    scale: *scale,
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let dofs = rig
        .dofs
        .iter()
        .map(|(&id, name)| {
            let target = match name.rsplit_once('.') {
                Some((joint, k)) if joint_of.contains_key(joint) && matches!(k, "q" | "t" | "s") => {
                    let j = joint_of[joint];
                    match k {
                        "q" => DofTarget::Rotation(j),
                        "t" => DofTarget::Translation(j),
                        _ => DofTarget::Scale(j),
                    }
                }
                _ => DofTarget::Other(name.clone()),
            };
            SkateDof {
                id,
                name: name.clone(),
                target,
            }
        })
        .collect::<Vec<_>>();
    let other: std::collections::BTreeSet<u32> = dofs
        .iter()
        .filter(|d| matches!(d.target, DofTarget::Other(_)))
        .map(|d| d.id)
        .collect();
    let other_defaults = rig
        .defaults
        .iter()
        .filter(|(id, _)| other.contains(id))
        .map(|(&id, &v)| (id, v))
        .collect();
    let e = world.spawn();
    world.insert(e, Skeleton { joints });
    world.insert(
        e,
        SkateAnimRig {
            name: rig.name.clone(),
            dofs,
            other_defaults,
        },
    );
    world.insert(e, Name(rig.name.clone()));
    world.insert(e, primase_core::Source(source.to_owned()));
    Ok(e)
}

/// The animation skeleton (`anim`, as [`insert_rig`] makes it) mapped onto the render skeleton (`render`, as
/// [`super::skeleton::read_skeleton`] makes it) by bone name, as `core.joint_map`'s pairs: every render bone is a joint
/// of the animation skeleton with the same name, and its parent the joint named as its parent; refused otherwise,
/// naming the bone. What else agrees (the default pose against the render skeleton's model poses) is the caller's to
/// check: the map states only the correspondence of names and hierarchy.
pub fn render_map(anim: &Skeleton, render: &Skeleton) -> Result<Vec<(JointId, JointId)>, String> {
    let by_name: BTreeMap<&str, &Joint> = anim.joints.iter().map(|j| (j.name.as_str(), j)).collect();
    let name_of = |s: &Skeleton, id: Option<JointId>| -> Option<String> {
        id.and_then(|p| s.joints.iter().find(|j| j.id == p).map(|j| j.name.clone()))
    };
    render
        .joints
        .iter()
        .map(|b| {
            let a = by_name
                .get(b.name.as_str())
                .ok_or_else(|| format!("render bone {} is not a joint of the animation skeleton", b.name))?;
            if name_of(anim, a.parent) != name_of(render, b.parent) {
                return Err(format!(
                    "render bone {}'s parent is {:?}, the animation joint's {:?}",
                    b.name,
                    name_of(render, b.parent),
                    name_of(anim, a.parent)
                ));
            }
            Ok((a.id, b.id))
        })
        .collect()
}
