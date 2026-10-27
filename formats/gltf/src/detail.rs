//! `PRIMASE_materials_detail_blend`: a material's detail blend (`pbr.detail`, [`primase_core::detail`]) as a vendor
//! extension of its glTF material, beside the `pbrMetallicRoughness` that is the same surface baked. The writer
//! ([`crate::write`]) writes it for a material carrying a `pbr.detail`; the reader keeps each slot's texture
//! (`gltf.detail_textures`, [`crate::textures`]) and `lower-gltf-materials` reads the extension back as `pbr.detail`
//! ([`read`]), strictly: an unknown key, a value outside the closed sets, a number that is not a finite 32-bit float or
//! a count the equation does not take is an error naming the material. The extension states its `revision`
//! ([`REVISION`]; absent means 3): a reader takes 3 and 4, a file that states a newer one is told so, and a file of
//! revision 3 may not use what 4 added. The extension's specification is
//! `docs/extensions/PRIMASE_materials_detail_blend.md`.

use primase_core::Semantic;
use primase_core::detail::{
    DETAIL_LIMIT, Detail, DetailLerp, DetailMix, DetailedMaterial, Frozen, GlossScale, REVISION,
    ReflectanceTerm, SurfaceLowering, WEIGHT_POWER_LIMIT, Weight, WeightStep,
};
use primase_core::layered::Source;
use primase_core::pass::PassLog;
use primase_core::pbr::TextureRef;
use primase_core::shader::ScalarEncoding;
use serde_json::{Map, Value};

use crate::lower::{object, only, texture_transform};
use crate::textures::{DetailSlot, GltfDetailTextures};

/// The extension's name, in a material's `extensions` and the document's `extensionsUsed`.
pub const NAME: &str = "PRIMASE_materials_detail_blend";

/// The extension object of `material`, where it carries one.
pub(crate) fn extension(material: &Value) -> Option<&Value> {
    material.get("extensions")?.get(NAME)
}

/// `material`'s extension as a `pbr.detail`: `None` where it carries none, or where a texture of it was not read (its
/// image refused, counted: the material is drawn by its `pbr.material` alone); an error naming what the extension
/// states that it may not.
pub(crate) fn read(
    material: &Value,
    textures: Option<&GltfDetailTextures>,
    log: &mut PassLog,
) -> Result<Option<DetailedMaterial>, String> {
    let Some(x) = extension(material) else {
        return Ok(None);
    };
    match parse(x, &mut Slots { textures, log }) {
        Ok(d) => Ok(Some(d)),
        Err(Refusal::Unread(slot)) => {
            log.approximate(
                &format!("detail-blend materials drawn by their pbr.material alone: the image of their {slot} was not read"),
                1,
            );
            Ok(None)
        }
        Err(Refusal::Malformed(why)) => Err(format!("{NAME}: {why}")),
    }
}

/// Why an extension gives no `pbr.detail`.
enum Refusal {
    /// It states what it may not.
    Malformed(String),
    /// The texture of this slot was not read.
    Unread(DetailSlot),
}

impl From<String> for Refusal {
    fn from(why: String) -> Self {
        Refusal::Malformed(why)
    }
}

impl From<&str> for Refusal {
    fn from(why: &str) -> Self {
        Refusal::Malformed(why.to_owned())
    }
}

