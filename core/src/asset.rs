//! One asset read by itself (`core.asset`): a root entity naming what the asset is made of, and `place-assets`, the
//! pass that places an asset a source did not place.
//!
//! A reader asked for one asset (a model, a character, a prop and its parts) spawns its entities as it reads any
//! world and a root holding [`Asset`]: each member with its [`Role`]. The members are references, so the asset's
//! closure ([`World::closure`]) is everything it needs; the asset's source identity is the root's `core.source`.
//! Optional members (clips, alternative parts) are members too: a selection pass prunes the ones a program does not
//! want, rewriting the root's list before it despawns them. An asset as a source authors it has no placement unless
//! the source places its parts (a prefab's renderers); a program that wants one drawn places it (`place-assets`).

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::pass::{Options, Pass, PassLog};
use crate::{
    Component, Entity, Instance, JsonError, Mesh, MeshFile, RigInstance, Skeleton, Skin, SkinInstance,
    Transform, World,
};

/// What a member is to its asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Role {
    /// Something it draws: a prototype (with its mesh or mesh file), or a placement the source authors in the asset's
    /// own space.
    Part,
    /// A skeleton its parts bind to (`core.skeleton`).
    Rig,
    /// A clip it comes with (`core.clip`).
    Clip,
    /// A part drawn instead of another or not at all (an outfit's alternative), as the source's state says.
    Alternative,
}

/// `core.asset` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Asset {
    pub members: Vec<(Entity, Role)>,
}

impl Component for Asset {
    const NAME: &'static str = "core.asset";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.members.iter().map(|(e, _)| *e));
    }
    /// Each member once and not the root; a rig a `core.skeleton`; a clip a `core.clip`; a part or an alternative a
    /// prototype (a mesh or a mesh file) or a placement.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let mut seen = BTreeSet::new();
        for &(m, role) in &self.members {
            if m == e || !seen.insert(m) {
                return Err(format!("member {m:?} is the root or listed twice"));
            }
            let fits = match role {
                Role::Rig => w.has::<Skeleton>(m),
                Role::Clip => w.has::<crate::Clip>(m),
                Role::Part | Role::Alternative => {
                    w.has::<Mesh>(m) || w.has::<MeshFile>(m) || w.has::<Instance>(m)
                }
            };
            if !fits {
                return Err(format!("member {m:?} is not what its role {role:?} says"));
            }
        }
        Ok(())
    }
}

/// `place-assets` (no options: at the world's origin): every asset whose parts nothing places is placed, once, at the
/// origin, as its source authors it:
/// - each `Rig` member gets a rig (`core.rig_instance` and `core.transform`) of its own;
/// - each `Part` prototype a placement (`core.instance`, `core.transform`), with a `core.skin_instance` when its mesh
///   is skinned to one of the asset's rigs (its first binding to that rig's skeleton);
/// - `Alternative` members are not placed (choosing one is a selection: counted), and a part the source already
///   places (a placement member, or a prototype something places) is left as it is.
///
/// The placed entities are content of their own ([`PassLog::place`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaceAssets;

/// The pass factory (`PassRegistry::register("place-assets", place_assets)`).
pub fn place_assets(o: &Options) -> Result<Box<dyn Pass>, String> {
    match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(PlaceAssets)),
    }
}

