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
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RigPose {
    pub joints: Vec<(JointId, Rest)>,
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
    /// On a rig; every joint of its skeleton exactly once; every transform finite.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let skeleton = rig_skeleton(w, e)?;
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
}
