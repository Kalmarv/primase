//! What the web format's tiles carry beyond the standard glTF primase-gltf writes, added through its writer
//! extension ([`primase_gltf::extend`]):
//!
//! - **`PRIMASE_layered_material`** (`core.layered`): one glTF material with the extension: the weight attribute
//!   (`COLOR_n`), the mapping and up to four layers, each an index of an ordinary material in the same file (the layer
//!   materials are written like any other; their images are shared) with the layer's UV set and transform. The host
//!   material is a verbatim copy of the first non-null layer's material (name and extras aside), so a reader that
//!   ignores the extension shows one plausible layer. The extension is only in `extensionsUsed`. A primitive drawn
//!   with a layered material but lacking its weight attribute is counted (the writer's
//!   `primitives_without_material_attribute`). Its `version` is 2 with per-pixel channels or the over composition, 3
//!   with the height composition (`"composition": {"height": {"contrast": c}}` and `weights.heights`, a source per
//!   slot), 4 with a power channel (`{"power": {"v": source, "exponent": n}}`); version 5 adds bounded products
//!   (`{"product": [source, ...]}`). An older consumer must not draw an unknown version as the form it knows.
//! - **`PRIMASE_materials_detail_blend`** (`pbr.detail`, `docs/extensions/PRIMASE_materials_detail_blend.md`) beside
//!   the material's `pbr.material`; a detail whose texture could not be written leaves the material its
//!   `pbr.material` alone, counted by reason.
//! - **Extras**, as the tiles have them: each object's components with a JSON form that the glTF object does not
//!   express, as `extras.primase["<component name>"] = {"version": n, "value": ...}` (placement -> node, prototype or
//!   piece -> mesh, material -> material, texture -> texture, resources but the environment -> the scene), a
//!   placement's `web.placement_key`, its shadows as `extras["primase.shadows"]`, and a data texture's image as
//!   `"primase.role": "data"`. These are what the web format's readers read today (the viewer's provenance, shadows
//!   and prototype keys; [`crate::instancing`], [`crate::provenance`], [`crate::ktx2`]). A component map in extras is
//!   not what primase's glTF output carries (core is the interchange; a format's own data comes from the passes that
//!   make its components): this keeps the tiles' bytes while the web format moves off it, and nothing new should go
//!   into it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use primase_core::detail::{
    DetailMix, DetailedMaterial, Frozen, GlossScale, ReflectanceTerm, SurfaceLowering, WeightStep,
};
use primase_core::layered::{Channel, Composition, Source};
use primase_core::pbr::{PbrMaterial, TextureRef, UvTransform};
use primase_core::state::{Condition, PrimitiveCondition, StateOverride};
use primase_core::{
    ColorSpace, Component, ComponentJson, Entity, Instance, LayeredMaterial, Light, MaterialVariants, Mesh,
    MeshFile, MorphWeights, Morphs, Name, Semantic, ShadowCast, Shadows, Skin, SkinInstance, Texture,
    TextureFile, Transform, WeightMapping, World,
};
use primase_gltf::extend::{Extension, MaterialExtension, MaterialWriter, Object, WholeMaterial};
use primase_gltf::{Error, Result};
use serde_json::{Map, Value, json};

/// The layered materials' extension.
pub const LAYERED: &str = "PRIMASE_layered_material";
/// The detail blend's extension.
pub const DETAIL: &str = "PRIMASE_materials_detail_blend";
/// The extras key a placement's shadows are written under.
pub const SHADOWS: &str = "primase.shadows";

// The components each glTF object expresses, as the tiles' extras leave them out.
const CONSUMED_LIGHT: &[&str] = &[Name::NAME, Transform::NAME, Light::NAME];
const CONSUMED_PLACEMENT: &[&str] = &[
    Name::NAME,
    Transform::NAME,
    Instance::NAME,
    Shadows::NAME,
    SkinInstance::NAME,
    MorphWeights::NAME,
    Condition::NAME,
    PrimitiveCondition::NAME,
    StateOverride::NAME,
];
const CONSUMED_PROTOTYPE: &[&str] = &[
    Name::NAME,
    Mesh::NAME,
    MaterialVariants::NAME,
    MeshFile::NAME,
    Shadows::NAME,
    Skin::NAME,
    Morphs::NAME,
    Condition::NAME,
    PrimitiveCondition::NAME,
    StateOverride::NAME,
];
const CONSUMED_MATERIAL: &[&str] = &[Name::NAME, PbrMaterial::NAME, LayeredMaterial::NAME];
const CONSUMED_TEXTURE: &[&str] = &[Name::NAME, TextureFile::NAME, Texture::NAME];

