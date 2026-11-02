//! A material blend evaluated per pixel from weight channels.
//!
//! Each channel is an interpolated vertex channel, or, with [`LayerWeights::channels`], a per-pixel expression of
//! vertex channels and mask textures ([`Channel`]). The mapping turns the channels into slot weights, and the
//! composition combines the layers: [`Composition::Sum`] is `sum(weight[i] * layer[i])` with the weights not
//! normalized (unused weight and null layers contribute black); [`Composition::Over`] starts from slot 0 and lerps
//! each later slot over it by its weight, in slot order, which equals Sum with weight `w_i * prod_{j>i}(1 - w_j)` per
//! pixel; [`Composition::Height`] blends by each layer's height ([`LayerWeights::heights`], [`height_blend`]): the
//! layers whose weighted height is near the highest show, the rest fade out within the contrast. Blend normals with the same weights and then normalize the resulting direction. An all-zero normal sum has
//! no direction; a consumer must state its fallback rather than inventing a source normal.
//!
//! A layer's colour is the one its material gives, or, with [`Layer::color`], an [`Expression`] of that colour's lanes
//! per lane (a colour moved part of the way to a constant, or scaled past one). With [`LayerWeights::saturate`] the
//! blended base colour is [`saturate`]d per lane.
use crate::{Component, Entity, JsonError, Semantic, World, pbr::TextureRef, pbr::UvTransform};
use serde::Serialize;
use serde_json::Value;

mod expression;
pub use expression::{Expression, ExpressionBuilder, TERM_LIMIT, Term, TermId, lerp, saturate};

/// One ordinary material sampled with this layer's UV set and transform.
/// Its colour, normal, mask, tint and alpha participate in the blend.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Layer {
    pub material: Entity,
    /// `None` samples TexCoord(0); retained separately from an explicit zero.
    pub texcoord: Option<u8>,
    /// Applied to the selected UVs before sampling every layer texture.
    pub transform: Option<UvTransform>,
    /// The colour the layer blends: red, green and blue, each an [`Expression`] of the lanes of the colour its material
    /// gives ([`ColorLane`]), under the expressions' numeric rules; `None` blends that colour as it is. Its alpha is the
    /// material's. A colour that is the material's own, or whose every lane is its own lane times a constant where the
    /// material's base colour factor is one (the material's factor, which says it), is stated without it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[Expression<ColorLane>; 3]>,
}

impl Layer {
    /// The layer of `material`, sampled with `texcoord` and `transform`, its colour the material's.
    pub fn new(material: Entity, texcoord: Option<u8>, transform: Option<UvTransform>) -> Layer {
        Layer {
            material,
            texcoord,
            transform,
            color: None,
        }
    }
}

/// A lane of the colour a layer's material gives at the pixel (`pbr.material`'s base colour texture times its factor,
/// times the vertex colour where the material takes it): 0 to 2 red, green and blue, 3 alpha. The source a layer's
/// [`Layer::color`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ColorLane(pub u8);

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
    /// With [`Composition::Height`], each slot's height per pixel (a layer's own height map, sampled with its own UV
    /// set and transform), one per slot; empty otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub heights: Vec<Source>,
    /// The blended base colour is [`saturate`]d, each lane (NaN gives 0): a blend of colours past one, or of weights
    /// past 0 to 1, held to 0 to 1 as the blend's program holds it. The other blended quantities are as they are.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub saturate: bool,
}

impl LayerWeights {
    /// Weights from a vertex attribute's channels, summed (the original form).
    pub fn new(attribute: Semantic, mapping: WeightMapping) -> Self {
        LayerWeights {
            attribute,
            mapping,
            channels: None,
            composition: Composition::Sum,
            heights: Vec::new(),
            saturate: false,
        }
    }
    /// Whether the slot weights are evaluated per pixel: per-pixel channels, or a composition other than the sum (what
    /// a bake of the weights to a vertex stream evaluates at the vertices).
    pub fn per_pixel_weights(&self) -> bool {
        self.channels.is_some() || self.composition != Composition::Sum
    }
    /// Whether a consumer must evaluate anything per pixel beyond interpolating one vertex attribute and summing: the
    /// weights ([`LayerWeights::per_pixel_weights`]), or a saturated blend. [`LayeredMaterial::per_pixel`] also counts
    /// the layers' colours.
    pub fn per_pixel(&self) -> bool {
        self.per_pixel_weights() || self.saturate
    }
}