impl Pass for PlaceAssets {
    fn name(&self) -> &'static str {
        "place-assets"
    }
    fn requires(&self) -> &[&'static str] {
        &[Asset::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Instance::NAME,
            Transform::NAME,
            RigInstance::NAME,
            SkinInstance::NAME,
        ]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let placed: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let assets: Vec<(Entity, Asset)> = w.iter::<Asset>().map(|(e, a)| (e, a.clone())).collect();
        for (root, asset) in assets {
            let mut rigs: Vec<(Entity, Entity)> = Vec::new();
            for &(m, role) in &asset.members {
                if role == Role::Rig {
                    let rig = w.spawn();
                    w.insert(rig, RigInstance { skeleton: m });
                    w.insert(rig, Transform::IDENTITY);
                    log.place_from(rig, root);
                    rigs.push((m, rig));
                }
            }
            for &(m, role) in &asset.members {
                match role {
                    Role::Alternative => log.count("alternative parts not placed (a selection chooses)", 1),
                    Role::Part if w.has::<Instance>(m) || placed.contains(&m) => {
                        log.count("parts the source places, left as they are", 1);
                    }
                    Role::Part => {
                        let p = w.spawn();
                        w.insert(
                            p,
                            Instance {
                                prototype: m,
                                variant: 0,
                            },
                        );
                        w.insert(p, Transform::IDENTITY);
                        let binding = w.get::<Skin>(m).and_then(|s| {
                            s.bindings.iter().find_map(|b| {
                                rigs.iter()
                                    .find(|(skel, _)| *skel == b.skeleton)
                                    .map(|&(_, rig)| (rig, b.id))
                            })
                        });
                        if let Some((rig, binding)) = binding {
                            w.insert(p, SkinInstance { rig, binding });
                            log.count("skinned parts placed with their rig", 1);
                        }
                        log.place_from(p, root);
                        log.count("parts placed", 1);
                    }
                    Role::Rig | Role::Clip => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass::{PassManager, PassRegistry, RunReport};
    use crate::{
        Binding, BindingId, Deformation, Influences, Joint, JointId, Primitive, Rest, Semantic, Slot, Stream,
    };
    use crate::{Topology, WeightRule};

    /// An asset of a skeleton, a skinned prototype, a static prototype and an alternative: (world, root, [skinned,
    /// static, alternative], skeleton).
    fn world() -> (World, Entity, [Entity; 3], Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![Joint {
                    id: JointId(0),
                    name: "root".into(),
                    parent: None,
                    rest: Rest::Affine(Transform::IDENTITY),
                }],
            },
        );
        let mesh = || {
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]]))].into(),
                indices: None,
                material: None,
            }])
        };
        let parts = [w.spawn(), w.spawn(), w.spawn()];
        for &p in &parts {
            w.insert(p, mesh());
        }
        w.insert(
            parts[0],
            Skin {
                influences: vec![Some(Influences::Whole { slot: 0 })],
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(2),
                    skeleton,
                    palettes: vec![Some(vec![Slot {
                        joint: JointId(0),
                        inverse_bind: Transform::IDENTITY,
                    }])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        let root = w.spawn();
        w.insert(
            root,
            Asset {
                members: vec![
                    (skeleton, Role::Rig),
                    (parts[0], Role::Part),
                    (parts[1], Role::Part),
                    (parts[2], Role::Alternative),
                ],
            },
        );
        (w, root, parts, skeleton)
    }

    #[test]
    fn an_asset_reaches_its_members_and_verifies() {
        let (mut w, root, parts, skeleton) = world();
        assert_eq!(w.verify_components(), vec![]);
        assert!(
            w.closure([root])
                .is_superset(&[skeleton, parts[0], parts[1], parts[2]].into())
        );
        w.get_mut::<Asset>(root)
            .unwrap()
            .members
            .push((parts[0], Role::Part));
        assert!(
            w.verify_components()
                .iter()
                .any(|(_, _, m)| m.contains("listed twice"))
        );
        w.get_mut::<Asset>(root).unwrap().members.pop();
        w.get_mut::<Asset>(root).unwrap().members[0].1 = Role::Clip;
        assert!(
            w.verify_components()
                .iter()
                .any(|(_, _, m)| m.contains("not what its role"))
        );
    }

    /// Placing: one rig, a placement per part (the skinned one with its skin instance on that rig), the alternative
    /// not placed; a part the source already places is left alone.
    #[test]
    fn place_assets_places_rigs_and_parts() {
        let (mut w, _, parts, skeleton) = world();
        let mut r = PassRegistry::new();
        r.register("place-assets", place_assets);
        let m = PassManager::new(r).with_pass_checks(true);
        let mut report = RunReport::default();
        m.run(&mut w, &m.registry.parse("place-assets").unwrap(), &mut report)
            .unwrap();
        let rigs: Vec<_> = w.iter::<RigInstance>().collect();
        assert_eq!(rigs.len(), 1);
        assert_eq!(rigs[0].1.skeleton, skeleton);
        let placed: Vec<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        assert_eq!(placed, [parts[0], parts[1]]);
        let skinned: Vec<_> = w.iter::<SkinInstance>().collect();
        assert_eq!(skinned.len(), 1);
        assert_eq!(
            (skinned[0].1.rig, skinned[0].1.binding),
            (rigs[0].0, BindingId(2))
        );
        assert_eq!(w.verify_components(), vec![]);
        let log = &report.steps[0].log;
        assert_eq!(
            log.counts["alternative parts not placed (a selection chooses)"],
            1
        );
        assert_eq!(log.placed.len(), 3);
        // placing again: every part already placed
        let mut again = RunReport::default();
        m.run(&mut w, &m.registry.parse("place-assets").unwrap(), &mut again)
            .unwrap();
        assert_eq!(
            again.steps[0].log.counts["parts the source places, left as they are"],
            2
        );
    }
}