/// What one tile's document carries beyond standard glTF (see the module documentation).
#[derive(Debug, Default)]
pub struct TileExtension {
    /// Each selected placement's key, in its node's extras.
    pub placement_keys: BTreeMap<Entity, u64>,
    /// Resources the scene's extras leave out (the tile set carries them elsewhere).
    pub omit_resources: BTreeSet<String>,
    pub counts: Mutex<Counts>,
}

/// What the extension wrote, and what it could not.
#[derive(Clone, Debug, Default)]
pub struct Counts {
    /// `core.layered` materials written with `PRIMASE_layered_material`.
    pub layered_materials: u64,
    /// Layer slots whose material has no `pbr.material`: written as empty slots.
    pub layers_without_pbr: u64,
    /// `pbr.detail` materials written with `PRIMASE_materials_detail_blend`, and those written without it, by reason.
    pub detail_materials: u64,
    pub detail_materials_dropped: BTreeMap<String, u64>,
    /// Components and resources with no JSON form, by name: not in the extras.
    pub extras_unencoded: BTreeMap<String, u64>,
}

impl TileExtension {
    /// What it counted.
    pub fn counts(&self) -> Counts {
        lock(&self.counts).clone()
    }

    /// `shown` without what `consumed` names, as `{"primase": {name: {"version", "value"}}}`; each component without
    /// a JSON form counted.
    fn primase(&self, shown: Vec<ComponentJson<'_>>, consumed: &[&str]) -> Option<Map<String, Value>> {
        let mut map = Map::new();
        for c in shown {
            if consumed.contains(&c.name) {
                continue;
            }
            match c.value {
                Some(v) => {
                    map.insert(c.name.to_owned(), json!({"version": c.version, "value": v}));
                }
                None => {
                    *lock(&self.counts)
                        .extras_unencoded
                        .entry(c.name.to_owned())
                        .or_default() += 1
                }
            }
        }
        (!map.is_empty()).then(|| {
            let mut out = Map::new();
            out.insert("primase".into(), Value::Object(map));
            out
        })
    }

    fn components(&self, world: &World, e: Entity, consumed: &[&str]) -> Result<Option<Map<String, Value>>> {
        let shown = world.components_json(e).map_err(|err| Error::World {
            entity: e,
            detail: err.to_string(),
        })?;
        Ok(self.primase(shown, consumed))
    }
}

fn lock(counts: &Mutex<Counts>) -> MutexGuard<'_, Counts> {
    counts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `extras` with `"primase.shadows"` added when anything about them is known.
fn with_shadows(
    extras: Option<Map<String, Value>>,
    cast: Option<&str>,
    receive: Option<bool>,
) -> Option<Map<String, Value>> {
    if cast.is_none() && receive.is_none() {
        return extras;
    }
    let mut shadows = Map::new();
    shadows.insert("version".into(), json!(1));
    if let Some(cast) = cast {
        shadows.insert("cast".into(), json!(cast));
    }
    if let Some(receive) = receive {
        shadows.insert("receive".into(), json!(receive));
    }
    let mut extras = extras.unwrap_or_default();
    extras.insert(SHADOWS.into(), Value::Object(shadows));
    Some(extras)
}

