//! `lower-gta-skin`: a skinned drawable's blend streams as a core skin on the skeleton the drawable carries.
//!
//! The reader keeps a skinned geometry's `BLENDWEIGHT0` and `BLENDINDICES0` as stored with its bone palette
//! ([`GtaBlendStreams`]) and the drawable's skeleton ([`GtaSkeleton`]: each bone's local transform and its stored
//! inverse bind, [`Skeleton`]). This pass makes them what core states:
//!
//! - **The skeleton** (`core.skeleton`, on the prototype that holds the [`GtaSkeleton`], shared by its levels of detail
//!   and by the prototypes that refer to it with [`GtaSkeletonRef`], a component ped's parts): a joint per bone, its
//!   id the bone's index, its name the bone's tag, its parent and its local transform as stored (`Rest::Affine`),
//!   in the world's frame.
//! - **The skin** (`core.skin`, on the entity holding the mesh): each skinned primitive's `Joints(0)` and `Weights(0)`
//!   are the stored streams (four unsigned bytes a vertex each; weights that sum to 255, [`WeightRule::SumsToOne`]),
//!   its palette the geometry's: slot `k` is the joint of bone `palette[k]` with that bone's stored inverse bind
//!   (the inverse of the bone's pose, mesh space to the bone's space), converted into the world's frame. The palette
//!   ids are bone indices: measured on every ped geometry of the install (identity palettes), and refused as an error
//!   where an id is past the skeleton's bones.
//!
//! Exact where it applies: a zero-weight influence whose slot index is past the palette (padding) is set to slot 0,
//! which a vertex's blend cannot see (counted). Left as `gta.blend_streams`, counted by why: a primitive whose
//! weights do not sum to 255 on every vertex (three ped dictionaries store a layout not yet known), and every skinned
//! primitive of a drawable that carries no skeleton or no inverse binds (a ped component's dictionary entry is
//! skinned to the ped's skeleton, which its fragment carries).
use std::collections::BTreeMap;

use primase_core::pass::{Pass, PassLog, PassSpec};
use primase_core::skeleton::{Joint, JointId, Rest, Skeleton};
use primase_core::skin::{Binding, BindingId, Deformation, Influences, Skin, Slot, WeightRule};
use primase_core::{Entity, Lods, Mesh, Semantic, Stream, Transform, World};

use crate::components::{BlendStreams, GTA, GtaBlendStreams, GtaSkeleton, GtaSkeletonRef};

/// A transform of the form `[x, y, z, translation]` rows (`p' = A (p, 1)`) as core's columns.
fn affine(rows: &[[f32; 4]; 3]) -> Transform {
    Transform {
        cols: std::array::from_fn(|c| std::array::from_fn(|r| rows[r][c])),
    }
}

/// `lower-gta-skin` (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LowerGtaSkin;

/// A drawable's skeleton as core's, in the world's frame.
fn skeleton_of(s: &crate::rage::drawable::Skeleton) -> Skeleton {
    let joints = (0u32..)
        .zip(s.local.iter().zip(&s.parents))
        .map(|(i, (local, parent))| Joint {
            id: JointId(i),
            name: match s.tags.as_ref().and_then(|t| t.get(i as usize)) {
                Some(tag) => format!("{tag:#06x}"),
                None => i.to_string(),
            },
            parent: parent.map(|p| JointId(u32::from(p))),
            rest: Rest::Affine(affine(local)),
        })
        .collect();
    Skeleton { joints }.converted(&GTA)
}

/// `lower-gta-skin`'s declaration ([`primase_core::pass::PassRegistry::add`]): no options, it lowers by the game's own rule.
pub const LOWER_GTA_SKIN: PassSpec = PassSpec {
    name: "lower-gta-skin",
    options: &[],
    make: |_| Ok(Box::new(LowerGtaSkin)),
};

