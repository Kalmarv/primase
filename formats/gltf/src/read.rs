//! The glTF reader: a glTF document (a GLB, or a `.gltf` with its buffers, as [`primase_glb::Gltf`] parses it) into a
//! world of one asset, faithfully: what the document states, in core's form where core states the same meaning, the
//! rest reported as not read. The document's default scene is read ([`Gltf::scene_roots`]); nodes outside it are
//! counted.
//!
//! What goes where:
//! - **Skeletons.** One `core.skeleton` per scene root the joints of the skins drawn in the scene hang from, its
//!   joints that root's tree: every node a drawn skin names as a joint, every node above one, and every other node of
//!   the tree that is no mesh or light node (nor below one), an unweighted joint as much as a weighted one. Each
//!   joint's id is its node's index, its name the node's, its parent its parent node (none below the rig's node), its
//!   rest the node's local transform as stored (`Rest::Trs` from translation/rotation/scale, `Rest::Affine` from a
//!   matrix). Core has no general node hierarchy, so the nodes above the joints are joints too: their transforms (and
//!   channels that animate them) are kept exactly, with no composing. Each skeleton has one rig
//!   (`core.rig_instance`). The writer writes a rig as a root node of its own (the rig's name and transform) with the
//!   joints below it ([`crate::write`]), and the reader reads that back as it is written: a root that no drawn skin
//!   names as a joint, with no mesh, light or camera and no animation channel, is the rig (its name the node's,
//!   `core.transform` its local transform), not a joint. Any other root is a joint, and the rig is at the origin, the
//!   scene's space.
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
//! - **Lights** (`KHR_lights_punctual`): each scene node naming one of the document's lights is a `core.light` entity
//!   (the node's name, `core.transform` its world transform at the stored pose, as a placement's): the light's type,
//!   colour, intensity, range and a spot's cones as stored, every default the extension's (white, intensity 1, no
//!   range, cones 0 and pi / 4). Its `cast_shadows` is the node's `extras["primase.shadows"].cast` (`"on"`, `"off"`),
//!   unknown where the node states none. A light on or under a skeleton's joint stays where the stored pose puts it
//!   (core's lights follow no rig), counted; a light no scene node places is counted.
//! - **Shadows**: a mesh node's `extras["primase.shadows"]` (version 1: `cast` `"off"`, `"on"` or `"only"`, `receive`
//!   a boolean) becomes its placement's `core.shadows`.
//! - **The asset**: a root with `core.asset` (each skeleton a rig, each placement a part, each clip a clip),
//!   `core.source` the caller's key and `core.name`.
//!
//! Not read, counted: cameras, scene nodes read as nothing of their own (no joint, rig, mesh, light or camera node:
//! their transforms are composed into the nodes below them, their names and grouping are not read), the skins'
//! `skeleton` hints (no effect on deformation), images no core material slot
//! samples, the images refused by name, every `extras` key of the objects read that is not read above (a mesh's
//! `targetNames`, a node's `primase.shadows`), by name (each component of primase's own `extras.primase` by its name,
//! `primase/<component>`), and every optional extension of the document, scene, nodes, meshes, primitives, skins,
//! animations and lights other than `KHR_lights_punctual`, by name. A material's extensions are its lowering's
//! (`lower-gltf-materials`), its `extras` are counted here.
use std::collections::{BTreeMap, BTreeSet};

use primase_animation::pose::{Affine, affine, compose, transform};
use primase_core::convert::{Notice, NoticeKind};
use primase_core::{
    Asset, Attachment, Binding, BindingId, Clip, ClipBinding, ClipTarget, Deformation, Deltas, Entity,
    Extrapolation, Influences, Instance, Interpolation, Joint, JointId, Light, LightKind, MorphFrame,
    MorphId, MorphTarget, MorphWeights, Morphs, Name, Opaque, OpaqueData, Property, Rest, RigInstance, Role,
    Semantic, ShadowCast, Shadows, Skeleton, Skin, SkinInstance, Slot, Socket, SocketId, Sockets, Source,
    Stream, Transform, WeightRule, World,
};
use primase_glb::{
    AnimationInterpolation, AnimationPath, Extensions, Gltf, Local, ReadMesh, ReadNode, ReadSkin,
};
use serde_json::Value;

