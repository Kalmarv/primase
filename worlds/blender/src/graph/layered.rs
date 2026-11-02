//! A `core.layered` blend as a Blender material: each layer's surface ([`pbr::surface`]) sampled with the layer's UV
//! set (`TexCoord(0)` where it names none) and its transform before each texture's own, weighted per pixel and
//! summed, then drawn with the host `pbr.material`'s alpha mode, sidedness and shading ([`pbr::finish`]).
//!
//! - **Sources**: a vertex source is its interpolated attribute: `COLOR_<n>` (the save stage's float copy, RGBA), or
//!   a UV set as `(u, v, 0, 1)` in core's frame (Blender's `v` flipped back). A mask source is its texture sampled as
//!   stored values (`Non-Color`) with its own UV set, wrap, transform and filters, `dot(rgba, channels) + bias`.
//! - **Channels**: every channel, named form or expression, as the operations of its expression in their order
//!   (`Channel::expression`): add, subtract, multiply, min and max as Math nodes, saturate as a Clamp to 0 to 1;
//!   without channels, the attribute's own four.
//! - **Mapping and composition**: identity or `rgb_product_4`; sum, over (`w_i prod_{j>i}(1 - w_j)`, slot 0 at full
//!   weight) and height (`u_i / sum u`, Cycles' division giving 0 where the sum is 0: the blend has no weight there
//!   and draws black, this writer's statement of core's open case).
//! - **What is blended**: every quantity of the surface, each summed with the weights as they are (not normalized; a
//!   null slot contributes zero): base colour and alpha, metallic, roughness, emission (colour times strength),
//!   specular level and tint. The normal is each layer's shading normal (its normal map's, else the surface's own),
//!   weighted, summed and normalized; where the sum is zero (it has no direction) it is the surface's own normal.
//! - **Vertex colour**: a layer takes `COLOR_0` as its own material states ([`VertexColor`]), except where the
//!   blend's weights read `COLOR_0` (the stream is the weights there, not a colour).
//!
//! A blend is drawn only where all it reads reaches Blender ([`Blocker`]): every layer a `pbr.material`, every vertex
//! stream and UV set it reads in the primitive as `scene.glb` carries it, and every mask texture sampled. Elsewhere the
//! host is drawn alone, as core states for a target that does not draw the blend, counted by the reason.

use primase_core::layered::{Channel, Composition, Source, Term};
use primase_core::pbr::PbrMaterial;
use primase_core::{Entity, LayeredMaterial, Semantic, VertexColor, WeightMapping, World};
use serde_json::json;

use super::pbr::{self, Surface};
use super::texture::{Coordinates, Reading, Sampler};
use super::{Float, Graph, Settings, Socket, Vec3};
use crate::{Error, Result};

/// Why a blend is not drawn for a primitive: its host is drawn instead.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Blocker {
    /// A layer's material has no `pbr.material`.
    LayerWithoutPbr,
    /// The weights read a vertex stream the primitive in `scene.glb` does not carry (its glTF name).
    StreamNotCarried(String),
    /// The weights read a stream glTF has no attribute for.
    StreamWithoutForm(Semantic),
    /// A mask or height texture is not sampled (its reason is counted with the slots).
    MaskNotSampled,
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocker::LayerWithoutPbr => write!(f, "a layer without pbr.material"),
            Blocker::StreamNotCarried(name) => write!(f, "reads {name}, which scene.glb does not carry"),
            Blocker::StreamWithoutForm(s) => write!(f, "reads {s:?}, which glTF has no attribute for"),
            Blocker::MaskNotSampled => write!(f, "a mask texture not sampled"),
        }
    }
}

/// The glTF attribute a vertex source of `semantic` reads, where glTF has one.
fn attribute_name(semantic: Semantic) -> Option<String> {
    match semantic {
        Semantic::Color(n) => Some(format!("COLOR_{n}")),
        Semantic::TexCoord(n) => Some(format!("TEXCOORD_{n}")),
        _ => None,
    }
}

