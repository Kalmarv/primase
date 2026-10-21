//! The passes that evaluate rig components: `evaluate-clip` poses rigs and sets morph weights from their bound clips at
//! a time; `pose-skin` bakes skinned and morphed occurrences into static geometry, for writers that cannot skin.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{
    Clip, ClipBinding, Component, Entity, Instance, Lods, Mesh, MeshFile, MorphId, MorphWeights, Morphs,
    RigInstance, RigPose, Semantic, Skeleton, Skin, SkinInstance, Stream, Transform, World,
};

use crate::deform::{Vertices, apply_morphs};
use crate::pose::{Affine, affine, compose, evaluate, locals, model_transforms, transform};

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
/// (`core.morph_weights`, the targets the clip drives set, others kept). Root motion as `root` says. A source control
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
                w.insert(r, out.pose);
                log.count("rigs posed", 1);
                match (self.root, out.root) {
                    (RootChoice::Apply, Some(root)) => {
                        let placed = w
                            .get::<Transform>(r)
                            .copied()
                            .ok_or("a rig without a transform")?;
                        w.insert(r, transform(&compose(&affine(&placed), &affine(&root))));
                        log.count("rigs moved by their root motion", 1);
                    }
                    (RootChoice::Ignore, Some(_)) => log.count("root motions left out (root=ignore)", 1),
                    (_, None) => {}
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
/// detail is refused: choose a form first (`select-lod`).
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
        &[Instance::NAME, Mesh::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let occurrences: BTreeSet<Entity> = w
            .iter::<SkinInstance>()
            .map(|(e, _)| e)
            .chain(w.iter::<MorphWeights>().map(|(e, _)| e))
            .collect();
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
                    v = crate::deform::skin(&v, &p, *how, skin.weights, binding.deformation, palette, model)
                        .map_err(|x| format!("{drawn:?} primitive {i}: {}", x.0))?;
                    log.count("primitives skinned at a pose", 1);
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
        }
        Ok(())
    }
    fn removes_by_log(&self) -> bool {
        true
    }
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
/// weights are all zero is refused (nothing to normalise). Counted: vertices changed, and the largest change of one
/// weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NormalizeSkinWeights;

/// Weights summing to `unit` from shares `w` (any non-negative values with a positive sum), by largest remainder.
fn quantize(w: &[f64], unit: u64) -> Vec<u64> {
    let sum: f64 = w.iter().sum();
    let exact: Vec<f64> = w.iter().map(|x| x / sum * unit as f64).collect();
    let mut q: Vec<u64> = exact.iter().map(|x| x.floor() as u64).collect();
    let mut left = unit - q.iter().sum::<u64>();
    let mut order: Vec<usize> = (0..w.len()).collect();
    order.sort_by(|&a, &b| {
        let (fa, fb) = (exact[a] - exact[a].floor(), exact[b] - exact[b].floor());
        fb.partial_cmp(&fa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    for &k in &order {
        if left == 0 {
            break;
        }
        q[k] += 1;
        left -= 1;
    }
    q
}

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
                let n = p.vertex_count();
                // every vertex's weights over all its sets, as floats in [0, 1]
                let mut all: Vec<Vec<f64>> = vec![Vec::new(); n];
                let mut units = Vec::new();
                for k in 0..sets {
                    let (values, unit): (Vec<[f64; 4]>, u64) = match p.streams.get(&Semantic::Weights(k)) {
                        Some(Stream::F32x4(v)) => (v.iter().map(|x| x.map(f64::from)).collect(), 0),
                        Some(Stream::U8x4(v)) => {
                            (v.iter().map(|x| x.map(|y| f64::from(y) / 255.0)).collect(), 255)
                        }
                        Some(Stream::U16x4(v)) => (
                            v.iter().map(|x| x.map(|y| f64::from(y) / 65535.0)).collect(),
                            65535,
                        ),
                        _ => {
                            return Err(format!(
                                "{e:?} primitive {i}: Weights({k}) is not a four-value stream"
                            ));
                        }
                    };
                    units.push(unit);
                    for (v, x) in all.iter_mut().zip(values) {
                        v.extend(x);
                    }
                }
                if units.iter().any(|&u| u != units[0]) {
                    return Err(format!("{e:?} primitive {i}: weight sets of different types"));
                }
                let unit = units[0];
                for (v, ws) in all.iter_mut().enumerate() {
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
                let streams = p.streams_mut();
                for k in 0..sets {
                    let at = |v: &Vec<f64>, c: usize| v[usize::from(k) * 4 + c];
                    let stream = match unit {
                        0 => Stream::F32x4(
                            all.iter()
                                .map(|v| std::array::from_fn(|c| at(v, c) as f32))
                                .collect(),
                        ),
                        255 => Stream::U8x4(
                            all.iter()
                                .map(|v| std::array::from_fn(|c| (at(v, c) * 255.0).round() as u8))
                                .collect(),
                        ),
                        _ => Stream::U16x4(
                            all.iter()
                                .map(|v| std::array::from_fn(|c| (at(v, c) * 65535.0).round() as u16))
                                .collect(),
                        ),
                    };
                    streams.insert(Semantic::Weights(k), stream);
                }
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
    reg.register("normalize-skin-weights", |o| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(NormalizeSkinWeights)),
    });
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
        Joint, JointId, Primitive, Property, Rest, Slot, Topology, WeightRule,
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
                meshes: vec![placed[0]],
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
        let (mut w, _, rigs, placed) = world();
        let clip = w.get::<ClipBinding>(Entity::from_index(9)).map(|b| b.clip);
        let clip = clip.unwrap_or_else(|| w.iter::<ClipBinding>().next().unwrap().1.clip);
        let second = w.spawn();
        w.insert(
            second,
            ClipBinding {
                clip,
                rig: Some(rigs[0]),
                meshes: vec![placed[0]],
            },
        );
        let err = e.run(&mut w, &mut PassLog::default()).unwrap_err();
        assert!(err.contains("a second clip binding"), "{err}");
    }
}
