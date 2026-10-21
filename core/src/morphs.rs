//! A mesh's morph targets (`core.morphs`, blend shapes): per-vertex deltas of its primitives, on the entity holding the
//! mesh, so every edit of the geometry carries them.
//!
//! A target's identity is its [`MorphId`], scoped to the asset's morph set: the prototype and each of its level-of-detail
//! forms carry their own `core.morphs` for their own vertices, under the same ids, so a clip or an occurrence's weights
//! name a target whichever form is drawn. Its name is the source's, provenance only.
//!
//! Meaning (core's evaluation rule; a source whose engine evaluates otherwise keeps its data in its namespace): a
//! target at weight `w` adds to each vertex its delta at `w`. With one frame at weight 1 (glTF's targets) that is
//! `w` times the frame's deltas. With several frames (in-between shapes) the delta is piecewise linear in `w` through
//! `(0, 0)` and each frame's `(weight, deltas)` in increasing weight, extrapolated along the last segment above the
//! last frame and along the first below 0. Targets add; they apply to the mesh as stored, before any skin. Normals
//! and tangents with deltas are renormalised after the deltas are added.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::{Component, Entity, JsonError, Mesh, World};

/// A morph target's identity within its asset's morph set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MorphId(pub u32);

/// One primitive's deltas in one frame: dense (one per vertex, `indices` `None`) or sparse (one per listed vertex,
/// `indices` ascending and unique). Each attribute is optional; a present one has one delta per entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Deltas {
    pub indices: Option<Vec<u32>>,
    pub positions: Option<Vec<[f32; 3]>>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 3]>>,
}

/// One frame of a target: its weight and, per primitive of the mesh, its deltas (`None`: none for that primitive).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frame {
    pub weight: f32,
    pub primitives: Vec<Option<Deltas>>,
}

/// One morph target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Target {
    pub id: MorphId,
    /// The source's name: provenance, not identity.
    pub name: String,
    /// The weight the source gives it when nothing else sets one.
    pub default_weight: f32,
    /// In increasing weight; at least one.
    pub frames: Vec<Frame>,
}

/// `core.morphs` (see the module documentation).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Morphs {
    pub targets: Vec<Target>,
}

impl Deltas {
    fn entries(&self) -> Option<usize> {
        [&self.positions, &self.normals, &self.tangents]
            .into_iter()
            .flatten()
            .map(Vec::len)
            .next()
    }

    fn attributes_mut(&mut self) -> impl Iterator<Item = &mut Vec<[f32; 3]>> {
        [&mut self.positions, &mut self.normals, &mut self.tangents]
            .into_iter()
            .flatten()
    }

    /// The deltas for a primitive whose vertices were compacted or reordered: new vertex `i` is old vertex `from[i]`.
    /// Dense deltas are gathered; sparse ones keep each entry for every new vertex made from its old one.
    fn remapped(&self, from: &[u32]) -> Result<Deltas, String> {
        let mut out = self.clone();
        match &self.indices {
            None => {
                for (dst, src) in out.attributes_mut().zip(
                    [&self.positions, &self.normals, &self.tangents]
                        .into_iter()
                        .flatten(),
                ) {
                    *dst = from
                        .iter()
                        .map(|&i| {
                            src.get(i as usize)
                                .copied()
                                .ok_or_else(|| format!("vertex {i} of {}", src.len()))
                        })
                        .collect::<Result<_, _>>()?;
                }
            }
            Some(indices) => {
                // for each new vertex, the entry of its old one, if it has one
                let picked: Vec<(u32, usize)> = (0u32..)
                    .zip(from)
                    .filter_map(|(new, old)| indices.binary_search(old).ok().map(|k| (new, k)))
                    .collect();
                out.indices = Some(picked.iter().map(|&(new, _)| new).collect());
                for (dst, src) in out.attributes_mut().zip(
                    [&self.positions, &self.normals, &self.tangents]
                        .into_iter()
                        .flatten(),
                ) {
                    *dst = picked.iter().map(|&(_, k)| src[k]).collect();
                }
            }
        }
        Ok(out)
    }
}

