//! `bake-layer-channels`: per-pixel layer weights evaluated at the vertices, for targets that blend by vertex weights
//! only (skate). Each layered material with channels or the over composition gets a new colour stream on every
//! primitive drawn with it, holding the channels evaluated at that vertex (masks sampled at its UVs), and becomes the
//! original form: that stream, the same mapping, summed. Between vertices the weights then interpolate instead of
//! following the mask: an approximation, counted.

use std::collections::{BTreeMap, BTreeSet};

use primase_core::layered::{Composition, Source, over_as_sum};
use primase_core::pass::{Options, Pass, PassLog, PassRegistry};
use primase_core::pbr::{TextureRef, Wrap};
use primase_core::{
    Component, Entity, LayeredMaterial, Mesh, Semantic, Stream, Texture, WeightMapping, World,
};
use primase_texcodec::{Decoder, TexDecoder};

/// Register this crate's passes (not load-textures: loading is a reader's step).
pub fn register(reg: &mut PassRegistry) {
    reg.register("cap-textures", crate::cap_textures);
    reg.register("bake-layer-channels", |o: &Options| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(BakeLayerChannels)),
    });
    reg.register("normalize-normals", |o: &Options| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(crate::NormalizeNormals)),
    });
    reg.register("bake-normal-scale", |o: &Options| match o.keys().next() {
        Some(k) => Err(format!("unknown option {k}")),
        None => Ok(Box::new(crate::BakeNormalScale)),
    });
}

#[derive(Debug)]
pub struct BakeLayerChannels;

impl Pass for BakeLayerChannels {
    fn name(&self) -> &'static str {
        "bake-layer-channels"
    }
    fn requires(&self) -> &[&'static str] {
        &[LayeredMaterial::NAME]
    }
    fn modifies(&self) -> &[&'static str] {
        &[LayeredMaterial::NAME, Mesh::NAME]
    }
    fn approximates(&self) -> &'static str {
        "per-pixel layer weights evaluated at the vertices and interpolated between them"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, LayeredMaterial)> = w
            .iter::<LayeredMaterial>()
            .filter(|(_, l)| l.weights.per_pixel())
            .map(|(e, l)| (e, l.clone()))
            .collect();
        if materials.is_empty() {
            return Ok(());
        }
        // decode every mask once (level 0, RGBA8)
        let mut masks: BTreeMap<Entity, Decoded> = BTreeMap::new();
        for (e, l) in &materials {
            for t in l.mask_textures() {
                if let std::collections::btree_map::Entry::Vacant(slot) = masks.entry(t.texture) {
                    let tex = w.get::<Texture>(t.texture).ok_or_else(|| {
                        format!("{e:?}: mask {:?} is not loaded (load textures first)", t.texture)
                    })?;
                    slot.insert(decode(tex).map_err(|m| format!("{:?}: {m}", t.texture))?);
                }
            }
        }
        // every primitive drawn with each material, and a colour stream none of them has
        let mut drawn: BTreeMap<Entity, Vec<(Entity, usize)>> = BTreeMap::new();
        for (mesh, m) in w.iter::<Mesh>() {
            for (i, p) in m.primitives().iter().enumerate() {
                if let Some(mat) = p.material
                    && materials.iter().any(|(e, _)| *e == mat)
                {
                    drawn.entry(mat).or_default().push((mesh, i));
                }
            }
        }
        let mut done = Vec::new();
        for (e, l) in &materials {
            let prims = drawn.get(e).cloned().unwrap_or_default();
            let used: BTreeSet<u8> = prims
                .iter()
                .filter_map(|&(mesh, i)| w.get::<Mesh>(mesh).map(|m| &m.primitives()[i]))
                .flat_map(|p| p.streams.keys())
                .filter_map(|s| match s {
                    Semantic::Color(n) => Some(*n),
                    _ => None,
                })
                .collect();
            let n = (0..=u8::MAX)
                .find(|n| !used.contains(n))
                .ok_or_else(|| format!("{e:?}: no free colour stream"))?;
            let slots = l.layers.len();
            let mut streams = Vec::new();
            for &(mesh, i) in &prims {
                let p = &w.get::<Mesh>(mesh).ok_or("mesh vanished")?.primitives()[i];
                let count = p.positions().map_or(0, |v| v.len());
                let mut out = Vec::with_capacity(count);
                for v in 0..count {
                    let ch =
                        channels(&l.weights, p, v, &masks).map_err(|m| format!("{e:?} on {mesh:?}: {m}"))?;
                    out.push(match l.weights.composition {
                        Composition::Sum => ch,
                        Composition::Over => over_as_sum(l.weights.mapping.evaluate(ch), slots),
                    });
                }
                streams.push((mesh, i, Stream::F32x4(out)));
            }
            done.push((*e, n, streams));
        }
        for (e, n, streams) in done {
            let Some(mut l) = w.get::<LayeredMaterial>(e).cloned() else {
                continue;
            };
            for (mesh, i, s) in streams {
                if let Some(m) = w.get_mut::<Mesh>(mesh) {
                    m.primitives_mut()[i].streams.insert(Semantic::Color(n), s);
                    log.approximate("primitives with layer weights baked at their vertices", 1);
                }
            }
            if l.weights.composition == Composition::Over {
                // the stream already holds the slot weights
                l.weights.mapping = WeightMapping::Identity;
                log.approximate("over compositions baked to summed vertex weights", 1);
            }
            l.weights.attribute = Semantic::Color(n);
            l.weights.channels = None;
            l.weights.composition = Composition::Sum;
            w.insert(e, l);
            log.count("layered materials baked", 1);
        }
        Ok(())
    }
}