use crate::textures::{ImageResolver, read_textures};
use crate::{Error, Result};

/// The component a material's glTF JSON is read into.
pub const MATERIAL: &str = "gltf.material";

/// The lights extension, read into `core.light`.
const LIGHTS: &str = "KHR_lights_punctual";

/// The `extras` key a node states its shadows under (the writer's, `crate::write`).
const SHADOWS: &str = "primase.shadows";

/// What [`read`] read, and what it did not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    pub placements: usize,
    pub meshes: usize,
    pub materials: usize,
    pub skeletons: usize,
    /// The skeletons' joints, and the rigs read from a rig node of their own (the others are at the origin).
    pub joints: usize,
    pub rig_nodes: usize,
    /// Glb skins read as bindings (one a skin and mesh it is drawn with).
    pub bindings: usize,
    pub morph_targets: usize,
    pub attachments: usize,
    pub clips: usize,
    pub channels: usize,
    /// Meshes whose skinned vertices' weights do not all sum to 1: read `AsStored`.
    pub weights_as_stored: usize,
    /// Not read: nodes outside the default scene, cameras, scene nodes read as nothing of their own (no joint, rig,
    /// mesh, light or camera node), skins' skeleton hints, animation channels on nodes that are not joints, joint
    /// streams of meshes no node draws with a skin.
    pub nodes_outside_scene: usize,
    pub cameras: usize,
    pub nodes_not_read: usize,
    pub skeleton_hints: usize,
    /// The `extras` keys the reader read nowhere, by name, each with the number of objects stating it: every key of
    /// the scene's, the scene nodes', drawn skins', drawn meshes', their primitives', the animations', the materials'
    /// and the lights' extras but a mesh's `targetNames` and a light or mesh node's `primase.shadows`; each component
    /// in primase's own `primase` key as `primase/<component name>`.
    pub extras_not_read: BTreeMap<String, usize>,
    /// The optional extensions the reader read nowhere, by name, each with the number of objects stating it: the
    /// document's, the scene's, the scene nodes', drawn meshes', their primitives', drawn skins', the animations' and
    /// the lights' (`KHR_lights_punctual` on the document and the nodes is read).
    pub extensions_not_read: BTreeMap<String, usize>,
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
    /// Lights read (`core.light`): one a scene node naming a `KHR_lights_punctual` light.
    pub lights: usize,
    /// The document's lights no scene node places, not read.
    pub lights_unplaced: usize,
    /// Lights on or under a skeleton's joint, placed where the stored pose puts them: a core light follows no rig.
    pub lights_at_stored_pose: usize,
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
                "scene nodes that are no joint, rig, mesh, light or camera, not read (their transforms are composed \
                 into the nodes below them)",
                self.nodes_not_read,
            ),
            n(
                "lights no scene node places (KHR_lights_punctual), not read",
                self.lights_unplaced,
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
        .chain(
            self.extras_not_read
                .iter()
                .map(|(key, count)| n(&format!("extras {key}, not read"), *count)),
        )
        .chain(
            self.extensions_not_read
                .iter()
                .map(|(name, count)| n(&format!("extension {name}, not read"), *count)),
        )
        .filter(|x| x.count > 0)
        .chain((self.lights_at_stored_pose > 0).then(|| {
            Notice::new(
                "gltf",
                NoticeKind::Approximation,
                "lights on or under a skeleton's joint, placed where the stored pose puts them (a core light follows no rig)",
                self.lights_at_stored_pose as u64,
            )
        }))
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

/// Counts the `extras` keys of one object that are not in `read`, by name: each component in primase's own `primase`
/// object as `primase/<name>`; extras that are not an object as `(not an object)`.
fn tally_extras(report: &mut ReadReport, extras: Option<&Value>, read: &[&str]) {
    let Some(extras) = extras else { return };
    let mut count = |key: String| *report.extras_not_read.entry(key).or_default() += 1;
    let Some(keys) = extras.as_object() else {
        count("(not an object)".to_owned());
        return;
    };
    for (key, value) in keys {
        if read.contains(&key.as_str()) {
            continue;
        }
        match (key.as_str(), value.as_object()) {
            ("primase", Some(components)) => components.keys().for_each(|c| count(format!("primase/{c}"))),
            _ => count(key.clone()),
        }
    }
}

/// Counts the extensions of one object that are not in `read`, by name.
fn tally_extensions(report: &mut ReadReport, extensions: &Extensions, read: &[&str]) {
    for name in extensions.keys().filter(|n| !read.contains(&n.as_str())) {
        *report.extensions_not_read.entry(name.clone()).or_default() += 1;
    }
}

/// A node's `extras["primase.shadows"]` (version 1), as the writer states it: `cast` and `receive`, each where stated.
/// `None` where the node states none, or states another version (counted as not read by the caller). A malformed
/// statement is an error naming the node.
fn node_shadows(node: &ReadNode, n: usize) -> Result<Option<(Option<ShadowCast>, Option<bool>)>> {
    let Some(v) = node.extras.as_ref().and_then(|x| x.get(SHADOWS)) else {
        return Ok(None);
    };
    let item = || format!("nodes[{n}].extras.{SHADOWS}");
    let o = v.as_object().ok_or_else(|| invalid(item(), "not an object"))?;
    if o.get("version").and_then(Value::as_u64) != Some(1) {
        return Ok(None);
    }
    if let Some(k) = o
        .keys()
        .find(|k| !["version", "cast", "receive"].contains(&k.as_str()))
    {
        return Err(invalid(item(), format!("unknown key {k:?}")));
    }
    let cast = match o.get("cast") {
        None => None,
        Some(c) => Some(match c.as_str() {
            Some("off") => ShadowCast::Off,
            Some("on") => ShadowCast::On,
            Some("only") => ShadowCast::Only,
            _ => {
                return Err(invalid(
                    item(),
                    format!("cast {c} is not \"off\", \"on\" or \"only\""),
                ));
            }
        }),
    };
    let receive = match o.get("receive") {
        None => None,
        Some(r) => Some(
            r.as_bool()
                .ok_or_else(|| invalid(item(), format!("receive {r} is not a boolean")))?,
        ),
    };
    Ok(Some((cast, receive)))
}

/// A number of a light object, finite and at least 0 (`min`), else an error naming it.
fn light_number(o: &serde_json::Map<String, Value>, key: &str, path: &str) -> Result<Option<f32>> {
    match o.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|x| x as f32)
            .filter(|x| x.is_finite() && *x >= 0.0)
            .map(Some)
            .ok_or_else(|| invalid(path, format!("{key} {v} is not a number of at least 0"))),
    }
}

