//! The components a lowered `core.layered` blend is drawn with ([`crate::passes::layered`]): what a vertex-blend
//! material draws and the per-vertex opacity of a mesh's primitives.

use primase_core::{Component, Entity, Mesh, World};
use serde::Serialize;

/// What a vertex-blend material draws (`reskate.vertex_blend`, on material entities): the ReSkate writer emits it
/// with Studio's `vertex_blend` flag and its primitives' opacities ([`BlendOpacity`]) as the per-vertex weight, and
/// records it for the post-compile stage, which moves it onto the stock decal row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum VertexBlend {
    /// A layer of a lowered `core.layered` blend drawn over its base (slot `>= 1`).
    Overlay { slot: u8 },
    /// The black overlay of a lowered blend whose weights sum below one.
    Darkening,
    /// A decal whose opacity is its vertex alpha.
    VertexAlpha,
}

impl VertexBlend {
    /// An overlay's slot is a layer above the base of a blend of at most four layers: 1..=3.
    pub fn check(self) -> Result<(), String> {
        match self {
            VertexBlend::Overlay { slot } if !(1..=3).contains(&slot) => {
                Err(format!("overlay slot {slot} outside 1..=3"))
            }
            _ => Ok(()),
        }
    }

    /// The overlay's place in its blend's lerp chain, drawn in increasing order: a slot's overlay its slot (1..=3),
    /// the darkening overlay after every slot (4). `None`: not part of a chain.
    pub fn chain_rank(self) -> Option<u8> {
        match self {
            VertexBlend::Overlay { slot } => Some(slot),
            VertexBlend::Darkening => Some(4),
            VertexBlend::VertexAlpha => None,
        }
    }
}

impl Component for VertexBlend {
    const NAME: &'static str = "reskate.vertex_blend";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn verify(&self, _: Entity, _: &World) -> Result<(), String> {
        self.check()
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// Per-vertex opacity of a mesh's primitives (`reskate.blend_opacity`, on mesh entities; `None`: the primitive is not
/// blended), 0..1. Core has no vertex semantic for a decal's opacity, so it travels beside `core.mesh`.
#[derive(Clone, Debug, PartialEq)]
pub struct BlendOpacity(pub Vec<Option<Vec<f32>>>);

impl Component for BlendOpacity {
    const NAME: &'static str = "reskate.blend_opacity";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerVertex;
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.0, from).map(BlendOpacity))
    }
    /// The opacities follow the primitive's compacted or reordered vertices; a primitive that isn't blended stays.
    fn remap_vertices(&self, primitive: usize, from: &[u32]) -> Option<Result<Self, String>> {
        let Some(entry) = self.0.get(primitive) else {
            return Some(Err(format!("primitive {primitive} of {}", self.0.len())));
        };
        let Some(opacity) = entry else {
            return Some(Ok(self.clone()));
        };
        let gathered: Result<Vec<f32>, String> = from
            .iter()
            .map(|&i| {
                usize::try_from(i)
                    .ok()
                    .and_then(|i| opacity.get(i).copied())
                    .ok_or_else(|| format!("vertex {i} of {}", opacity.len()))
            })
            .collect();
        Some(gathered.map(|o| {
            let mut out = self.clone();
            out.0[primitive] = Some(o);
            out
        }))
    }
    /// Opacity is linear over a triangle, like the colour alpha from which an overlay was lowered.
    fn blend_vertices(
        &self,
        primitive: usize,
        from: &[primase_core::VertexBlend],
    ) -> Option<Result<Self, String>> {
        Some((|| {
            let entry = self
                .0
                .get(primitive)
                .ok_or_else(|| format!("primitive {primitive} of {}", self.0.len()))?;
            let Some(opacity) = entry else {
                return Ok(self.clone());
            };
            let values = from
                .iter()
                .map(|b| {
                    let mut sum = 0.0f64;
                    for (v, weight) in b.terms() {
                        let value = opacity
                            .get(v as usize)
                            .ok_or_else(|| format!("vertex {v} of {}", opacity.len()))?;
                        sum += f64::from(*value) * f64::from(weight);
                    }
                    Ok((sum as f32).clamp(0.0, 1.0))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let mut out = self.clone();
            out.0[primitive] = Some(values);
            Ok(out)
        })())
    }
    fn verify(&self, e: Entity, w: &World) -> Result<(), String> {
        let mesh = w.get::<Mesh>(e).ok_or("blend opacity without core.mesh")?;
        if self.0.len() != mesh.primitives().len() {
            return Err(format!(
                "{} opacity lists for {} primitives",
                self.0.len(),
                mesh.primitives().len()
            ));
        }
        for (i, (o, p)) in self.0.iter().zip(mesh.primitives()).enumerate() {
            if let Some(o) = o {
                if o.len() != p.vertex_count() {
                    return Err(format!(
                        "primitive {i}: {} opacities for {} vertices",
                        o.len(),
                        p.vertex_count()
                    ));
                }
                if !o.iter().all(|x| x.is_finite() && (0.0..=1.0).contains(x)) {
                    return Err(format!("primitive {i}: opacity outside 0..1"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-vertex opacity follows a compaction of its primitive's vertices; a primitive that isn't blended stays, and a
    /// vertex the list lacks is an error.
    #[test]
    fn opacity_follows_compacted_vertices() {
        let o = BlendOpacity(vec![None, Some(vec![0.0, 0.5, 1.0])]);
        let r = o.remap_vertices(1, &[2, 0]).unwrap().unwrap();
        assert_eq!(r.0, [None, Some(vec![1.0, 0.0])]);
        assert_eq!(o.remap_vertices(0, &[0]).unwrap().unwrap(), o);
        assert!(o.remap_vertices(1, &[3]).unwrap().is_err());
        assert!(o.remap_vertices(2, &[0]).unwrap().is_err());
    }
}