/// The extension object `x` as a `pbr.detail`, its textures from `slots`.
fn parse(x: &Value, slots: &mut Slots) -> Result<DetailedMaterial, Refusal> {
    let x = object(x, "the extension")?;
    only(
        x,
        &[
            "revision",
            "equation",
            "lowering",
            "baseColorTexture",
            "baseNormalTexture",
            "baseMaskTexture",
            "baseSaturation",
            "details",
            "weights",
            "gain",
            "normalStrength",
            "dielectricF0",
            "color",
            "glossEncoding",
            "alphaDetail",
            "frozen",
            "extras",
        ],
        "the extension",
    )?;
    let revision = match x.get("revision") {
        None => 3,
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("revision {v} is not a whole number"))?,
    };
    if revision > REVISION {
        return Err(format!("revision {revision} is newer than the {REVISION} this reader takes").into());
    }
    if revision < 3 {
        return Err(format!("revision {revision} is older than the 3 this reader takes").into());
    }
    let fourth = revision >= 4;
    match text(x, "equation")? {
        "detail_lerp" => {}
        other => return Err(format!("equation {other:?} is not detail_lerp").into()),
    }
    let lowering = match text(x, "lowering")? {
        "gloss_reflectance" => SurfaceLowering::GlossReflectance {
            dielectric_f0: field(x, "dielectricF0")?,
        },
        "metallic_gloss" if fourth => {
            if x.contains_key("dielectricF0") {
                return Err(
                    "dielectricF0 belongs to the gloss_reflectance lowering, not metallic_gloss".into(),
                );
            }
            SurfaceLowering::MetallicGloss
        }
        other => {
            return Err(format!(
                "lowering {other:?} is not {}",
                if fourth {
                    "gloss_reflectance or metallic_gloss"
                } else {
                    "gloss_reflectance (revision 3)"
                }
            )
            .into());
        }
    };
    let base_saturation = match x.get("baseSaturation") {
        None => 0.0,
        Some(_) if !fourth => return Err("baseSaturation needs revision 4".into()),
        Some(v) => float(v, "baseSaturation")?,
    };
    let base_colour = slots.get(x, "baseColorTexture", DetailSlot::BaseColor)?;
    let base_normal = slots.get(x, "baseNormalTexture", DetailSlot::BaseNormal)?;
    let base_mask = slots.get(x, "baseMaskTexture", DetailSlot::BaseMask)?;
    let listed = array(x, "details")?;
    if !(1..=DETAIL_LIMIT).contains(&listed.len()) {
        return Err(format!("{} details: the equation takes 1 to {DETAIL_LIMIT}", listed.len()).into());
    }
    let mut details = Vec::with_capacity(listed.len());
    for (i, d) in (0u8..).zip(listed) {
        let at = format!("details[{i}]");
        let d = object(d, &at)?;
        only(
            d,
            &[
                "colorTexture",
                "normalTexture",
                "tint",
                "glossBias",
                "glossScale",
                "reflectance",
                "extras",
            ],
            &at,
        )?;
        let colour = slots.get(d, "colorTexture", DetailSlot::Color(i))?;
        let normal = slots.get(d, "normalTexture", DetailSlot::Normal(i))?;
        let gloss_scale = match one_of(d, "glossScale", &["baseNormalZ", "constant"])? {
            ("baseNormalZ", v) => {
                only(object(v, "baseNormalZ")?, &[], "baseNormalZ")?;
                GlossScale::BaseNormalZ
            }
            (_, v) => GlossScale::Constant(float(v, "glossScale.constant")?),
        };
        let reflectance = match one_of(d, "reflectance", &["maskPlusBias", "bias"])? {
            ("maskPlusBias", v) => ReflectanceTerm::MaskPlusBias(float(v, "reflectance.maskPlusBias")?),
            (_, v) => ReflectanceTerm::Bias(float(v, "reflectance.bias")?),
        };
        details.push(Detail {
            colour,
            normal,
            tint: floats(d, "tint")?,
            gloss_bias: field(d, "glossBias")?,
            gloss_scale,
            reflectance,
        });
    }
    let listed = array(x, "weights")?;
    if listed.len() + 1 != details.len() {
        return Err(format!(
            "{} weights for {} details: one fewer",
            listed.len(),
            details.len()
        )
        .into());
    }
    let mut weights = Vec::with_capacity(listed.len());
    for (i, w) in (0u8..).zip(listed) {
        let at = format!("weights[{i}]");
        let w = object(w, &at)?;
        only(w, &["source", "steps", "extras"], &at)?;
        let source = match one_of(w, "source", &["mask", "vertex"])? {
            ("mask", v) => {
                let m = object(v, "mask")?;
                only(m, &["texture", "channels", "bias", "extras"], "mask")?;
                Source::Mask {
                    texture: slots.get(m, "texture", DetailSlot::WeightMask(i))?,
                    channels: floats(m, "channels")?,
                    bias: field(m, "bias")?,
                }
            }
            (_, v) => vertex(object(v, "vertex")?)?,
        };
        let mut steps = Vec::new();
        let kinds: &[&'static str] = if fourth {
            &["power", "min", "max", "affine", "saturate"]
        } else {
            &["power", "min"]
        };
        for step in array(w, "steps")? {
            steps.push(match one_key(object(step, "a step")?, "a step", kinds)? {
                ("power", v) => {
                    let n = v
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok())
                        .filter(|n| (1..=WEIGHT_POWER_LIMIT).contains(n))
                        .ok_or_else(|| format!("power {v} is not 1 to {WEIGHT_POWER_LIMIT}"))?;
                    WeightStep::Power(n)
                }
                ("min", v) => WeightStep::Min(float(v, "min")?),
                ("max", v) => WeightStep::Max(float(v, "max")?),
                ("affine", v) => {
                    let a = object(v, "affine")?;
                    only(a, &["scale", "bias", "extras"], "affine")?;
                    WeightStep::Affine {
                        scale: field(a, "scale")?,
                        bias: field(a, "bias")?,
                    }
                }
                (_, v) => {
                    only(object(v, "saturate")?, &[], "saturate")?;
                    WeightStep::Saturate
                }
            });
        }
        weights.push(Weight { source, steps });
    }
    let encoding = object(x.get("glossEncoding").ok_or("no glossEncoding")?, "glossEncoding")?;
    only(encoding, &["power", "root", "scale", "extras"], "glossEncoding")?;
    let small = |key: &str| {
        encoding
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u8::try_from(n).ok())
            .filter(|n| *n != 0)
            .ok_or_else(|| format!("glossEncoding.{key} is not 1 to 255"))
    };
    let gloss_encoding = ScalarEncoding {
        power: small("power")?,
        root: small("root")?,
        scale_bits: field(encoding, "scale")?.to_bits(),
    };
    let alpha = match x.get("alphaDetail") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|j| usize::try_from(j).ok())
                .filter(|j| *j < details.len())
                .ok_or_else(|| format!("alphaDetail {v} is not a detail"))?,
        ),
    };
    let kinds = ["frontFacing", "vertexInput", "engineConstant", "engineTexture"];
    let mut frozen = Vec::new();
    for f in array(x, "frozen")? {
        let (kind, v) = one_key(object(f, "a frozen entry")?, "a frozen entry", &kinds)?;
        let v = object(v, kind)?;
        frozen.push(match kind {
            "frontFacing" => {
                only(v, &[], kind)?;
                Frozen::FrontFacing
            }
            "vertexInput" => {
                only(v, &["semantic", "index", "value"], kind)?;
                Frozen::VertexInput {
                    semantic: text(v, "semantic")?.to_owned(),
                    index: v
                        .get("index")
                        .and_then(Value::as_u64)
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or("vertexInput.index is not an index")?,
                    value: floats(v, "value")?,
                }
            }
            "engineConstant" => {
                only(v, &["name", "value"], kind)?;
                let value = array(v, "value")?
                    .iter()
                    .map(|n| float(n, "engineConstant.value"))
                    .collect::<Result<Vec<_>, _>>()?;
                if value.is_empty() {
                    return Err("engineConstant.value is empty".into());
                }
                Frozen::EngineConstant {
                    name: text(v, "name")?.to_owned(),
                    value,
                }
            }
            _ => {
                only(v, &["name", "value"], kind)?;
                Frozen::EngineTexture {
                    name: text(v, "name")?.to_owned(),
                    value: floats(v, "value")?,
                }
            }
        });
    }
    Ok(DetailedMaterial {
        mix: DetailMix::DetailLerp(DetailLerp {
            base_colour,
            base_normal,
            base_mask,
            base_saturation,
            details,
            weights,
            gain: field(x, "gain")?,
            normal_strength: field(x, "normalStrength")?,
            colour: floats(x, "color")?,
            gloss_encoding,
            alpha,
        }),
        lowering,
        frozen,
    })
}