impl Extension for TileExtension {
    fn names(&self) -> &'static [&'static str] {
        &[DETAIL, LAYERED]
    }

    fn material(&self, w: &mut MaterialWriter<'_, '_>, material: Entity) -> Result<Option<WholeMaterial>> {
        let Some(layered) = w.world().get::<LayeredMaterial>(material).cloned() else {
            return Ok(None);
        };
        layered_material(&self.counts, w, material, &layered).map(Some)
    }

    fn material_extension(
        &self,
        w: &mut MaterialWriter<'_, '_>,
        material: Entity,
        sets: &mut BTreeSet<u8>,
    ) -> Result<Option<MaterialExtension>> {
        let Some(detail) = w.world().get::<DetailedMaterial>(material).cloned() else {
            return Ok(None);
        };
        Ok(match detail_blend(w, material, &detail, sets)? {
            Ok(value) => {
                lock(&self.counts).detail_materials += 1;
                Some(MaterialExtension {
                    name: DETAIL,
                    value,
                    components: &[DetailedMaterial::NAME],
                })
            }
            Err(why) => {
                *lock(&self.counts)
                    .detail_materials_dropped
                    .entry(why)
                    .or_default() += 1;
                None
            }
        })
    }

    fn extras(&self, world: &World, object: Object<'_>) -> Result<Option<Map<String, Value>>> {
        match object {
            Object::Scene => {
                let mut resources = world.resources_json().map_err(|e| Error::Invalid {
                    item: format!("resource {}", e.component),
                    detail: e.message,
                })?;
                resources.retain(|r| !self.omit_resources.contains(r.name));
                Ok(self.primase(resources, &[]))
            }
            Object::Light(e) => {
                let extras = self.components(world, e, CONSUMED_LIGHT)?;
                let cast = world
                    .get::<Light>(e)
                    .and_then(|l| l.cast_shadows)
                    .map(|c| if c { "on" } else { "off" });
                Ok(with_shadows(extras, cast, None))
            }
            Object::Placement { placement, prototype } => {
                let extras = match self.placement_keys.get(&placement) {
                    None => self.components(world, placement, CONSUMED_PLACEMENT)?,
                    Some(&key) => {
                        let mut shown = world.components_json(placement).map_err(|err| Error::World {
                            entity: placement,
                            detail: err.to_string(),
                        })?;
                        shown.push(ComponentJson {
                            name: "web.placement_key",
                            version: 1,
                            value: Some(json!(key)),
                        });
                        shown.sort_by(|a, b| a.name.cmp(b.name));
                        self.primase(shown, CONSUMED_PLACEMENT)
                    }
                };
                // the placement's own, else its prototype's
                let own = world.get::<Shadows>(placement).copied().unwrap_or_default();
                let inherited = world.get::<Shadows>(prototype).copied().unwrap_or_default();
                let cast = own.cast.or(inherited.cast).map(|c| match c {
                    ShadowCast::Off => "off",
                    ShadowCast::On => "on",
                    ShadowCast::Only => "only",
                });
                Ok(with_shadows(extras, cast, own.receive.or(inherited.receive)))
            }
            Object::Mesh {
                prototype,
                piece: None,
            } => self.components(world, prototype, CONSUMED_PROTOTYPE),
            Object::Mesh {
                prototype,
                piece: Some((mesh, from)),
            } => {
                // the prototype's components on a copy of it rebuilt to the piece (per-primitive ones remapped by
                // `from`, checked by the world; the world itself is unchanged)
                let failed = |detail: String| Error::World {
                    entity: prototype,
                    detail,
                };
                let mut copy = world.subset(&BTreeSet::from([prototype]));
                copy.rebuild_primitives(prototype, |_| Ok((mesh.primitives().to_vec(), from.to_vec())))
                    .map_err(failed)?;
                let shown = copy
                    .components_json(prototype)
                    .map_err(|err| failed(err.to_string()))?;
                Ok(self.primase(shown, CONSUMED_PROTOTYPE))
            }
            Object::Material(e) => self.components(world, e, CONSUMED_MATERIAL),
            Object::Texture(e) => self.components(world, e, CONSUMED_TEXTURE),
            // a data texture must reach a consumer whole: no resizing or lossy coding of its image
            Object::Image { texture } => Ok(world
                .get::<Texture>(texture)
                .is_some_and(|t| t.role == primase_core::TexRole::Data)
                .then(|| {
                    let mut m = Map::new();
                    m.insert("primase.role".into(), json!("data"));
                    m
                })),
        }
    }
}

