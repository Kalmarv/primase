//! Rig occurrences: a skeleton placed and posed, and the meshes, weights and objects that follow it. Definitions
//! (`core.skeleton`, `core.skin`, `core.morphs`, `core.sockets`) are shared; an occurrence holds what is its own, so
//! two placed characters sharing one mesh, skin and clips never share a pose.
//!
//! - A **rig** is an entity with [`RigInstance`] (which skeleton) and `core.transform` (the rig's space in the world,
//!   flat like any placement); its [`RigPose`], when it has one, is its joints' local transforms (an authored pose or
//!   one a pass evaluated), in place of the skeleton's rest.
//! - A **skinned occurrence** is a placement (or an entity drawing its own mesh) with [`SkinInstance`]: the rig it
//!   follows and the binding of its mesh's skin it is drawn with.
//! - [`MorphWeights`] are an occurrence's weights of its mesh's morph targets, with or without a rig.
//! - An [`Attachment`] places an object at a socket of a rig ([`Sockets`], on the skeleton's entity).
//!
//! Nothing here evaluates a pose: deforming a mesh by its rig is a pass's work (toolkit math), so a writer that can't
//! skin is given geometry a pass has posed.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::morphs::{MorphId, Morphs};
use crate::skeleton::{JointId, Rest, Skeleton};
use crate::skin::{BindingId, Skin};
use crate::{Component, Entity, Instance, JsonError, Transform, World};

/// `core.rig_instance`: a placed skeleton (see the module documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RigInstance {
    /// The entity holding the `core.skeleton`.
    pub skeleton: Entity,
}

/// `core.rig_pose`: a rig's joints' local transforms, every joint of its skeleton once, each in its parent's space as
/// the skeleton's rests are.
///
/// `root_applied` is the root motion a pass has already moved the rig's `core.transform` by (`placed * root_applied`
/// is the transform): a pass that applies root motion again takes that one off first, so repeating it, or moving to
/// another time, leaves the rig where its own placement and the new root motion put it. A rig moved by anything else
/// stays as it was moved.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigPose {
    pub joints: Vec<(JointId, Rest)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_applied: Option<Transform>,
}

/// `core.skin_instance`: the rig a mesh occurrence follows and the binding of its mesh's skin it is drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SkinInstance {
    /// The rig entity ([`RigInstance`]).
    pub rig: Entity,
    pub binding: BindingId,
}

/// `core.morph_weights`: an occurrence's weights of its mesh's morph targets, each target at most once; a target not
/// listed is at its default weight.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MorphWeights(pub Vec<(MorphId, f32)>);

/// A socket's identity within its skeleton's sockets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SocketId(pub u32);

/// A named place on a joint where things attach (a hand's grip, a sheath).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Socket {
    pub id: SocketId,
    /// The source's name: provenance, not identity.
    pub name: String,
    pub joint: JointId,
    /// The socket's space in its joint's: `joint = M * socket`.
    pub offset: Transform,
}

/// `core.sockets`, on the entity holding the `core.skeleton` they belong to.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sockets(pub Vec<Socket>);

/// `core.attachment`: the object this placement draws follows a rig's socket, placed at `offset` in the socket's
/// space. Its `core.transform` stays a flat placement (where the object is at the rig's pose as read); a pass that
/// poses the rig moves it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Attachment {
    /// The rig entity ([`RigInstance`]).
    pub rig: Entity,
    pub socket: SocketId,
    pub offset: Transform,
}

fn finite(m: &Transform) -> bool {
    m.cols.iter().flatten().all(|x| x.is_finite())
}

/// The skeleton of the rig entity `rig`, or why there is none.
fn rig_skeleton(w: &World, rig: Entity) -> Result<&Skeleton, String> {
    let r = w
        .get::<RigInstance>(rig)
        .ok_or_else(|| format!("{rig:?} is not a rig (no core.rig_instance)"))?;
    w.get::<Skeleton>(r.skeleton)
        .ok_or_else(|| format!("rig {rig:?}'s skeleton {:?} holds no core.skeleton", r.skeleton))
}