/// What keeps `l` from being drawn on a primitive carrying `attributes` (`scene.glb`'s names), if anything.
pub fn blocker(
    world: &World,
    sampler: &Sampler,
    l: &LayeredMaterial,
    attributes: &std::collections::BTreeSet<String>,
) -> Result<Option<Blocker>> {
    let mut sets = Vec::new();
    for layer in l.layers.iter().flatten() {
        let Some(m) = world.get::<PbrMaterial>(layer.material) else {
            return Ok(Some(Blocker::LayerWithoutPbr));
        };
        if slots(m).next().is_some() {
            sets.push(Semantic::TexCoord(layer.texcoord.unwrap_or(0)));
        }
    }
    let channels = l.weights.channels.iter().flatten().flat_map(Channel::sources);
    let mut sources: Vec<&Source> = channels.chain(&l.weights.heights).collect();
    let attribute = Source::Vertex {
        semantic: l.weights.attribute,
        channel: 0,
    };
    if l.weights.channels.is_none() {
        sources.push(&attribute);
    }
    for s in sources {
        match s {
            Source::Vertex { semantic, .. } => sets.push(*semantic),
            Source::Mask { texture, .. } => {
                sets.push(Semantic::TexCoord(texture.texcoord));
                if sampler.unsampled(texture.texture)?.is_some() {
                    return Ok(Some(Blocker::MaskNotSampled));
                }
            }
        }
    }
    for semantic in sets {
        match attribute_name(semantic) {
            None => return Ok(Some(Blocker::StreamWithoutForm(semantic))),
            Some(name) if !attributes.contains(&name) => return Ok(Some(Blocker::StreamNotCarried(name))),
            Some(_) => {}
        }
    }
    Ok(None)
}

/// The texture slots of a `pbr.material`.
fn slots(m: &PbrMaterial) -> impl Iterator<Item = &primase_core::pbr::TextureRef> {
    let specular = m.specular.iter().flat_map(|s| [&s.texture, &s.color_texture]);
    [
        &m.base_color_texture,
        &m.metallic_roughness_texture,
        &m.normal_texture,
        &m.occlusion_texture,
        &m.emissive_texture,
    ]
    .into_iter()
    .chain(specular)
    .flatten()
}

/// A source's value per pixel.
fn source(s: &mut Sampler, g: &mut Graph, src: &Source) -> Result<Float> {
    Ok(match src {
        Source::Vertex { semantic, channel } => vertex(g, *semantic)[usize::from(*channel)],
        Source::Mask {
            texture,
            channels,
            bias,
        } => {
            let x = s
                .sample(g, texture, Reading::Data, Coordinates::default())?
                .expect("the blend's masks are sampleable (blocker)");
            let [r, gr, b] = g.separate(x.color);
            let mut v = Float::Const(*bias);
            for (c, k) in [r, gr, b, x.alpha].into_iter().zip(channels) {
                let term = g.mul(c, Float::Const(*k));
                v = g.add(v, term);
            }
            v
        }
    })
}

/// A vertex stream's four channels as core reads them.
fn vertex(g: &mut Graph, semantic: Semantic) -> [Float; 4] {
    match semantic {
        Semantic::TexCoord(n) => {
            let uv = g.node("ShaderNodeUVMap", &[("uv_map", json!(Sampler::uv_map(n)))]);
            let [u, v, _] = g.separate(Vec3::Out(Socket {
                node: uv,
                output: "UV",
            }));
            [
                u,
                g.sub(Float::Const(1.0), v),
                Float::Const(0.0),
                Float::Const(1.0),
            ]
        }
        _ => {
            let name = attribute_name(semantic).expect("a carried stream (blocker)");
            let (c, a) = g.attribute(&name);
            let [r, gr, b] = g.separate(c);
            [r, gr, b, a]
        }
    }
}