/// A texture's level 0 as RGBA8.
struct Decoded {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn decode(t: &Texture) -> Result<Decoded, String> {
    let level = t.levels.first().ok_or("no levels")?;
    let rgba = Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, level)
        .map_err(|e| e.to_string())?;
    Ok(Decoded {
        width: t.width,
        height: t.height,
        rgba,
    })
}

/// The four channel values at vertex `v` (unused ones 0).
fn channels(
    weights: &primase_core::LayerWeights,
    p: &primase_core::Primitive,
    v: usize,
    masks: &BTreeMap<Entity, Decoded>,
) -> Result<[f32; 4], String> {
    let mut out = [0.0; 4];
    match &weights.channels {
        None => {
            let a = vertex_value(p, weights.attribute, v)?;
            out = a;
        }
        Some(channels) => {
            for (k, c) in channels.iter().enumerate().take(4) {
                let values: Vec<f32> = c
                    .sources()
                    .into_iter()
                    .map(|s| source(s, p, v, masks))
                    .collect::<Result<_, _>>()?;
                out[k] = c.evaluate(&values);
            }
        }
    }
    Ok(out)
}

fn vertex_value(p: &primase_core::Primitive, semantic: Semantic, v: usize) -> Result<[f32; 4], String> {
    let s = p
        .streams
        .get(&semantic)
        .ok_or_else(|| format!("no {semantic:?} stream"))?;
    // as a weight or texture coordinate stream: floats as they are, unsigned integers normalized; a missing fourth
    // channel is one (glTF colours) and a missing third zero
    let value = match s {
        Stream::F32x2(x) => x.get(v).map(|&[a, b]| [a, b, 0.0, 1.0]),
        Stream::F32x3(x) => x.get(v).map(|&[a, b, c]| [a, b, c, 1.0]),
        Stream::F32x4(x) => x.get(v).copied(),
        Stream::U8x4(x) => x.get(v).map(|c| c.map(|c| f32::from(c) / 255.0)),
        Stream::U16x4(x) => x.get(v).map(|c| c.map(|c| f32::from(c) / 65535.0)),
    };
    value.ok_or_else(|| format!("{semantic:?} has no value at vertex {v}"))
}

fn source(
    s: &Source,
    p: &primase_core::Primitive,
    v: usize,
    masks: &BTreeMap<Entity, Decoded>,
) -> Result<f32, String> {
    match s {
        Source::Vertex { semantic, channel } => Ok(vertex_value(p, *semantic, v)?[usize::from(*channel)]),
        Source::Mask {
            texture,
            channels,
            bias,
        } => {
            let uv = vertex_value(p, Semantic::TexCoord(texture.texcoord), v)?;
            let rgba = sample(&masks[&texture.texture], texture, [uv[0], uv[1]]);
            Ok(primase_core::layered::mask_value(*channels, *bias, rgba))
        }
    }
}

/// Bilinear sample (texel centres at (i + 0.5) / size, v down the image as glTF) with the reference's wrap and
/// transform, 0..1 per channel.
fn sample(m: &Decoded, r: &TextureRef, uv: [f32; 2]) -> [f32; 4] {
    let uv = r.transform.map_or(uv, |t| t.apply(uv));
    let texel = |x: i64, y: i64| -> [f32; 4] {
        let wrap = |i: i64, n: u32, mode: Wrap| -> usize {
            let n = i64::from(n);
            (match mode {
                Wrap::Repeat => i.rem_euclid(n),
                Wrap::Clamp => i.clamp(0, n - 1),
                Wrap::Mirror => {
                    let period = 2 * n;
                    let k = i.rem_euclid(period);
                    if k < n { k } else { period - 1 - k }
                }
            }) as usize
        };
        let (x, y) = (wrap(x, m.width, r.wrap[0]), wrap(y, m.height, r.wrap[1]));
        let o = (y * m.width as usize + x) * 4;
        std::array::from_fn(|c| f32::from(m.rgba[o + c]) / 255.0)
    };
    let fx = uv[0] * m.width as f32 - 0.5;
    let fy = uv[1] * m.height as f32 - 0.5;
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (
        texel(x0, y0),
        texel(x0 + 1, y0),
        texel(x0, y0 + 1),
        texel(x0 + 1, y0 + 1),
    );
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bottom = c[k] + (d[k] - c[k]) * tx;
        top + (bottom - top) * ty
    })
}