/// A `core.layered` material: its layers' materials first, then the host (a copy of the first non-null layer's
/// material) carrying `PRIMASE_layered_material`. The sets are every set a layer's textures or the layer's own
/// `texcoord` selects.
fn layered_material(
    counts: &Mutex<Counts>,
    w: &mut MaterialWriter<'_, '_>,
    e: Entity,
    layered: &LayeredMaterial,
) -> Result<WholeMaterial> {
    let attribute = match layered.weights.attribute {
        Semantic::Color(n) => format!("COLOR_{n}"),
        other => {
            return Err(Error::World {
                entity: e,
                detail: format!("layer weights must be a COLOR_n stream, not {other:?}"),
            });
        }
    };
    let mut sets = BTreeSet::new();
    let mut layers = Vec::with_capacity(layered.layers.len());
    let mut first = None;
    for layer in &layered.layers {
        let Some(layer) = layer else {
            layers.push(Value::Null);
            continue;
        };
        let Some((index, layer_sets)) = w.material(layer.material)? else {
            // a layer with no material the writer can express: an empty slot, as the format has
            lock(counts).layers_without_pbr += 1;
            layers.push(Value::Null);
            continue;
        };
        first.get_or_insert(index);
        sets.extend(layer_sets);
        sets.insert(layer.texcoord.unwrap_or(0));
        let mut o = Map::new();
        o.insert("material".into(), json!(index));
        if let Some(n) = layer.texcoord {
            o.insert("texCoord".into(), json!(n));
        }
        if let Some(x) = layer.transform.filter(|x| *x != UvTransform::IDENTITY) {
            o.insert(
                "transform".into(),
                Value::Object(w.uv_transform(layer.material, x)?),
            );
        }
        layers.push(Value::Object(o));
    }
    let mapping = match layered.weights.mapping {
        WeightMapping::Identity => "identity",
        WeightMapping::RgbProduct4 => "rgb_product_4",
    };
    let mut weights = Map::new();
    weights.insert("attribute".into(), json!(attribute));
    weights.insert("mapping".into(), json!(mapping));
    if let Some(channels) = &layered.weights.channels {
        let mut out = Vec::with_capacity(channels.len());
        for c in channels {
            out.push(layer_channel(w, e, c, &mut sets)?);
        }
        weights.insert("channels".into(), Value::Array(out));
    }
    if !layered.weights.heights.is_empty() {
        let mut out = Vec::with_capacity(layered.weights.heights.len());
        for h in &layered.weights.heights {
            out.push(layer_source(w, e, h, &mut sets)?);
        }
        weights.insert("heights".into(), Value::Array(out));
    }
    let mut host = match first {
        Some(i) => match w.written(i) {
            Value::Object(m) => m.clone(),
            _ => unreachable!("materials are objects"),
        },
        // every slot null: the blend is black everywhere
        None => {
            let mut m = Map::new();
            m.insert(
                "pbrMetallicRoughness".into(),
                json!({"baseColorFactor": [0.0, 0.0, 0.0, 1.0], "metallicFactor": 0.0}),
            );
            m
        }
    };
    host.insert("name".into(), Value::String(w.name_of(e, "material")));
    host.remove("extras");
    if let Some(extras) = w.extras(Object::Material(e))? {
        host.insert("extras".into(), Value::Object(extras));
    }
    let mut ext = Map::new();
    // version 2: per-pixel channels or the over composition; 3: the height composition; 4: a power channel. An
    // older consumer must not draw it as the version it knows; 5: a product channel
    let power = layered
        .weights
        .channels
        .iter()
        .flatten()
        .any(|c| matches!(c, Channel::Power { .. }));
    let product = layered
        .weights
        .channels
        .iter()
        .flatten()
        .any(|c| matches!(c, Channel::Product { .. }));
    match layered.weights.composition {
        _ if product => {
            ext.insert("version".into(), json!(5));
        }
        _ if power => {
            ext.insert("version".into(), json!(4));
        }
        Composition::Height { .. } => {
            ext.insert("version".into(), json!(3));
        }
        _ if layered.weights.per_pixel() => {
            ext.insert("version".into(), json!(2));
        }
        _ => {}
    }
    ext.insert("weights".into(), Value::Object(weights));
    match layered.weights.composition {
        Composition::Over => {
            ext.insert("composition".into(), json!("over"));
        }
        Composition::Height { contrast } => {
            let c = w.factors(e, &[contrast])?[0].clone();
            ext.insert("composition".into(), json!({"height": {"contrast": c}}));
        }
        Composition::Sum => {}
    }
    ext.insert("layers".into(), Value::Array(layers));
    host.insert("extensions".into(), json!({ LAYERED: Value::Object(ext) }));
    lock(counts).layered_materials += 1;
    Ok(WholeMaterial {
        material: host,
        extension: LAYERED,
        sets,
        attribute: Some(attribute),
        components: &[LayeredMaterial::NAME],
    })
}

/// A texture slot of `PRIMASE_materials_detail_blend`: its texture info and `colorSpace` (the texture's own, else
/// sRGB for a colour slot), or `None` where its texture could not be written; adds its set to `sets`.
fn detail_slot(
    w: &mut MaterialWriter<'_, '_>,
    r: TextureRef,
    colour: bool,
    sets: &mut BTreeSet<u8>,
) -> Result<Option<Value>> {
    let Some(Value::Object(mut info)) = w.texture_info(r, sets)? else {
        return Ok(None);
    };
    let srgb = w
        .world()
        .get::<Texture>(r.texture)
        .map_or(colour, |t| t.color_space == ColorSpace::Srgb);
    info.insert("colorSpace".into(), json!(if srgb { "srgb" } else { "linear" }));
    Ok(Some(Value::Object(info)))
}