/// Light `i` of the document's `KHR_lights_punctual` list as a core light (its shadows the node's), strictly: an
/// unknown property, type or value out of the extension's ranges is an error naming it. Its extras and extensions are
/// counted the `first` time a node names it.
fn punctual_light(
    report: &mut ReadReport,
    lights: &[Value],
    i: usize,
    cast_shadows: Option<bool>,
    first: bool,
) -> Result<Light> {
    let path = format!("extensions.{LIGHTS}.lights[{i}]");
    let o = lights
        .get(i)
        .ok_or_else(|| invalid(&path, format!("does not exist ({} lights)", lights.len())))?
        .as_object()
        .ok_or_else(|| invalid(&path, "not an object"))?;
    let allowed = [
        "name",
        "type",
        "color",
        "intensity",
        "range",
        "spot",
        "extras",
        "extensions",
    ];
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(invalid(&path, format!("unsupported property {k:?}")));
    }
    if let Some(e) = o.get("extensions") {
        let e = e
            .as_object()
            .ok_or_else(|| invalid(&path, "extensions is not an object"))?;
        for name in e.keys().filter(|_| first) {
            *report.extensions_not_read.entry(name.clone()).or_default() += 1;
        }
    }
    if first {
        tally_extras(report, o.get("extras"), &[]);
    }
    let kind = match o.get("type").and_then(Value::as_str) {
        Some("directional") => LightKind::Directional,
        Some("point") => LightKind::Point,
        Some("spot") => {
            let spot = o
                .get("spot")
                .and_then(Value::as_object)
                .ok_or_else(|| invalid(&path, "a spot light without its spot object"))?;
            let spot_path = format!("{path}.spot");
            if let Some(k) = spot
                .keys()
                .find(|k| !["innerConeAngle", "outerConeAngle", "extras", "extensions"].contains(&k.as_str()))
            {
                return Err(invalid(&spot_path, format!("unsupported property {k:?}")));
            }
            if first {
                tally_extras(report, spot.get("extras"), &[]);
            }
            let inner_cone = light_number(spot, "innerConeAngle", &spot_path)?.unwrap_or(0.0);
            let outer_cone =
                light_number(spot, "outerConeAngle", &spot_path)?.unwrap_or(std::f32::consts::FRAC_PI_4);
            if !(inner_cone <= outer_cone && outer_cone > 0.0 && outer_cone <= std::f32::consts::FRAC_PI_2) {
                return Err(invalid(
                    &spot_path,
                    format!(
                        "cones {inner_cone} and {outer_cone}: glTF needs 0 <= inner <= outer <= pi / 2, outer above 0"
                    ),
                ));
            }
            LightKind::Spot {
                inner_cone,
                outer_cone,
            }
        }
        other => {
            return Err(invalid(
                &path,
                format!("type {other:?} is not directional, point or spot"),
            ));
        }
    };
    if !matches!(kind, LightKind::Spot { .. }) && o.contains_key("spot") {
        return Err(invalid(&path, "a spot object on a light that is not a spot"));
    }
    let color = match o.get("color") {
        None => [1.0; 3],
        Some(c) => {
            let a = c
                .as_array()
                .filter(|a| a.len() == 3)
                .ok_or_else(|| invalid(&path, "color is not three numbers"))?;
            let mut out = [0.0f32; 3];
            for (k, (x, v)) in out.iter_mut().zip(a).enumerate() {
                *x = v
                    .as_f64()
                    .map(|x| x as f32)
                    .filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(|| invalid(&path, format!("color[{k}] {v} is not a number of at least 0")))?;
            }
            out
        }
    };
    let range = light_number(o, "range", &path)?;
    if range == Some(0.0) {
        return Err(invalid(&path, "range 0: glTF's range is above 0"));
    }
    Ok(Light {
        kind,
        color,
        intensity: light_number(o, "intensity", &path)?.unwrap_or(1.0),
        range,
        cast_shadows,
        radius: None,
        specular: None,
    })
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
    tally_extras(&mut report, gltf.scene_extras()?.as_ref(), &[]);
    tally_extensions(&mut report, &gltf.scene_extensions()?, &[]);
    // the document's own extensions: the lights are read below
    let document_extensions = match json.get("extensions") {
        None => serde_json::Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(invalid("extensions", "not an object")),
    };
    for name in document_extensions.keys().filter(|n| n.as_str() != LIGHTS) {
        *report.extensions_not_read.entry(name.clone()).or_default() += 1;
    }
    let lights: Vec<Value> = match document_extensions.get(LIGHTS) {
        None => Vec::new(),
        Some(l) => {
            let o = l
                .as_object()
                .ok_or_else(|| invalid(format!("extensions.{LIGHTS}"), "not an object"))?;
            if let Some(k) = o
                .keys()
                .find(|k| !["lights", "extras", "extensions"].contains(&k.as_str()))
            {
                return Err(invalid(
                    format!("extensions.{LIGHTS}"),
                    format!("unsupported property {k:?}"),
                ));
            }
            tally_extras(&mut report, o.get("extras"), &[]);
            match o.get("lights") {
                None => Vec::new(),
                Some(Value::Array(a)) => a.clone(),
                Some(_) => return Err(invalid(format!("extensions.{LIGHTS}.lights"), "not an array")),
            }
        }
    };

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
    for &n in &order {
        let node = &nodes[n];
        // a light or mesh node's shadows are read (a version this reader does not know is counted with the rest)
        let shadows_read =
            (node.mesh.is_some() || node.extensions.contains_key(LIGHTS)) && node_shadows(node, n)?.is_some();
        tally_extras(
            &mut report,
            node.extras.as_ref(),
            if shadows_read { &[SHADOWS] } else { &[] },
        );
        tally_extensions(&mut report, &node.extensions, &[LIGHTS]);
    }

    // the skins drawn in the scene, and their joints with every node above them
    let drawn_skins: BTreeSet<usize> = order.iter().filter_map(|&n| nodes[n].skin).collect();
    let mut skin_joints: BTreeSet<usize> = BTreeSet::new();
    let mut above_joints: BTreeSet<usize> = BTreeSet::new();
    for &s in &drawn_skins {
        let skin = &skins[s];
        if skin.skeleton.is_some() {
            report.skeleton_hints += 1;
        }
        tally_extras(&mut report, skin.extras.as_ref(), &[]);
        tally_extensions(&mut report, &skin.extensions, &[]);
        for &j in &skin.joints {
            if !in_scene.contains(&j) {
                return Err(invalid(
                    format!("skins[{s}]"),
                    format!("joint {j} is not in the scene its skinned nodes are"),
                ));
            }
            skin_joints.insert(j);
            let mut at = Some(j);
            while let Some(n) = at {
                if !above_joints.insert(n) {
                    break;
                }
                at = parent.get(&n).copied();
            }
        }
    }
    // each skeleton: a scene root those hang from, and every node of its tree that is no mesh or light node (nor below
    // one) but those above a joint; the root is the rig when it is a rig node as the writer writes one, else a joint
    let animated: BTreeSet<usize> = animations
        .iter()
        .flat_map(|a| a.channels.iter().map(|c| c.node))
        .collect();
    let is_light = |n: usize| nodes[n].extensions.contains_key(LIGHTS);
    let w = world;
    let mut skeletons = Skeletons { of: BTreeMap::new() };
    let mut rig_nodes: BTreeSet<usize> = BTreeSet::new();
    let mut rigs: Vec<(Entity, Entity)> = Vec::new();
    let tree_roots: BTreeSet<usize> = above_joints.iter().map(|n| root_of[n]).collect();
    for root in tree_roots {
        let r = &nodes[root];
        let rig_node = !skin_joints.contains(&root)
            && r.mesh.is_none()
            && !is_light(root)
            && r.camera.is_none()
            && !animated.contains(&root);
        let mut members = Vec::new();
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            if !above_joints.contains(&n) && (nodes[n].mesh.is_some() || is_light(n)) {
                continue;
            }
            if !(rig_node && n == root) {
                members.push(n);
            }
            stack.extend(&nodes[n].children);
        }
        members.sort_unstable();
        let skeleton = w.spawn();
        let joints: Vec<Joint> = members
            .iter()
            .map(|&n| Joint {
                id: JointId(n as u32),
                name: node_name(&nodes[n], n),
                parent: parent
                    .get(&n)
                    .filter(|&&p| !(rig_node && p == root))
                    .map(|&p| JointId(p as u32)),
                rest: rest(&nodes[n].local),
            })
            .collect();
        report.joints += joints.len();
        w.insert(skeleton, Name(format!("skeleton {}", node_name(r, root))));
        w.insert(skeleton, Skeleton { joints });
        let rig = w.spawn();
        w.insert(rig, RigInstance { skeleton });
        if rig_node {
            w.insert(rig, Name(node_name(r, root)));
            w.insert(rig, transform(&world_of[&root]));
            rig_nodes.insert(root);
        } else {
            w.insert(rig, Name(format!("rig {}", node_name(r, root))));
            w.insert(rig, Transform::IDENTITY);
        }
        for &n in &members {
            skeletons.of.insert(n, (skeleton, rig));
        }
        rigs.push((skeleton, rig));
    }
    report.skeletons = rigs.len();
    report.rig_nodes = rig_nodes.len();
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
            extensions,
        } = read;
        // a mesh's `targetNames` are read below; any other key of its extras is not
        tally_extras(&mut report, extras.as_ref(), &["targetNames"]);
        tally_extensions(&mut report, &extensions, &[]);
        for p in &primitives {
            tally_extras(&mut report, p.extras.as_ref(), &[]);
            tally_extensions(&mut report, &p.extensions, &[]);
        }
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
        if let Some((cast, receive)) = node_shadows(node, n)? {
            w.insert(p, Shadows { cast, receive });
        }
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

    // lights: each scene node naming one of the document's lights
    let mut placed_lights = BTreeSet::new();
    for &n in &order {
        let node = &nodes[n];
        let Some(l) = node.extensions.get(LIGHTS) else {
            continue;
        };
        let path = format!("nodes[{n}].extensions.{LIGHTS}");
        let o = l.as_object().ok_or_else(|| invalid(&path, "not an object"))?;
        if let Some(k) = o
            .keys()
            .find(|k| !["light", "extras", "extensions"].contains(&k.as_str()))
        {
            return Err(invalid(&path, format!("unsupported property {k:?}")));
        }
        tally_extras(&mut report, o.get("extras"), &[]);
        let i = o
            .get("light")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
            .ok_or_else(|| invalid(&path, "light is not an index"))?;
        let cast_shadows = match node_shadows(node, n)? {
            None => None,
            Some((None, None)) => None,
            Some((Some(ShadowCast::On), None)) => Some(true),
            Some((Some(ShadowCast::Off), None)) => Some(false),
            Some(_) => {
                return Err(invalid(
                    format!("nodes[{n}].extras.{SHADOWS}"),
                    "a light's shadows are cast \"on\" or \"off\" alone",
                ));
            }
        };
        let first = placed_lights.insert(i);
        let light = punctual_light(&mut report, &lights, i, cast_shadows, first)?;
        let e = w.spawn();
        w.insert(e, light);
        w.insert(e, transform(&world_of[&n]));
        w.insert(e, Name(node_name(node, n)));
        // on or under a joint: the stored pose's place, which the rig does not move
        let mut at = Some(n);
        while let Some(a) = at {
            if skeletons.of.contains_key(&a) {
                report.lights_at_stored_pose += 1;
                break;
            }
            at = parent.get(&a).copied();
        }
        report.lights += 1;
    }
    report.lights_unplaced = (0..lights.len()).filter(|i| !placed_lights.contains(i)).count();
    report.nodes_not_read = order
        .iter()
        .filter(|&&n| {
            !skeletons.of.contains_key(&n)
                && !rig_nodes.contains(&n)
                && !placements.contains_key(&n)
                && !is_light(n)
                && nodes[n].camera.is_none()
        })
        .count();

    // animations
    let mut clips = Vec::new();
    for (i, a) in animations.iter().enumerate() {
        tally_extras(&mut report, a.extras.as_ref(), &[]);
        tally_extensions(&mut report, &a.extensions, &[]);
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

    // the materials' extras: kept with their glTF JSON, read by nothing
    for &i in material_entities.keys() {
        tally_extras(&mut report, materials_json[i].get("extras"), &[]);
    }

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