/// The slots' textures as the reader read them, and the log a texture info's other extensions are counted in.
struct Slots<'a, 'l> {
    textures: Option<&'a GltfDetailTextures>,
    log: &'l mut PassLog,
}

impl Slots<'_, '_> {
    /// The texture info `o[key]` (the extension's slot `slot`) as a texture reference: its texture as read, its
    /// `texCoord` and `KHR_texture_transform`, its sampler's wrap and filters; [`Refusal::Unread`] where its image was
    /// not read.
    fn get(&mut self, o: &Map<String, Value>, key: &str, slot: DetailSlot) -> Result<TextureRef, Refusal> {
        let info = object(o.get(key).ok_or_else(|| format!("no {slot}"))?, key)?;
        only(
            info,
            &["index", "texCoord", "colorSpace", "extensions", "extras"],
            &slot.to_string(),
        )?;
        match info.get("colorSpace").and_then(Value::as_str) {
            Some("srgb" | "linear") => {}
            _ => return Err(format!("{slot}.colorSpace is not srgb or linear").into()),
        }
        let (transform, texcoord) = texture_transform(info, self.log)?;
        let read = self
            .textures
            .and_then(|t| t.0.get(&slot))
            .ok_or(Refusal::Unread(slot))?;
        Ok(TextureRef {
            texture: read.texture,
            texcoord,
            wrap: read.wrap,
            transform,
            filters: read.filters,
        })
    }
}