/// The entity whose mesh an occurrence draws: its prototype's, or its own when it draws one.
fn drawn(w: &World, e: Entity) -> Entity {
    w.get::<Instance>(e).map_or(e, |i| i.prototype)
}

impl Component for RigInstance {
    const NAME: &'static str = "core.rig_instance";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.skeleton);
    }
    /// Placed (`core.transform`), its skeleton a `core.skeleton`.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Transform>(e) {
            return Err("a rig without a core.transform".into());
        }
        rig_skeleton(w, e).map(|_| ())
    }
}

impl Component for RigPose {
    const NAME: &'static str = "core.rig_pose";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// On a rig; every joint of its skeleton exactly once; every transform finite, the applied root motion too.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let skeleton = rig_skeleton(w, e)?;
        if self.root_applied.as_ref().is_some_and(|m| !finite(m)) {
            return Err("the applied root motion is not finite".into());
        }
        let mut seen = BTreeSet::new();
        for (j, rest) in &self.joints {
            if skeleton.index_of(*j).is_none() {
                return Err(format!("joint {j:?} is not a joint of the rig's skeleton"));
            }
            if !seen.insert(*j) {
                return Err(format!("joint {j:?} posed twice"));
            }
            if !finite(&rest.matrix()) {
                return Err(format!("joint {j:?}: a transform not finite"));
            }
        }
        if seen.len() != skeleton.joints.len() {
            return Err(format!(
                "{} of the skeleton's {} joints posed",
                seen.len(),
                skeleton.joints.len()
            ));
        }
        Ok(())
    }
}

impl Component for SkinInstance {
    const NAME: &'static str = "core.skin_instance";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.rig);
    }
    /// The occurrence draws a skinned mesh (its prototype's `core.skin`, or its own) with this binding, and the binding
    /// binds to the rig's skeleton.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let geometry = drawn(w, e);
        let skin = w
            .get::<Skin>(geometry)
            .ok_or_else(|| format!("{geometry:?}, which it draws, has no core.skin"))?;
        let binding = skin
            .binding(self.binding)
            .ok_or_else(|| format!("binding {:?} is not one of {geometry:?}'s skin", self.binding))?;
        rig_skeleton(w, self.rig)?;
        let rig_skeleton_entity = w.get::<RigInstance>(self.rig).map(|r| r.skeleton);
        if rig_skeleton_entity != Some(binding.skeleton) {
            return Err(format!(
                "binding {:?} binds to {:?}, the rig poses {rig_skeleton_entity:?}",
                self.binding, binding.skeleton
            ));
        }
        Ok(())
    }
}

impl Component for MorphWeights {
    const NAME: &'static str = "core.morph_weights";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// The occurrence draws a mesh with `core.morphs` holding each target listed, once, at a finite weight.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let geometry = drawn(w, e);
        let morphs = w
            .get::<Morphs>(geometry)
            .ok_or_else(|| format!("{geometry:?}, which it draws, has no core.morphs"))?;
        let mut seen = BTreeSet::new();
        for (id, weight) in &self.0 {
            if morphs.target(*id).is_none() {
                return Err(format!("target {id:?} is not one of {geometry:?}'s morphs"));
            }
            if !seen.insert(*id) || !weight.is_finite() {
                return Err(format!("target {id:?} listed twice, or its weight not finite"));
            }
        }
        Ok(())
    }
}

impl Component for Sockets {
    const NAME: &'static str = "core.sockets";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Beside a `core.skeleton` holding each socket's joint; ids unique; offsets finite.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let skeleton = w
            .get::<Skeleton>(e)
            .ok_or("sockets on an entity without a core.skeleton")?;
        let mut ids = BTreeSet::new();
        for s in &self.0 {
            if !ids.insert(s.id) {
                return Err(format!("socket {:?} listed twice", s.id));
            }
            if skeleton.index_of(s.joint).is_none() {
                return Err(format!(
                    "socket {:?}'s joint {:?} is not in the skeleton",
                    s.id, s.joint
                ));
            }
            if !finite(&s.offset) {
                return Err(format!("socket {:?}: offset not finite", s.id));
            }
        }
        Ok(())
    }
}

