//! A material blend evaluated from interpolated vertex channels.
//!
//! Interpolate the channels first, then evaluate the mapping per pixel. Weights
//! are not normalized: colour is `sum(weight[i] * layer[i])`; unused weight and
//! null layers contribute black. Blend normals with the same weights and then
//! normalize the resulting direction. An all-zero normal sum has no direction;
//! a consumer must state its fallback rather than inventing a source normal.
use crate::{Component, Entity, JsonError, Semantic, World, pbr::UvTransform};
use serde::Serialize;
use serde_json::Value;

/// One ordinary material sampled with this layer's UV set and transform.
/// Its colour, normal, mask, tint and alpha participate in the blend.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layer {
    pub material: Entity,
    /// `None` samples TexCoord(0); retained separately from an explicit zero.
    pub texcoord: Option<u8>,
    /// Applied to the selected UVs before sampling every layer texture.
    pub transform: Option<UvTransform>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum WeightMapping {
    /// The first N interpolated channels weight the N slots directly.
    Identity,
    /// Four slots: (1-r)(1-g)(1-b), (1-r)(1-g)b, (1-r)g(1-b), (1-r)gb.
    /// Their sum is 1-r, not one: red darkens the blend.
    RgbProduct4,
}
impl WeightMapping {
    /// Map already-interpolated channels without clamping or normalization.
    /// This is the material's pixel equation, not a strongest-layer approximation.
    pub fn evaluate(self, [r, g, b, a]: [f32; 4]) -> [f32; 4] {
        match self {
            Self::Identity => [r, g, b, a],
            Self::RgbProduct4 => [
                (1. - r) * (1. - g) * (1. - b),
                (1. - r) * (1. - g) * b,
                (1. - r) * g * (1. - b),
                (1. - r) * g * b,
            ],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayerWeights {
    /// A VEC3/VEC4 float or normalized unsigned vertex channel stream.
    /// VEC3 supplies an implicit fourth channel of one, as for glTF colours.
    /// Consumers validate the stream type on each primitive before evaluating it.
    pub attribute: Semantic,
    pub mapping: WeightMapping,
}

/// One to four ordered material slots. A null slot contributes black without
/// redistributing its weight. Referenced materials must not themselves be layered.
/// Consumers validate each referenced entity's material dialect; core checks
/// entity existence and disallows nested blends independently of dialect.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayeredMaterial {
    pub layers: Vec<Option<Layer>>,
    pub weights: LayerWeights,
}
impl Component for LayeredMaterial {
    const NAME: &'static str = "core.layered";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.layers.iter().flatten().map(|layer| layer.material));
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn verify(&self, entity: Entity, world: &World) -> Result<(), String> {
        if !(1..=4).contains(&self.layers.len()) {
            return Err("layered material requires one to four slots".into());
        }
        if self.weights.mapping == WeightMapping::RgbProduct4 && self.layers.len() != 4 {
            return Err("rgb_product_4 requires four slots".into());
        }
        for (i, layer) in self.layers.iter().enumerate() {
            let Some(layer) = layer else { continue };
            if !world.is_alive(layer.material) {
                return Err(format!(
                    "layer {i} references missing material {:?}",
                    layer.material
                ));
            }
            if layer.material == entity || world.get::<Self>(layer.material).is_some() {
                return Err(format!("layer {i} references a layered material"));
            }
            if let Some(t) = layer.transform
                && !t
                    .offset
                    .into_iter()
                    .chain(t.scale)
                    .chain([t.rotation])
                    .all(f32::is_finite)
            {
                return Err(format!("layer {i} has a nonfinite UV transform"));
            }
        }
        Ok(())
    }
}
