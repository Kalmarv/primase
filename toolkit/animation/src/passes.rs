//! The passes that evaluate rig components: `evaluate-clip` poses rigs and sets morph weights from their bound clips at
//! a time; `pose-skin` bakes skinned and morphed occurrences into static geometry, for writers that cannot skin.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Attachment, Clip, ClipBinding, Component, Entity, Instance, Lods, Mesh, MeshFile, MorphId, MorphWeights,
    Morphs, RigInstance, RigPose, Semantic, Skeleton, Skin, SkinInstance, Sockets, Stream, Transform, World,
};

use crate::deform::{Vertices, apply_morphs};
use crate::pose::{Affine, affine, compose, evaluate, inverse, locals, model_transforms, transform};
use crate::weights::{check_weights, quantize, read_weights, write_weights};

/// What `evaluate-clip` does with a clip's root motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootChoice {
    /// Left out: the rig stays where it is placed (counted).
    Ignore,
    /// The rig's `core.transform` moved by it (`rig * root`).
    Apply,
}

/// `evaluate-clip{time=S,root=ignore|apply}`: every `core.clip_binding`'s clip at `time` seconds: its rig's pose (a
/// `core.rig_pose` over the rig's own pose, else the rests) and the morph weights of its mesh occurrences
/// (`core.morph_weights`, the targets the clip drives set, others kept). Root motion as `root` says; `root=apply` takes
/// off the root motion an earlier run applied (the pose's `root_applied`) before applying this one, so the same run
/// twice, or a later time, leaves the rig at its placement times that time's root motion, and a rig moved by anything
/// else stays so moved. The objects attached to a posed rig (`core.attachment`) follow its pose and placement: each
/// is placed at the rig's transform times its socket joint's pose, the socket's offset and its own. A source control
/// channel has no core meaning to evaluate into: counted. Two bindings driving one rig are refused: how they layer is
/// not stated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EvaluateClip {
    time: f64,
    root: RootChoice,
}

impl EvaluateClip {
    /// At `time` seconds (finite, not negative).
    pub fn new(time: f64, root: RootChoice) -> Result<EvaluateClip, String> {
        if !time.is_finite() || time < 0.0 {
            return Err(format!(
                "time {time} is not a finite, non-negative number of seconds"
            ));
        }
        Ok(EvaluateClip { time, root })
    }

