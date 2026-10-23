//! A mesh's skin (`core.skin`): how its vertices are bound to joints, on the entity holding the mesh (a prototype or
//! one of its level-of-detail forms), so every edit of the geometry carries it.
//!
//! Two parts, as sources split them:
//! - **Influences**, per primitive of the mesh: how its vertices name their joints. With [`Influences::Streams`] the
//!   joints and weights are the primitive's own vertex streams (`Joints(k)` and `Weights(k)` for each set `k`, four
//!   influences a set), so every vertex edit carries them with the rest of the vertex; [`Influences::SingleIndex`] is
//!   one joint a vertex (`Joints(0)` alone, weight 1), [`Influences::Whole`] one joint for the whole primitive (no
//!   per-vertex data). The joint a vertex names is a slot of the primitive's palette, not a joint itself.
//! - **Bindings**: each gives the bound primitives a palette, slot by slot a joint of a skeleton and its inverse bind
//!   (mesh space to that joint's space at bind time), and says how the mesh deforms. One geometry may have several
//!   bindings (Unity: renderers sharing one mesh, each binding its bone slots to other bones); an occurrence names the
//!   binding it draws with. Inverse binds belong to the binding: two meshes on one skeleton may bind differently.
//!
//! The only deformation core states is linear blend skinning ([`Deformation::Linear`]); a source deforming otherwise
//! keeps its data in its namespace until a pass lowers it. Weights are as the source stores them ([`WeightRule`]); no
//! reader renormalises them.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::frame::SourceFrame;
use crate::skeleton::{JointId, Skeleton};
use crate::{Component, Entity, JsonError, Mesh, Primitive, Semantic, Stream, Transform, World};

/// A binding's identity within its skin (what an occurrence names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct BindingId(pub u32);

/// How a primitive's vertices name the slots of their palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Influences {
    /// `Joints(k)` (slot indices, `U8x4` or `U16x4`) and `Weights(k)` (`F32x4`, or `U8x4`/`U16x4` normalised) for each
    /// set `k` below `sets`: four influences a set. Unused influences are zero-weight padding; their slot indices are
    /// still in range.
    Streams { sets: u8 },
    /// `Joints(0)` alone: each vertex bound wholly to the slot in its first index (FromSoft's static meshes).
    SingleIndex,
    /// Every vertex bound wholly to palette slot `slot`, with no per-vertex data (a model on a bone).
    Whole { slot: u32 },
}

/// What a source says of its stored weights.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum WeightRule {
    /// Each vertex's weights sum to 1 as stored (within quantisation).
    SumsToOne,
    /// The engine divides each weight by `max(sum, floor)` when it draws: a vertex whose weights sum to `floor` or
    /// more is normalised to sum 1, one that sums to less keeps a total below 1 (all four weights zero leave the
    /// zero matrix). `floor` is the divisor's lower bound, finite and zero or more; zero is the plain sum (a vertex
    /// with no weight has nothing to divide, and a pass using the rule refuses it). FromSoft's skinning programs
    /// use a floor of 0.001.
    EngineNormalizes { floor: f32 },
    /// The source states nothing of their sum: used as stored.
    AsStored,
}

/// How a binding deforms its mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Deformation {
    /// Linear blend skinning: a vertex goes to `sum_i w_i J_i B_i v`, `J_i` its joint's pose in the skeleton's space
    /// and `B_i` the slot's inverse bind; normals and tangents by the same blend of linear parts.
    Linear,
}

/// One slot of a palette: the joint and its inverse bind (mesh space to the joint's space at bind time).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Slot {
    pub joint: JointId,
    pub inverse_bind: Transform,
}

/// One way the mesh is bound: to which skeleton, each bound primitive's palette, and how it deforms.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Binding {
    pub id: BindingId,
    /// The entity holding the `core.skeleton`.
    pub skeleton: Entity,
    /// Per primitive of the mesh, its palette: `Some` exactly for the primitives the skin's influences bind.
    pub palettes: Vec<Option<Vec<Slot>>>,
    pub deformation: Deformation,
}