/// A `pbr.detail` as `PRIMASE_materials_detail_blend` ; `Err` with why where a texture of it
/// could not be written (the material is then written without it, its `pbr.material` alone).
fn detail_blend(
    w: &mut MaterialWriter<'_, '_>,
    e: Entity,
    d: &DetailedMaterial,
    sets: &mut BTreeSet<u8>,
) -> Result<std::result::Result<Value, String>> {
    let DetailMix::DetailLerp(m) = &d.mix;
    let mut written = BTreeSet::new();
    // the slots in the order they are written, each a texture and whether it is a colour
    let mut textures = vec![
        (m.base_colour, true),
        (m.base_normal, false),
        (m.base_mask, false),
    ];
    for detail in &m.details {
        textures.extend([(detail.colour, true), (detail.normal, false)]);
    }
    for weight in &m.weights {
        if let Source::Mask { texture, .. } = weight.source {
            textures.push((texture, false));
        }
    }
    let mut infos = Vec::with_capacity(textures.len());
    for (r, colour) in textures {
        match detail_slot(w, r, colour, &mut written)? {
            Some(info) => infos.push(info),
            None => return Ok(Err(format!("texture {:?} could not be written", r.texture))),
        }
    }
    let mut infos = infos.into_iter();
    let mut next = || infos.next().expect("one info a slot");
    let mut x = Map::new();
    x.insert("revision".into(), json!(primase_core::detail::REVISION));
    x.insert("equation".into(), json!("detail_lerp"));
    x.insert(
        "lowering".into(),
        json!(match d.lowering {
            SurfaceLowering::GlossReflectance { .. } => "gloss_reflectance",
            SurfaceLowering::MetallicGloss => "metallic_gloss",
        }),
    );
    x.insert("baseColorTexture".into(), next());
    x.insert("baseNormalTexture".into(), next());
    x.insert("baseMaskTexture".into(), next());
    let mut details = Vec::new();
    for detail in &m.details {
        let (colour, normal) = (next(), next());
        let gloss_scale = match detail.gloss_scale {
            GlossScale::BaseNormalZ => json!({"baseNormalZ": {}}),
            GlossScale::Constant(c) => json!({"constant": w.factors(e, &[c])?[0].clone()}),
        };
        let reflectance = match detail.reflectance {
            ReflectanceTerm::MaskPlusBias(b) => {
                json!({"maskPlusBias": w.factors(e, &[b])?[0].clone()})
            }
            ReflectanceTerm::Bias(b) => json!({"bias": w.factors(e, &[b])?[0].clone()}),
        };
        let mut o = Map::new();
        o.insert("colorTexture".into(), colour);
        o.insert("normalTexture".into(), normal);
        o.insert("tint".into(), w.factors(e, &detail.tint)?);
        o.insert("glossBias".into(), w.factors(e, &[detail.gloss_bias])?[0].clone());
        o.insert("glossScale".into(), gloss_scale);
        o.insert("reflectance".into(), reflectance);
        details.push(Value::Object(o));
    }
    x.insert("details".into(), Value::Array(details));
    let mut weights = Vec::new();
    for weight in &m.weights {
        let source = match &weight.source {
            Source::Mask { channels, bias, .. } => json!({"mask": {
                "texture": next(),
                "channels": w.factors(e, channels)?,
                "bias": w.factors(e, &[*bias])?[0].clone(),
            }}),
            vertex @ Source::Vertex { .. } => layer_source(w, e, vertex, &mut written)?,
        };
        let mut steps = Vec::new();
        for step in &weight.steps {
            steps.push(match *step {
                WeightStep::Power(n) => json!({"power": n}),
                WeightStep::Min(b) => json!({"min": w.factors(e, &[b])?[0].clone()}),
                WeightStep::Max(b) => json!({"max": w.factors(e, &[b])?[0].clone()}),
                WeightStep::Affine { scale, bias } => json!({"affine": {
                    "scale": w.factors(e, &[scale])?[0].clone(),
                    "bias": w.factors(e, &[bias])?[0].clone(),
                }}),
                WeightStep::Saturate => json!({"saturate": {}}),
            });
        }
        weights.push(json!({"source": source, "steps": steps}));
    }
    x.insert("weights".into(), Value::Array(weights));
    x.insert("gain".into(), w.factors(e, &[m.gain])?[0].clone());
    x.insert(
        "normalStrength".into(),
        w.factors(e, &[m.normal_strength])?[0].clone(),
    );
    if let SurfaceLowering::GlossReflectance { dielectric_f0 } = d.lowering {
        x.insert("dielectricF0".into(), w.factors(e, &[dielectric_f0])?[0].clone());
    }
    if m.base_saturation != 0.0 {
        x.insert(
            "baseSaturation".into(),
            w.factors(e, &[m.base_saturation])?[0].clone(),
        );
    }
    x.insert("color".into(), w.factors(e, &m.colour)?);
    let encoding = m.gloss_encoding;
    x.insert(
        "glossEncoding".into(),
        json!({
            "power": encoding.power,
            "root": encoding.root,
            "scale": w.factors(e, &[f32::from_bits(encoding.scale_bits)])?[0].clone(),
        }),
    );
    if let Some(j) = m.alpha {
        x.insert("alphaDetail".into(), json!(j));
    }
    let mut frozen = Vec::new();
    for f in &d.frozen {
        frozen.push(match f {
            Frozen::FrontFacing => json!({"frontFacing": {}}),
            Frozen::VertexInput {
                semantic,
                index,
                value,
            } => json!({"vertexInput": {
                "semantic": semantic, "index": index, "value": w.factors(e, value)?,
            }}),
            Frozen::EngineConstant { name, value } => json!({"engineConstant": {
                "name": name, "value": w.factors(e, value)?,
            }}),
            Frozen::EngineTexture { name, value } => json!({"engineTexture": {
                "name": name, "value": w.factors(e, value)?,
            }}),
        });
    }
    x.insert("frozen".into(), Value::Array(frozen));
    sets.extend(written);
    Ok(Ok(Value::Object(x)))
}