impl Pass for LowerGtaSkin {
    fn name(&self) -> &'static str {
        "lower-gta-skin"
    }
    fn provides(&self) -> &[&'static str] {
        &["core.skeleton", "core.skin"]
    }
    fn removes(&self) -> &[&'static str] {
        &["gta.blend_streams"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.blend_streams"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        // a level-of-detail form is skinned to its prototype's skeleton
        let owner: BTreeMap<Entity, Entity> = w
            .iter::<Lods>()
            .flat_map(|(proto, l)| l.forms.iter().map(move |f| (f.form, proto)))
            .collect();
        let holders: Vec<Entity> = w.iter::<GtaBlendStreams>().map(|(e, _)| e).collect();
        // the entities that got their core.skeleton
        let mut made: std::collections::BTreeSet<Entity> = std::collections::BTreeSet::new();
        for holder in holders {
            let proto = owner.get(&holder).copied().unwrap_or(holder);
            // the prototype's skeleton is its own, or the one it refers to
            let proto = w.get::<GtaSkeletonRef>(proto).map_or(proto, |r| r.0);
            let blend = w
                .get::<GtaBlendStreams>(holder)
                .map(|b| b.0.clone())
                .unwrap_or_default();
            let skinned = blend
                .iter()
                .flatten()
                .filter(|b| b.weights.is_some() && b.indices.is_some())
                .count() as u64;
            let Some(bones) = w.get::<GtaSkeleton>(proto).map(|s| s.0.skeleton.clone()) else {
                log.count(
                    "skinned primitives of a drawable with no skeleton, left as gta.blend_streams",
                    skinned,
                );
                continue;
            };
            let Some(bones) = bones else {
                log.count(
                    "skinned primitives of a drawable with no skeleton, left as gta.blend_streams",
                    skinned,
                );
                continue;
            };
            let Some(inverse) = bones.inverse.as_ref() else {
                log.count(
                    "skinned primitives of a skeleton with no inverse binds, left as gta.blend_streams",
                    skinned,
                );
                continue;
            };
            if w.has::<Skin>(holder) {
                continue;
            }
            let mesh = w
                .get::<Mesh>(holder)
                .ok_or_else(|| format!("{holder:?}: gta.blend_streams without core.mesh"))?;
            if mesh.primitives().len() != blend.len() {
                return Err(format!(
                    "{holder:?}: {} blend entries for {} primitives",
                    blend.len(),
                    mesh.primitives().len()
                ));
            }
            // per primitive: the streams and palette to lower, or the reason it stays as stored
            let mut lowered: Vec<Option<Lowered>> = Vec::with_capacity(blend.len());
            for (i, b) in blend.iter().enumerate() {
                lowered.push(match b {
                    Some(b) => lower_primitive(b, bones.local.len(), inverse, log)
                        .map_err(|e| format!("{holder:?} primitive {i}: {e}"))?,
                    None => None,
                });
            }
            if lowered.iter().all(Option::is_none) {
                continue;
            }
            let skeleton = proto;
            if made.insert(proto) {
                w.insert(proto, skeleton_of(&bones));
                log.count("skeletons made", 1);
            }
            // the streams onto the primitives
            let mut mesh = w.remove::<Mesh>(holder).ok_or("a mesh")?;
            let mut influences = Vec::with_capacity(lowered.len());
            let mut palettes = Vec::with_capacity(lowered.len());
            for (mut p, l) in mesh.primitives_mut().zip(&lowered) {
                match l {
                    Some(l) => {
                        p.streams_mut()
                            .insert(Semantic::Joints(0), Stream::U8x4(l.indices.clone()));
                        p.streams_mut()
                            .insert(Semantic::Weights(0), Stream::U8x4(l.weights.clone()));
                        influences.push(Some(Influences::Streams { sets: 1 }));
                        palettes.push(Some(
                            l.palette
                                .iter()
                                .map(|&id| Slot {
                                    joint: JointId(u32::from(id)),
                                    inverse_bind: affine(&inverse[usize::from(id)]),
                                })
                                .collect(),
                        ));
                        log.count("primitives skinned", 1);
                    }
                    None => {
                        influences.push(None);
                        palettes.push(None);
                    }
                }
            }
            w.insert(holder, mesh);
            let skin = Skin {
                influences,
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton,
                    palettes,
                    deformation: Deformation::Linear,
                }],
            };
            w.insert(holder, skin.converted(&GTA));
            // what was lowered leaves gta.blend_streams
            let rest: Vec<Option<BlendStreams>> = blend
                .into_iter()
                .zip(&lowered)
                .map(|(b, l)| if l.is_some() { None } else { b })
                .collect();
            if rest.iter().all(Option::is_none) {
                w.remove::<GtaBlendStreams>(holder);
            } else {
                w.insert(holder, GtaBlendStreams(rest));
            }
        }
        Ok(())
    }
}

/// One primitive's streams and palette, ready for core.
struct Lowered {
    indices: Vec<[u8; 4]>,
    weights: Vec<[u8; 4]>,
    palette: Vec<u16>,
}