/// `core.skin` (see the module documentation).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Skin {
    /// Per primitive of the mesh: how its vertices name their slots; `None` for a primitive not skinned (drawn as
    /// stored, with no joint or weight streams).
    pub influences: Vec<Option<Influences>>,
    pub weights: WeightRule,
    pub bindings: Vec<Binding>,
}

impl WeightRule {
    /// The divisor the engine applies to a vertex whose weights sum to `sum`; `None` for the rules that apply none
    /// (the weights are used as stored).
    pub fn divisor(self, sum: f64) -> Option<f64> {
        match self {
            WeightRule::EngineNormalizes { floor } => Some(sum.max(f64::from(floor))),
            WeightRule::SumsToOne | WeightRule::AsStored => None,
        }
    }
}

/// How far a [`WeightRule::SumsToOne`] vertex's weights may sum from 1: one step of 8-bit storage over four
/// influences, rounded each.
pub const SUM_TOLERANCE: f32 = 4.0 / 255.0;

/// A weights stream decoded to floats (integers as normalised); `None` for a type weights can't be.
fn weights(s: &Stream) -> Option<Vec<[f32; 4]>> {
    Some(match s {
        Stream::F32x4(v) => v.clone(),
        Stream::U8x4(v) => v.iter().map(|w| w.map(|x| f32::from(x) / 255.0)).collect(),
        Stream::U16x4(v) => v.iter().map(|w| w.map(|x| f32::from(x) / 65535.0)).collect(),
        Stream::F32x2(_) | Stream::F32x3(_) => return None,
    })
}

/// A joints stream's slot indices; `None` for a type joints can't be.
fn joints(s: &Stream) -> Option<Vec<[u32; 4]>> {
    Some(match s {
        Stream::U8x4(v) => v.iter().map(|j| j.map(u32::from)).collect(),
        Stream::U16x4(v) => v.iter().map(|j| j.map(u32::from)).collect(),
        Stream::F32x2(_) | Stream::F32x3(_) | Stream::F32x4(_) => return None,
    })
}

/// Whether a primitive has any joint or weight stream.
pub fn has_skin_streams(p: &Primitive) -> bool {
    p.streams
        .keys()
        .any(|s| matches!(s, Semantic::Joints(_) | Semantic::Weights(_)))
}