/// A `vertex` source: channel `channel` of `COLOR_n` or `TEXCOORD_n`.
fn vertex(v: &Map<String, Value>) -> Result<Source, String> {
    only(v, &["attribute", "channel", "extras"], "vertex")?;
    let attribute = text(v, "attribute")?;
    let set = |prefix: &str| attribute.strip_prefix(prefix).and_then(|n| n.parse::<u8>().ok());
    let semantic = match (set("COLOR_"), set("TEXCOORD_")) {
        (Some(n), _) => Semantic::Color(n),
        (_, Some(n)) => Semantic::TexCoord(n),
        _ => {
            return Err(format!(
                "vertex.attribute {attribute} is not COLOR_n or TEXCOORD_n"
            ));
        }
    };
    let channel = v
        .get("channel")
        .and_then(Value::as_u64)
        .and_then(|c| u8::try_from(c).ok())
        .filter(|c| *c <= 3)
        .ok_or("vertex.channel is not 0 to 3")?;
    Ok(Source::Vertex { semantic, channel })
}

/// The one key of the object `o[key]`, among `keys`, with its value: a closed set's member.
fn one_of<'a>(
    o: &'a Map<String, Value>,
    key: &str,
    keys: &[&'static str],
) -> Result<(&'static str, &'a Value), String> {
    one_key(
        object(o.get(key).ok_or_else(|| format!("no {key}"))?, key)?,
        key,
        keys,
    )
}

/// The one key of `v` (`what`), among `keys`, with its value.
fn one_key<'a>(
    v: &'a Map<String, Value>,
    what: &str,
    keys: &[&'static str],
) -> Result<(&'static str, &'a Value), String> {
    match v.iter().next() {
        Some((k, value)) if v.len() == 1 => keys
            .iter()
            .find(|known| **known == k.as_str())
            .map(|known| (*known, value))
            .ok_or_else(|| format!("{what} {k} is not one of {}", keys.join(", "))),
        _ => Err(format!("{what} is not one key of {}", keys.join(", "))),
    }
}

/// `o[key]`, a string.
fn text<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    o.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is not a string"))
}

/// `o[key]`, an array.
fn array<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], String> {
    o.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{key} is not an array"))
}

/// `v`, a finite 32-bit float: the writer writes each as the decimal that reads back to its bits.
fn float(v: &Value, what: &str) -> Result<f32, String> {
    let x = v.as_f64().ok_or_else(|| format!("{what} is not a number"))? as f32;
    if x.is_finite() {
        Ok(x)
    } else {
        Err(format!("{what} {v} is not a finite 32-bit float"))
    }
}

/// `o[key]`, a finite 32-bit float.
fn field(o: &Map<String, Value>, key: &str) -> Result<f32, String> {
    float(o.get(key).ok_or_else(|| format!("no {key}"))?, key)
}

