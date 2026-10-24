//! The glTF reader: a glTF document (a GLB, or a `.gltf` with its buffers, as [`primase_glb::Gltf`] parses it) into a
//! world of one asset, faithfully: what the document states, in core's form where core states the same meaning, the
//! rest reported as not read. The document's default scene is read ([`Gltf::scene_roots`]); nodes outside it are
//! counted.
//!
//! What goes where:
//! - **Skeletons.** Every node a skin in the scene names as a joint, and every node above one up to its scene root,
//!   is a joint of a `core.skeleton`, one skeleton per scene root they hang from: each joint's id its node's index,
//!   its name the node's, its parent its parent node, its rest the node's local transform as stored (`Rest::Trs` from
//!   translation/rotation/scale, `Rest::Affine` from a matrix). Core has no general node hierarchy, so the nodes
//!   above the joints are joints too: their transforms (and channels that animate them) are kept exactly, with no
//!   composing. Each skeleton has one rig (`core.rig_instance`) at the origin, the scene's space.
//! - **Meshes** each glTF mesh a scene node draws becomes a prototype (`core.mesh`, its name), its primitives drawing
//!   the material entities. **Materials** are read as their glTF JSON (`gltf.material`, with `core.name`), and the
//!   images their core texture slots sample as `core.texture`s with a `gltf.material_textures` component ([`crate::textures`]:
//!   PNG decoded, other formats refused by name and counted), and those of `PRIMASE_materials_detail_blend`'s slots
//!   with `gltf.detail_textures`; `lower-gltf-materials` ([`crate::lower`]) turns them into `pbr.material` and
//!   `pbr.detail` ([`crate::detail`]). A mesh drawn with a skin gets `core.skin`: per primitive with joint
//!   streams, `Streams` influences of its `JOINTS_n`/`WEIGHTS_n` sets as stored; one binding per glTF skin it is
//!   drawn with (`BindingId` the skin's index), its palette the skin's joints in order with their inverse bind
//!   matrices as stored (identity where the skin gives none), deformation linear (glTF's). The weight rule is
//!   `SumsToOne` (glTF's) when every vertex's weights sum to 1 within core's tolerance, else `AsStored`, counted
//!   (glTF evaluates them as stored and only asks that they sum to 1). Joint streams of a mesh no scene node draws
//!   with a skin are not read, counted. Morph targets become `core.morphs`: target `k` is `MorphId(k)`, named by
//!   `extras.targetNames` (else `target k`), its default the mesh's `weights` (else 0), one frame at weight 1 of the
//!   primitives' deltas.
//! - **Placements**: each scene node with a mesh is a placement (`core.instance`, `core.transform` its world
//!   transform at the stored pose, composed in f64 and rounded once, its node's name). With a skin it gets
//!   `core.skin_instance` (its skeleton's rig, the skin's binding); with node `weights`, `core.morph_weights`. A node
//!   drawn without a skin at or under a skeleton's joint follows it: a socket on that joint (named by the node,
//!   identity offset; `core.sockets` on the skeleton) and `core.attachment` (the rig, that socket, the product of the
//!   local transforms from the joint down to the node).
//! - **Animations**: each becomes a `core.clip` (its name, `Hold` extrapolation, its duration its last key) bound by
//!   a `core.clip_binding` to its rig and the placements whose morph weights it drives. A translation, rotation or
//!   scale channel on a joint drives that joint; a `weights` channel drives each morph target of its node's mesh as
//!   a channel of its own (the values split per target, unchanged). Interpolation as stored: `STEP`, `LINEAR` and
//!   `CUBICSPLINE` are core's `Step`, `Linear` and `Hermite` (glTF's layout and rules). An animation driving joints
//!   of several skeletons is one clip a skeleton, its morph channels with the first. Channels on nodes that are not
//!   joints have no core target: not read, counted.
//! - **The asset**: a root with `core.asset` (each skeleton a rig, each placement a part, each clip a clip),
//!   `core.source` the caller's key and `core.name`.
//!
//! Not read, counted: cameras, the `extras` of the objects read (but a mesh's `targetNames`), the skins' `skeleton` hints (no effect on deformation), images no core material slot
//! samples, and the images refused by name.
use std::collections::{BTreeMap, BTreeSet};