/// A primitive's blend streams as a skin of a skeleton of `bones` bones with `inverse` binds: `None` (counted) when
/// they stay as stored, an error where the palette names a bone the skeleton does not have.
fn lower_primitive(
    b: &BlendStreams,
    bones: usize,
    inverse: &[[[f32; 4]; 3]],
    log: &mut PassLog,
) -> Result<Option<Lowered>, String> {
    let (Some(weights), Some(indices)) = (&b.weights, &b.indices) else {
        return Ok(None);
    };
    if b.palette.is_empty() {
        log.count(
            "skinned primitives with no bone palette, left as gta.blend_streams",
            1,
        );
        return Ok(None);
    }
    if let Some(&id) = b
        .palette
        .iter()
        .find(|&&id| usize::from(id) >= bones.min(inverse.len()))
    {
        return Err(format!("palette id {id} of a skeleton of {bones} bones"));
    }
    if weights
        .iter()
        .any(|w| w.iter().map(|&x| u32::from(x)).sum::<u32>() != 255)
    {
        log.count(
            "skinned primitives whose weights do not sum to 255, left as gta.blend_streams",
            1,
        );
        return Ok(None);
    }
    let mut padded = 0u64;
    let indices: Vec<[u8; 4]> = indices
        .iter()
        .zip(weights)
        .map(|(i, w)| {
            std::array::from_fn(|k| {
                if w[k] == 0 && usize::from(i[k]) >= b.palette.len() {
                    padded += 1;
                    0
                } else {
                    i[k]
                }
            })
        })
        .collect();
    log.count(
        "zero-weight influences past the palette set to slot 0 (padding)",
        padded,
    );
    // a weighted index past the palette cannot be skinned
    if let Some((v, k)) = indices
        .iter()
        .zip(weights)
        .flat_map(|(i, w)| i.iter().zip(w))
        .enumerate()
        .find(|(_, (i, w))| **w != 0 && usize::from(**i) >= b.palette.len())
        .map(|(n, _)| (n / 4, n % 4))
    {
        return Err(format!(
            "vertex {v} weights slot {} of a palette of {}",
            indices[v][k],
            b.palette.len()
        ));
    }
    Ok(Some(Lowered {
        indices,
        weights: weights.clone(),
        palette: b.palette.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rage::drawable::{Bones, Skeleton as Rage};
    use primase_core::{Primitive, Topology};

    const ID: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
    const UP: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 2.0]];
    const DOWN: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, -2.0]];

    fn triangle() -> Primitive {
        Primitive {
            topology: Topology::Triangles,
            streams: [(
                Semantic::Position,
                Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]
            .into(),
            indices: Some(vec![0, 1, 2]),
            material: None,
        }
    }

    /// A prototype of two bones (the second 2 m above the first in GTA's frame) with a skinned primitive and one whose
    /// weights do not sum to 255.
    fn world(second: BlendStreams) -> (World, Entity) {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![triangle(), triangle()]));
        w.insert(
            e,
            GtaBlendStreams(vec![
                Some(BlendStreams {
                    weights: Some(vec![[255, 0, 0, 0], [128, 127, 0, 0], [0, 255, 0, 0]]),
                    // the last lane of a padding influence names slot 7 of a palette of two
                    indices: Some(vec![[0, 0, 0, 0], [0, 1, 0, 7], [1, 0, 0, 0]]),
                    palette: vec![1, 0],
                }),
                Some(second),
            ]),
        );
        w.insert(
            e,
            GtaSkeleton(Bones {
                skeleton: Some(Rage {
                    local: vec![ID, UP],
                    parents: vec![None, Some(0)],
                    tags: Some(vec![0, 0x2e28]),
                    inverse: Some(vec![ID, DOWN]),
                }),
                fragment_pose: Vec::new(),
            }),
        );
        (w, e)
    }

    /// Hand-worked: the palette `[1, 0]` makes slot 0 bone 1 (inverse bind: 2 m down) and slot 1 bone 0; the stored
    /// streams become `Joints(0)` and `Weights(0)` as they are (padding slot 7 set to 0); a primitive whose weights
    /// sum to 254 stays in `gta.blend_streams`; the frame is GTA's (the z of a bone, 2 m up, is the world's y).
    #[test]
    fn a_skinned_primitive_binds_the_palette_to_the_stored_bones() {
        let odd = BlendStreams {
            weights: Some(vec![[254, 0, 0, 0]; 3]),
            indices: Some(vec![[0; 4]; 3]),
            palette: vec![0],
        };
        let (mut w, e) = world(odd.clone());
        let mut log = PassLog::default();
        LowerGtaSkin.run(&mut w, &mut log).unwrap();
        assert_eq!(w.verify_components(), vec![]);
        let skin = w.get::<Skin>(e).unwrap();
        assert_eq!(skin.influences, vec![Some(Influences::Streams { sets: 1 }), None]);
        let binding = &skin.bindings[0];
        let slots = binding.palettes[0].as_ref().unwrap();
        assert_eq!(
            slots.iter().map(|s| s.joint).collect::<Vec<_>>(),
            [JointId(1), JointId(0)]
        );
        // bone 1's inverse bind is 2 m down in GTA's z: 2 m down the world's y
        assert_eq!(slots[0].inverse_bind.cols[3], [0.0, -2.0, 0.0]);
        assert_eq!(slots[1].inverse_bind, Transform::IDENTITY);
        assert!(binding.palettes[1].is_none());
        let p = &w.get::<Mesh>(e).unwrap().primitives()[0];
        assert_eq!(
            p.streams[&Semantic::Weights(0)],
            Stream::U8x4(vec![[255, 0, 0, 0], [128, 127, 0, 0], [0, 255, 0, 0]])
        );
        assert_eq!(
            p.streams[&Semantic::Joints(0)],
            Stream::U8x4(vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]])
        );
        // the skeleton: bone 1 above bone 0, named by its tag
        let sk = w.get::<Skeleton>(binding.skeleton).unwrap();
        assert_eq!(
            (sk.joints[1].parent, sk.joints[1].name.as_str()),
            (Some(JointId(0)), "0x2e28")
        );
        assert_eq!(sk.joints[1].rest.matrix().cols[3], [0.0, 2.0, 0.0]);
        // what was lowered left gta.blend_streams; the odd primitive stayed
        assert_eq!(w.get::<GtaBlendStreams>(e).unwrap().0, vec![None, Some(odd)]);
        assert_eq!(log.counts["primitives skinned"], 1);
        assert_eq!(
            log.counts["skinned primitives whose weights do not sum to 255, left as gta.blend_streams"],
            1
        );
        assert_eq!(
            log.counts["zero-weight influences past the palette set to slot 0 (padding)"],
            1
        );
    }

    /// A palette id past the skeleton's bones is an error; a drawable without a skeleton (a ped component) keeps its
    /// streams, counted.
    #[test]
    fn a_palette_past_the_skeleton_is_refused_and_no_skeleton_leaves_the_streams() {
        let past = BlendStreams {
            weights: Some(vec![[255, 0, 0, 0]; 3]),
            indices: Some(vec![[0; 4]; 3]),
            palette: vec![5],
        };
        let (mut w, _) = world(past);
        let error = LowerGtaSkin.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(error.contains("palette id 5"), "{error}");

        let (mut w, e) = world(BlendStreams {
            weights: None,
            indices: None,
            palette: vec![],
        });
        w.remove::<GtaSkeleton>(e);
        let mut log = PassLog::default();
        LowerGtaSkin.run(&mut w, &mut log).unwrap();
        assert!(w.get::<Skin>(e).is_none() && w.get::<GtaBlendStreams>(e).is_some());
        assert_eq!(
            log.counts["skinned primitives of a drawable with no skeleton, left as gta.blend_streams"],
            1
        );
    }

    /// A prototype that refers to another's skeleton (a ped's part and the ped's fragment) is skinned to it: the
    /// core.skeleton goes on the referred entity, once for any number of parts.
    #[test]
    fn a_part_is_skinned_to_the_skeleton_it_refers_to() {
        let (mut w, part) = world(BlendStreams {
            weights: None,
            indices: None,
            palette: vec![],
        });
        // the skeleton moves to an entity of its own
        let bones = w.remove::<GtaSkeleton>(part).unwrap();
        let rig = w.spawn();
        w.insert(rig, bones);
        w.insert(part, GtaSkeletonRef(rig));
        let second = w.spawn();
        w.insert(second, w.get::<Mesh>(part).unwrap().clone());
        w.insert(second, w.get::<GtaBlendStreams>(part).unwrap().clone());
        w.insert(second, GtaSkeletonRef(rig));
        let mut log = PassLog::default();
        LowerGtaSkin.run(&mut w, &mut log).unwrap();
        assert_eq!(w.verify_components(), vec![]);
        for e in [part, second] {
            assert_eq!(w.get::<Skin>(e).unwrap().bindings[0].skeleton, rig);
            assert!(!w.has::<Skeleton>(e));
        }
        assert!(w.has::<Skeleton>(rig));
        assert_eq!(log.counts["skeletons made"], 1);
        assert_eq!(log.counts["primitives skinned"], 2);
    }
}