/// A weight or height source as `PRIMASE_layered_material` writes it.
fn layer_source(
    w: &mut MaterialWriter<'_, '_>,
    e: Entity,
    s: &Source,
    sets: &mut BTreeSet<u8>,
) -> Result<Value> {
    Ok(match s {
        Source::Vertex { semantic, channel } => {
            let attribute = match semantic {
                Semantic::Color(n) => format!("COLOR_{n}"),
                Semantic::TexCoord(n) => format!("TEXCOORD_{n}"),
                other => {
                    return Err(Error::World {
                        entity: e,
                        detail: format!("a weight channel reads {other:?}: COLOR_n or TEXCOORD_n only"),
                    });
                }
            };
            json!({"vertex": {"attribute": attribute, "channel": channel}})
        }
        Source::Mask {
            texture,
            channels,
            bias,
        } => {
            let info = w
                .texture_info(*texture, sets)?
                .ok_or_else(|| Error::World {
                    entity: e,
                    detail: format!(
                        "mask texture {:?} could not be written (not loaded, or a format an 8-bit PNG cannot hold \
                         exactly: lower-textures-rgba8)",
                        texture.texture
                    ),
                })?;
            json!({"mask": {
                "texture": info,
                "channels": w.factors(e, channels)?,
                "bias": w.factors(e, &[*bias])?[0].clone(),
            }})
        }
    })
}

/// A per-pixel weight channel as `PRIMASE_layered_material` writes it.
fn layer_channel(
    w: &mut MaterialWriter<'_, '_>,
    e: Entity,
    c: &Channel,
    sets: &mut BTreeSet<u8>,
) -> Result<Value> {
    let mut source = |s: &Source| layer_source(w, e, s, sets);
    Ok(match c {
        Channel::Source(s) => json!({"source": source(s)?}),
        Channel::Lerp { a, b, t } => json!({"lerp": {"a": source(a)?, "b": source(b)?, "t": source(t)?}}),
        Channel::Threshold { v, m, k } => {
            let (v, m) = (source(v)?, source(m)?);
            json!({"threshold": {"v": v, "m": m, "k": w.factors(e, k)?}})
        }
        Channel::Power { v, exponent } => json!({"power": {"v": source(v)?, "exponent": exponent}}),
        Channel::Product { sources } => {
            let factors = sources.iter().map(source).collect::<Result<Vec<_>>>()?;
            json!({"product": factors})
        }
    })
}