/// How the weighted layers combine.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Composition {
    /// `sum(weight[i] * layer[i])`.
    Sum,
    /// Slot 0, then `lerp(colour, layer[i], weight[i])` for each later slot in order.
    Over,
    /// Each slot's weight `w_i` times its height `h_i` ([`LayerWeights::heights`]) is `p_i`, the highest `m`; slot `i`
    /// gets `u_i = max(p_i - (m - (1 - contrast)), 0)`, normalized by their sum ([`height_blend`]). Where every `u_i`
    /// is 0 (a contrast of 1 or more) the blend has no weight: a consumer states its fallback.
    Height { contrast: f32 },
}

/// [`Composition::Height`]'s weights of `slots` slots from their mapped weights and heights; all 0 where every
/// `u_i` is (see there).
pub fn height_blend(weights: [f32; 4], heights: [f32; 4], slots: usize, contrast: f32) -> [f32; 4] {
    let slots = slots.min(4);
    let p: Vec<f32> = (0..slots).map(|i| weights[i] * heights[i]).collect();
    let m = p.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let u: Vec<f32> = p.iter().map(|&x| (x - (m - (1.0 - contrast))).max(0.0)).collect();
    let sum: f32 = u.iter().sum();
    let mut out = [0.0; 4];
    if sum > 0.0 {
        for (o, x) in out.iter_mut().zip(&u) {
            *o = x / sum;
        }
    }
    out
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

/// One weight channel per pixel: a named form of its sources, or an [`Expression`] of them for what no named form
/// spells. Every form follows the [numeric rules](expression) and is the operations of its [`Channel::expression`].
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
    /// `saturate(k0 + k1 v + k2 m + k3 v m)` ([`saturate`]), summed from the left with `k3 v` before its `m`.
    Threshold {
        v: Source,
        m: Source,
        k: [f32; 4],
    },
    /// Product of 2 to [`PRODUCT_LIMIT`] sources, multiplied from left to right without clamping.
    Product {
        sources: Vec<Source>,
    },
    /// `v^exponent` ([`power`]), `exponent` 1 to 16.
    Power {
        v: Source,
        exponent: u8,
    },
    /// Operations on its sources, in its canonical spelling and no named form's ([`Channel::from_expression`]).
    Expression(Expression),
}

/// Maximum number of factors in [`Channel::Product`]; products require at least two.
pub const PRODUCT_LIMIT: usize = 4;

/// The largest exponent of [`Channel::Power`].
pub const POWER_LIMIT: u8 = 16;

/// `v^exponent` in f32 in a stated order, so a program computing it alike gives the same bits: by squaring where
/// `exponent` is a power of two (`v^8` is three squarings, as a program's `(1 - a)^8` is written), otherwise by
/// multiplying `v` in from the left (`((v * v) * v) * ...`).
pub fn power(v: f32, exponent: u8) -> f32 {
    if exponent.is_power_of_two() {
        let mut x = v;
        for _ in 0..exponent.trailing_zeros() {
            x *= x;
        }
        x
    } else {
        (1..exponent).fold(v, |x, _| x * v)
    }
}

impl Channel {
    /// The sources it reads.
    pub fn sources(&self) -> Vec<&Source> {
        match self {
            Channel::Source(s) => vec![s],
            Channel::Lerp { a, b, t } => vec![a, b, t],
            Channel::Threshold { v, m, .. } => vec![v, m],
            Channel::Power { v, .. } => vec![v],
            Channel::Product { sources } => sources.iter().collect(),
            Channel::Expression(e) => e.sources.iter().collect(),
        }
    }
    /// The channel's value from its sources' values (in [`Channel::sources`] order).
    pub fn evaluate(&self, values: &[f32]) -> f32 {
        match self {
            Channel::Source(_) => values[0],
            Channel::Lerp { .. } => lerp(values[0], values[1], values[2]),
            Channel::Threshold { k, .. } => {
                let (v, m) = (values[0], values[1]);
                saturate(k[0] + k[1] * v + k[2] * m + k[3] * v * m)
            }
            Channel::Power { exponent, .. } => power(values[0], *exponent),
            Channel::Product { sources } => values[..sources.len()].iter().fold(1., |p, v| p * v),
            Channel::Expression(e) => e.evaluate(values),
        }
    }