use primase_animation::pose::{Affine, affine, compose, transform};
use primase_core::convert::{Notice, NoticeKind};
use primase_core::{
    Asset, Attachment, Binding, BindingId, Clip, ClipBinding, ClipTarget, Deformation, Deltas, Entity,
    Extrapolation, Influences, Instance, Interpolation, Joint, JointId, MorphFrame, MorphId, MorphTarget,
    MorphWeights, Morphs, Name, Opaque, OpaqueData, Property, Rest, RigInstance, Role, Semantic, Skeleton,
    Skin, SkinInstance, Slot, Socket, SocketId, Sockets, Source, Stream, Transform, WeightRule, World,
};
use primase_glb::{AnimationInterpolation, AnimationPath, Gltf, Local, ReadMesh, ReadNode, ReadSkin};
use serde_json::Value;

use crate::textures::{ImageResolver, read_textures};
use crate::{Error, Result};

/// The component a material's glTF JSON is read into.
pub const MATERIAL: &str = "gltf.material";

/// What [`read`] read, and what it did not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    pub placements: usize,
    pub meshes: usize,
    pub materials: usize,
    pub skeletons: usize,
    pub joints: usize,
    /// Glb skins read as bindings (one a skin and mesh it is drawn with).
    pub bindings: usize,
    pub morph_targets: usize,
    pub attachments: usize,
    pub clips: usize,
    pub channels: usize,
    /// Meshes whose skinned vertices' weights do not all sum to 1: read `AsStored`.
    pub weights_as_stored: usize,
    /// Not read: nodes outside the default scene, cameras, skins' skeleton hints, animation channels on nodes that
    /// are not joints, joint streams of meshes no node draws with a skin.
    pub nodes_outside_scene: usize,
    pub cameras: usize,
    pub skeleton_hints: usize,
    /// Objects the reader read whose `extras` it keeps nowhere (a mesh's `targetNames` is read; other keys, and the extras
    /// of primitives, nodes, skins, animations and the scene, are not).
    pub extras_not_read: usize,
    pub channels_off_joints: usize,
    pub unskinned_joint_streams: usize,
    /// The document's textures and images, and what became of its images: those a core material slot samples were
    /// read (`core.texture`, [`crate::textures`]) or refused by name (the reason and how many images), the others
    /// not read.
    pub textures: usize,
    pub images: usize,
    pub images_read: usize,
    pub images_refused: BTreeMap<String, usize>,
    pub images_unused: usize,
}

impl ReadReport {
    /// What it did not read, as notices (stage `gltf`).
    pub fn notices(&self) -> Vec<Notice> {
        let n =
            |reason: &str, count: usize| Notice::new("gltf", NoticeKind::Unsupported, reason, count as u64);
        [
            n(
                "nodes outside the default scene, not read",
                self.nodes_outside_scene,
            ),
            n("cameras, not read", self.cameras),
            n(
                "extras of meshes, primitives, nodes, skins, animations and the scene (other than a mesh's targetNames), not read",
                self.extras_not_read,
            ),
            n(
                "skins' skeleton hints, not read (no effect on deformation)",
                self.skeleton_hints,
            ),
            n(
                "animation channels on nodes that are not joints (no core target), not read",
                self.channels_off_joints,
            ),
            n(
                "meshes' joint and weight streams no node draws with a skin, not read",
                self.unskinned_joint_streams,
            ),
            n(
                "images no core material slot samples, not read",
                self.images_unused,
            ),
        ]
        .into_iter()
        .chain(
            self.images_refused
                .iter()
                .map(|(reason, count)| n(&format!("images refused: {reason}"), *count)),
        )
        .filter(|x| x.count > 0)
        .collect()
    }
}

/// A node's local transform as an affine map (f64).
fn local(l: &Local) -> Affine {
    affine(&rest(l).matrix())
}

/// A node's local transform as a joint's rest, as stored.
fn rest(l: &Local) -> Rest {
    match *l {
        Local::Matrix(cols) => Rest::Affine(Transform { cols }),
        Local::Trs {
            translation,
            rotation,
            scale,
        } => Rest::Trs {
            translation,
            rotation,
            scale,
        },
    }
}

