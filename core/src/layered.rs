//! A material blend evaluated per pixel from weight channels.
//!
//! Each channel is an interpolated vertex channel, or, with [`LayerWeights::channels`], a per-pixel expression of
//! vertex channels and mask textures ([`Channel`]). The mapping turns the channels into slot weights, and the
//! composition combines the layers: [`Composition::Sum`] is `sum(weight[i] * layer[i])` with the weights not
//! normalized (unused weight and null layers contribute black); [`Composition::Over`] starts from slot 0 and lerps
//! each later slot over it by its weight, in slot order, which equals Sum with weight `w_i * prod_{j>i}(1 - w_j)` per
//! pixel. Blend normals with the same weights and then normalize the resulting direction. An all-zero normal sum has
//! no direction; a consumer must state its fallback rather than inventing a source normal.
use crate::{Component, Entity, JsonError, Semantic, World, pbr::TextureRef, pbr::UvTransform};
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

impl WeightMapping {
    /// The channels it maps for `slots` slots: one per slot, or three for [`WeightMapping::RgbProduct4`].
    pub fn inputs(self, slots: usize) -> usize {
        match self {
            Self::Identity => slots,
            Self::RgbProduct4 => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayerWeights {
    /// A VEC3/VEC4 float or normalized unsigned vertex channel stream.
    /// VEC3 supplies an implicit fourth channel of one, as for glTF colours.
    /// Consumers validate the stream type on each primitive before evaluating it.
    /// With [`LayerWeights::channels`] it is unused.
    pub attribute: Semantic,
    pub mapping: WeightMapping,
    /// Per-pixel channels, one per mapping input ([`WeightMapping::inputs`]); `None`: the attribute's channels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<Channel>>,
    pub composition: Composition,
}

impl LayerWeights {
    /// Weights from a vertex attribute's channels, summed (the original form).
    pub fn new(attribute: Semantic, mapping: WeightMapping) -> Self {
        LayerWeights {
            attribute,
            mapping,
            channels: None,
            composition: Composition::Sum,
        }
    }
    /// Whether a consumer must evaluate anything per pixel beyond interpolating one vertex attribute and summing.
    pub fn per_pixel(&self) -> bool {
        self.channels.is_some() || self.composition != Composition::Sum
    }
}

/// How the weighted layers combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Composition {
    /// `sum(weight[i] * layer[i])`.
    Sum,
    /// Slot 0, then `lerp(colour, layer[i], weight[i])` for each later slot in order.
    Over,
}

/// A value read per pixel.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Channel `channel` (0..=3) of an interpolated vertex stream.
    Vertex { semantic: Semantic, channel: u8 },
    /// `dot(sample.rgba, channels) + bias` of a texture sampled with its own UV set, wrap and transform.
    Mask {
        texture: TextureRef,
        channels: [f32; 4],
        bias: f32,
    },
}

/// One weight channel per pixel: a source, or one of two combinations of sources. Bounded by its type (no nesting).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Source(Source),
    /// `lerp(a, b, t)`.
    Lerp {
        a: Source,
        b: Source,
        t: Source,
    },
    /// `saturate(k0 + k1 v + k2 m + k3 v m)`.
    Threshold {
        v: Source,
        m: Source,
        k: [f32; 4],
    },
}

impl Channel {
    /// The sources it reads.
    pub fn sources(&self) -> Vec<&Source> {
        match self {
            Channel::Source(s) => vec![s],
            Channel::Lerp { a, b, t } => vec![a, b, t],
            Channel::Threshold { v, m, .. } => vec![v, m],
        }
    }
    /// The channel's value from its sources' values (in [`Channel::sources`] order).
    pub fn evaluate(&self, values: &[f32]) -> f32 {
        match self {
            Channel::Source(_) => values[0],
            Channel::Lerp { .. } => values[0] + (values[1] - values[0]) * values[2],
            Channel::Threshold { k, .. } => {
                let (v, m) = (values[0], values[1]);
                (k[0] + k[1] * v + k[2] * m + k[3] * v * m).clamp(0.0, 1.0)
            }
        }
    }
}

/// The value of a mask texture's sample `rgba` (0..1 per channel).
pub fn mask_value(channels: [f32; 4], bias: f32, rgba: [f32; 4]) -> f32 {
    (0..4).map(|i| channels[i] * rgba[i]).sum::<f32>() + bias
}

/// [`Composition::Over`]'s per-pixel weights as Sum weights: `w_i * prod_{j>i}(1 - w_j)`, slot 0 at full weight.
pub fn over_as_sum(weights: [f32; 4], slots: usize) -> [f32; 4] {
    let mut out = [0.0; 4];
    for i in 0..slots {
        let own = if i == 0 { 1.0 } else { weights[i] };
        out[i] = own * (i + 1..slots).map(|j| 1.0 - weights[j]).product::<f32>();
    }
    out
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
impl LayeredMaterial {
    /// Whether its weights read vertex stream `semantic`: the attribute, or with per-pixel channels a vertex source.
    pub fn reads(&self, semantic: Semantic) -> bool {
        match &self.weights.channels {
            None => self.weights.attribute == semantic,
            Some(channels) => channels
                .iter()
                .flat_map(Channel::sources)
                .any(|s| matches!(s, Source::Vertex { semantic: v, .. } if *v == semantic)),
        }
    }
    /// The mask textures its channels sample.
    pub fn mask_textures(&self) -> impl Iterator<Item = &TextureRef> {
        self.weights
            .channels
            .iter()
            .flatten()
            .flat_map(Channel::sources)
            .filter_map(|s| match s {
                Source::Mask { texture, .. } => Some(texture),
                Source::Vertex { .. } => None,
            })
    }
}

impl Component for LayeredMaterial {
    const NAME: &'static str = "core.layered";
    /// 2: per-pixel `channels` and the `composition`.
    const VERSION: u32 = 2;
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.layers.iter().flatten().map(|layer| layer.material));
        out.extend(self.mask_textures().map(|t| t.texture));
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
        if let Some(channels) = &self.weights.channels {
            let want = self.weights.mapping.inputs(self.layers.len());
            if channels.len() != want {
                return Err(format!("{} channels for a mapping of {want}", channels.len()));
            }
            for (i, c) in channels.iter().enumerate() {
                if let Channel::Threshold { k, .. } = c
                    && !k.iter().all(|x| x.is_finite())
                {
                    return Err(format!("channel {i}: threshold {k:?} is not finite"));
                }
                for s in c.sources() {
                    match s {
                        Source::Vertex { channel, .. } if *channel > 3 => {
                            return Err(format!("channel {i}: vertex channel {channel} past 3"));
                        }
                        Source::Vertex { .. } => {}
                        Source::Mask {
                            texture,
                            channels,
                            bias,
                        } => {
                            if !world.is_alive(texture.texture) {
                                return Err(format!(
                                    "channel {i}: mask texture {:?} is not alive",
                                    texture.texture
                                ));
                            }
                            if !channels.iter().chain([bias]).all(|x| x.is_finite()) {
                                return Err(format!(
                                    "channel {i}: mask weights {channels:?} + {bias} not finite"
                                ));
                            }
                        }
                    }
                }
            }
        }
        for (i, layer) in self.layers.iter().enumerate() {
            // that the material is alive, the world checks for every reference
            let Some(layer) = layer else { continue };
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
