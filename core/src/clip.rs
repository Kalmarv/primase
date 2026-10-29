//! An animation clip (`core.clip`) and what binds one to occurrences (`core.clip_binding`).
//!
//! A clip is a definition: channels over time, each driving one property of one target. Targets name definitions,
//! not occurrences: a joint of a skeleton ([`ClipTarget::Joint`]), a morph target of a geometry's morph set
//! ([`ClipTarget::Morph`], named by the prototype that holds the set, so it holds whichever level-of-detail form is
//! drawn), or a source's control ([`ClipTarget::Control`]: a scalar whose meaning stays in its source's namespace,
//! kept so it is not lost), a source-scoped draw group ([`ClipTarget::DrawGroup`]), or a named material
//! parameter ([`ClipTarget::MaterialParameter`]). Draw groups use Step visibility (-1 inherits,
//! 0 hides, 1 shows); material vectors replace same-width defaults in `core.material_parameters`.
//! One clip may drive a skeleton and several morph sets together. A [`ClipBinding`] says
//! which rig and which mesh occurrences a clip drives, so occurrences sharing a clip still animate apart.
//!
//! Meaning (core's evaluation rule; a source's curves that evaluate otherwise, as Havok's compressed splines or one
//! engine's weighted tangents or Euler curves, stay in its namespace until a pass lowers them):
//! - [`Interpolation::Step`] holds a key's value until the next key; [`Interpolation::Linear`] interpolates
//!   linearly, rotations by spherical linear interpolation (the shorter arc); [`Interpolation::Hermite`] is a cubic
//!   Hermite spline through the keys with each key's in and out tangents (glTF's `CUBICSPLINE`: values stored as
//!   in-tangent, value, out-tangent per key, tangents per second times the segment's length), rotations componentwise
//!   then normalised.
//! - Before the first key and after the last, a channel holds its end values ([`Extrapolation::Hold`]) or the clip
//!   repeats over `[0, duration]` ([`Extrapolation::Loop`]).
//! - A joint channel replaces the joint's local translation, rotation or scale (in its parent's space); a joint no
//!   channel drives keeps its pose. With an [`AdditiveReference`] the clip's values are taken relative to the
//!   reference and applied on top of a base pose, joint by joint in local space: `T = T_base + (T_clip - T_ref)`,
//!   `R = R_base * R_ref^-1 * R_clip`, `S = S_base * S_clip / S_ref`; a channel the clip lacks adds nothing.
//!   [`AdditiveReference::Identity`] is the reference of a clip whose values are already deltas (`T_ref = 0`, `R_ref` the
//!   identity, `S_ref = 1`): `T = T_base + T_clip`, `R = R_base * R_clip`, `S = S_base * S_clip`.
//!   [`AdditiveReference::IdentityParent`] is the same deltas composed on the other side, the parent's: `R = R_clip * R_base`
//!   (`T` and `S` as for `Identity`: a sum and a product, which do not depend on the order).
//! - Root motion is not a joint channel: [`RootMotion`] states the space it moves in and is applied (or not) by a pass.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::morphs::{MorphId, Morphs};
use crate::rig::{RigInstance, RigPose};
use crate::skeleton::{JointId, Skeleton};
use crate::{Component, Entity, Instance, JsonError, World};

/// What a channel drives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ClipTarget {
    /// A joint of the skeleton held by `skeleton`.
    Joint {
        skeleton: Entity,
        #[serde(deserialize_with = "joint_id")]
        joint: JointId,
    },
    /// A morph target of the morph set held by `geometry` (a prototype's `core.morphs`).
    Morph {
        geometry: Entity,
        #[serde(deserialize_with = "morph_id")]
        morph: MorphId,
    },
    /// A source's scalar control by its name (its meaning is its source's).
    Control { name: String },
    /// A draw group in a source-defined scope. Core supports groups 0..64:
    /// sources with more groups must refuse by name rather than truncate.
    DrawGroup { scope: String, group: u32 },
    /// A named parameter on resolved material entities. Each target must carry
    /// the same-width named default in core.material_parameters.
    MaterialParameter { materials: Vec<Entity>, name: String },
}

