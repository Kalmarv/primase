//! `lower-anvil-skin`: the meshes of the Entities that own a SkeletonComponent are bound to its rig (`core.skin`), and
//! the rig and the placements that follow it become `core.rig_instance` and `core.skin_instance`.
//!
//! - **Rig occurrences** ([`AnvilRigOccurrence`], one per placed SkeletonComponent, at its owner's transform) become
//!   `core.rig_instance` of the rig's `core.skeleton` (`lower-anvil-rig`'s).
//! - **Bindings**: each placement that follows a rig ([`AnvilSkinned`]) names a prototype; that prototype's mesh and its
//!   level-of-detail forms' meshes each get a binding to the rig (`core.skin`; its id the rig's position among the
//!   rigs placed, so every form names it alike), unless they have one already. Every primitive of a mesh takes the
//!   mesh's whole bone list as its palette, slot by slot: the joint of the rig named as the bone (the CRC-32 the
//!   `BoneID` and a bone's name both are) and the mesh's `InitialMatrixInverse` as the slot's inverse bind, as stored:
//!   a 4x4 of rows, so its first three rows are the columns of the affine map, converted from the reader's frame into
//!   the world's (`B' = C B C^-1`). Joints and weights are the streams `lower-anvil-mesh` wrote (weights as stored:
//!   they sum to 255 of 255, [`WeightRule::SumsToOne`]).
//! - **A slot the rig has no bone for** is bound to the rig's first joint when no vertex gives it a weight (the 48
//!   animal and bike meshes of Riders Republic each have exactly one such slot, `8f6a7739`, last in their list, with no
//!   weight on any vertex): the result is the same as without it. A slot with a weight and no bone is an error naming
//!   the mesh's entity and the bone.
//! - **The pose is not chosen**: an inverse bind is the mesh's own bind pose, which is the stored rest pose of its
//!   Skeleton on some meshes and not on others (the notes' census: equal for every bone on 233 of 571, one rigid
//!   transform away on 136, a different pose on the rest). Binding takes the stored value; a pass that poses the rig
//!   at its rest shows the mesh as that product, not as it was authored.
//!
//! A mesh that has bones and follows no rig keeps no joint and weight streams (counted): drawn as stored. A
//! placement of a mesh without bones in an Entity that owns a rig follows nothing (counted).

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog, PassSpec};
use primase_core::rig::{RigInstance, SkinInstance};
use primase_core::skeleton::{JointId, Skeleton};
use primase_core::skin::{Binding, BindingId, Deformation, Influences, Skin, Slot, WeightRule};
use primase_core::{Component, Entity, Instance, Lods, Mesh, Semantic, Stream, Transform, World};

use crate::components::{AnvilPalette, AnvilRigOccurrence, AnvilSkinned};

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerAnvilSkin;

/// How far a stored inverse bind's last row may be from 0 0 0 1: the files store it to float noise.
const AFFINE_TOLERANCE: f32 = 1e-4;

/// A mesh's stored inverse bind (four rows of x, y, z, w: the rows are the matrix's columns) as the affine map it is
/// ([`Transform::from_columns4`], within [`AFFINE_TOLERANCE`]).
fn affine(m: &[[f32; 4]; 4]) -> Result<Transform, String> {
    Transform::from_columns4(*m, AFFINE_TOLERANCE).map_err(|e| format!("an inverse bind: {e}"))
}

/// Each slot of the palette that some vertex gives a weight.
fn weighted_slots(palette: &AnvilPalette) -> Result<Vec<bool>, String> {
    let slots = palette.bones.len();
    let mut weighted = vec![false; slots];
    for (i, streams) in palette.streams.iter().enumerate() {
        for set in 0..palette.sets {
            let (Some(Stream::U8x4(j)), Some(Stream::U8x4(w))) = (
                streams.get(&Semantic::Joints(set)),
                streams.get(&Semantic::Weights(set)),
            ) else {
                return Err(format!(
                    "primitive {i}: no joint and weight streams for set {set}"
                ));
            };
            for (j, w) in j.iter().zip(w) {
                for (&slot, &w) in j.iter().zip(w) {
                    if w != 0 {
                        *weighted
                            .get_mut(usize::from(slot))
                            .ok_or_else(|| format!("primitive {i}: slot {slot} of {slots}"))? = true;
                    }
                }
            }
        }
    }
    Ok(weighted)
}

/// `lower-anvil-skin`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const LOWER_ANVIL_SKIN: PassSpec = PassSpec {
    name: "lower-anvil-skin",
    options: &[],
    make: |_| Ok(Box::new(LowerAnvilSkin)),
};