impl Skin {
    /// The binding `id`.
    pub fn binding(&self, id: BindingId) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.id == id)
    }

    /// The skin with its inverse binds converted from `frame` into the world's, for a reader whose mesh and skeleton
    /// are both in that frame: `B' = C B C^-1` ([`SourceFrame::transform`]), exact.
    pub fn converted(&self, frame: &SourceFrame) -> Skin {
        let mut out = self.clone();
        for slot in out
            .bindings
            .iter_mut()
            .flat_map(|b| b.palettes.iter_mut().flatten().flatten())
        {
            slot.inverse_bind = frame.transform(&slot.inverse_bind);
        }
        out
    }

    /// Primitive `i`'s palettes, which need no vertex: each binding has one exactly when the primitive has influences,
    /// naming joints of its skeleton with finite inverse binds, and a whole-primitive slot is within every one. Returns
    /// the number of slots every palette has (the shortest's): every slot a vertex names must be below it.
    fn verify_palettes(&self, i: usize, w: &World) -> Result<usize, String> {
        let influences = self.influences.get(i).copied().flatten();
        let slots_in_every_palette = self
            .bindings
            .iter()
            .map(|b| b.palettes.get(i).cloned().flatten().map_or(0, |p| p.len()))
            .min()
            .unwrap_or(0);
        for b in &self.bindings {
            let palette = b.palettes.get(i).cloned().flatten();
            if palette.is_some() != influences.is_some() {
                return Err(format!(
                    "binding {:?}: primitive {i} has a palette {} influences",
                    b.id,
                    if palette.is_some() {
                        "without"
                    } else {
                        "missing for its"
                    }
                ));
            }
            if let Some(slots) = palette {
                let skeleton = w
                    .get::<Skeleton>(b.skeleton)
                    .ok_or_else(|| format!("binding {:?}: {:?} holds no core.skeleton", b.id, b.skeleton))?;
                for (k, s) in slots.iter().enumerate() {
                    if skeleton.index_of(s.joint).is_none() {
                        return Err(format!(
                            "binding {:?}: primitive {i} slot {k} names joint {:?}, not in its skeleton",
                            b.id, s.joint
                        ));
                    }
                    if !s.inverse_bind.cols.iter().flatten().all(|x| x.is_finite()) {
                        return Err(format!(
                            "binding {:?}: primitive {i} slot {k}: inverse bind not finite",
                            b.id
                        ));
                    }
                }
            }
        }
        if let Some(Influences::Whole { slot }) = influences
            && usize::try_from(slot).map_or(true, |s| s >= slots_in_every_palette)
        {
            return Err(format!(
                "primitive {i}: slot {slot} of a palette of {slots_in_every_palette}"
            ));
        }
        Ok(slots_in_every_palette)
    }

    /// Primitive `i`'s influences against its streams, and its palettes ([`Skin::verify_palettes`]).
    fn verify_primitive(&self, i: usize, p: &Primitive, w: &World) -> Result<(), String> {
        let influences = self.influences.get(i).copied().flatten();
        let slots_in_every_palette = self.verify_palettes(i, w)?;
        let skin_streams: BTreeSet<Semantic> = p
            .streams
            .keys()
            .copied()
            .filter(|s| matches!(s, Semantic::Joints(_) | Semantic::Weights(_)))
            .collect();
        let expected: BTreeSet<Semantic> = match influences {
            None | Some(Influences::Whole { .. }) => BTreeSet::new(),
            Some(Influences::SingleIndex) => [Semantic::Joints(0)].into(),
            Some(Influences::Streams { sets }) => (0..sets)
                .flat_map(|k| [Semantic::Joints(k), Semantic::Weights(k)])
                .collect(),
        };
        if skin_streams != expected {
            return Err(format!(
                "primitive {i}: joint and weight streams {skin_streams:?} for influences {influences:?}"
            ));
        }
        let in_range = |slot: u32| usize::try_from(slot).is_ok_and(|s| s < slots_in_every_palette);
        match influences {
            None => {}
            // checked with the palettes
            Some(Influences::Whole { .. }) => {}
            Some(Influences::SingleIndex) => {
                let j = joints(&p.streams[&Semantic::Joints(0)])
                    .ok_or_else(|| format!("primitive {i}: Joints(0) is not an integer stream"))?;
                if let Some(v) = j.iter().position(|j| !in_range(j[0])) {
                    return Err(format!(
                        "primitive {i}: vertex {v} names slot {} of a palette of {slots_in_every_palette}",
                        j[v][0]
                    ));
                }
            }
            Some(Influences::Streams { sets }) => {
                let vertices = p.vertex_count();
                let mut sums = vec![0.0f32; vertices];
                for k in 0..sets {
                    let j = joints(&p.streams[&Semantic::Joints(k)])
                        .ok_or_else(|| format!("primitive {i}: Joints({k}) is not an integer stream"))?;
                    let wt = weights(&p.streams[&Semantic::Weights(k)])
                        .ok_or_else(|| format!("primitive {i}: Weights({k}) is not a four-value stream"))?;
                    if j.len() != vertices || wt.len() != vertices {
                        return Err(format!(
                            "primitive {i}: Joints({k}) and Weights({k}) hold {} and {} values for {vertices} vertices",
                            j.len(),
                            wt.len()
                        ));
                    }
                    for (v, (j, wt)) in j.iter().zip(&wt).enumerate() {
                        if let Some(x) = j.iter().find(|&&x| !in_range(x)) {
                            return Err(format!(
                                "primitive {i}: vertex {v} names slot {x} of a palette of {slots_in_every_palette}"
                            ));
                        }
                        if wt.iter().any(|x| !x.is_finite() || *x < 0.0) {
                            return Err(format!(
                                "primitive {i}: vertex {v} has a weight not finite or negative"
                            ));
                        }
                        sums[v] += wt.iter().sum::<f32>();
                    }
                }
                if self.weights == WeightRule::SumsToOne
                    && let Some(v) = sums.iter().position(|s| (s - 1.0).abs() > SUM_TOLERANCE)
                {
                    return Err(format!(
                        "primitive {i}: vertex {v}'s weights sum to {}, not 1",
                        sums[v]
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Component for Skin {
    const NAME: &'static str = "core.skin";
    /// Per primitive (influences, palettes); its per-vertex data are the mesh's own streams, which carry themselves.
    const GEOMETRY: crate::Geometry = crate::Geometry::PerPrimitive;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.bindings.iter().map(|b| b.skeleton));
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        let remapped = (|| {
            let mut out = self.clone();
            out.influences = crate::remap_list(&self.influences, from)?;
            for b in &mut out.bindings {
                b.palettes = crate::remap_list(&b.palettes, from)?;
            }
            Ok(out)
        })();
        Some(remapped)
    }
    /// The entity's mesh space mapped by `l` (a pass baking a placement's mirror or shear into its geometry): each
    /// inverse bind takes the new mesh space back first, `B' = B L^-1`. A singular `l` has no inverse: unsupported, so
    /// the World refuses the mapping.
    fn apply_linear(&self, l: &crate::Linear) -> crate::Linearity<Self> {
        let Some(inv) = l.inverse_columns() else {
            return crate::Linearity::Unsupported;
        };
        let mut out = self.clone();
        for slot in out
            .bindings
            .iter_mut()
            .flat_map(|b| b.palettes.iter_mut().flatten().flatten())
        {
            let b: [[f64; 3]; 4] = slot.inverse_bind.cols.map(|c| c.map(f64::from));
            let mut cols = [[0.0f32; 3]; 4];
            for (j, inv_col) in inv.iter().enumerate() {
                cols[j] = std::array::from_fn(|r| (0..3).map(|k| b[k][r] * inv_col[k]).sum::<f64>() as f32);
            }
            cols[3] = slot.inverse_bind.cols[3];
            slot.inverse_bind = Transform { cols };
        }
        crate::Linearity::Applied(out)
    }
    /// One entry of influences and of each binding's palettes per primitive of the entity's mesh (when it has one
    /// loaded); binding ids unique; each binding's skeleton a `core.skeleton` holding every joint its palettes name;
    /// inverse binds finite; per primitive, joint and weight streams exactly those its influences say, every slot a
    /// vertex names within every binding's palette (padding included), weights finite and non-negative, and summing to
    /// 1 under [`WeightRule::SumsToOne`].
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        if let WeightRule::EngineNormalizes { floor } = self.weights
            && (!floor.is_finite() || floor < 0.0)
        {
            return Err(format!("weight rule floor {floor}: finite, zero or more"));
        }
        let mut ids = BTreeSet::new();
        for b in &self.bindings {
            if !ids.insert(b.id) {
                return Err(format!("binding {:?} listed twice", b.id));
            }
            if b.palettes.len() != self.influences.len() {
                return Err(format!(
                    "binding {:?}: {} palettes for {} primitives",
                    b.id,
                    b.palettes.len(),
                    self.influences.len()
                ));
            }
        }
        // a mesh not loaded (only its `core.mesh_file`) has no streams to check: its palettes still are
        let Some(mesh) = w.get::<Mesh>(e) else {
            for i in 0..self.influences.len() {
                self.verify_palettes(i, w)?;
            }
            return Ok(());
        };
        if mesh.primitives().len() != self.influences.len() {
            return Err(format!(
                "{} primitives, {} influence entries",
                mesh.primitives().len(),
                self.influences.len()
            ));
        }
        for (i, p) in mesh.primitives().iter().enumerate() {
            self.verify_primitive(i, p, w)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{Joint, Rest};
    use crate::{Linear, Topology};

    fn translate(t: [f32; 3]) -> Transform {
        let mut m = Transform::IDENTITY;
        m.cols[3] = t;
        m
    }

    fn joint(id: u32, parent: Option<u32>, t: [f32; 3]) -> Joint {
        Joint {
            id: JointId(id),
            name: format!("j{id}"),
            parent: parent.map(JointId),
            rest: Rest::Affine(translate(t)),
        }
    }

    /// A skeleton of two joints (a root at the origin, a child 1 m up), a mesh of two primitives: a triangle skinned
    /// with one set of streams (vertex 0 wholly on slot 0, vertex 1 split half and half, vertex 2 wholly on slot 1)
    /// and three points bound whole to slot 1.
    fn world() -> (World, Entity, Entity) {
        let mut w = World::new();
        let skeleton = w.spawn();
        w.insert(
            skeleton,
            Skeleton {
                joints: vec![joint(10, None, [0.0; 3]), joint(11, Some(10), [0.0, 1.0, 0.0])],
            },
        );
        let e = w.spawn();
        let triangle = Primitive {
            topology: Topology::Triangles,
            streams: [
                (
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [0.0, 1.0, 0.0], [0.0, 2.0, 0.0]]),
                ),
                (
                    Semantic::Joints(0),
                    Stream::U8x4(vec![[0, 0, 0, 0], [0, 1, 0, 0], [1, 0, 0, 0]]),
                ),
                (
                    Semantic::Weights(0),
                    Stream::U8x4(vec![[255, 0, 0, 0], [128, 127, 0, 0], [255, 0, 0, 0]]),
                ),
            ]
            .into(),
            indices: None,
            material: None,
        };
        let points = Primitive {
            topology: Topology::Points,
            streams: [(Semantic::Position, Stream::F32x3(vec![[0.0, 1.0, 0.0]; 3]))].into(),
            indices: None,
            material: None,
        };
        w.insert(e, Mesh::new(vec![triangle, points]));
        let palette = vec![
            Slot {
                joint: JointId(10),
                inverse_bind: Transform::IDENTITY,
            },
            Slot {
                joint: JointId(11),
                inverse_bind: translate([0.0, -1.0, 0.0]),
            },
        ];
        w.insert(
            e,
            Skin {
                influences: vec![
                    Some(Influences::Streams { sets: 1 }),
                    Some(Influences::Whole { slot: 1 }),
                ],
                weights: WeightRule::SumsToOne,
                bindings: vec![Binding {
                    id: BindingId(0),
                    skeleton,
                    palettes: vec![Some(palette.clone()), Some(palette)],
                    deformation: Deformation::Linear,
                }],
            },
        );
        (w, e, skeleton)
    }

    fn errors(w: &World) -> Vec<String> {
        w.verify_components().into_iter().map(|(_, _, m)| m).collect()
    }

    #[test]
    fn a_skinned_mesh_verifies_and_reaches_its_skeleton() {
        let (w, e, skeleton) = world();
        assert_eq!(errors(&w), Vec::<String>::new());
        assert_eq!(w.closure([e]), [e, skeleton].into());
    }

    /// Each broken invariant fails verification, naming what broke.
    #[test]
    fn malformed_skins_fail_verification() {
        let fails = |edit: &dyn Fn(&mut World, Entity, Entity), what: &str| {
            let (mut w, e, s) = world();
            edit(&mut w, e, s);
            let errs = errors(&w);
            assert!(errs.iter().any(|m| m.contains(what)), "{what}: {errs:?}");
        };
        // joint streams on a mesh without a skin: the mesh's own check
        fails(
            &|w, e, _| {
                w.remove::<Skin>(e);
            },
            "without a core.skin",
        );
        // a vertex naming a slot past the palette
        fails(
            &|w, e, _| {
                let mut m = w.get::<Mesh>(e).unwrap().clone();
                m.primitive_mut(0).unwrap().streams_mut().insert(
                    Semantic::Joints(0),
                    Stream::U8x4(vec![[0, 0, 0, 0], [0, 2, 0, 0], [1, 0, 0, 0]]),
                );
                w.insert(e, m);
            },
            "names slot 2 of a palette of 2",
        );
        // weights not summing to one under SumsToOne; fine as stored
        let low = |w: &mut World, e: Entity| {
            let mut m = w.get::<Mesh>(e).unwrap().clone();
            m.primitive_mut(0).unwrap().streams_mut().insert(
                Semantic::Weights(0),
                Stream::U8x4(vec![[255, 0, 0, 0], [100, 100, 0, 0], [255, 0, 0, 0]]),
            );
            w.insert(e, m);
        };
        fails(&|w, e, _| low(w, e), "sum to");
        let (mut w, e, _) = world();
        low(&mut w, e);
        w.get_mut::<Skin>(e).unwrap().weights = WeightRule::AsStored;
        assert_eq!(errors(&w), Vec::<String>::new());
        // the divisor floor of the engine-normalised rule: finite and not negative
        for floor in [f32::NAN, -0.001, f32::INFINITY] {
            let (mut w, e, _) = world();
            w.get_mut::<Skin>(e).unwrap().weights = WeightRule::EngineNormalizes { floor };
            assert!(
                errors(&w).iter().any(|m| m.contains("floor")),
                "{floor}: {:?}",
                errors(&w)
            );
        }
        let (mut w, e, _) = world();
        w.get_mut::<Skin>(e).unwrap().weights = WeightRule::EngineNormalizes { floor: 0.001 };
        assert_eq!(errors(&w), Vec::<String>::new());
        // the divisor is max(sum, floor)
        let rule = WeightRule::EngineNormalizes { floor: 0.001 };
        assert_eq!(rule.divisor(0.996), Some(0.996));
        assert_eq!(rule.divisor(0.0), Some(0.001));
        assert_eq!(WeightRule::AsStored.divisor(0.5), None);
        // a negative float weight
        fails(
            &|w, e, _| {
                let mut m = w.get::<Mesh>(e).unwrap().clone();
                m.primitive_mut(0).unwrap().streams_mut().insert(
                    Semantic::Weights(0),
                    Stream::F32x4(vec![
                        [1.0, 0.0, 0.0, 0.0],
                        [1.5, -0.5, 0.0, 0.0],
                        [1.0, 0.0, 0.0, 0.0],
                    ]),
                );
                w.insert(e, m);
            },
            "not finite or negative",
        );
        // joint and weight streams longer than the positions: named, not a panic (the mesh's own check names it too)
        fails(
            &|w, e, _| {
                let mut m = w.get::<Mesh>(e).unwrap().clone();
                let mut p = m.primitive_mut(0).unwrap();
                let streams = p.streams_mut();
                streams.insert(Semantic::Joints(0), Stream::U8x4(vec![[0, 0, 0, 0]; 4]));
                streams.insert(Semantic::Weights(0), Stream::U8x4(vec![[255, 0, 0, 0]; 4]));
                w.insert(e, m);
            },
            "hold 4 and 4 values for 3 vertices",
        );
        // streams that do not match the influences: two sets said, one stored
        fails(
            &|w, e, _| w.get_mut::<Skin>(e).unwrap().influences[0] = Some(Influences::Streams { sets: 2 }),
            "joint and weight streams",
        );
        // a palette joint the skeleton lacks; a skeleton entity without a skeleton
        fails(
            &|w, e, _| {
                w.get_mut::<Skin>(e).unwrap().bindings[0].palettes[1]
                    .as_mut()
                    .unwrap()[0]
                    .joint = JointId(99)
            },
            "not in its skeleton",
        );
        fails(
            &|w, _, s| {
                w.remove::<Skeleton>(s);
            },
            "holds no core.skeleton",
        );
        // a whole-bound slot past the palette; a palette for a primitive the influences do not bind
        fails(
            &|w, e, _| w.get_mut::<Skin>(e).unwrap().influences[1] = Some(Influences::Whole { slot: 5 }),
            "slot 5 of a palette of 2",
        );
        fails(
            &|w, e, _| w.get_mut::<Skin>(e).unwrap().influences[1] = None,
            "a palette without influences",
        );
        // a mesh still only in its file: no streams to check, its palettes still are
        let unloaded = |w: &mut World, e: Entity| {
            w.remove::<Mesh>(e);
            w.insert(
                e,
                crate::MeshFile {
                    path: "skinned.glb".into(),
                    mesh: 0,
                    materials: vec![],
                },
            );
        };
        fails(
            &|w, e, _| {
                unloaded(w, e);
                w.get_mut::<Skin>(e).unwrap().bindings[0].palettes[1]
                    .as_mut()
                    .unwrap()[0]
                    .joint = JointId(99)
            },
            "not in its skeleton",
        );
        fails(
            &|w, e, _| {
                unloaded(w, e);
                w.get_mut::<Skin>(e).unwrap().influences[1] = Some(Influences::Whole { slot: 5 })
            },
            "slot 5 of a palette of 2",
        );
        // the entries do not match the mesh's primitives
        fails(
            &|w, e, _| {
                let mut s = w.get::<Skin>(e).unwrap().clone();
                s.influences.pop();
                s.bindings[0].palettes.pop();
                w.insert(e, s);
            },
            "2 primitives, 1 influence entries",
        );
    }

    /// A rebuild of the primitive list carries the influences and every binding's palettes (the triangle kept twice,
    /// the points dropped), and the skin still verifies with the rebuilt mesh.
    #[test]
    fn the_skin_follows_a_primitive_rebuild() {
        let (mut w, e, _) = world();
        w.rebuild_primitives(e, |m| {
            Ok((
                vec![m.primitives()[0].clone(), m.primitives()[0].clone()],
                vec![0, 0],
            ))
        })
        .unwrap();
        let s = w.get::<Skin>(e).unwrap();
        assert_eq!(s.influences, [Some(Influences::Streams { sets: 1 }); 2]);
        assert_eq!(s.bindings[0].palettes.len(), 2);
        assert_eq!(errors(&w), Vec::<String>::new());
    }

    /// Hand-worked: a slot binding joint 11 (at y = 1) has inverse bind T(0, -1, 0); the mesh's space scaled by 2 along
    /// y puts the vertex that was at (0, 1, 0) at (0, 2, 0). B' = B L^-1 takes (0, 2, 0) to the origin of the joint's
    /// space, as B took (0, 1, 0): its linear part is diag(1, 0.5, 1), its translation unchanged. A singular map is
    /// refused.
    #[test]
    fn inverse_binds_follow_a_linear_map_of_the_mesh() {
        let (w, e, _) = world();
        let skin = w.get::<Skin>(e).unwrap();
        let l = Linear::new([[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 1.0]]);
        let crate::Linearity::Applied(mapped) = skin.apply_linear(&l) else {
            panic!("an invertible map applies")
        };
        let b = mapped.bindings[0].palettes[0].as_ref().unwrap()[1].inverse_bind;
        assert_eq!(
            b.cols,
            [
                [1.0, 0.0, 0.0],
                [0.0, 0.5, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, -1.0, 0.0]
            ]
        );
        assert_eq!(b.apply([0.0, 2.0, 0.0]), [0.0, 0.0, 0.0]);
        let flat = Linear::new([[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
        assert_eq!(skin.apply_linear(&flat), crate::Linearity::Unsupported);
    }

    /// A reader converting mesh, skeleton and skin from one source frame keeps the bind: the converted joint's rest
    /// times the converted inverse bind takes the converted vertex where the source's took the source's, converted.
    #[test]
    fn frame_conversion_keeps_the_bind() {
        let (w, e, s) = world();
        let frame = SourceFrame::ENU;
        let skin = w.get::<Skin>(e).unwrap().converted(&frame);
        let skeleton = w.get::<Skeleton>(s).unwrap().converted(&frame);
        let joint = skeleton.joints[1].rest.matrix();
        let b = skin.bindings[0].palettes[0].as_ref().unwrap()[1].inverse_bind;
        let v = [0.0, 2.0, 0.0];
        // source: rest T(0, 1, 0), inverse bind T(0, -1, 0): v stays where it is
        assert_eq!(joint.apply(b.apply(frame.point(v))), frame.point(v));
        assert_eq!(b, frame.transform(&translate([0.0, -1.0, 0.0])));
    }
}