/// A channel's value per pixel: the operations of its expression ([`Channel::expression`]), so a named form is drawn
/// as core evaluates it and an expression alike.
fn channel(s: &mut Sampler, g: &mut Graph, entity: Entity, c: &Channel) -> Result<Float> {
    let e = c.expression().map_err(|e| Error::World {
        entity,
        detail: format!("core.layered weight channel: {e}"),
    })?;
    let v: Vec<Float> = e
        .sources
        .iter()
        .map(|src| source(s, g, src))
        .collect::<Result<_>>()?;
    let mut t: Vec<Float> = Vec::with_capacity(e.terms.len());
    for term in &e.terms {
        let at = |k: &u8| t[usize::from(*k)];
        let x = match term {
            Term::Constant(x) => Float::Const(*x),
            Term::Source(i) => v[usize::from(*i)],
            Term::Add([a, b]) => g.add(at(a), at(b)),
            Term::Subtract([a, b]) => g.sub(at(a), at(b)),
            Term::Multiply([a, b]) => g.mul(at(a), at(b)),
            Term::Min([a, b]) => g.math("MINIMUM", &[at(a), at(b)]),
            Term::Max([a, b]) => g.math("MAXIMUM", &[at(a), at(b)]),
            Term::Saturate(a) => g.clamp01(at(a)),
        };
        t.push(x);
    }
    Ok(*t.last().expect("a canonical expression has terms"))
}

/// The slot weights per pixel, one per slot.
fn weights(s: &mut Sampler, g: &mut Graph, entity: Entity, l: &LayeredMaterial) -> Result<Vec<Float>> {
    let slots = l.layers.len();
    let inputs = l.weights.mapping.inputs(slots);
    let raw: Vec<Float> = match &l.weights.channels {
        Some(channels) => channels
            .iter()
            .map(|c| channel(s, g, entity, c))
            .collect::<Result<_>>()?,
        None => vertex(g, l.weights.attribute)[..inputs.min(4)].to_vec(),
    };
    let one = Float::Const(1.0);
    let mut w: Vec<Float> = match l.weights.mapping {
        WeightMapping::Identity => raw,
        WeightMapping::RgbProduct4 => {
            let [r, gr, b] = [raw[0], raw[1], raw[2]];
            let (nr, ng, nb) = (g.sub(one, r), g.sub(one, gr), g.sub(one, b));
            let (rg, rng) = (g.mul(nr, gr), g.mul(nr, ng));
            vec![g.mul(rng, nb), g.mul(rng, b), g.mul(rg, nb), g.mul(rg, b)]
        }
    };
    match l.weights.composition {
        Composition::Sum => {}
        Composition::Over => {
            let mut out = Vec::with_capacity(slots);
            for i in 0..slots {
                let mut x = if i == 0 { one } else { w[i] };
                for &later in &w[i + 1..slots] {
                    let keep = g.sub(one, later);
                    x = g.mul(x, keep);
                }
                out.push(x);
            }
            w = out;
        }
        Composition::Height { contrast } => {
            let mut p = Vec::with_capacity(slots);
            for (i, h) in l.weights.heights.iter().enumerate() {
                let h = source(s, g, h)?;
                p.push(g.mul(w[i], h));
            }
            let m = p[1..].iter().fold(p[0], |m, x| g.math("MAXIMUM", &[m, *x]));
            let floor = g.sub(m, Float::Const(1.0 - contrast));
            let u: Vec<Float> = p
                .iter()
                .map(|x| {
                    let d = g.sub(*x, floor);
                    g.math("MAXIMUM", &[d, Float::Const(0.0)])
                })
                .collect();
            let sum = u[1..].iter().fold(u[0], |a, x| g.add(a, *x));
            // Cycles' division is 0 by 0: no weight where every u_i is 0
            w = u.iter().map(|x| g.math("DIVIDE", &[*x, sum])).collect();
        }
    }
    w.truncate(slots);
    Ok(w)
}