fn joint_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<JointId, D::Error> {
    u32::deserialize(d).map(JointId)
}
fn morph_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<MorphId, D::Error> {
    u32::deserialize(d).map(MorphId)
}

/// Which property of its target a channel drives, with the values it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Property {
    /// A joint's local translation (three values).
    Translation,
    /// A joint's local rotation, a unit quaternion `[x, y, z, w]` (four values).
    Rotation,
    /// A joint's local scale (three values).
    Scale,
    /// A morph target's weight, or a control's value (one value).
    Scalar,
    /// DrawGroup only: one Step value per key, -1 inherit the occurrence's
    /// default, 0 hidden, 1 shown. No interpolation between visibility states.
    Visibility,
    /// MaterialParameter only: 1..=4 finite floats per key, Step or Linear.
    Values(u8),
}

impl Property {
    /// Number of floats per key value (Hermite adds in/out tangent records).
    /// Values(0) and Values(5..) are invalid, rejected by the clip verifier.
    pub fn width(self) -> usize {
        match self {
            Property::Translation | Property::Scale => 3,
            Property::Rotation => 4,
            Property::Scalar | Property::Visibility => 1,
            Property::Values(n) => usize::from(n),
        }
    }
}

/// How a channel's values are interpolated between keys (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    Step,
    Linear,
    Hermite,
}

/// One channel: key times in seconds, strictly increasing, and its values: per key [`Property`]'s width of values,
/// three times that for [`Interpolation::Hermite`] (in-tangent, value, out-tangent), flattened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub target: ClipTarget,
    pub property: Property,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    pub values: Vec<f32>,
}

/// What a channel does outside its keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Extrapolation {
    Hold,
    Loop,
}

/// The space root motion is stated in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RootSpace {
    /// The rig's own space (`core.rig_instance`'s placement).
    Rig,
}

/// A clip's root motion: how the whole rig moves, in `space`, apart from its joints. Translation (three values a key)
/// and rotation (four), each a channel without a target of its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RootMotion {
    pub space: RootSpace,
    pub translation: Option<(Interpolation, Vec<f32>, Vec<f32>)>,
    pub rotation: Option<(Interpolation, Vec<f32>, Vec<f32>)>,
}

/// What an additive clip is relative to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum AdditiveReference {
    /// Nothing: the clip's values are deltas, relative to zero, the identity and one (the module documentation states
    /// the composition). A source that stores additive animation as deltas lowers to this; no entity stands for the
    /// reference.
    Identity,
    /// As [`AdditiveReference::Identity`] (deltas from zero, the identity and one), composed on the parent's side of the
    /// base rotation: `R = R_clip * R_base`, where `Identity` has `R = R_base * R_clip`. Translations add and scales
    /// multiply as for `Identity`. A source whose engine composes its additive animation this way (Sekiro's blend hint 1,
    /// as HKLib's `ADDITIVE_PARENT_SPACE` names it) lowers to this.
    IdentityParent,
    /// The `core.rig_pose` held by this entity.
    Pose(Entity),
    /// Another clip's (the `core.clip` held by `clip`) values at `time` seconds.
    ClipFrame { clip: Entity, time: f32 },
}

/// `core.clip` (see the module documentation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// The source's name: provenance.
    pub name: String,
    /// Seconds; every key within `[0, duration]`.
    pub duration: f32,
    pub extrapolation: Extrapolation,
    pub channels: Vec<Channel>,
    pub root_motion: Option<RootMotion>,
    pub additive: Option<AdditiveReference>,
}

