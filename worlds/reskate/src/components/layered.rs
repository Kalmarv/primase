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
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(primase_core::remap_list(&self.0, from).map(BlendOpacity))
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