impl Component for Attachment {
    const NAME: &'static str = "core.attachment";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.rig);
    }
    /// Placed (`core.transform`); the rig's skeleton has the socket; the offset finite.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if !w.has::<Transform>(e) {
            return Err("an attachment without a core.transform".into());
        }
        rig_skeleton(w, self.rig)?;
        let skeleton = w.get::<RigInstance>(self.rig).map(|r| r.skeleton);
        let has = skeleton
            .and_then(|s| w.get::<Sockets>(s))
            .is_some_and(|s| s.0.iter().any(|k| k.id == self.socket));
        if !has {
            return Err(format!(
                "socket {:?} is not one of the rig's skeleton's",
                self.socket
            ));
        }
        if !finite(&self.offset) {
            return Err("offset not finite".into());
        }
        Ok(())
    }
}

/// `core.joint_map`, on an entity of its own: which joint of skeleton `to` each listed joint of skeleton `from` is, as a
/// source states it (a table, or a correspondence a pass proved). Each joint at most once on each side; joints of either
/// skeleton not listed are unmapped (a pass using the map states what it does with them). Names are not evidence: a map
/// a pass builds by name states what else it checked.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JointMap {
    /// The entity holding the source `core.skeleton`.
    pub from: Entity,
    /// The entity holding the target `core.skeleton`.
    pub to: Entity,
    pub pairs: Vec<(JointId, JointId)>,
}

impl Component for JointMap {
    const NAME: &'static str = "core.joint_map";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.from);
        out.push(self.to);
    }
    /// Both ends skeletons; every listed joint in its skeleton; no joint twice on either side.
    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        let from = w
            .get::<Skeleton>(self.from)
            .ok_or_else(|| format!("{:?} holds no core.skeleton", self.from))?;
        let to = w
            .get::<Skeleton>(self.to)
            .ok_or_else(|| format!("{:?} holds no core.skeleton", self.to))?;
        let (mut a, mut b) = (BTreeSet::new(), BTreeSet::new());
        for &(x, y) in &self.pairs {
            if from.index_of(x).is_none() || to.index_of(y).is_none() {
                return Err(format!("pair {x:?} -> {y:?}: a joint not in its skeleton"));
            }
            if !a.insert(x) || !b.insert(y) {
                return Err(format!("pair {x:?} -> {y:?}: a joint mapped twice"));
            }
        }
        Ok(())
    }
}

/// Where a target joint stands inside the source mesh, as an offset from where a fit would put it: the target joint's
/// frame (its rest, carried into the source's model space) is moved by `translation` (metres, along the source's model
/// axes) and turned by `rotation` (a unit quaternion `[x, y, z, w]`, about the joint's own origin, in the source's
/// model axes). The identity offset (zero, `[0, 0, 0, 1]`) is the fit's own placement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct JointOffset {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
}

impl JointOffset {
    /// The offset that moves nothing.
    pub const IDENTITY: JointOffset = JointOffset {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };

    /// How far a rotation's length may be from 1 and still be a unit quaternion.
    pub const UNIT_TOLERANCE: f32 = 1e-3;

    /// Finite entries and a rotation of unit length (within [`Self::UNIT_TOLERANCE`]).
    pub fn check(&self) -> Result<(), String> {
        if !self
            .translation
            .iter()
            .chain(&self.rotation)
            .all(|x| x.is_finite())
        {
            return Err("an entry not finite".into());
        }
        let len = self.rotation.iter().map(|x| x * x).sum::<f32>().sqrt();
        if (len - 1.0).abs() > Self::UNIT_TOLERANCE {
            return Err(format!("rotation of length {len}, not a unit quaternion"));
        }
        Ok(())
    }

    /// Whether this is the identity offset (the fit's own placement).
    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }
}