/// A node's name, else `node <i>`.
fn node_name(n: &ReadNode, i: usize) -> String {
    n.name.clone().unwrap_or_else(|| format!("node {i}"))
}

fn invalid(item: impl Into<String>, detail: impl Into<String>) -> Error {
    Error::Invalid {
        item: item.into(),
        detail: detail.into(),
    }
}

/// The world's skeletons: for each, its entity, its rig and its nodes.
struct Skeletons {
    /// Node -> (skeleton entity, rig entity), for every joint node.
    of: BTreeMap<usize, (Entity, Entity)>,
}

/// Every vertex's weights in `p` sum to 1 within core's tolerance.
fn sums_to_one(p: &primase_core::Primitive, sets: u8) -> bool {
    let weights = |s: &Stream| -> Option<Vec<[f32; 4]>> {
        Some(match s {
            Stream::F32x4(v) => v.clone(),
            Stream::U8x4(v) => v.iter().map(|w| w.map(|x| f32::from(x) / 255.0)).collect(),
            Stream::U16x4(v) => v.iter().map(|w| w.map(|x| f32::from(x) / 65535.0)).collect(),
            _ => return None,
        })
    };
    let n = p.streams.get(&Semantic::Position).map_or(0, Stream::len);
    let mut sums = vec![0.0f32; n];
    for k in 0..sets {
        let Some(w) = p.streams.get(&Semantic::Weights(k)).and_then(weights) else {
            return false;
        };
        for (s, w) in sums.iter_mut().zip(w) {
            *s += w.iter().sum::<f32>();
        }
    }
    sums.iter()
        .all(|s| (s - 1.0).abs() <= primase_core::skin::SUM_TOLERANCE)
}

/// The number of `JOINTS_n` sets of a primitive (0 when it has none).
fn joint_sets(p: &primase_core::Primitive) -> u8 {
    (0..=u8::MAX)
        .take_while(|k| p.streams.contains_key(&Semantic::Joints(*k)))
        .count() as u8
}

/// Read `gltf` into `world`, which must be empty, as one asset whose source is `source` (see the module
/// documentation). Images the document stores in its buffers are read; one it names by `uri` is refused (counted),
/// as no resolver is given: [`read_with_images`] takes one.
pub fn read(gltf: &Gltf, source: &str, world: &mut World) -> Result<ReadReport> {
    read_with_images(gltf, source, world, &mut |_| {
        Err("no image resolver was given".to_owned())
    })
}