/// `core.clip_binding`: a clip driving a rig (its joint channels, on the rig's skeleton) and mesh occurrences (its
/// morph channels, on the morph sets they draw). On its own entity: several bindings may drive one occurrence (a
/// pass layering them states how).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipBinding {
    pub clip: Entity,
    pub rig: Option<Entity>,
    pub meshes: Vec<Entity>,
}

const UNIT_TOLERANCE: f32 = 1e-3;

/// A channel's keys and values against its interpolation and width, by `what` (for errors).
fn verify_keys(
    what: &str,
    interpolation: Interpolation,
    width: usize,
    times: &[f32],
    values: &[f32],
    duration: f32,
    unit: bool,
) -> Result<(), String> {
    if times.is_empty() {
        return Err(format!("{what}: no keys"));
    }
    if !times
        .iter()
        .all(|t| t.is_finite() && (0.0..=duration).contains(t))
        || !times.windows(2).all(|p| p[0] < p[1])
    {
        return Err(format!("{what}: key times not increasing within [0, {duration}]"));
    }
    let per_key = if interpolation == Interpolation::Hermite {
        3 * width
    } else {
        width
    };
    if values.len() != times.len() * per_key {
        return Err(format!(
            "{what}: {} values for {} keys of {per_key}",
            values.len(),
            times.len()
        ));
    }
    if !values.iter().all(|x| x.is_finite()) {
        return Err(format!("{what}: a value not finite"));
    }
    if unit {
        // the key values (the middle of each Hermite triple) are rotations
        let offset = if interpolation == Interpolation::Hermite {
            width
        } else {
            0
        };
        for (k, key) in values.chunks(per_key).enumerate() {
            let q = &key[offset..offset + width];
            let len = q.iter().map(|x| x * x).sum::<f32>().sqrt();
            if (len - 1.0).abs() > UNIT_TOLERANCE {
                return Err(format!("{what}: key {k}'s rotation of length {len}"));
            }
        }
    }
    Ok(())
}