impl Morphs {
    /// Target `id`.
    pub fn target(&self, id: MorphId) -> Option<&Target> {
        self.targets.iter().find(|t| t.id == id)
    }
}

impl Component for Morphs {
    const NAME: &'static str = "core.morphs";
    const GEOMETRY: crate::Geometry = crate::Geometry::PerVertex;
    /// Ids, names and weights, not the deltas.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let mut targets = Vec::new();
        for t in &self.targets {
            let frames: Vec<f32> = t.frames.iter().map(|f| f.weight).collect();
            targets.push(serde_json::json!({
                "id": t.id,
                "name": t.name,
                "default_weight": crate::json::to_value(&t.default_weight)?,
                "frames": crate::json::to_value(&frames)?,
            }));
        }
        Ok(Some(serde_json::json!({ "targets": targets })))
    }
    fn payload_bytes(&self) -> u64 {
        let deltas = self
            .targets
            .iter()
            .flat_map(|t| &t.frames)
            .flat_map(|f| f.primitives.iter().flatten());
        deltas
            .map(|d| {
                let values: usize = [&d.positions, &d.normals, &d.tangents]
                    .into_iter()
                    .flatten()
                    .map(Vec::len)
                    .sum();
                values as u64 * 12 + d.indices.as_ref().map_or(0, |i| i.len() as u64 * 4)
            })
            .sum()
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        let remapped = (|| {
            let mut out = self.clone();
            for f in out.targets.iter_mut().flat_map(|t| &mut t.frames) {
                f.primitives = crate::remap_list(&f.primitives, from)?;
            }
            Ok(out)
        })();
        Some(remapped)
    }
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        let remapped = (|| {
            let mut out = self.clone();
            for f in out.targets.iter_mut().flat_map(|t| &mut t.frames) {
                let n = f.primitives.len();
                let slot = f
                    .primitives
                    .get_mut(primitive)
                    .ok_or_else(|| format!("primitive {primitive} of {n}"))?;
                if let Some(d) = slot {
                    *d = d.remapped(from)?;
                }
            }
            Ok(out)
        })();
        Some(remapped)
    }
    /// Position deltas map by the linear part exactly. Normal and tangent deltas have an exact image only under a
    /// similarity (a rotation or mirror with one scale), where they turn with the rotation; under any other map a
    /// target with them is unsupported, so the World refuses the mapping.
    fn apply_linear(&self, l: &crate::Linear) -> crate::Linearity<Self> {
        let directions = self
            .targets
            .iter()
            .flat_map(|t| &t.frames)
            .flat_map(|f| f.primitives.iter().flatten())
            .any(|d| d.normals.is_some() || d.tangents.is_some());
        let scale = l.similarity();
        if directions && scale.is_none() {
            return crate::Linearity::Unsupported;
        }
        let mut out = self.clone();
        for d in out
            .targets
            .iter_mut()
            .flat_map(|t| &mut t.frames)
            .flat_map(|f| f.primitives.iter_mut().flatten())
        {
            for p in d.positions.iter_mut().flatten() {
                *p = l.point(*p);
            }
            if let Some(s) = scale {
                // the rotation (or mirror) alone: the map over its scale
                let turn = |v: [f32; 3]| l.point(v).map(|x| (f64::from(x) / s) as f32);
                for v in d.normals.iter_mut().chain(d.tangents.iter_mut()).flatten() {
                    *v = turn(*v);
                }
            }
        }
        crate::Linearity::Applied(out)
    }
    /// Ids unique; frames present, their weights finite and increasing, defaults finite; per frame one entry per
    /// primitive of the entity's mesh (when it has one loaded); dense deltas one per vertex, sparse ones ascending,
    /// unique and within the primitive; every attribute one delta per entry; every delta finite.
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mesh = w.get::<Mesh>(e);
        for t in &self.targets {
            let at = |m: String| format!("target {:?}: {m}", t.id);
            if !ids.insert(t.id) {
                return Err(at("listed twice".into()));
            }
            if !t.default_weight.is_finite() || t.frames.is_empty() {
                return Err(at("no frames, or a default weight not finite".into()));
            }
            if !t.frames.iter().all(|f| f.weight.is_finite())
                || !t.frames.windows(2).all(|p| p[0].weight < p[1].weight)
            {
                return Err(at("frame weights not finite and increasing".into()));
            }
            for (k, f) in t.frames.iter().enumerate() {
                if let Some(m) = mesh
                    && m.primitives().len() != f.primitives.len()
                {
                    return Err(at(format!(
                        "frame {k}: {} entries for {} primitives",
                        f.primitives.len(),
                        m.primitives().len()
                    )));
                }
                for (i, d) in f.primitives.iter().enumerate() {
                    let Some(d) = d else { continue };
                    let n = mesh.and_then(|m| m.primitives().get(i)).map(|p| p.vertex_count());
                    let entries = match &d.indices {
                        None => n.unwrap_or_else(|| d.entries().unwrap_or(0)),
                        Some(idx) => {
                            if !idx.windows(2).all(|p| p[0] < p[1]) {
                                return Err(at(format!(
                                    "frame {k} primitive {i}: sparse indices not ascending and unique"
                                )));
                            }
                            if let (Some(n), Some(&last)) = (n, idx.last())
                                && last as usize >= n
                            {
                                return Err(at(format!("frame {k} primitive {i}: vertex {last} of {n}")));
                            }
                            idx.len()
                        }
                    };
                    for a in [&d.positions, &d.normals, &d.tangents].into_iter().flatten() {
                        if a.len() != entries {
                            return Err(at(format!(
                                "frame {k} primitive {i}: {} deltas for {entries} entries",
                                a.len()
                            )));
                        }
                        if !a.iter().flatten().all(|x| x.is_finite()) {
                            return Err(at(format!("frame {k} primitive {i}: a delta not finite")));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Linear, Primitive, Semantic, Stream, Topology};

    fn points(n: usize) -> Primitive {
        Primitive {
            topology: Topology::Points,
            streams: [(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; n]))].into(),
            indices: None,
            material: None,
        }
    }

    /// Two primitives of 3 and 4 vertices; a target with one dense frame on the first, and a target with two frames
    /// (in-betweens at 0.5 and 1) sparse on the second.
    fn morphs() -> Morphs {
        Morphs {
            targets: vec![
                Target {
                    id: MorphId(1),
                    name: "smile".into(),
                    default_weight: 0.0,
                    frames: vec![Frame {
                        weight: 1.0,
                        primitives: vec![
                            Some(Deltas {
                                positions: Some(vec![[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]]),
                                normals: Some(vec![[0.0, 1.0, 0.0]; 3]),
                                ..Deltas::default()
                            }),
                            None,
                        ],
                    }],
                },
                Target {
                    id: MorphId(2),
                    name: "blink".into(),
                    default_weight: 0.25,
                    frames: [0.5, 1.0]
                        .map(|weight| Frame {
                            weight,
                            primitives: vec![
                                None,
                                Some(Deltas {
                                    indices: Some(vec![1, 2]),
                                    positions: Some(vec![[0.0, weight, 0.0], [0.0, 2.0 * weight, 0.0]]),
                                    ..Deltas::default()
                                }),
                            ],
                        })
                        .into(),
                },
            ],
        }
    }

    fn errors(m: Morphs) -> Vec<String> {
        let mut w = World::new();
        let e = w.spawn();
        w.insert(e, Mesh::new(vec![points(3), points(4)]));
        w.insert(e, m);
        w.verify_components().into_iter().map(|(_, _, m)| m).collect()
    }

    #[test]
    fn morphs_verify_and_refuse_malformed_targets() {
        assert_eq!(errors(morphs()), Vec::<String>::new());
        let fails = |edit: &dyn Fn(&mut Morphs), what: &str| {
            let mut m = morphs();
            edit(&mut m);
            let e = errors(m);
            assert!(e.iter().any(|x| x.contains(what)), "{what}: {e:?}");
        };
        fails(&|m| m.targets[1].id = MorphId(1), "listed twice");
        fails(&|m| m.targets[1].frames.reverse(), "increasing");
        fails(&|m| m.targets[0].frames.clear(), "no frames");
        fails(
            &|m| {
                m.targets[0].frames[0].primitives[0]
                    .as_mut()
                    .unwrap()
                    .positions
                    .as_mut()
                    .unwrap()
                    .pop()
                    .pipe(drop)
            },
            "2 deltas for 3 entries",
        );
        fails(
            &|m| m.targets[1].frames[0].primitives[1].as_mut().unwrap().indices = Some(vec![2, 1]),
            "not ascending",
        );
        fails(
            &|m| m.targets[1].frames[0].primitives[1].as_mut().unwrap().indices = Some(vec![1, 4]),
            "vertex 4 of 4",
        );
        fails(
            &|m| m.targets[0].frames[0].primitives.pop().pipe(drop),
            "1 entries for 2 primitives",
        );
        fails(
            &|m| {
                m.targets[0].frames[0].primitives[0]
                    .as_mut()
                    .unwrap()
                    .normals
                    .as_mut()
                    .unwrap()[0][0] = f32::NAN
            },
            "not finite",
        );
    }

    /// Hand-worked vertex remaps: dense deltas gathered (new 0 from old 2, new 1 from old 0); sparse entries kept for
    /// every new vertex made from a listed old one (old 1 and 2 listed; new 0 and 2 from old 2, new 1 from old 0, not
    /// listed): indices [0, 2], both the old 2's delta.
    #[test]
    fn deltas_follow_compacted_vertices_dense_and_sparse() {
        let m = morphs();
        let dense = m.remap_vertices(0, &[2, 0]).unwrap().unwrap();
        let d = dense.targets[0].frames[0].primitives[0].as_ref().unwrap();
        assert_eq!(d.positions, Some(vec![[3.0, 0.0, 0.0], [1.0, 0.0, 0.0]]));
        assert_eq!(d.normals.as_ref().map(Vec::len), Some(2));
        let sparse = m.remap_vertices(1, &[2, 0, 2]).unwrap().unwrap();
        let s = sparse.targets[1].frames[1].primitives[1].as_ref().unwrap();
        assert_eq!(s.indices, Some(vec![0, 2]));
        assert_eq!(s.positions, Some(vec![[0.0, 2.0, 0.0], [0.0, 2.0, 0.0]]));
        // a dense delta the list lacks is an error
        assert!(m.remap_vertices(0, &[5]).unwrap().is_err());
        // a rebuild of the primitive list: the second primitive alone
        let rebuilt = m.remap_primitives(&[1]).unwrap().unwrap();
        assert!(rebuilt.targets[0].frames[0].primitives == [None]);
        assert!(rebuilt.targets[1].frames[0].primitives[0].is_some());
    }

    /// Positions map by the linear part; normals turn under a similarity (90 degrees about z: (0, 1, 0) to (-1, 0, 0),
    /// also with a scale of 2) and make any other map unsupported.
    #[test]
    fn deltas_follow_a_linear_map() {
        let m = morphs();
        let turn2 = Linear::new([[0.0, 2.0, 0.0], [-2.0, 0.0, 0.0], [0.0, 0.0, 2.0]]);
        let crate::Linearity::Applied(t) = m.apply_linear(&turn2) else {
            panic!("a similarity applies")
        };
        let d = t.targets[0].frames[0].primitives[0].as_ref().unwrap();
        assert_eq!(d.positions.as_ref().unwrap()[0], [0.0, 2.0, 0.0]);
        assert_eq!(d.normals.as_ref().unwrap()[0], [-1.0, 0.0, 0.0]);
        let stretch = Linear::new([[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert_eq!(m.apply_linear(&stretch), crate::Linearity::Unsupported);
        // without normal or tangent deltas, any map applies to the positions
        let mut positions_only = morphs();
        positions_only.targets[0].frames[0].primitives[0]
            .as_mut()
            .unwrap()
            .normals = None;
        let crate::Linearity::Applied(s) = positions_only.apply_linear(&stretch) else {
            panic!("positions alone follow any map")
        };
        assert_eq!(
            s.targets[0].frames[0].primitives[0]
                .as_ref()
                .unwrap()
                .positions
                .as_ref()
                .unwrap()[2],
            [6.0, 0.0, 0.0]
        );
    }

    trait Pipe: Sized {
        fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
            f(self)
        }
    }
    impl<T> Pipe for T {}
}