/// `core.joint_placement`, on an entity of its own: where a person (or a tool) stands the target joints of a
/// [`JointMap`] inside the source mesh, as offsets from the fit's own placement. Offsets are keyed by target joint, each
/// at most once; a target joint not listed has the identity offset. A map has at most one placement. The placement is
/// not part of the map: a pass that reads the map's pairs (`map-clip`) never reads it, and only the passes that fit a
/// mesh to the target (`retarget-skin{placement=stated}`) do.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JointPlacement {
    /// The entity holding the `core.joint_map` the offsets belong to.
    pub map: Entity,
    pub offsets: Vec<(JointId, JointOffset)>,
}

impl Component for JointPlacement {
    const NAME: &'static str = "core.joint_placement";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.map);
    }
    /// The map holds a `core.joint_map`, no other placement names it, and each offset is of a target joint the map
    /// lists, once, finite and of a unit rotation.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let map = w
            .get::<JointMap>(self.map)
            .ok_or_else(|| format!("{:?} holds no core.joint_map", self.map))?;
        if let Some((other, _)) = w
            .iter::<JointPlacement>()
            .find(|(o, p)| *o != e && p.map == self.map)
        {
            return Err(format!("{other:?} also places the joints of map {:?}", self.map));
        }
        let mut seen = BTreeSet::new();
        for (joint, offset) in &self.offsets {
            if !map.pairs.iter().any(|(_, to)| to == joint) {
                return Err(format!("joint {joint:?} is not a target joint the map lists"));
            }
            if !seen.insert(*joint) {
                return Err(format!("joint {joint:?} placed twice"));
            }
            offset.check().map_err(|why| format!("joint {joint:?}: {why}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::morphs::{Deltas, Frame, Target};
    use crate::skeleton::Joint;
    use crate::skin::{Binding, Deformation, Influences, Slot, WeightRule};
    use crate::{Mesh, Primitive, Semantic, Stream, Topology};

    fn joint(id: u32, parent: Option<u32>) -> Joint {
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest: Rest::Affine(Transform::IDENTITY),
        }
    }

    /// A shared definition (skeleton with a socket, a prototype whose one-primitive mesh is bound whole to joint 2 and
    /// has one morph target) and two rigs of it, each with a skinned placement of the prototype, posed differently:
    /// (world, prototype, [rig a, rig b], [placement a, placement b], skeleton).
    fn world() -> (World, Entity, [Entity; 2], [Entity; 2], Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![joint(1, None), joint(2, Some(1))],
            },
        );
        w.insert(
            skeleton,
            Sockets(vec![Socket {
                id: SocketId(5),
                name: "grip".into(),
                joint: JointId(2),
                offset: Transform::IDENTITY,
            }]),
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
            Skin {
                influences: vec![Some(Influences::Whole { slot: 0 })],
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(3),
                    skeleton,
                    palettes: vec![Some(vec![Slot {
                        joint: JointId(2),
                        inverse_bind: Transform::IDENTITY,
                    }])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        w.insert(
            proto,
            Morphs {
                targets: vec![Target {
                    id: MorphId(9),
                    name: "puff".into(),
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
        let mut rigs = [Entity::from_index(0); 2];
        let mut placements = rigs;
        for (k, lift) in [0.0f32, 1.0].into_iter().enumerate() {
            let rig = w.spawn();
            w.insert(rig, RigInstance { skeleton });
            w.insert(rig, Transform::IDENTITY);
            let mut up = Transform::IDENTITY;
            up.cols[3] = [0.0, lift, 0.0];
            w.insert(
                rig,
                RigPose {
                    joints: vec![
                        (JointId(2), Rest::Affine(up)),
                        (JointId(1), Rest::Affine(Transform::IDENTITY)),
                    ],
                    root_applied: None,
                },
            );
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(p, Transform::IDENTITY);
            w.insert(
                p,
                SkinInstance {
                    rig,
                    binding: BindingId(3),
                },
            );
            w.insert(p, MorphWeights(vec![(MorphId(9), lift)]));
            rigs[k] = rig;
            placements[k] = p;
        }
        (w, proto, rigs, placements, skeleton)
    }

    fn errors(w: &World) -> Vec<String> {
        w.verify_components().into_iter().map(|(_, _, m)| m).collect()
    }

    /// Two occurrences share one mesh, skin and morphs, each with its own pose and weights; a placement reaches its
    /// rig and through it the skeleton.
    #[test]
    fn occurrences_share_definitions_not_poses() {
        let (w, proto, rigs, placements, skeleton) = world();
        assert_eq!(errors(&w), Vec::<String>::new());
        assert_ne!(w.get::<RigPose>(rigs[0]), w.get::<RigPose>(rigs[1]));
        assert_ne!(
            w.get::<MorphWeights>(placements[0]),
            w.get::<MorphWeights>(placements[1])
        );
        assert_eq!(
            w.closure([placements[0]]),
            [placements[0], proto, rigs[0], skeleton].into()
        );
        // an attachment at the socket of rig b
        let mut w = w;
        let sword = w.spawn();
        w.insert(sword, Transform::IDENTITY);
        w.insert(
            sword,
            Attachment {
                rig: rigs[1],
                socket: SocketId(5),
                offset: Transform::IDENTITY,
            },
        );
        assert_eq!(errors(&w), Vec::<String>::new());
        assert!(w.closure([sword]).contains(&skeleton));
    }

    #[test]
    fn malformed_occurrences_fail_verification() {
        type Edit<'a> = &'a dyn Fn(&mut World, [Entity; 2], [Entity; 2], Entity);
        let fails = |edit: Edit, what: &str| {
            let (mut w, _, rigs, placements, skeleton) = world();
            edit(&mut w, rigs, placements, skeleton);
            let e = errors(&w);
            assert!(e.iter().any(|m| m.contains(what)), "{what}: {e:?}");
        };
        // a pose missing a joint, posing one twice, or naming one the skeleton lacks
        fails(
            &|w, r, _, _| {
                w.get_mut::<RigPose>(r[0]).unwrap().joints.pop();
            },
            "1 of the skeleton's 2",
        );
        fails(
            &|w, r, _, _| {
                let p = w.get_mut::<RigPose>(r[0]).unwrap();
                p.joints[1].0 = JointId(2);
            },
            "posed twice",
        );
        fails(
            &|w, r, _, _| w.get_mut::<RigPose>(r[0]).unwrap().joints[0].0 = JointId(7),
            "not a joint",
        );
        // an applied root motion that is not finite
        fails(
            &|w, r, _, _| {
                let mut m = Transform::IDENTITY;
                m.cols[3][0] = f32::NAN;
                w.get_mut::<RigPose>(r[0]).unwrap().root_applied = Some(m);
            },
            "applied root motion is not finite",
        );
        // a rig unplaced; a skin instance with a binding the skin lacks, or following a rig of another skeleton
        fails(
            &|w, r, _, _| {
                w.remove::<Transform>(r[0]);
            },
            "a rig without a core.transform",
        );
        fails(
            &|w, _, p, _| w.get_mut::<SkinInstance>(p[0]).unwrap().binding = BindingId(4),
            "not one of",
        );
        fails(
            &|w, r, _, _| {
                let other = w.spawn();
                w.insert(
                    other,
                    Skeleton {
                        joints: vec![joint(1, None), joint(2, Some(1))],
                    },
                );
                w.get_mut::<RigInstance>(r[1]).unwrap().skeleton = other;
                w.get_mut::<RigPose>(r[1]).unwrap();
            },
            "the rig poses",
        );
        // weights of a target the mesh lacks, or listed twice
        fails(
            &|w, _, p, _| w.get_mut::<MorphWeights>(p[0]).unwrap().0[0].0 = MorphId(1),
            "not one of",
        );
        fails(
            &|w, _, p, _| {
                let m = w.get_mut::<MorphWeights>(p[0]).unwrap();
                m.0.push(m.0[0]);
            },
            "listed twice",
        );
        // a socket on a joint the skeleton lacks; an attachment to a socket the rig's skeleton lacks
        fails(
            &|w, _, _, s| w.get_mut::<Sockets>(s).unwrap().0[0].joint = JointId(8),
            "not in the skeleton",
        );
        fails(
            &|w, r, _, _| {
                let a = w.spawn();
                w.insert(a, Transform::IDENTITY);
                w.insert(
                    a,
                    Attachment {
                        rig: r[0],
                        socket: SocketId(6),
                        offset: Transform::IDENTITY,
                    },
                );
            },
            "socket SocketId(6)",
        );
    }

    /// Two skeletons of two joints, a map pairing them, and a placement of the target's joint 20.
    fn placed() -> (World, Entity, Entity) {
        let mut w = World::new();
        let (a, b) = (w.spawn(), w.spawn());
        for (e, ids) in [(a, [1, 2]), (b, [10, 20])] {
            w.insert(
                e,
                Skeleton {
                    joints: vec![joint(ids[0], None), joint(ids[1], Some(ids[0]))],
                },
            );
        }
        let map = w.spawn();
        w.insert(
            map,
            JointMap {
                from: a,
                to: b,
                pairs: vec![(JointId(1), JointId(10)), (JointId(2), JointId(20))],
            },
        );
        let placement = w.spawn();
        w.insert(
            placement,
            JointPlacement {
                map,
                offsets: vec![(
                    JointId(20),
                    JointOffset {
                        translation: [0.0, 0.1, 0.0],
                        ..JointOffset::IDENTITY
                    },
                )],
            },
        );
        (w, map, placement)
    }

    /// A placement is of one map, of target joints the map lists, once each, with finite entries and a unit rotation.
    #[test]
    fn malformed_placements_fail_verification() {
        let (w, ..) = placed();
        assert_eq!(errors(&w), Vec::<String>::new());
        let fails = |edit: &dyn Fn(&mut World, Entity, Entity), what: &str| {
            let (mut w, map, placement) = placed();
            edit(&mut w, map, placement);
            let e = errors(&w);
            assert!(e.iter().any(|m| m.contains(what)), "{what}: {e:?}");
        };
        // a source joint of the map is not one of its targets; neither is a joint of no skeleton
        fails(
            &|w, _, p| w.get_mut::<JointPlacement>(p).unwrap().offsets[0].0 = JointId(2),
            "not a target joint the map lists",
        );
        fails(
            &|w, _, p| w.get_mut::<JointPlacement>(p).unwrap().offsets[0].0 = JointId(99),
            "not a target joint the map lists",
        );
        fails(
            &|w, _, p| {
                let o = w.get::<JointPlacement>(p).unwrap().offsets[0];
                w.get_mut::<JointPlacement>(p).unwrap().offsets.push(o);
            },
            "placed twice",
        );
        fails(
            &|w, _, p| w.get_mut::<JointPlacement>(p).unwrap().offsets[0].1.translation[1] = f32::NAN,
            "not finite",
        );
        fails(
            &|w, _, p| w.get_mut::<JointPlacement>(p).unwrap().offsets[0].1.rotation = [0.0, 0.0, 0.0, 2.0],
            "not a unit quaternion",
        );
        // a placement of an entity that is no map, and a second placement of one map
        fails(
            &|w, _, p| {
                let other = w.spawn();
                w.get_mut::<JointPlacement>(p).unwrap().map = other;
            },
            "holds no core.joint_map",
        );
        fails(
            &|w, map, _| {
                let second = w.spawn();
                w.insert(
                    second,
                    JointPlacement {
                        map,
                        offsets: Vec::new(),
                    },
                );
            },
            "also places the joints of map",
        );
    }

    #[test]
    fn the_identity_offset_is_the_one_that_moves_nothing() {
        assert!(JointOffset::IDENTITY.is_identity());
        assert!(JointOffset::IDENTITY.check().is_ok());
        let moved = JointOffset {
            rotation: [0.0, 1.0, 0.0, 0.0],
            ..JointOffset::IDENTITY
        };
        assert!(!moved.is_identity() && moved.check().is_ok());
    }
}