impl Pass for LowerAnvilSkin {
    fn name(&self) -> &'static str {
        "lower-anvil-skin"
    }
    fn options(&self) -> Options {
        Options::new()
    }
    fn requires(&self) -> &[&'static str] {
        &[
            AnvilRigOccurrence::NAME,
            AnvilSkinned::NAME,
            AnvilPalette::NAME,
            Skeleton::NAME,
        ]
    }
    fn provides(&self) -> &[&'static str] {
        &[RigInstance::NAME, SkinInstance::NAME, Skin::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[AnvilRigOccurrence::NAME, AnvilSkinned::NAME, AnvilPalette::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let occurrences: Vec<(Entity, Entity)> =
            w.iter::<AnvilRigOccurrence>().map(|(e, o)| (e, o.rig)).collect();
        let rigs: BTreeSet<Entity> = occurrences.iter().map(|&(_, r)| r).collect();
        let serial: BTreeMap<Entity, u32> = (0u32..).zip(&rigs).map(|(i, &r)| (r, i)).collect();
        for &(e, rig) in &occurrences {
            if w.get::<Skeleton>(rig).is_none() {
                return Err(format!("{e:?}: its rig {rig:?} holds no core.skeleton"));
            }
            w.remove::<AnvilRigOccurrence>(e);
            w.insert(e, RigInstance { skeleton: rig });
        }
        log.count("rig occurrences lowered", occurrences.len() as u64);
        // the rig's joints by the bone name each is named for
        let joints: BTreeMap<Entity, BTreeMap<u32, JointId>> = rigs
            .iter()
            .map(|&r| {
                let by_name = w.get::<Skeleton>(r).map_or_else(BTreeMap::new, |s| {
                    let mut m = BTreeMap::new();
                    for j in s.joints.iter().rev() {
                        if let Ok(h) = u32::from_str_radix(&j.name, 16) {
                            m.insert(h, j.id);
                        }
                    }
                    m
                });
                (r, by_name)
            })
            .collect();
        let placements: Vec<(Entity, Entity)> =
            w.iter::<AnvilSkinned>().map(|(p, s)| (p, s.occurrence)).collect();
        for (p, occurrence) in placements {
            w.remove::<AnvilSkinned>(p);
            let Some(rig) = w.get::<RigInstance>(occurrence).map(|r| r.skeleton) else {
                return Err(format!("{p:?} follows {occurrence:?}, which isn't a placed rig"));
            };
            let Some(prototype) = w.get::<Instance>(p).map(|i| i.prototype) else {
                return Err(format!("{p:?} follows a rig and places no prototype"));
            };
            let mut meshes = vec![prototype];
            if let Some(l) = w.get::<Lods>(prototype) {
                meshes.extend(l.forms.iter().map(|f| f.form));
            }
            let id = BindingId(serial[&rig]);
            let mut bound = false;
            for m in meshes {
                let Some(palette) = w.get::<AnvilPalette>(m).cloned() else {
                    continue;
                };
                bound |= m == prototype;
                if w.get::<Skin>(m)
                    .is_some_and(|s| s.bindings.iter().any(|b| b.id == id))
                {
                    continue;
                }
                let primitives = w
                    .get::<Mesh>(m)
                    .ok_or_else(|| format!("{m:?} has a bone list and no core.mesh"))?
                    .primitives()
                    .len();
                if palette.streams.len() != primitives {
                    return Err(format!(
                        "{m:?}: skin streams for {} primitives of {primitives}",
                        palette.streams.len()
                    ));
                }
                let used = weighted_slots(&palette).map_err(|e| format!("{m:?}: {e}"))?;
                let by_name = &joints[&rig];
                let first = w
                    .get::<Skeleton>(rig)
                    .and_then(|s| s.joints.first())
                    .map(|j| j.id);
                let mut slots = Vec::with_capacity(palette.bones.len());
                for (k, bone) in palette.bones.iter().enumerate() {
                    let joint = match by_name.get(&bone.bone_id) {
                        Some(&j) => j,
                        None if !used.get(k).copied().unwrap_or(false) => {
                            log.omit(
                                "bone slots the rig has no bone for, weighted by no vertex: its first joint",
                                1,
                            );
                            first.ok_or_else(|| format!("{rig:?} has no joints"))?
                        }
                        None => {
                            return Err(format!(
                                "{m:?}: slot {k} (bone {:08x}) has a weight and the rig has no such bone",
                                bone.bone_id
                            ));
                        }
                    };
                    slots.push(Slot {
                        joint,
                        inverse_bind: affine(&bone.initial_matrix_inverse)
                            .map_err(|e| format!("{m:?}: {e}"))?,
                    });
                }
                let binding = Binding {
                    id,
                    skeleton: rig,
                    palettes: vec![Some(slots); primitives],
                    deformation: Deformation::Linear,
                };
                let converted = |skin: Skin| skin.converted(&palette.frame);
                match w.get_mut::<Skin>(m) {
                    Some(skin) => {
                        let mut one = Skin {
                            influences: skin.influences.clone(),
                            weights: skin.weights,
                            bindings: vec![binding],
                        };
                        one = converted(one);
                        skin.bindings.extend(one.bindings);
                    }
                    None => {
                        w.edit_mesh(m, |mesh| {
                            for (mut p, extra) in mesh.primitives_mut().zip(&palette.streams) {
                                p.streams_mut().extend(extra.clone());
                            }
                            Ok(None)
                        })?;
                        let skin = Skin {
                            influences: vec![Some(Influences::Streams { sets: palette.sets }); primitives],
                            weights: WeightRule::SumsToOne,
                            bindings: vec![binding],
                        };
                        w.insert(m, converted(skin));
                        log.count("skins made", 1);
                    }
                }
                log.count("bindings made", 1);
            }
            if bound {
                w.insert(
                    p,
                    SkinInstance {
                        rig: occurrence,
                        binding: id,
                    },
                );
                log.count("placements following a rig", 1);
            } else {
                log.omit(
                    "placements in a rig's Entity of a mesh without bones: they follow nothing",
                    1,
                );
            }
        }
        // a mesh with bones that follows no rig is drawn as stored: its streams were never put back
        let unbound = w
            .iter::<AnvilPalette>()
            .filter(|(e, _)| w.get::<Skin>(*e).is_none())
            .count();
        if unbound > 0 {
            log.omit(
                "skinned meshes that follow no rig: drawn without their joint and weight streams",
                unbound as u64,
            );
        }
        let palettes: Vec<Entity> = w.iter::<AnvilPalette>().map(|(e, _)| e).collect();
        for e in palettes {
            w.remove::<AnvilPalette>(e);
        }
        Ok(())
    }
}