    /// Its own checks (its sources' apart): finite threshold factors, a bounded power exponent or product arity, an
    /// expression in its canonical spelling and no named form.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Channel::Product { sources } if !(2..=PRODUCT_LIMIT).contains(&sources.len()) => Err(format!(
                "product has {} sources, requires 2 to {PRODUCT_LIMIT}",
                sources.len()
            )),
            Channel::Threshold { k, .. } if !k.iter().all(|x| x.is_finite()) => {
                Err(format!("threshold {k:?} is not finite"))
            }
            Channel::Power { exponent, .. } if !(1..=POWER_LIMIT).contains(exponent) => {
                Err(format!("power exponent {exponent} is not 1 to {POWER_LIMIT}"))
            }
            Channel::Expression(e) => {
                e.check()?;
                match Channel::from_expression(e)? {
                    Channel::Expression(_) => Ok(()),
                    named @ (Channel::Source(_)
                    | Channel::Lerp { .. }
                    | Channel::Threshold { .. }
                    | Channel::Product { .. }
                    | Channel::Power { .. }) => Err(format!("expression is a named form: {named:?}")),
                }
            }
            Channel::Source(_)
            | Channel::Lerp { .. }
            | Channel::Threshold { .. }
            | Channel::Product { .. }
            | Channel::Power { .. } => Ok(()),
        }
    }
}

/// A weight or height source's own checks: a vertex channel within 0..=3, a mask texture alive with finite weights.
pub(crate) fn check_source(world: &World, what: &str, s: &Source) -> Result<(), String> {
    match s {
        Source::Vertex { channel, .. } if *channel > 3 => {
            Err(format!("{what}: vertex channel {channel} past 3"))
        }
        Source::Vertex { .. } => Ok(()),
        Source::Mask {
            texture,
            channels,
            bias,
        } => {
            if !world.is_alive(texture.texture) {
                return Err(format!("{what}: mask texture {:?} is not alive", texture.texture));
            }
            if !channels.iter().chain([bias]).all(|x| x.is_finite()) {
                return Err(format!("{what}: mask weights {channels:?} + {bias} not finite"));
            }
            Ok(())
        }
    }
}

/// The value of a mask texture's sample `rgba` (0..1 per channel).
pub fn mask_value(channels: [f32; 4], bias: f32, rgba: [f32; 4]) -> f32 {
    (0..4).map(|i| channels[i] * rgba[i]).sum::<f32>() + bias
}

/// [`Composition::Over`]'s per-pixel weights as Sum weights: `w_i * prod_{j>i}(1 - w_j)`, slot 0 at full weight, over
/// the first `slots` slots: at most the four a layered material has (more are the four).
pub fn over_as_sum(weights: [f32; 4], slots: usize) -> [f32; 4] {
    let slots = slots.min(weights.len());
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
///
/// A `pbr.material` on the same entity is its host, as for `pbr.detail`: it states the material's alpha mode and
/// sidedness, which the blend does not, and it is the complete material for every target that does not draw the blend.
/// A consumer that draws the blend draws the layers with the host's alpha mode and sidedness. A layered material
/// without a host is not drawn with a default a writer picks: the writer leaves it out and reports it, and a program
/// that wants it drawn states a pass that gives it a `pbr.material`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LayeredMaterial {
    pub layers: Vec<Option<Layer>>,
    pub weights: LayerWeights,
}
impl LayeredMaterial {
    /// Whether a consumer must evaluate anything per pixel beyond interpolating one vertex attribute and summing the
    /// layers' colours as their materials give them: its weights' ([`LayerWeights::per_pixel`]), or a layer's colour
    /// ([`Layer::color`]).
    pub fn per_pixel(&self) -> bool {
        self.vertex_weight_blocker().is_some()
    }
    /// What of it a consumer that blends by vertex weights alone (one interpolated attribute, the layers' colours
    /// summed as their materials give them) cannot draw, named; `None` where it can draw all of it. A bake of the
    /// weights to a vertex stream (`bake-layer-channels`) removes the first two; a saturated blend and a layer's colour
    /// have no vertex-weight form.
    pub fn vertex_weight_blocker(&self) -> Option<&'static str> {
        if self.weights.channels.is_some() {
            Some("per-pixel weight channels")
        } else if self.weights.composition != Composition::Sum {
            Some("the over or height composition")
        } else if self.weights.saturate {
            Some("a saturated blend")
        } else if self.layers.iter().flatten().any(|l| l.color.is_some()) {
            Some("a layer's colour expression")
        } else {
            None
        }
    }
    /// Whether its weights read vertex stream `semantic`: the attribute, or with per-pixel channels a vertex source;
    /// or a height a vertex source.
    pub fn reads(&self, semantic: Semantic) -> bool {
        let reads = |s: &Source| matches!(s, Source::Vertex { semantic: v, .. } if *v == semantic);
        let weights = match &self.weights.channels {
            None => self.weights.attribute == semantic,
            Some(channels) => channels.iter().flat_map(Channel::sources).any(reads),
        };
        weights || self.weights.heights.iter().any(reads)
    }
    /// The mask textures its channels and heights sample.
    pub fn mask_textures(&self) -> impl Iterator<Item = &TextureRef> {
        self.weights
            .channels
            .iter()
            .flatten()
            .flat_map(Channel::sources)
            .chain(&self.weights.heights)
            .filter_map(|s| match s {
                Source::Mask { texture, .. } => Some(texture),
                Source::Vertex { .. } => None,
            })
    }
}