impl Component for Clip {
    const NAME: &'static str = "core.clip";
    /// 2: draw-group visibility and named material-parameter targets/properties.
    const VERSION: u32 = 2;
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// Name, duration, targets and key counts, not the values.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let channels: Vec<Value> = self
            .channels
            .iter()
            .map(|c| {
                serde_json::json!({
                    "target": c.target,
                    "property": c.property,
                    "interpolation": c.interpolation,
                    "keys": c.times.len(),
                })
            })
            .collect();
        Ok(Some(serde_json::json!({
            "name": self.name,
            "duration": crate::json::to_value(&self.duration)?,
            "extrapolation": self.extrapolation,
            "channels": channels,
            "root_motion": self.root_motion.is_some(),
            "additive": crate::json::to_value(&self.additive)?,
        })))
    }
    fn references(&self, out: &mut Vec<Entity>) {
        for c in &self.channels {
            match &c.target {
                ClipTarget::Joint { skeleton, .. } => out.push(*skeleton),
                ClipTarget::Morph { geometry, .. } => out.push(*geometry),
                ClipTarget::MaterialParameter { materials, .. } => out.extend(materials),
                ClipTarget::DrawGroup { .. } | ClipTarget::Control { .. } => {}
            }
        }
        match self.additive {
            Some(AdditiveReference::Pose(e)) => out.push(e),
            Some(AdditiveReference::ClipFrame { clip, .. }) => out.push(clip),
            Some(AdditiveReference::Identity | AdditiveReference::IdentityParent) | None => {}
        }
    }
    /// Duration finite and non-negative; each channel's property fits its target (joints take translation, rotation
    /// or scale; morphs and controls a scalar; scoped draw groups Step visibility; named material parameters
    /// Values(1..=4) matching every target's default), its target exists in the definition it names, one channel per target
    /// and property; keys and values as [`Channel`] says, rotations unit quaternions (within 1e-3); root motion's
    /// likewise; an additive reference a `core.rig_pose` or a `core.clip` with the frame's time within its duration.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !self.duration.is_finite() || self.duration < 0.0 {
            return Err(format!("duration {} not finite and non-negative", self.duration));
        }
        let mut seen = BTreeSet::new();
        let mut material_channels = BTreeSet::new();
        for (k, c) in self.channels.iter().enumerate() {
            let what = format!("channel {k}");
            match (&c.target, c.property) {
                (
                    ClipTarget::Joint { skeleton, joint },
                    Property::Translation | Property::Rotation | Property::Scale,
                ) => {
                    let s = w
                        .get::<Skeleton>(*skeleton)
                        .ok_or_else(|| format!("{what}: {skeleton:?} holds no core.skeleton"))?;
                    if s.index_of(*joint).is_none() {
                        return Err(format!("{what}: joint {joint:?} is not in its skeleton"));
                    }
                }
                (ClipTarget::Morph { geometry, morph }, Property::Scalar) => {
                    let m = w
                        .get::<Morphs>(*geometry)
                        .ok_or_else(|| format!("{what}: {geometry:?} holds no core.morphs"))?;
                    if m.target(*morph).is_none() {
                        return Err(format!("{what}: morph {morph:?} is not in its set"));
                    }
                }
                (ClipTarget::Control { .. }, Property::Scalar) => {}
                (ClipTarget::DrawGroup { scope, group }, Property::Visibility) => {
                    if scope.trim().is_empty() || *group >= 64 {
                        return Err(format!(
                            "{what}: draw-group scope must be nonempty and group below core's limit of 64"
                        ));
                    }
                    if c.interpolation != Interpolation::Step {
                        return Err(format!("{what}: visibility requires Step interpolation"));
                    }
                    if c.values.iter().any(|v| ![-1.0, 0.0, 1.0].contains(v)) {
                        return Err(format!(
                            "{what}: visibility values must be -1 (default), 0 (hidden) or 1 (shown)"
                        ));
                    }
                }
                (ClipTarget::MaterialParameter { materials, name }, Property::Values(width)) => {
                    if !(1..=4).contains(&width) || name.trim().is_empty() || materials.is_empty() {
                        return Err(format!(
                            "{what}: material parameter needs a name, targets and Values(1..=4)"
                        ));
                    }
                    if c.interpolation == Interpolation::Hermite {
                        return Err(format!(
                            "{what}: material values require Step or Linear interpolation"
                        ));
                    }
                    let mut targets = BTreeSet::new();
                    for &material in materials {
                        if !targets.insert(material) {
                            return Err(format!("{what}: repeated material target {material:?}"));
                        }
                        let defaults = w.get::<crate::MaterialParameters>(material).ok_or_else(|| {
                            format!("{what}: {material:?} holds no core.material_parameters default")
                        })?;
                        let value = defaults.0.get(name).ok_or_else(|| {
                            format!("{what}: {material:?} has no default for material parameter {name:?}")
                        })?;
                        if value.len() != usize::from(width) || !value.iter().all(|v| v.is_finite()) {
                            return Err(format!(
                                "{what}: {material:?} default for {name:?} is not {width} finite values"
                            ));
                        }
                        if !material_channels.insert((material, name)) {
                            return Err(format!(
                                "{what}: a second channel for material {material:?} parameter {name:?}"
                            ));
                        }
                    }
                }
                (t, p) => return Err(format!("{what}: {p:?} does not fit {t:?}")),
            }
            if !seen.insert((&c.target, c.property)) {
                return Err(format!(
                    "{what}: a second channel for {:?} {:?}",
                    c.target, c.property
                ));
            }
            verify_keys(
                &what,
                c.interpolation,
                c.property.width(),
                &c.times,
                &c.values,
                self.duration,
                c.property == Property::Rotation,
            )?;
        }
        if let Some(r) = &self.root_motion {
            for (what, curve, width) in [
                ("root translation", &r.translation, 3),
                ("root rotation", &r.rotation, 4),
            ] {
                if let Some((i, times, values)) = curve {
                    verify_keys(what, *i, width, times, values, self.duration, width == 4)?;
                }
            }
        }
        match self.additive {
            Some(AdditiveReference::Pose(e)) if !w.has::<RigPose>(e) => {
                return Err(format!("additive reference {e:?} holds no core.rig_pose"));
            }
            Some(AdditiveReference::ClipFrame { clip, time }) => {
                let c = w
                    .get::<Clip>(clip)
                    .ok_or_else(|| format!("additive reference {clip:?} holds no core.clip"))?;
                if !(time.is_finite() && (0.0..=c.duration).contains(&time)) {
                    return Err(format!(
                        "additive reference time {time} outside its clip's [0, {}]",
                        c.duration
                    ));
                }
                // clips additive to clips' frames: the chain ends (in a pose, or a clip that is not additive), or a
                // clip's values would be relative to themselves
                let mut seen = BTreeSet::from([e]);
                let mut next = Some(clip);
                while let Some(at) = next {
                    if !seen.insert(at) {
                        return Err(format!(
                            "additive references from {e:?} come back to {at:?}: a cycle"
                        ));
                    }
                    next = match w.get::<Clip>(at).and_then(|c| c.additive) {
                        Some(AdditiveReference::ClipFrame { clip, .. }) => Some(clip),
                        _ => None,
                    };
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl Component for ClipBinding {
    const NAME: &'static str = "core.clip_binding";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.clip);
        out.extend(self.rig);
        out.extend(&self.meshes);
    }
    /// The clip a `core.clip`; its joint channels on the rig's skeleton (so it needs a rig when it has any); each of its
    /// meshes an occurrence drawing a morph set the clip's morph channels drive (its prototype's, or its own mesh). A
    /// morph channel none of its meshes draws drives nothing through this binding (an occurrence posed into a static
    /// mesh leaves it).
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        let clip = w
            .get::<Clip>(self.clip)
            .ok_or_else(|| format!("{:?} holds no core.clip", self.clip))?;
        let skeleton = match self.rig {
            Some(r) => Some(
                w.get::<RigInstance>(r)
                    .ok_or_else(|| format!("{r:?} is not a rig"))?
                    .skeleton,
            ),
            None => None,
        };
        let mut driven = BTreeSet::new();
        for c in &clip.channels {
            match c.target {
                ClipTarget::Joint { skeleton: s, .. } if Some(s) != skeleton => {
                    return Err(format!("a joint channel on {s:?}, the rig poses {skeleton:?}"));
                }
                ClipTarget::Morph { geometry, .. } => {
                    driven.insert(geometry);
                }
                ClipTarget::Joint { .. }
                | ClipTarget::Control { .. }
                | ClipTarget::DrawGroup { .. }
                | ClipTarget::MaterialParameter { .. } => {}
            }
        }
        for &m in &self.meshes {
            let drawn = w.get::<Instance>(m).map_or(m, |i| i.prototype);
            if !driven.contains(&drawn) {
                return Err(format!(
                    "mesh {m:?} draws {drawn:?}, whose morph set the clip does not drive"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morphs::{Deltas, Frame, Target};
    use crate::skeleton::{Joint, Rest};
    use crate::{Mesh, Primitive, Semantic, Stream, Topology, Transform};

    const S: f32 = std::f32::consts::FRAC_1_SQRT_2;

    /// A skeleton (joints 1 and 2), a prototype with a morph set (target 9), a rig and a placement of the prototype,
    /// and a clip turning joint 2 a quarter turn about z over a second (linear, two keys), raising it with a Hermite
    /// translation, driving morph 9 and a source control: (world, clip, binding, skeleton, prototype, rig).
    fn world() -> (World, Entity, Entity, Entity, Entity, Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: [(1, None), (2, Some(1))]
                    .map(|(id, parent): (u32, Option<u32>)| Joint {
                        id: JointId(id),
                        name: format!("j{id}"),
                        parent: parent.map(JointId),
                        rest: Rest::Affine(Transform::IDENTITY),
                    })
                    .into(),
            },
        );
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]]))].into(),
                indices: None,
                material: None,
            }]),
        );
        w.insert(
            proto,
            Morphs {
                targets: vec![Target {
                    id: MorphId(9),
                    name: "blink".into(),
                    default_weight: 0.0,
                    frames: vec![Frame {
                        weight: 1.0,
                        primitives: vec![Some(Deltas {
                            positions: Some(vec![[0.0, 0.1, 0.0]]),
                            ..Deltas::default()
                        })],
                    }],
                }],
            },
        );
        let rig = w.spawn();
        w.insert(rig, RigInstance { skeleton });
        w.insert(rig, Transform::IDENTITY);
        let placed = w.spawn();
        w.insert(
            placed,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(placed, Transform::IDENTITY);
        let joint2 = ClipTarget::Joint {
            skeleton,
            joint: JointId(2),
        };
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "wave".into(),
                duration: 1.0,
                extrapolation: Extrapolation::Loop,
                channels: vec![
                    Channel {
                        target: joint2.clone(),
                        property: Property::Rotation,
                        interpolation: Interpolation::Linear,
                        times: vec![0.0, 1.0],
                        values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, S, S],
                    },
                    Channel {
                        target: joint2,
                        property: Property::Translation,
                        interpolation: Interpolation::Hermite,
                        times: vec![0.0, 1.0],
                        values: vec![0.0; 3]
                            .into_iter()
                            .chain([0.0, 0.0, 0.0])
                            .chain([0.0; 3])
                            .chain([0.0; 3])
                            .chain([0.0, 1.0, 0.0])
                            .chain([0.0; 3])
                            .collect(),
                    },
                    Channel {
                        target: ClipTarget::Morph {
                            geometry: proto,
                            morph: MorphId(9),
                        },
                        property: Property::Scalar,
                        interpolation: Interpolation::Step,
                        times: vec![0.0, 0.5],
                        values: vec![0.0, 1.0],
                    },
                    Channel {
                        target: ClipTarget::Control {
                            name: "board_flip".into(),
                        },
                        property: Property::Scalar,
                        interpolation: Interpolation::Linear,
                        times: vec![0.25],
                        values: vec![0.5],
                    },
                ],
                root_motion: Some(RootMotion {
                    space: RootSpace::Rig,
                    translation: Some((
                        Interpolation::Linear,
                        vec![0.0, 1.0],
                        vec![0.0, 0.0, 0.0, 0.0, 0.0, -2.0],
                    )),
                    rotation: None,
                }),
                additive: None,
            },
        );
        let binding = w.spawn();
        w.insert(
            binding,
            ClipBinding {
                clip,
                rig: Some(rig),
                meshes: vec![placed],
            },
        );
        (w, clip, binding, skeleton, proto, rig)
    }

    fn errors(w: &World) -> Vec<String> {
        w.verify_components().into_iter().map(|(_, _, m)| m).collect()
    }

    /// One clip drives a joint, a morph and a control together; bound to a rig and a placement it reaches the
    /// skeleton, the morph set's prototype and the occurrences.
    #[test]
    fn a_clip_drives_joints_morphs_and_controls_together() {
        let (w, clip, binding, skeleton, proto, rig) = world();
        assert_eq!(errors(&w), Vec::<String>::new());
        assert_eq!(w.closure([clip]), [clip, skeleton, proto].into());
        assert!(w.closure([binding]).contains(&rig));
    }

    fn with_display_channels() -> (World, Entity, Entity) {
        let (mut w, clip, ..) = world();
        let material = w.spawn();
        w.insert(
            material,
            crate::MaterialParameters(std::collections::BTreeMap::from([(
                "tint".into(),
                vec![0.25, 0.5],
            )])),
        );
        w.get_mut::<Clip>(clip).unwrap().channels.extend([
            Channel {
                target: ClipTarget::DrawGroup {
                    scope: "character".into(),
                    group: 63,
                },
                property: Property::Visibility,
                interpolation: Interpolation::Step,
                times: vec![0.0, 0.5, 1.0],
                values: vec![-1.0, 0.0, 1.0],
            },
            Channel {
                target: ClipTarget::MaterialParameter {
                    materials: vec![material],
                    name: "tint".into(),
                },
                property: Property::Values(2),
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.25, 0.5, 0.75, 1.0],
            },
        ]);
        (w, clip, material)
    }

    #[test]
    fn display_channels_roundtrip_and_keep_material_references_in_the_closure() {
        let (w, clip, material) = with_display_channels();
        assert!(errors(&w).is_empty(), "{:?}", errors(&w));
        let c = w.get::<Clip>(clip).unwrap();
        let copy: Clip = serde_json::from_value(serde_json::to_value(c).unwrap()).unwrap();
        assert_eq!(
            &copy, c,
            "includes old joint/morph ids and new targets/properties"
        );
        assert_eq!(copy.channels.last().unwrap().property.width(), 2);
        let closure = w.closure([clip]);
        assert!(closure.contains(&material));
        let part = w.subset(&closure);
        assert_eq!(part.get::<Clip>(clip), Some(c));
        assert!(
            c.remap_primitives(&[1, 0]).is_none(),
            "clip channels are not mesh-primitive arrays"
        );
        assert!(errors(&part).is_empty());
    }

    #[test]
    fn visibility_is_step_only_three_valued_and_scope_bounded() {
        for interpolation in [Interpolation::Linear, Interpolation::Hermite] {
            let (mut w, c, _) = with_display_channels();
            w.get_mut::<Clip>(c).unwrap().channels[4].interpolation = interpolation;
            assert!(errors(&w).iter().any(|e| e.contains("visibility requires Step")));
        }
        for value in [-2.0, 0.5, 2.0, f32::NAN] {
            let (mut w, c, _) = with_display_channels();
            w.get_mut::<Clip>(c).unwrap().channels[4].values[1] = value;
            assert!(errors(&w).iter().any(|e| e.contains("visibility values")));
        }
        for (scope, group) in [(" ", 0), ("character", 64)] {
            let (mut w, c, _) = with_display_channels();
            w.get_mut::<Clip>(c).unwrap().channels[4].target = ClipTarget::DrawGroup {
                scope: scope.into(),
                group,
            };
            assert!(errors(&w).iter().any(|e| e.contains("scope")));
        }
    }

    #[test]
    fn material_channels_require_defaults_widths_unique_targets_and_no_overlap() {
        for width in [0, 1, 3, 4, 5] {
            let (mut w, c, _) = with_display_channels();
            w.get_mut::<Clip>(c).unwrap().channels[5].property = Property::Values(width);
            assert!(!errors(&w).is_empty(), "default width 2 does not match {width}");
        }
        let (mut w, _c, m) = with_display_channels();
        w.remove::<crate::MaterialParameters>(m);
        assert!(
            errors(&w)
                .iter()
                .any(|e| e.contains("no core.material_parameters"))
        );
        let (mut w, c, m) = with_display_channels();
        w.get_mut::<Clip>(c).unwrap().channels[5].target = ClipTarget::MaterialParameter {
            materials: vec![m, m],
            name: "tint".into(),
        };
        assert!(errors(&w).iter().any(|e| e.contains("repeated material")));
        let (mut w, c, _) = with_display_channels();
        let duplicate = w.get::<Clip>(c).unwrap().channels[5].clone();
        w.get_mut::<Clip>(c).unwrap().channels.push(duplicate);
        assert!(errors(&w).iter().any(|e| e.contains("second channel")));
        let (mut w, c, _) = with_display_channels();
        w.get_mut::<Clip>(c).unwrap().channels[5].interpolation = Interpolation::Hermite;
        assert!(errors(&w).iter().any(|e| e.contains("Step or Linear")));
    }

    #[test]
    fn malformed_clips_and_bindings_fail_verification() {
        let fails = |edit: &dyn Fn(&mut World, Entity, Entity), what: &str| {
            let (mut w, clip, binding, ..) = world();
            edit(&mut w, clip, binding);
            let e = errors(&w);
            assert!(e.iter().any(|m| m.contains(what)), "{what}: {e:?}");
        };
        fails(
            &|w, c, _| w.get_mut::<Clip>(c).unwrap().channels[0].times = vec![1.0, 0.0],
            "not increasing",
        );
        fails(
            &|w, c, _| w.get_mut::<Clip>(c).unwrap().channels[0].times = vec![0.0, 2.0],
            "within [0, 1]",
        );
        fails(
            &|w, c, _| {
                w.get_mut::<Clip>(c).unwrap().channels[0].values.pop();
            },
            "7 values for 2 keys of 4",
        );
        fails(
            &|w, c, _| w.get_mut::<Clip>(c).unwrap().channels[0].values[3] = 2.0,
            "rotation of length",
        );
        fails(
            &|w, c, _| {
                w.get_mut::<Clip>(c).unwrap().channels[1].values.pop();
            },
            "values for 2 keys of 9",
        );
        fails(
            &|w, c, _| w.get_mut::<Clip>(c).unwrap().channels[2].property = Property::Rotation,
            "does not fit",
        );
        fails(
            &|w, c, _| {
                let clip = w.get_mut::<Clip>(c).unwrap();
                let again = clip.channels[2].clone();
                clip.channels.push(again);
            },
            "a second channel",
        );
        fails(
            &|w, c, _| {
                if let ClipTarget::Joint { joint, .. } = &mut w.get_mut::<Clip>(c).unwrap().channels[0].target
                {
                    *joint = JointId(7);
                }
            },
            "not in its skeleton",
        );
        fails(
            &|w, c, _| {
                if let ClipTarget::Morph { morph, .. } = &mut w.get_mut::<Clip>(c).unwrap().channels[2].target
                {
                    *morph = MorphId(1);
                }
            },
            "not in its set",
        );
        fails(
            &|w, c, _| w.get_mut::<Clip>(c).unwrap().duration = f32::NAN,
            "duration",
        );
        // an additive reference that is no pose, or a frame outside its clip
        fails(
            &|w, c, _| w.get_mut::<Clip>(c).unwrap().additive = Some(AdditiveReference::Pose(c)),
            "no core.rig_pose",
        );
        fails(
            &|w, c, _| {
                w.get_mut::<Clip>(c).unwrap().additive =
                    Some(AdditiveReference::ClipFrame { clip: c, time: 3.0 })
            },
            "outside its clip",
        );
        // additive to its own frame, or to a clip additive to it: a cycle
        fails(
            &|w, c, _| {
                w.get_mut::<Clip>(c).unwrap().additive =
                    Some(AdditiveReference::ClipFrame { clip: c, time: 0.0 })
            },
            "a cycle",
        );
        fails(
            &|w, c, _| {
                let mut other = w.get::<Clip>(c).unwrap().clone();
                other.additive = Some(AdditiveReference::ClipFrame { clip: c, time: 0.0 });
                let o = w.spawn();
                w.insert(o, other);
                w.get_mut::<Clip>(c).unwrap().additive =
                    Some(AdditiveReference::ClipFrame { clip: o, time: 0.0 });
            },
            "a cycle",
        );
        // a binding without the rig its joint channels need, or listing a mesh whose morph set the clip does not drive
        fails(
            &|w, _, b| w.get_mut::<ClipBinding>(b).unwrap().rig = None,
            "the rig poses None",
        );
        fails(
            &|w, c, b| w.get_mut::<ClipBinding>(b).unwrap().meshes.push(c),
            "whose morph set the clip does not drive",
        );
    }
}