/// `o[key]`, `N` finite 32-bit floats.
fn floats<const N: usize>(o: &Map<String, Value>, key: &str) -> Result<[f32; N], String> {
    let a = array(o, key)?;
    if a.len() != N {
        return Err(format!("{key} is not {N} numbers"));
    }
    let mut out = [0.; N];
    for (slot, v) in out.iter_mut().zip(a) {
        *slot = float(v, key)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use iridia_interface::{Filter, Filters};
    use primase_core::pass::Pass;
    use primase_core::{
        ColorSpace, Entity, Instance, Mesh, Name, Opaque, OpaqueData, Primitive, Stream, TexFormat, TexRole,
        Texture, Topology, Transform, World,
    };
    use primase_core::{pbr::PbrMaterial, pbr::UvTransform, pbr::Wrap};
    use serde_json::json;

    use super::*;
    use crate::lower::LowerGltfMaterials;
    use crate::read::MATERIAL;
    use crate::textures::SlotTexture;

    /// Every texture reference of `m`, in the extension's slot order.
    fn textures_mut(m: &mut DetailLerp) -> Vec<&mut TextureRef> {
        let mut out = vec![&mut m.base_colour, &mut m.base_normal, &mut m.base_mask];
        for d in &mut m.details {
            out.push(&mut d.colour);
            out.push(&mut d.normal);
        }
        for w in &mut m.weights {
            if let Source::Mask { texture, .. } = &mut w.source {
                out.push(texture);
            }
        }
        out
    }

    /// A 2 by 2 RGBA8 texture of `seed`'s texels, as colour (sRGB) or packed data (linear).
    fn texture(w: &mut World, seed: u8, colour: bool) -> Entity {
        let e = w.spawn();
        w.insert(
            e,
            Texture {
                width: 2,
                height: 2,
                format: TexFormat::Rgba8Unorm,
                levels: vec![
                    (0..16)
                        .map(|i| seed.wrapping_mul(31).wrapping_add(i * 17))
                        .collect(),
                ],
                color_space: if colour {
                    ColorSpace::Srgb
                } else {
                    ColorSpace::Linear
                },
                role: if colour { TexRole::Color } else { TexRole::Mask },
                normal: None,
            },
        );
        e
    }

    /// A drawn material with a `pbr.detail` stating every field: two details (the second sampled on set 1, tiled,
    /// mirrored and filtered), the weight a mask of the base normal's alpha raised and capped, the alpha the second
    /// detail's, every kind of frozen entry, and numbers whose decimals do not read back through f64 alone.
    fn world() -> (World, Entity, DetailedMaterial) {
        let mut w = World::new();
        let r = |texture, texcoord, transform| TextureRef {
            texture,
            texcoord,
            wrap: [Wrap::Repeat; 2],
            transform,
            filters: Filters::default(),
        };
        let tiled = Some(UvTransform {
            offset: [0.5, -0.25],
            scale: [16., 12.5],
            rotation: 0.,
        });
        let base_normal = r(texture(&mut w, 2, false), 0, None);
        let mut detail_colour = r(texture(&mut w, 4, true), 1, tiled);
        detail_colour.wrap = [Wrap::Mirror, Wrap::Clamp];
        detail_colour.filters = Filters {
            mag: Some(Filter::Nearest),
            min: None,
            mip: None,
        };
        let awkward = f32::from_bits(0x15ae_43fd);
        let mix = DetailLerp {
            base_colour: r(texture(&mut w, 1, true), 0, None),
            base_normal,
            base_mask: r(texture(&mut w, 3, false), 0, None),
            base_saturation: 0.3,
            details: vec![
                Detail {
                    colour: r(texture(&mut w, 5, true), 0, tiled),
                    normal: r(texture(&mut w, 6, false), 0, tiled),
                    tint: [3.64, 3.185, 2.73],
                    gloss_bias: 0.25,
                    gloss_scale: GlossScale::BaseNormalZ,
                    reflectance: ReflectanceTerm::MaskPlusBias(0.375),
                },
                Detail {
                    colour: detail_colour,
                    normal: r(texture(&mut w, 7, false), 1, tiled),
                    tint: [5.46, 1. / 3., awkward],
                    gloss_bias: -0.1,
                    gloss_scale: GlossScale::Constant(0.5),
                    reflectance: ReflectanceTerm::Bias(0.6),
                },
            ],
            weights: vec![Weight {
                source: Source::Mask {
                    texture: base_normal,
                    channels: [0., 0., 0., 1.],
                    bias: -0.0,
                },
                steps: vec![
                    WeightStep::Max(0.),
                    WeightStep::Affine {
                        scale: 2.05,
                        bias: -1.05,
                    },
                    WeightStep::Saturate,
                    WeightStep::Power(260),
                    WeightStep::Min(1.),
                ],
            }],
            gain: 1.1,
            normal_strength: 0.5,
            colour: [0.9, 0.85, 0.8],
            gloss_encoding: ScalarEncoding {
                power: 1,
                root: 2,
                scale_bits: 0.95f32.to_bits(),
            },
            alpha: Some(1),
        };
        let detail = DetailedMaterial {
            mix: DetailMix::DetailLerp(mix),
            lowering: SurfaceLowering::GlossReflectance { dielectric_f0: 0.03 },
            frozen: vec![
                Frozen::FrontFacing,
                Frozen::VertexInput {
                    semantic: "COLOR".into(),
                    index: 0,
                    value: [1., 1., 1., 0.],
                },
                Frozen::EngineConstant {
                    name: "aColor".into(),
                    value: vec![1., 1., 1., 1., 0., awkward],
                },
                Frozen::EngineTexture {
                    name: "g_DecalTexture".into(),
                    value: [0., 0., 0., 0.5],
                },
            ],
        };
        let material = w.spawn();
        w.insert(material, Name("scarf".into()));
        w.insert(material, PbrMaterial::default());
        w.insert(material, detail.clone());
        let uv = || Stream::F32x2(vec![[0., 0.], [1., 0.], [0., 1.]]);
        let mesh = w.spawn();
        w.insert(
            mesh,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (
                        Semantic::Position,
                        Stream::F32x3(vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.]]),
                    ),
                    (Semantic::TexCoord(0), uv()),
                    (Semantic::TexCoord(1), uv()),
                ]),
                indices: Some(vec![0, 1, 2]),
                material: Some(material),
            }]),
        );
        let place = w.spawn();
        w.insert(
            place,
            Instance {
                prototype: mesh,
                variant: 0,
            },
        );
        w.insert(place, Transform::IDENTITY);
        (w, material, detail)
    }

    /// The writer's `pbr.detail` read back by the reader and `lower-gltf-materials` is the one written, every number
    /// to its bits, each texture its texels, colour space and role, at the same coordinate set, tiling and sampler.
    #[test]
    fn the_extension_reads_back_what_the_writer_wrote() -> Result<(), Box<dyn std::error::Error>> {
        let (w, _, written) = world();
        let (bytes, report) = crate::write::write(&w, &crate::write::WriteOptions::default())?;
        assert_eq!(report.detail_materials, 1);
        assert!(report.detail_materials_dropped.is_empty());
        let mut back = World::new();
        crate::read::read_glb(&bytes, "detail.glb", &mut back)?;
        let mut log = PassLog::default();
        LowerGltfMaterials.run(&mut back, &mut log)?;
        assert_eq!(
            log.counts.get("detail-blend materials read as pbr.detail"),
            Some(&1)
        );
        let read: Vec<DetailedMaterial> = back.iter::<DetailedMaterial>().map(|(_, d)| d.clone()).collect();
        let [mut read] =
            <[DetailedMaterial; 1]>::try_from(read).map_err(|r| format!("{} pbr.detail", r.len()))?;
        let (DetailMix::DetailLerp(r), DetailMix::DetailLerp(mut o)) = (&mut read.mix, written.mix.clone());
        for (r, o) in textures_mut(r).into_iter().zip(textures_mut(&mut o)) {
            let (a, b) = (
                back.get::<Texture>(r.texture).ok_or("a read texture")?,
                w.get::<Texture>(o.texture).ok_or("a written texture")?,
            );
            assert_eq!(
                (&a.levels, a.color_space, a.role),
                (&b.levels, b.color_space, b.role)
            );
            r.texture = o.texture;
        }
        assert_eq!(read, written);
        // the bits, not only the values: -0 and the decimal that rounds twice through f64
        let (DetailMix::DetailLerp(r), DetailMix::DetailLerp(o)) = (&read.mix, &written.mix);
        assert_eq!(r.details[1].tint[2].to_bits(), o.details[1].tint[2].to_bits());
        let (Source::Mask { bias: rb, .. }, Source::Mask { bias: ob, .. }) =
            (&r.weights[0].source, &o.weights[0].source)
        else {
            return Err("the weight is not a mask".into());
        };
        assert_eq!(rb.to_bits(), ob.to_bits());
        Ok(())
    }

    /// The extension as the writer writes it for [`world`]'s material, and a texture for each of its slots.
    fn document() -> Result<(Value, GltfDetailTextures, World), Box<dyn std::error::Error>> {
        let (w, _, _) = world();
        let (doc, _) = crate::write::write_document(&w, &crate::write::WriteOptions::default())?;
        let material = doc.json["materials"]
            .as_array()
            .and_then(|m| m.iter().find(|m| m["name"] == "scarf"))
            .ok_or("the material")?
            .clone();
        let mut back = World::new();
        let slots = DetailSlot::of(&material)
            .into_iter()
            .filter(|s| s.info(&material).is_some())
            .map(|s| {
                (
                    s,
                    SlotTexture {
                        texture: back.spawn(),
                        filters: Filters::default(),
                        wrap: [Wrap::Repeat; 2],
                    },
                )
            })
            .collect();
        Ok((material, GltfDetailTextures(slots), back))
    }

    /// `lower-gltf-materials` on one material holding `material` and `textures`.
    fn lower(
        material: Value,
        textures: GltfDetailTextures,
        mut w: World,
    ) -> (Result<(), String>, PassLog, World) {
        let e = w.spawn();
        w.insert_opaque(
            e,
            MATERIAL,
            Opaque {
                version: 1,
                data: OpaqueData::Json(material),
            },
        )
        .expect("an opaque material");
        w.insert(e, Name("scarf".into()));
        w.insert(e, textures);
        let mut log = PassLog::default();
        let run = LowerGltfMaterials.run(&mut w, &mut log);
        (run, log, w)
    }

    /// A change to the extension's JSON.
    type Edit = fn(&mut Value);

    /// Each thing the extension may not state is an error naming the material and what it states.
    #[test]
    fn a_malformed_extension_is_an_error_naming_what_it_states() -> Result<(), Box<dyn std::error::Error>> {
        let cases: [(&str, Edit); 21] = [
            ("unknown key colour", |x| x["colour"] = json!([1, 1, 1])),
            ("equation \"detail_add\"", |x| x["equation"] = json!("detail_add")),
            ("lowering \"metal_rough\"", |x| {
                x["lowering"] = json!("metal_rough")
            }),
            ("4 details", |x| {
                let d = x["details"][0].clone();
                x["details"] = json!([d, d, d, d]);
            }),
            ("0 weights for 2 details", |x| x["weights"] = json!([])),
            ("power 513", |x| {
                x["weights"][0]["steps"][0] = json!({"power": 513})
            }),
            ("a step is not one key", |x| {
                x["weights"][0]["steps"][0] = json!({"power": 2, "min": 1})
            }),
            ("baseMaskTexture.colorSpace", |x| {
                x["baseMaskTexture"]["colorSpace"] = json!("rgb")
            }),
            ("not a finite 32-bit float", |x| x["gain"] = json!(1e39)),
            ("glossScale lerp", |x| {
                x["details"][0]["glossScale"] = json!({"lerp": 1})
            }),
            ("a frozen entry instanceColour", |x| {
                x["frozen"][0] = json!({"instanceColour": {}})
            }),
            ("alphaDetail 2", |x| x["alphaDetail"] = json!(2)),
            ("vertex.attribute NORMAL", |x| {
                x["weights"][0]["source"] = json!({"vertex": {"attribute": "NORMAL", "channel": 0}})
            }),
            ("revision 5 is newer", |x| x["revision"] = json!(5)),
            ("revision 2 is older", |x| x["revision"] = json!(2)),
            ("dielectricF0 belongs to the gloss_reflectance lowering", |x| {
                x["lowering"] = json!("metallic_gloss")
            }),
            ("not a finite 32-bit float", |x| x["baseSaturation"] = json!(1e39)),
            ("a step is not one key", |x| {
                x["weights"][0]["steps"][2] = json!({"saturate": {}, "min": 1})
            }),
            ("a step lerp is not one of", |x| {
                x["weights"][0]["steps"][0] = json!({"lerp": 1})
            }),
            ("affine", |x| {
                x["weights"][0]["steps"][1] = json!({"affine": {"scale": 2, "offset": 1}})
            }),
            ("no dielectricF0", |x| {
                if let Some(o) = x.as_object_mut() {
                    o.remove("dielectricF0");
                }
            }),
        ];
        for (expected, edit) in cases {
            let (mut material, textures, w) = document()?;
            edit(&mut material["extensions"][NAME]);
            let (run, _, w) = lower(material, textures, w);
            let e = run.expect_err(expected);
            assert!(
                e.starts_with("material scarf: PRIMASE_materials_detail_blend: "),
                "{e}"
            );
            assert!(e.contains(expected), "{expected}: {e}");
            assert_eq!(w.iter::<DetailedMaterial>().count(), 0);
        }
        // and the document unedited is read
        let (material, textures, w) = document()?;
        let (run, _, w) = lower(material, textures, w);
        run?;
        assert_eq!(w.iter::<DetailedMaterial>().count(), 1);
        Ok(())
    }

    /// A material whose detail texture was not read (its image refused) keeps its `pbr.material` alone, counted by the
    /// slot.
    #[test]
    fn an_unread_detail_texture_leaves_the_pbr_material_alone() -> Result<(), Box<dyn std::error::Error>> {
        let (material, mut textures, w) = document()?;
        textures.0.remove(&DetailSlot::Normal(1));
        let (run, log, w) = lower(material, textures, w);
        run?;
        assert_eq!(w.iter::<DetailedMaterial>().count(), 0);
        assert_eq!(w.iter::<PbrMaterial>().count(), 1);
        assert_eq!(
            log.approximations.get(
                "detail-blend materials drawn by their pbr.material alone: the image of their details[1].normalTexture \
                 was not read"
            ),
            Some(&1)
        );
        Ok(())
    }

    /// The writer's revision, `metallic_gloss` without a dielectric reflectance and no saturation key where there is none.
    #[test]
    fn a_metallic_gloss_material_round_trips_without_what_it_does_not_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut w, material, _) = world();
        let mut d = w.get::<DetailedMaterial>(material).ok_or("the detail")?.clone();
        d.lowering = SurfaceLowering::MetallicGloss;
        let DetailMix::DetailLerp(m) = &mut d.mix;
        m.base_saturation = 0.;
        w.insert(material, d.clone());
        let (doc, _) = crate::write::write_document(&w, &crate::write::WriteOptions::default())?;
        let x = &doc.json["materials"][0]["extensions"][NAME];
        assert_eq!(x["revision"], json!(REVISION));
        assert_eq!(x["lowering"], json!("metallic_gloss"));
        assert!(
            x.get("dielectricF0").is_none() && x.get("baseSaturation").is_none(),
            "{x}"
        );
        let (bytes, _) = crate::write::write(&w, &crate::write::WriteOptions::default())?;
        let mut back = World::new();
        crate::read::read_glb(&bytes, "detail.glb", &mut back)?;
        let mut log = PassLog::default();
        LowerGltfMaterials.run(&mut back, &mut log)?;
        let read: Vec<&DetailedMaterial> = back.iter::<DetailedMaterial>().map(|(_, d)| d).collect();
        let [read] =
            <[&DetailedMaterial; 1]>::try_from(read).map_err(|r| format!("{} pbr.detail", r.len()))?;
        assert_eq!(read.lowering, SurfaceLowering::MetallicGloss);
        let DetailMix::DetailLerp(r) = &read.mix;
        assert_eq!(r.base_saturation.to_bits(), 0f32.to_bits());
        Ok(())
    }

    /// A file of revision 3 (no `revision` key, no 4 keys) reads as before; one of revision 3 using what 4 added is refused.
    #[test]
    fn a_revision_3_extension_reads_as_a_subset() -> Result<(), Box<dyn std::error::Error>> {
        let drop_new = |x: &mut Value| {
            let o = x.as_object_mut().expect("the extension");
            o.remove("revision");
            o.remove("baseSaturation");
            x["weights"][0]["steps"] = json!([{"power": 260}, {"min": 1}]);
        };
        let (mut material, textures, w) = document()?;
        drop_new(&mut material["extensions"][NAME]);
        let (run, _, w) = lower(material, textures, w);
        run?;
        let read: Vec<&DetailedMaterial> = w.iter::<DetailedMaterial>().map(|(_, d)| d).collect();
        let [read] =
            <[&DetailedMaterial; 1]>::try_from(read).map_err(|r| format!("{} pbr.detail", r.len()))?;
        assert_eq!(
            read.lowering,
            SurfaceLowering::GlossReflectance { dielectric_f0: 0.03 }
        );
        let DetailMix::DetailLerp(r) = &read.mix;
        assert_eq!(r.base_saturation, 0.);
        let refused: [(&str, Edit); 3] = [
            ("baseSaturation needs revision 4", |x| {
                x["baseSaturation"] = json!(0.5)
            }),
            (
                "lowering \"metallic_gloss\" is not gloss_reflectance (revision 3)",
                |x| x["lowering"] = json!("metallic_gloss"),
            ),
            ("a step saturate is not one of power, min", |x| {
                x["weights"][0]["steps"] = json!([{"saturate": {}}])
            }),
        ];
        for (expected, edit) in refused {
            let (mut material, textures, w) = document()?;
            drop_new(&mut material["extensions"][NAME]);
            edit(&mut material["extensions"][NAME]);
            let (run, _, _) = lower(material, textures, w);
            let e = run.expect_err(expected);
            assert!(e.contains(expected), "{expected}: {e}");
        }
        Ok(())
    }
}