/// The graph of layered material `l` (entity `entity`) with host `host`, on a primitive that carries `COLOR_0` where
/// `color0`.
pub fn graph(
    s: &mut Sampler,
    world: &World,
    entity: Entity,
    l: &LayeredMaterial,
    host: &PbrMaterial,
    color0: bool,
) -> Result<(Graph, Settings)> {
    let mut g = Graph::new();
    let w = weights(s, &mut g, entity, l)?;
    let weights_read_color0 = l.reads(Semantic::Color(0));
    let mut sum = Blend::default();
    let normal = Vec3::Out(g.input_node("ShaderNodeNewGeometry", "Normal"));
    for (layer, w) in l.layers.iter().zip(w) {
        let Some(layer) = layer else { continue };
        let m = world
            .get::<PbrMaterial>(layer.material)
            .expect("every layer has pbr.material (blocker)");
        let takes = color0 && !weights_read_color0 && !world.has::<VertexColor>(layer.material);
        let at = Coordinates {
            texcoord: Some(layer.texcoord.unwrap_or(0)),
            before: layer.transform,
            scroll: None,
        };
        let surface = pbr::surface(s, &mut g, m, at, takes)?;
        sum.add(&mut g, &surface, w, normal);
    }
    let surface = sum.surface(&mut g, normal);
    let settings = pbr::finish(&mut g, &surface, host);
    Ok((g, settings))
}

/// The weighted sums of the layers' surfaces.
#[derive(Default)]
struct Blend {
    base: Option<Vec3>,
    alpha: Option<Float>,
    metallic: Option<Float>,
    roughness: Option<Float>,
    normal: Option<Vec3>,
    emission: Option<Vec3>,
    specular_level: Option<Float>,
    specular_tint: Option<Vec3>,
}

fn add_float(g: &mut Graph, sum: &mut Option<Float>, x: Float, w: Float) {
    let t = g.mul(x, w);
    *sum = Some(match *sum {
        None => t,
        Some(a) => g.add(a, t),
    });
}

fn add_vec(g: &mut Graph, sum: &mut Option<Vec3>, x: Vec3, w: Float) {
    let t = g.vscale(x, w);
    *sum = Some(match *sum {
        None => t,
        Some(a) => g.vadd(a, t),
    });
}

impl Blend {
    fn add(&mut self, g: &mut Graph, s: &Surface, w: Float, own_normal: Vec3) {
        add_vec(g, &mut self.base, s.base, w);
        add_float(g, &mut self.alpha, s.alpha, w);
        add_float(g, &mut self.metallic, s.metallic, w);
        add_float(g, &mut self.roughness, s.roughness, w);
        let n = s.normal.map_or(own_normal, Vec3::Out);
        add_vec(g, &mut self.normal, n, w);
        let e = g.vscale(s.emission, s.emission_strength);
        add_vec(g, &mut self.emission, e, w);
        add_float(g, &mut self.specular_level, s.specular_level, w);
        add_vec(g, &mut self.specular_tint, s.specular_tint, w);
    }

    /// The blended surface; `own_normal` the surface's own shading normal.
    fn surface(self, g: &mut Graph, own_normal: Vec3) -> Surface {
        let zero = Float::Const(0.0);
        let normal = self.normal.map(|n| {
            // a zero sum has no direction: the surface's own normal there
            let length = g.vlength(n);
            let none = g.math("COMPARE", &[length, zero, zero]);
            let fill = g.vscale(own_normal, none);
            let n = g.vadd(n, fill);
            g.vector_math("NORMALIZE", &[n])
        });
        Surface {
            base: self.base.unwrap_or(Vec3::Const([0.0; 3])),
            alpha: self.alpha.unwrap_or(zero),
            metallic: self.metallic.unwrap_or(zero),
            roughness: self.roughness.unwrap_or(zero),
            normal: normal.map(|n| match n {
                Vec3::Out(s) => s,
                Vec3::Const(_) => unreachable!("a normalized sum of node outputs is a node"),
            }),
            emission: self.emission.unwrap_or(Vec3::Const([0.0; 3])),
            emission_strength: Float::Const(1.0),
            specular_level: self.specular_level.unwrap_or(zero),
            specular_tint: self.specular_tint.unwrap_or(Vec3::Const([0.0; 3])),
        }
    }
}