/// [`read`], fetching the images the document names by `uri` through `resolve` (a relative file, a `data:` URI:
/// resolving is the caller's, as for [`Gltf::from_gltf`]'s buffers).
pub fn read_with_images(
    gltf: &Gltf,
    source: &str,
    world: &mut World,
    resolve: &mut ImageResolver,
) -> Result<ReadReport> {
    if world.entities().next().is_some() {
        return Err(invalid("glTF reader", "the world must be empty"));
    }
    let mut report = ReadReport::default();
    let json = gltf.json();
    let count = |key: &str| json.get(key).and_then(Value::as_array).map_or(0, Vec::len);
    report.textures = count("textures");
    report.images = count("images");
    let nodes = gltf.nodes()?;
    let skins = gltf.skins()?;
    let animations = gltf.animations()?;
    let roots = gltf.scene_roots()?.unwrap_or_default();
    report.extras_not_read += usize::from(gltf.scene_extras()?.is_some());

    // the scene's nodes, their parents and world transforms (composed in f64 from the roots down)
    let mut parent: BTreeMap<usize, usize> = BTreeMap::new();
    let mut root_of: BTreeMap<usize, usize> = BTreeMap::new();
    let mut world_of: BTreeMap<usize, Affine> = BTreeMap::new();
    let mut order = Vec::new();
    let mut stack: Vec<(usize, Option<usize>)> = roots.iter().rev().map(|&r| (r, None)).collect();
    while let Some((n, p)) = stack.pop() {
        let at = local(&nodes[n].local);
        let (w, root) = match p {
            Some(p) => (compose(&world_of[&p], &at), root_of[&p]),
            None => (at, n),
        };
        if let Some(p) = p {
            parent.insert(n, p);
        }
        root_of.insert(n, root);
        world_of.insert(n, w);
        order.push(n);
        stack.extend(nodes[n].children.iter().rev().map(|&c| (c, Some(n))));
    }
    let in_scene: BTreeSet<usize> = order.iter().copied().collect();
    report.nodes_outside_scene = nodes.len() - in_scene.len();
    report.cameras = order.iter().filter(|&&n| nodes[n].camera.is_some()).count();
    report.extras_not_read += order.iter().filter(|&&n| nodes[n].extras.is_some()).count();

    // the skins drawn in the scene, and their joints with every node above them: the skeletons' nodes
    let drawn_skins: BTreeSet<usize> = order.iter().filter_map(|&n| nodes[n].skin).collect();
    let mut skeleton_nodes: BTreeSet<usize> = BTreeSet::new();
    for &s in &drawn_skins {
        let skin = &skins[s];
        if skin.skeleton.is_some() {
            report.skeleton_hints += 1;
        }
        report.extras_not_read += usize::from(skin.extras.is_some());
        for &j in &skin.joints {
            if !in_scene.contains(&j) {
                return Err(invalid(
                    format!("skins[{s}]"),
                    format!("joint {j} is not in the scene its skinned nodes are"),
                ));
            }
            let mut at = Some(j);
            while let Some(n) = at {
                if !skeleton_nodes.insert(n) {
                    break;
                }
                at = parent.get(&n).copied();
            }
        }
    }
    let w = world;
    let mut skeletons = Skeletons { of: BTreeMap::new() };
    let mut by_root: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for &n in &skeleton_nodes {
        by_root.entry(root_of[&n]).or_default().push(n);
    }
    let mut rigs: Vec<(Entity, Entity)> = Vec::new();
    for (root, members) in &by_root {
        let skeleton = w.spawn();
        let joints: Vec<Joint> = members
            .iter()
            .map(|&n| Joint {
                id: JointId(n as u32),
                name: node_name(&nodes[n], n),
                parent: parent.get(&n).map(|&p| JointId(p as u32)),
                rest: rest(&nodes[n].local),
            })
            .collect();
        report.joints += joints.len();
        w.insert(
            skeleton,
            Name(format!("skeleton {}", node_name(&nodes[*root], *root))),
        );
        w.insert(skeleton, Skeleton { joints });
        let rig = w.spawn();
        w.insert(rig, Name(format!("rig {}", node_name(&nodes[*root], *root))));
        w.insert(rig, RigInstance { skeleton });
        w.insert(rig, Transform::IDENTITY);
        for &n in members {
            skeletons.of.insert(n, (skeleton, rig));
        }
        rigs.push((skeleton, rig));
    }
    report.skeletons = rigs.len();
    // each skin's skeleton: all its joints hang from one scene root
    let skin_skeleton = |s: usize| -> Result<(Entity, Entity)> {
        let first = skeletons.of[&skins[s].joints[0]];
        if skins[s].joints.iter().any(|j| skeletons.of[j] != first) {
            return Err(invalid(
                format!("skins[{s}]"),
                "its joints hang from different scene roots: not one skeleton",
            ));
        }
        Ok(first)
    };

    // materials, as their glTF JSON
    let materials_json = json
        .get("materials")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut material_entities: BTreeMap<usize, Entity> = BTreeMap::new();
    let mut material = |w: &mut World, i: usize| -> Entity {
        *material_entities.entry(i).or_insert_with(|| {
            let e = w.spawn();
            let m = &materials_json[i];
            w.insert(
                e,
                Name(
                    m.get("name")
                        .and_then(Value::as_str)
                        .map_or_else(|| format!("material {i}"), str::to_owned),
                ),
            );
            w.insert_opaque(
                e,
                MATERIAL,
                Opaque {
                    version: 1,
                    data: OpaqueData::Json(m.clone()),
                },
            )
            .expect("gltf.material is no typed component's name");
            e
        })
    };

    // prototypes: each mesh a scene node draws, with the skins it is drawn with
    let mut drawn_with: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for &n in &order {
        if let Some(m) = nodes[n].mesh {
            let skins_of = drawn_with.entry(m).or_default();
            skins_of.extend(nodes[n].skin);
        }
    }
    let mut prototypes: BTreeMap<usize, (Entity, usize)> = BTreeMap::new();
    for (&m, skins_of) in &drawn_with {
        let read = gltf.read_mesh(m)?;
        let e = w.spawn();
        let ReadMesh {
            name,
            primitives,
            weights,
            extras,
        } = read;
        // a mesh's `targetNames` are read below; any other key of its extras is not
        report.extras_not_read += usize::from(
            extras
                .as_ref()
                .is_some_and(|x| x.as_object().is_none_or(|o| o.keys().any(|k| k != "targetNames"))),
        );
        report.extras_not_read += primitives.iter().filter(|p| p.extras.is_some()).count();
        w.insert(e, Name(name.unwrap_or_else(|| format!("mesh {m}"))));
        let mut prims = Vec::with_capacity(primitives.len());
        for p in &primitives {
            let mut prim = p.primitive.clone();
            prim.material = p.material.map(|i| material(w, i));
            prims.push(prim);
        }
        // skin
        if skins_of.is_empty() {
            let mut dropped = false;
            for p in &mut prims {
                let before = p.streams.len();
                p.streams
                    .retain(|s, _| !matches!(s, Semantic::Joints(_) | Semantic::Weights(_)));
                dropped |= p.streams.len() != before;
            }
            report.unskinned_joint_streams += usize::from(dropped);
        } else {
            let influences: Vec<Option<Influences>> = prims
                .iter()
                .map(|p| match joint_sets(p) {
                    0 => None,
                    sets => Some(Influences::Streams { sets }),
                })
                .collect();
            if influences.iter().all(Option::is_none) {
                return Err(invalid(
                    format!("meshes[{m}]"),
                    "drawn with a skin but no primitive has JOINTS_0",
                ));
            }
            let summed = prims.iter().zip(&influences).all(|(p, i)| match i {
                Some(Influences::Streams { sets }) => sums_to_one(p, *sets),
                _ => true,
            });
            if !summed {
                report.weights_as_stored += 1;
            }
            let mut bindings = Vec::new();
            for &s in skins_of {
                let (skeleton, _) = skin_skeleton(s)?;
                let ReadSkin {
                    joints,
                    inverse_binds,
                    ..
                } = &skins[s];
                let palette: Vec<Slot> = joints
                    .iter()
                    .zip(inverse_binds)
                    .map(|(&j, &cols)| Slot {
                        joint: JointId(j as u32),
                        inverse_bind: Transform { cols },
                    })
                    .collect();
                bindings.push(Binding {
                    id: BindingId(s as u32),
                    skeleton,
                    palettes: influences.iter().map(|i| i.map(|_| palette.clone())).collect(),
                    deformation: Deformation::Linear,
                });
            }
            report.bindings += bindings.len();
            w.insert(
                e,
                Skin {
                    influences,
                    weights: if summed {
                        WeightRule::SumsToOne
                    } else {
                        WeightRule::AsStored
                    },
                    bindings,
                },
            );
        }
        // morph targets
        let targets = primitives.first().map_or(0, |p| p.targets.len());
        if targets > 0 {
            let names: Vec<Option<String>> = extras
                .as_ref()
                .and_then(|x| x.get("targetNames"))
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let morphs = Morphs {
                targets: (0..targets)
                    .map(|k| MorphTarget {
                        id: MorphId(k as u32),
                        name: names
                            .get(k)
                            .cloned()
                            .flatten()
                            .unwrap_or_else(|| format!("target {k}")),
                        default_weight: weights.as_ref().map_or(0.0, |d| d[k]),
                        frames: vec![MorphFrame {
                            weight: 1.0,
                            primitives: primitives
                                .iter()
                                .map(|p| {
                                    let t = &p.targets[k];
                                    Some(Deltas {
                                        indices: None,
                                        positions: t.position.clone(),
                                        normals: t.normal.clone(),
                                        tangents: t.tangent.clone(),
                                    })
                                })
                                .collect(),
                        }],
                    })
                    .collect(),
            };
            report.morph_targets += targets;
            w.insert(e, morphs);
        }
        w.insert(e, primase_core::Mesh::new(prims));
        prototypes.insert(m, (e, targets));
    }
    report.meshes = prototypes.len();
    report.materials = material_entities.len();

    // placements
    let mut placements: BTreeMap<usize, Entity> = BTreeMap::new();
    let mut sockets: BTreeMap<Entity, Vec<Socket>> = BTreeMap::new();
    for &n in &order {
        let node = &nodes[n];
        let Some(m) = node.mesh else { continue };
        let (prototype, targets) = prototypes[&m];
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(p, transform(&world_of[&n]));
        w.insert(p, Name(node_name(node, n)));
        if let Some(s) = node.skin {
            let (_, rig) = skin_skeleton(s)?;
            w.insert(
                p,
                SkinInstance {
                    rig,
                    binding: BindingId(s as u32),
                },
            );
        } else {
            // the nearest joint at or above the node: it follows it
            let mut offset: Affine = affine(&Transform::IDENTITY);
            let mut at = n;
            let joint = loop {
                if skeletons.of.contains_key(&at) {
                    break Some(at);
                }
                offset = compose(&local(&nodes[at].local), &offset);
                match parent.get(&at) {
                    Some(&up) => at = up,
                    None => break None,
                }
            };
            if let Some(j) = joint {
                let (skeleton, rig) = skeletons.of[&j];
                let list = sockets.entry(skeleton).or_default();
                let id = SocketId(list.len() as u32);
                list.push(Socket {
                    id,
                    name: node_name(node, n),
                    joint: JointId(j as u32),
                    offset: Transform::IDENTITY,
                });
                w.insert(
                    p,
                    Attachment {
                        rig,
                        socket: id,
                        offset: transform(&offset),
                    },
                );
                report.attachments += 1;
            }
        }
        if let Some(weights) = &node.weights {
            if weights.len() != targets {
                return Err(invalid(
                    format!("nodes[{n}]"),
                    format!("{} weights for a mesh of {targets} morph targets", weights.len()),
                ));
            }
            w.insert(
                p,
                MorphWeights(
                    weights
                        .iter()
                        .enumerate()
                        .map(|(k, &x)| (MorphId(k as u32), x))
                        .collect(),
                ),
            );
        }
        placements.insert(n, p);
    }
    for (skeleton, list) in sockets {
        w.insert(skeleton, Sockets(list));
    }
    report.placements = placements.len();

    // animations
    let mut clips = Vec::new();
    for (i, a) in animations.iter().enumerate() {
        report.extras_not_read += usize::from(a.extras.is_some());
        let name = a.name.clone().unwrap_or_else(|| format!("animation {i}"));
        // channels by skeleton (None: morph channels on no joint), morph-driven placements by skeleton
        let mut groups: BTreeMap<Option<Entity>, (Vec<primase_core::clip::Channel>, BTreeSet<Entity>)> =
            BTreeMap::new();
        let mut morph_channels = Vec::new();
        let mut morph_meshes = BTreeSet::new();
        for c in &a.channels {
            let interpolation = match c.interpolation {
                AnimationInterpolation::Step => Interpolation::Step,
                AnimationInterpolation::Linear => Interpolation::Linear,
                AnimationInterpolation::CubicSpline => Interpolation::Hermite,
            };
            let property = match c.path {
                AnimationPath::Translation => Property::Translation,
                AnimationPath::Rotation => Property::Rotation,
                AnimationPath::Scale => Property::Scale,
                AnimationPath::Weights => {
                    let node = &nodes[c.node];
                    let (Some(m), Some(&p)) = (node.mesh, placements.get(&c.node)) else {
                        if in_scene.contains(&c.node) {
                            return Err(invalid(
                                format!("animations[{i}]"),
                                format!("a weights channel on node {} with no mesh", c.node),
                            ));
                        }
                        report.channels_off_joints += 1;
                        continue;
                    };
                    let (geometry, targets) = prototypes[&m];
                    let keys = c.times.len();
                    let per = if interpolation == Interpolation::Hermite {
                        3
                    } else {
                        1
                    };
                    if targets == 0 || c.values.len() != keys * per * targets {
                        return Err(invalid(
                            format!("animations[{i}]"),
                            format!(
                                "a weights channel of {} values for {keys} keys and {targets} morph targets",
                                c.values.len()
                            ),
                        ));
                    }
                    for k in 0..targets {
                        // per key (and per tangent part), target k's value of the `targets` the key holds
                        let values = (0..keys * per).map(|part| c.values[part * targets + k]).collect();
                        morph_channels.push(primase_core::clip::Channel {
                            target: ClipTarget::Morph {
                                geometry,
                                morph: MorphId(k as u32),
                            },
                            property: Property::Scalar,
                            interpolation,
                            times: c.times.clone(),
                            values,
                        });
                    }
                    morph_meshes.insert(p);
                    continue;
                }
            };
            let Some(&(skeleton, _)) = skeletons.of.get(&c.node) else {
                report.channels_off_joints += 1;
                continue;
            };
            groups
                .entry(Some(skeleton))
                .or_default()
                .0
                .push(primase_core::clip::Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(c.node as u32),
                    },
                    property,
                    interpolation,
                    times: c.times.clone(),
                    values: c.values.clone(),
                });
        }
        if !morph_channels.is_empty() {
            let first = groups.keys().next().copied().unwrap_or(None);
            let g = groups.entry(first).or_default();
            g.0.extend(morph_channels);
            g.1.extend(morph_meshes);
        }
        let split = groups.len() > 1;
        for (k, (skeleton, (channels, meshes))) in groups.into_iter().enumerate() {
            let duration = channels
                .iter()
                .filter_map(|c| c.times.last().copied())
                .fold(0.0f32, f32::max);
            let clip = w.spawn();
            let clip_name = if split {
                format!("{name} [{k}]")
            } else {
                name.clone()
            };
            report.channels += channels.len();
            w.insert(clip, Name(clip_name.clone()));
            w.insert(
                clip,
                Clip {
                    name: clip_name,
                    duration,
                    extrapolation: Extrapolation::Hold,
                    channels,
                    root_motion: None,
                    additive: None,
                },
            );
            let rig = skeleton.and_then(|s| rigs.iter().find(|(sk, _)| *sk == s).map(|&(_, r)| r));
            let binding = w.spawn();
            w.insert(
                binding,
                ClipBinding {
                    clip,
                    rig,
                    meshes: meshes.into_iter().collect(),
                },
            );
            clips.push(clip);
        }
    }
    report.clips = clips.len();

    // the images the materials' slots sample
    let images = read_textures(gltf, source, w, &material_entities, resolve)?;
    report.images_read = images.read;
    report.images_refused = images.refused;
    report.images_unused = images.unused;

    // the asset
    let root = w.spawn();
    w.insert(root, Source(source.to_owned()));
    w.insert(root, Name(source.to_owned()));
    let members = rigs
        .iter()
        .map(|&(s, _)| (s, Role::Rig))
        .chain(placements.values().map(|&p| (p, Role::Part)))
        .chain(clips.iter().map(|&c| (c, Role::Clip)))
        .collect();
    w.insert(root, Asset { members });
    // whatever the world's own verifier refuses, the reader refuses here by name: a document with a joint index outside its
    // skin's palette, a negative weight, a rotation that is not a unit quaternion or a transform that overflows is not a
    // world the passes can be handed
    if let Some((component, entity, detail)) = w.verify_components().into_iter().next() {
        let who = w
            .get::<Name>(entity)
            .map_or_else(|| format!("{entity:?}"), |n| format!("{:?} ({entity:?})", n.0));
        return Err(invalid(
            "glTF document",
            format!("{component} of {who} does not verify: {detail}"),
        ));
    }
    if let Some((resource, detail)) = w.verify_resources().into_iter().next() {
        return Err(invalid(
            "glTF document",
            format!("{resource} does not verify: {detail}"),
        ));
    }
    Ok(report)
}

/// [`read`] from GLB bytes.
pub fn read_glb(bytes: &[u8], source: &str, world: &mut World) -> Result<ReadReport> {
    read(&Gltf::from_glb(bytes)?, source, world)
}