impl Component for LayeredMaterial {
    const NAME: &'static str = "core.layered";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// 2: per-pixel `channels` and the `composition`; 3: each mask texture's `filters`; 4: the height composition
    /// and its `heights`; 5: bounded product channels; 6: expression channels, and a threshold saturating NaN to 0;
    /// 7: a layer's `color` and the weights' `saturate`.
    const VERSION: u32 = 7;
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
                c.check().map_err(|e| format!("channel {i}: {e}"))?;
                for s in c.sources() {
                    check_source(world, &format!("channel {i}"), s)?;
                }
            }
        }
        match self.weights.composition {
            Composition::Height { contrast } => {
                if !contrast.is_finite() {
                    return Err(format!("height composition: contrast {contrast} is not finite"));
                }
                if self.weights.heights.len() != self.layers.len() {
                    return Err(format!(
                        "height composition: {} heights for {} slots",
                        self.weights.heights.len(),
                        self.layers.len()
                    ));
                }
                for (i, s) in self.weights.heights.iter().enumerate() {
                    check_source(world, &format!("height {i}"), s)?;
                }
            }
            Composition::Sum | Composition::Over if !self.weights.heights.is_empty() => {
                return Err("heights without the height composition".into());
            }
            Composition::Sum | Composition::Over => {}
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
            if let Some(color) = &layer.color {
                check_color(world, layer.material, color).map_err(|e| format!("layer {i} color: {e}"))?;
            }
        }
        Ok(())
    }
}

/// A layer's [`Layer::color`] of `material`'s colour: each lane canonical and reading lanes 0 to 3, and the colour
/// not one the layer states without it (the material's own, or its factor).
fn check_color(world: &World, material: Entity, color: &[Expression<ColorLane>; 3]) -> Result<(), String> {
    for (lane, e) in color.iter().enumerate() {
        e.check().map_err(|why| format!("lane {lane}: {why}"))?;
        if let Some(ColorLane(c)) = e.sources.iter().find(|ColorLane(c)| *c > 3) {
            return Err(format!("lane {lane} reads colour lane {c}, past 3"));
        }
    }
    // the lane's own lane, and its own lane times a constant
    let own = |lane: u8, e: &Expression<ColorLane>| {
        e.sources.as_slice() == [ColorLane(lane)] && e.terms.as_slice() == [Term::Source(0)]
    };
    let scaled = |lane: u8, e: &Expression<ColorLane>| {
        e.sources.as_slice() == [ColorLane(lane)]
            && matches!(
                e.terms.as_slice(),
                [Term::Constant(_), Term::Source(0), Term::Multiply(_)]
            )
    };
    let lanes = || (0u8..).zip(color);
    if lanes().all(|(lane, e)| own(lane, e)) {
        return Err("the material's own colour, which a layer states with no color".into());
    }
    let unit_factor = world
        .get::<crate::pbr::PbrMaterial>(material)
        .is_some_and(|m| m.base_color[..3] == [1.0; 3]);
    if unit_factor && lanes().all(|(lane, e)| own(lane, e) || scaled(lane, e)) {
        return Err("each lane its own times a constant: the material's base colour factor".into());
    }
    Ok(())
}