    pub fn from_options(o: &Options) -> Result<EvaluateClip, String> {
        let mut time = None;
        let mut root = None;
        for (k, v) in o {
            match k.as_str() {
                "time" => {
                    time = Some(
                        v.parse::<f64>()
                            .map_err(|_| format!("time={v} is not a number"))?,
                    )
                }
                "root" => {
                    root = Some(match v.as_str() {
                        "ignore" => RootChoice::Ignore,
                        "apply" => RootChoice::Apply,
                        _ => return Err(format!("root={v} is not ignore or apply")),
                    })
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        EvaluateClip::new(
            time.ok_or("time is required")?,
            root.ok_or("root is required (ignore or apply)")?,
        )
    }
}

impl Pass for EvaluateClip {
    fn name(&self) -> &'static str {
        "evaluate-clip"
    }
    fn options(&self) -> Options {
        Options::from([
            ("time".to_owned(), self.time.to_string()),
            (
                "root".to_owned(),
                match self.root {
                    RootChoice::Ignore => "ignore",
                    RootChoice::Apply => "apply",
                }
                .to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[ClipBinding::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[RigPose::NAME, MorphWeights::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[RigPose::NAME, MorphWeights::NAME, Transform::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let bindings: Vec<(Entity, ClipBinding)> =
            w.iter::<ClipBinding>().map(|(e, b)| (e, b.clone())).collect();
        let mut rigs = BTreeSet::new();
        for (e, b) in &bindings {
            if let Some(r) = b.rig
                && !rigs.insert(r)
            {
                return Err(format!(
                    "{e:?}: a second clip binding drives rig {r:?}; how they layer is not stated"
                ));
            }
        }
        for (_, b) in bindings {
            let clip = w.get::<Clip>(b.clip).ok_or("a binding's clip is gone")?.clone();
            let (skeleton_entity, skeleton, base) = match b.rig {
                Some(r) => {
                    let s = w
                        .get::<RigInstance>(r)
                        .ok_or("a binding's rig is not a rig")?
                        .skeleton;
                    let skel = w.get::<Skeleton>(s).ok_or("a rig's skeleton is gone")?.clone();
                    (s, skel, w.get::<RigPose>(r).cloned())
                }
                // morphs and controls only: an empty skeleton
                None => (Entity::from_index(u32::MAX), Skeleton::default(), None),
            };
            let out =
                evaluate(&clip, skeleton_entity, &skeleton, base.as_ref(), self.time).map_err(|e| e.0)?;
            if let Some(r) = b.rig {
                let mut pose = out.pose;
                match self.root {
                    RootChoice::Apply => {
                        let previous = pose.root_applied;
                        if out.root != previous {
                            let placed = w
                                .get::<Transform>(r)
                                .copied()
                                .ok_or("a rig without a transform")?;
                            let mut moved = affine(&placed);
                            if let Some(p) = previous {
                                let undo = inverse(&affine(&p)).ok_or_else(|| {
                                    format!("{r:?}: the root motion applied before has no inverse")
                                })?;
                                moved = compose(&moved, &undo);
                            }
                            if let Some(now) = out.root {
                                moved = compose(&moved, &affine(&now));
                            }
                            w.insert(r, transform(&moved));
                            log.count("rigs moved by their root motion", 1);
                        }
                        pose.root_applied = out.root;
                    }
                    RootChoice::Ignore if out.root.is_some() => {
                        log.count("root motions left out (root=ignore)", 1);
                    }
                    RootChoice::Ignore => {}
                }
                let posed = locals(&skeleton, Some(&pose));
                w.insert(r, pose);
                log.count("rigs posed", 1);
                let moved = follow_rig(w, r, skeleton_entity, &skeleton, &posed)?;
                if moved > 0 {
                    log.count("attached objects moved with their rig", moved);
                }
            }
            for &m in &b.meshes {
                let drawn = w.get::<Instance>(m).map_or(m, |i| i.prototype);
                let set: Vec<(MorphId, f32)> = out
                    .morphs
                    .iter()
                    .filter(|(g, _, _)| *g == drawn)
                    .map(|&(_, id, v)| (id, v))
                    .collect();
                if set.is_empty() {
                    continue;
                }
                let mut weights = w.get::<MorphWeights>(m).cloned().unwrap_or_default();
                for (id, v) in set {
                    match weights.0.iter_mut().find(|(t, _)| *t == id) {
                        Some(slot) => slot.1 = v,
                        None => weights.0.push((id, v)),
                    }
                }
                w.insert(m, weights);
                log.count("mesh occurrences given morph weights", 1);
            }
            log.count("source control channels not evaluated", out.controls.len() as u64);
            log.count(
                "joint channels on another skeleton than the bound rig's",
                out.other_skeletons as u64,
            );
        }
        Ok(())
    }
}

/// `pose-skin`: every skinned occurrence (`core.skin_instance`) and every occurrence with morph weights drawn as a static
/// mesh of its own: its mesh's morphs at its weights, then its skin at its rig's pose (a `core.rig_pose`, else the
/// rests), in the occurrence's own space. Each such occurrence gets a prototype of its own (a copy of the one it drew,
/// with the posed mesh and without skin, morphs, file link or joint and weight streams); one drawing its own mesh is
/// changed in place. Exact for that pose; the animation is not kept (counted). A skinned prototype with levels of
/// detail is refused: choose a form first (`select-lod`). A vertex with no weight keeps the zero matrix its weights
/// give it and lands at the origin (counted). A clip binding listing a posed occurrence no longer drives
/// it: the occurrence leaves its list (counted).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoseSkin;

impl Pass for PoseSkin {
    fn name(&self) -> &'static str {
        "pose-skin"
    }
    fn provides(&self) -> &[&'static str] {
        &[Mesh::NAME]
    }
    fn removes(&self) -> &[&'static str] {
        &[
            SkinInstance::NAME,
            MorphWeights::NAME,
            Skin::NAME,
            Morphs::NAME,
            MeshFile::NAME,
        ]
    }
    fn modifies(&self) -> &[&'static str] {
        &[Instance::NAME, Mesh::NAME, ClipBinding::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let occurrences: BTreeSet<Entity> = w
            .iter::<SkinInstance>()
            .map(|(e, _)| e)
            .chain(w.iter::<MorphWeights>().map(|(e, _)| e))
            .collect();
        let mut posed_occurrences = BTreeSet::new();
        for e in occurrences {
            let drawn = w.get::<Instance>(e).map_or(e, |i| i.prototype);
            if w.has::<Lods>(drawn) {
                return Err(format!(
                    "{drawn:?}: a skinned prototype with levels of detail; choose a form first"
                ));
            }
            let mesh = w
                .get::<Mesh>(drawn)
                .ok_or_else(|| format!("{drawn:?}: its mesh is not loaded"))?
                .clone();
            let weights: BTreeMap<MorphId, f32> = w
                .get::<MorphWeights>(e)
                .map(|m| m.0.iter().copied().collect())
                .unwrap_or_default();
            let morphs = w.get::<Morphs>(drawn).cloned();
            // the skin: binding, palettes, and each joint's transform in the occurrence's space (rig to occurrence)
            let skinning = match w.get::<SkinInstance>(e).copied() {
                Some(si) => {
                    let skin = w
                        .get::<Skin>(drawn)
                        .ok_or_else(|| format!("{drawn:?} has no core.skin"))?
                        .clone();
                    let binding = skin
                        .binding(si.binding)
                        .ok_or_else(|| format!("{e:?}: binding {:?} is not its mesh's", si.binding))?
                        .clone();
                    let rig = w
                        .get::<RigInstance>(si.rig)
                        .ok_or("a skin instance's rig is gone")?;
                    let skeleton = w
                        .get::<Skeleton>(rig.skeleton)
                        .ok_or("a rig's skeleton is gone")?;
                    let pose = locals(skeleton, w.get::<RigPose>(si.rig));
                    let model = model_transforms(skeleton, &pose).map_err(|e| e.0)?;
                    let to_occurrence = rig_to_occurrence(w, si.rig, e)?;
                    let model = model
                        .into_iter()
                        .map(|(j, m)| (j, compose(&to_occurrence, &m)))
                        .collect();
                    Some((skin, binding, model))
                }
                None => None,
            };
            let mut posed = mesh.clone();
            for (i, mut p) in posed.primitives_mut().enumerate() {
                let mut v = Vertices::of(&p).map_err(|x| format!("{drawn:?} primitive {i}: {}", x.0))?;
                if let Some(m) = &morphs {
                    v = apply_morphs(&v, i, m, &weights).map_err(|x| x.0)?;
                }
                if let Some((skin, binding, model)) = &skinning
                    && let (Some(Some(how)), Some(Some(palette))) =
                        (skin.influences.get(i), binding.palettes.get(i))
                {
                    let (posed_v, none) = crate::deform::skin_counted(
                        &v,
                        &p,
                        *how,
                        skin.weights,
                        binding.deformation,
                        palette,
                        model,
                    )
                    .map_err(|x| format!("{drawn:?} primitive {i}: {}", x.0))?;
                    v = posed_v;
                    log.count("primitives skinned at a pose", 1);
                    if none > 0 {
                        log.count(
                            "skinned vertices with no weight, left at the origin (their blend matrix is zero)",
                            none,
                        );
                    }
                }
                let streams = p.streams_mut();
                streams.retain(|s, _| !matches!(s, Semantic::Joints(_) | Semantic::Weights(_)));
                streams.insert(Semantic::Position, Stream::F32x3(v.positions));
                if let Some(n) = v.normals {
                    streams.insert(Semantic::Normal, Stream::F32x3(n));
                }
                if let Some(t) = v.tangents {
                    streams.insert(Semantic::Tangent, Stream::F32x4(t));
                }
            }
            let holder = if drawn == e {
                e
            } else {
                let own = w.duplicate(drawn).ok_or("an occurrence's prototype is gone")?;
                w.get_mut::<Instance>(e).ok_or("an instance is gone")?.prototype = own;
                own
            };
            for name in [Skin::NAME, Morphs::NAME, MeshFile::NAME] {
                if w.entity_has_named(holder, name) {
                    log.record_removed(name);
                }
            }
            w.remove::<Skin>(holder);
            w.remove::<Morphs>(holder);
            w.remove::<MeshFile>(holder);
            w.insert(holder, posed);
            w.remove::<SkinInstance>(e);
            w.remove::<MorphWeights>(e);
            log.count(
                "occurrences posed into static meshes (their animation not kept)",
                1,
            );
            posed_occurrences.insert(e);
        }
        // a clip no longer drives what was posed: its bindings leave those occurrences
        let bindings: Vec<Entity> = w.iter::<ClipBinding>().map(|(b, _)| b).collect();
        for b in bindings {
            let binding = w.get_mut::<ClipBinding>(b).ok_or("a clip binding is gone")?;
            let before = binding.meshes.len();
            binding.meshes.retain(|m| !posed_occurrences.contains(m));
            let left = before - binding.meshes.len();
            if left > 0 {
                log.count("posed occurrences taken out of clip bindings", left as u64);
            }
        }
        Ok(())
    }
    fn removes_by_log(&self) -> bool {
        true
    }
}

/// Places every object attached to rig `rig` (skeleton `skeleton_entity`, `skeleton`) at the rig's transform times its
/// socket joint's transform under `locals`, the socket's offset and its own; returns how many.
fn follow_rig(
    w: &mut World,
    rig: Entity,
    skeleton_entity: Entity,
    skeleton: &Skeleton,
    locals: &BTreeMap<primase_core::JointId, primase_core::Rest>,
) -> Result<u64, String> {
    let attached: Vec<(Entity, Attachment)> = w
        .iter::<Attachment>()
        .filter(|(_, a)| a.rig == rig)
        .map(|(e, a)| (e, *a))
        .collect();
    if attached.is_empty() {
        return Ok(0);
    }
    let model = model_transforms(skeleton, locals).map_err(|e| e.0)?;
    let sockets = w
        .get::<Sockets>(skeleton_entity)
        .ok_or("an attachment on a rig whose skeleton has no sockets")?
        .clone();
    let placed = affine(w.get::<Transform>(rig).ok_or("a rig without a transform")?);
    for (e, a) in &attached {
        let socket = sockets
            .0
            .iter()
            .find(|s| s.id == a.socket)
            .ok_or_else(|| format!("{e:?}: socket {:?} is not its rig's skeleton's", a.socket))?;
        let joint = model
            .get(&socket.joint)
            .ok_or_else(|| format!("{e:?}: socket {:?}'s joint is not posed", a.socket))?;
        let at = compose(
            &compose(&compose(&placed, joint), &affine(&socket.offset)),
            &affine(&a.offset),
        );
        w.insert(*e, transform(&at));
    }
    Ok(attached.len() as u64)
}

/// The map from rig `rig`'s space to occurrence `e`'s: `inverse(e's placement) * rig's placement`.
fn rig_to_occurrence(w: &World, rig: Entity, e: Entity) -> Result<Affine, String> {
    let r = affine(w.get::<Transform>(rig).ok_or("a rig without a transform")?);
    let o = w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
    let inv = primase_core::Linear::of(&o)
        .inverse_columns()
        .ok_or_else(|| format!("{e:?}: its placement has no inverse"))?;
    let t = o.cols[3].map(f64::from);
    // inverse of [M | t]: [M^-1 | -M^-1 t]
    let mt: [f64; 3] = std::array::from_fn(|row| (0..3).map(|k| inv[k][row] * t[k]).sum());
    let inverse: Affine = [inv[0], inv[1], inv[2], mt.map(|x| -x)];
    Ok(compose(&inverse, &r))
}

/// `normalize-skin-weights`: every skinned primitive's weights (`core.skin` with [`Influences::Streams`]) made to sum
/// to 1 per vertex, as glTF and most engines require: float weights divided by their sum; normalised byte or short
/// weights requantised to sum to exactly 255 or 65535 (each influence's share rounded down, the remainder given to the
/// largest fractional parts, ties to the earlier influence). The skin's rule becomes `SumsToOne`. A vertex whose
/// weights are all zero is refused (nothing to normalise), as is a NaN, infinite or negative weight. Counted: vertices changed, and the largest change of one
/// weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NormalizeSkinWeights;

impl Pass for NormalizeSkinWeights {
    fn name(&self) -> &'static str {
        "normalize-skin-weights"
    }
    fn modifies(&self) -> &[&'static str] {
        &[Mesh::NAME, Skin::NAME]
    }
    fn approximates(&self) -> &'static str {
        "skin weights rescaled to sum to 1 (requantised for integer weights)"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let skinned: Vec<Entity> = w.iter::<Skin>().map(|(e, _)| e).collect();
        let mut changed = 0u64;
        let mut largest = 0.0f64;
        for e in skinned {
            let skin = w.get::<Skin>(e).expect("listed").clone();
            let Some(mut mesh) = w.get::<Mesh>(e).cloned() else {
                continue;
            };
            for (i, mut p) in mesh.primitives_mut().enumerate() {
                let Some(Some(primase_core::Influences::Streams { sets })) = skin.influences.get(i).copied()
                else {
                    continue;
                };
                let (mut all, unit) =
                    read_weights(&p, sets).map_err(|m| format!("{e:?} primitive {i}: {m}"))?;
                for (v, ws) in all.iter_mut().enumerate() {
                    check_weights(ws).map_err(|m| format!("{e:?} primitive {i}: vertex {v}: {m}"))?;
                    let sum: f64 = ws.iter().sum();
                    if sum <= 0.0 {
                        return Err(format!(
                            "{e:?} primitive {i}: vertex {v} has no weight to normalise"
                        ));
                    }
                    let new: Vec<f64> = if unit == 0 {
                        ws.iter().map(|x| x / sum).collect()
                    } else {
                        quantize(ws, unit)
                            .iter()
                            .map(|&q| q as f64 / unit as f64)
                            .collect()
                    };
                    let delta = ws
                        .iter()
                        .zip(&new)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0, f64::max);
                    if delta > 0.0 {
                        changed += 1;
                        largest = largest.max(delta);
                    }
                    *ws = new;
                }
                write_weights(p.streams_mut(), &all, sets, unit);
            }
            w.insert(e, mesh);
            if let Some(s) = w.get_mut::<Skin>(e) {
                s.weights = primase_core::WeightRule::SumsToOne;
            }
        }
        log.approximate(
            "skinned vertices whose weights were rescaled to sum to 1",
            changed,
        );
        log.count(
            "largest change of one weight, in millionths",
            (largest * 1e6).round() as u64,
        );
        Ok(())
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::*;
    use primase_core::{Binding, BindingId, Deformation, Influences, Primitive, Topology, WeightRule};

    /// Hand-worked: bytes 128 + 125 (253) become 129 + 126 by largest remainder (exact shares 128.99 and 126.01:
    /// floors 128 + 126 = 254, the one left to the larger fraction, the first); floats 0.5 + 0.25 become 2/3 + 1/3.
    #[test]
    fn weights_sum_to_one_after() {
        assert_eq!(quantize(&[128.0, 125.0, 0.0, 0.0], 255), [129, 126, 0, 0]);
        let mut w = World::new();
        let skeleton = w.spawn();
        let e = w.spawn();
        let prim = |weights: Stream| Primitive {
            topology: Topology::Points,
            streams: [
                (Semantic::Position, Stream::F32x3(vec![[0.0; 3]])),
                (Semantic::Joints(0), Stream::U8x4(vec![[0, 1, 0, 0]])),
                (Semantic::Weights(0), weights),
            ]
            .into(),
            indices: None,
            material: None,
        };
        w.insert(
            e,
            Mesh::new(vec![
                prim(Stream::U8x4(vec![[128, 125, 0, 0]])),
                prim(Stream::F32x4(vec![[0.5, 0.25, 0.0, 0.0]])),
            ]),
        );
        w.insert(
            e,
            Skin {
                influences: vec![Some(Influences::Streams { sets: 1 }); 2],
                weights: WeightRule::AsStored,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton,
                    palettes: vec![None, None],
                    deformation: Deformation::Linear,
                }],
            },
        );
        let mut log = PassLog::default();
        NormalizeSkinWeights.run(&mut w, &mut log).unwrap();
        let m = w.get::<Mesh>(e).unwrap();
        assert_eq!(
            m.primitives()[0].streams[&Semantic::Weights(0)],
            Stream::U8x4(vec![[129, 126, 0, 0]])
        );
        let Stream::F32x4(f) = &m.primitives()[1].streams[&Semantic::Weights(0)] else {
            panic!("floats stay floats")
        };
        assert!((f[0][0] - 2.0 / 3.0).abs() < 1e-6 && (f[0][1] - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(w.get::<Skin>(e).unwrap().weights, WeightRule::SumsToOne);
        assert_eq!(
            log.approximations["skinned vertices whose weights were rescaled to sum to 1"],
            2
        );
    }
}

/// Register this crate's passes.
pub fn register(reg: &mut primase_core::pass::PassRegistry) {
    reg.register_requiring(
        "evaluate-clip",
        |o| Ok(Box::new(EvaluateClip::from_options(o)?)),
        &[("time", "0"), ("root", "ignore")],
    );
    reg.register_requiring(
        "assemble-character",
        crate::assemble::assemble_character,
        &[
            ("correspondence", "by-name"),
            ("reference-pose", "own"),
            ("tolerance", "0.001"),
            ("unweighted-rests", "report"),
            ("weighted-rests", "refuse"),
            ("name", "character"),
        ],
    );
    reg.register_requiring(
        "correspond-skeletons",
        crate::correspond::correspond_skeletons,
        &[
            ("from", "a"),
            ("to", "b"),
            ("tolerance", "0.001"),
            ("parents", "same"),
        ],
    );
    reg.register_requiring(
        "retarget-skin",
        crate::retarget_skin::retarget_skin,
        &[("unmapped", "refuse")],
    );
    reg.register_requiring(
        "retarget-clip",
        crate::retarget::retarget_clip,
        &[("reference", "rest"), ("translation", "none")],
    );
    reg.register_requiring(
        "map-clip",
        crate::map::map_clip,
        &[
            ("unmapped", "refuse"),
            ("rests", "target"),
            ("reparented", "refuse"),
        ],
    );
    reg.register("normalize-skin-weights", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(NormalizeSkinWeights)),
    });
    reg.register_requiring(
        "limit-skin-influences",
        |o| Ok(Box::new(crate::weights::LimitSkinInfluences::from_options(o)?)),
        &[("max", "4")],
    );
    reg.register_requiring(
        "quantize-skin-weights",
        |o| Ok(Box::new(crate::weights::QuantizeSkinWeights::from_options(o)?)),
        &[("bits", "8")],
    );
    reg.register("pose-skin", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(PoseSkin)),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use primase_core::pass::{PassManager, PassRegistry, RunReport};
    use primase_core::{
        Binding, BindingId, Channel, ClipTarget, Deformation, Extrapolation, Influences, Interpolation,
        Joint, JointId, Primitive, Property, Rest, RootMotion, RootSpace, Slot, Socket, SocketId, Topology,
        WeightRule,
    };

    fn translate(t: [f32; 3]) -> Transform {
        let mut m = Transform::IDENTITY;
        m.cols[3] = t;
        m
    }

    /// A skeleton (joint 1 at the origin, joint 2 at y=1), a prototype of one point at (0, 1, 0) bound wholly to joint
    /// 2, two rigs placed at x=0 and x=10 each with a skinned placement, and a clip raising joint 2 by 2 over 1 s (linear)
    /// bound to the first rig only: (world, prototype, [rig a, rig b], [placement a, placement b]).
    fn world() -> (World, Entity, [Entity; 2], [Entity; 2]) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![
                    Joint {
                        id: JointId(1),
                        name: "root".into(),
                        parent: None,
                        rest: Rest::Affine(Transform::IDENTITY),
                    },
                    Joint {
                        id: JointId(2),
                        name: "tip".into(),
                        parent: Some(JointId(1)),
                        rest: Rest::Affine(translate([0.0, 1.0, 0.0])),
                    },
                ],
            },
        );
        let proto = w.spawn();
        w.insert(
            proto,
            Mesh::new(vec![Primitive {
                topology: Topology::Points,
                streams: [(Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0]]))].into(),
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
                    id: BindingId(0),
                    skeleton,
                    palettes: vec![Some(vec![Slot {
                        joint: JointId(2),
                        inverse_bind: translate([0.0, -1.0, 0.0]),
                    }])],
                    deformation: Deformation::Linear,
                }],
            },
        );
        let mut rigs = [proto; 2];
        let mut placed = [proto; 2];
        for (k, x) in [0.0f32, 10.0].into_iter().enumerate() {
            let rig = w.spawn();
            w.insert(rig, RigInstance { skeleton });
            w.insert(rig, translate([x, 0.0, 0.0]));
            let p = w.spawn();
            w.insert(
                p,
                Instance {
                    prototype: proto,
                    variant: 0,
                },
            );
            w.insert(p, translate([x, 0.0, 0.0]));
            w.insert(
                p,
                SkinInstance {
                    rig,
                    binding: BindingId(0),
                },
            );
            rigs[k] = rig;
            placed[k] = p;
        }
        let clip = w.spawn();
        w.insert(
            clip,
            Clip {
                name: "raise".into(),
                duration: 1.0,
                extrapolation: Extrapolation::Hold,
                channels: vec![Channel {
                    target: ClipTarget::Joint {
                        skeleton,
                        joint: JointId(2),
                    },
                    property: Property::Translation,
                    interpolation: Interpolation::Linear,
                    times: vec![0.0, 1.0],
                    values: vec![0.0, 1.0, 0.0, 0.0, 3.0, 0.0],
                }],
                root_motion: None,
                additive: None,
            },
        );
        let binding = w.spawn();
        w.insert(
            binding,
            ClipBinding {
                clip,
                rig: Some(rigs[0]),
                meshes: Vec::new(),
            },
        );
        (w, proto, rigs, placed)
    }

    fn manager() -> PassManager {
        let mut r = PassRegistry::new();
        register(&mut r);
        PassManager::new(r).with_pass_checks(true)
    }

    fn position(w: &World, p: Entity) -> [f32; 3] {
        let proto = w.get::<Instance>(p).unwrap().prototype;
        let Stream::F32x3(v) = &w.get::<Mesh>(proto).unwrap().primitives()[0].streams[&Semantic::Position]
        else {
            panic!("positions")
        };
        v[0]
    }

    /// Hand-worked: at t=0.5 the clip has joint 2 at y=2 (from 1 to 3), so rig a's point, bound wholly to joint 2 with
    /// inverse bind T(0,-1,0), is at y=2 in its rig's space, and the rig and placement share x=0: (0, 2, 0) in the
    /// placement's space. Rig b has no clip: its point stays at rest, (0, 1, 0). Each placement gets its own posed
    /// prototype; the shared one keeps its skin.
    #[test]
    fn two_occurrences_of_one_skinned_mesh_pose_apart() {
        let (mut w, proto, rigs, placed) = world();
        let m = manager();
        let mut report = RunReport::default();
        m.run(
            &mut w,
            &m.registry
                .parse("evaluate-clip{time=0.5,root=ignore},pose-skin")
                .unwrap(),
            &mut report,
        )
        .unwrap();
        assert!(w.has::<RigPose>(rigs[0]) && !w.has::<RigPose>(rigs[1]));
        assert_eq!(position(&w, placed[0]), [0.0, 2.0, 0.0]);
        assert_eq!(position(&w, placed[1]), [0.0, 1.0, 0.0]);
        let protos: BTreeSet<Entity> = placed
            .iter()
            .map(|&p| w.get::<Instance>(p).unwrap().prototype)
            .collect();
        assert_eq!(protos.len(), 2);
        assert!(!protos.contains(&proto) && w.has::<Skin>(proto));
        assert!(placed.iter().all(|&p| !w.has::<SkinInstance>(p)));
        assert_eq!(
            report.steps[1].log.counts["occurrences posed into static meshes (their animation not kept)"],
            2
        );
        assert_eq!(w.verify_components(), vec![]);
    }

    /// The rig placed away from its occurrence: the skinned point is in the rig's space, so the placement's own space
    /// holds it at the difference (rig at x=10, placement at x=0: the point at x=10 in the placement's space).
    #[test]
    fn a_skin_is_posed_in_its_rigs_space() {
        let (mut w, _, rigs, placed) = world();
        w.insert(rigs[0], translate([10.0, 0.0, 0.0]));
        let m = manager();
        m.run(
            &mut w,
            &m.registry.parse("pose-skin").unwrap(),
            &mut RunReport::default(),
        )
        .unwrap();
        assert_eq!(position(&w, placed[0]), [10.0, 1.0, 0.0]);
    }

    /// The world's clip given a root motion of 4 m along x a second (linear over 0 to 1 s), and its rig 0 placed at
    /// x=10: returns the rig.
    fn with_root_motion(w: &mut World, rig: Entity) -> Entity {
        let clip = w.iter::<ClipBinding>().next().unwrap().1.clip;
        w.get_mut::<Clip>(clip).unwrap().root_motion = Some(RootMotion {
            space: RootSpace::Rig,
            translation: Some((
                Interpolation::Linear,
                vec![0.0, 1.0],
                vec![0.0, 0.0, 0.0, 4.0, 0.0, 0.0],
            )),
            rotation: None,
        });
        w.insert(rig, translate([10.0, 0.0, 0.0]));
        rig
    }

    fn evaluate_at(w: &mut World, time: f64) {
        let m = manager();
        m.run(
            w,
            &m.registry
                .parse(&format!("evaluate-clip{{time={time},root=apply}}"))
                .unwrap(),
            &mut RunReport::default(),
        )
        .unwrap();
    }

    /// Hand-worked: the rig is placed at x=10 and the clip moves it 4 m a second. At 0.5 s it is at x=12, and
    /// evaluating that time again leaves exactly that transform (the root motion applied before is taken off first).
    /// From there to 1 s is x=14, what 1 s alone gives from the placement; a rig moved by something else in between
    /// (5 m along x) keeps the move: 17 - 2 + 4 = 19.
    #[test]
    fn root_motion_is_applied_from_the_placement_not_the_current_transform() {
        let (mut w, _, rigs, _) = world();
        let rig = with_root_motion(&mut w, rigs[0]);
        evaluate_at(&mut w, 0.5);
        let once = *w.get::<Transform>(rig).unwrap();
        assert_eq!(once.cols[3], [12.0, 0.0, 0.0]);
        evaluate_at(&mut w, 0.5);
        assert_eq!(*w.get::<Transform>(rig).unwrap(), once);
        evaluate_at(&mut w, 1.0);
        assert_eq!(w.get::<Transform>(rig).unwrap().cols[3], [14.0, 0.0, 0.0]);
        let (mut alone, _, rigs, _) = world();
        let rig_alone = with_root_motion(&mut alone, rigs[0]);
        evaluate_at(&mut alone, 1.0);
        assert_eq!(w.get::<Transform>(rig), alone.get::<Transform>(rig_alone));
        assert_eq!(
            w.get::<RigPose>(rig).unwrap().root_applied,
            alone.get::<RigPose>(rig_alone).unwrap().root_applied
        );
        // moved by something else: 14 + 5, then back to 0.5 s (-4 + 2)
        w.insert(rig, translate([19.0, 0.0, 0.0]));
        evaluate_at(&mut w, 0.5);
        assert_eq!(w.get::<Transform>(rig).unwrap().cols[3], [17.0, 0.0, 0.0]);
    }

    /// Hand-worked: a socket on joint 2 (at y=1 at rest; the clip raises it to y=2 at 0.5 s), an object attached at
    /// (0, 0, 1) in the socket's space. The rig is at x=10 and the root motion moves it to x=12: the object sits at
    /// (12, 2, 1), and does again when the same time is evaluated twice.
    #[test]
    fn attached_objects_follow_the_posed_and_moved_rig() {
        let (mut w, _, rigs, _) = world();
        let rig = with_root_motion(&mut w, rigs[0]);
        let skeleton = w.get::<RigInstance>(rig).unwrap().skeleton;
        w.insert(
            skeleton,
            Sockets(vec![Socket {
                id: SocketId(0),
                name: "grip".into(),
                joint: JointId(2),
                offset: Transform::IDENTITY,
            }]),
        );
        let held = w.spawn();
        w.insert(
            held,
            Attachment {
                rig,
                socket: SocketId(0),
                offset: translate([0.0, 0.0, 1.0]),
            },
        );
        w.insert(held, Transform::IDENTITY);
        for _ in 0..2 {
            evaluate_at(&mut w, 0.5);
            assert_eq!(w.get::<Transform>(held).unwrap().cols[3], [12.0, 2.0, 1.0]);
        }
        assert_eq!(w.verify_components(), vec![]);
    }

    /// Options are required, checked and printed; two bindings on one rig are refused.
    #[test]
    fn evaluate_clip_options_and_refusals() {
        assert!(EvaluateClip::from_options(&Options::from([("time".into(), "1".into())])).is_err());
        assert!(EvaluateClip::new(-1.0, RootChoice::Apply).is_err());
        let e = EvaluateClip::from_options(&Options::from([
            ("time".into(), "0.25".into()),
            ("root".into(), "apply".into()),
        ]))
        .unwrap();
        assert_eq!(e.options()["time"], "0.25");
        let (mut w, _, rigs, _) = world();
        let clip = w.get::<ClipBinding>(Entity::from_index(9)).map(|b| b.clip);
        let clip = clip.unwrap_or_else(|| w.iter::<ClipBinding>().next().unwrap().1.clip);
        let second = w.spawn();
        w.insert(
            second,
            ClipBinding {
                clip,
                rig: Some(rigs[0]),
                meshes: Vec::new(),
            },
        );
        let err = e.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(err.contains("a second clip binding"), "{err}");
    }
}
