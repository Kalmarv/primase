//! The glTF reader's textures: the images a material's texture slots sample, read as `core.texture`, and the
//! `gltf.material_textures` component that says which texture entity each slot of a `gltf.material` samples (with
//! the sampler it states). [`crate::lower`] turns the two into `pbr.material`.
//!
//! - **Which images.** Those a core material slot (`baseColorTexture`, `metallicRoughnessTexture`, `normalTexture`,
//!   `occlusionTexture`, `emissiveTexture`) samples through `textures[i].source`; the others are counted
//!   ([`ReadReport::images_unused`]). An image is read once however many slots sample it.
//! - **Usage.** glTF gives an image no colour space or role of its own: its use does (§3.9.4 and §3.9.1). A slot's
//!   texture entity is the image read for that use: sRGB `Color` for base colour and emissive, linear `Mask` for the
//!   metallic-roughness and occlusion slots (one entity when a packed map serves both), linear `Normal` in glTF's
//!   convention for the normal slot. An image used in two ways is two entities.
//! - **Formats.** Told by the bytes, not the declared MIME type: PNG is decoded (`primase_image::read_png`, 8 and 16
//!   bits, to RGBA); JPEG, KTX2/Basis, WebP and anything else is refused by name, counted in
//!   [`ReadReport::images_refused`], and its slots stay without a texture entry.
//! - **Sources.** A GLB's `bufferView` images and, through the caller's resolver, `uri` images (a relative file, a
//!   `data:` URI: resolving is the caller's, as for buffers).

use std::collections::BTreeMap;

use primase_core::pbr::{Filters, Wrap};
use primase_core::{
    ColorSpace, Component, Entity, Geometry, Name, NormalConvention, Source, TexFormat, TexRole, Texture,
    World,
};
use primase_glb::Gltf;
use primase_glb::sampler_filters;
use primase_image::{Samples, read_png};
use serde::Serialize;
use serde_json::Value;

/// Fetches an image file by its `uri` as the document writes it.
pub type ImageResolver<'a> = dyn FnMut(&str) -> Result<Vec<u8>, String> + 'a;

/// A core texture slot of a glTF material.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum GltfSlot {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
}

impl GltfSlot {
    /// Every slot, in glTF's order.
    pub const ALL: [GltfSlot; 5] = [
        GltfSlot::BaseColor,
        GltfSlot::MetallicRoughness,
        GltfSlot::Normal,
        GltfSlot::Occlusion,
        GltfSlot::Emissive,
    ];

    /// The slot's texture info object in a material, if the material has one.
    pub fn info(self, material: &Value) -> Option<&Value> {
        let pbr = || material.get("pbrMetallicRoughness");
        match self {
            GltfSlot::BaseColor => pbr()?.get("baseColorTexture"),
            GltfSlot::MetallicRoughness => pbr()?.get("metallicRoughnessTexture"),
            GltfSlot::Normal => material.get("normalTexture"),
            GltfSlot::Occlusion => material.get("occlusionTexture"),
            GltfSlot::Emissive => material.get("emissiveTexture"),
        }
    }

    fn usage(self) -> Usage {
        match self {
            GltfSlot::BaseColor | GltfSlot::Emissive => Usage::Colour,
            GltfSlot::MetallicRoughness | GltfSlot::Occlusion => Usage::Mask,
            GltfSlot::Normal => Usage::Normal,
        }
    }
}

/// How an image is used: its colour space and role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Usage {
    Colour,
    Mask,
    Normal,
}

/// The texture a slot samples and the sampler it states.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SlotTexture {
    pub texture: Entity,
    pub filters: Filters,
    /// Wrap along u, then v.
    pub wrap: [Wrap; 2],
}

/// The textures of a `gltf.material`'s slots that were read (a slot whose image was refused has no entry).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GltfMaterialTextures(pub BTreeMap<GltfSlot, SlotTexture>);

impl Component for GltfMaterialTextures {
    const NAME: &'static str = "gltf.material_textures";
    const GEOMETRY: Geometry = Geometry::None;
    fn to_json(&self) -> Result<Option<Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.0.values().map(|t| t.texture));
    }
}

/// What reading the images did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ImageCounts {
    pub read: usize,
    pub refused: BTreeMap<String, usize>,
    pub unused: usize,
}

/// A decoded image.
struct Decoded {
    width: u32,
    height: u32,
    format: TexFormat,
    bytes: Vec<u8>,
}

/// Read the images the materials' slots sample and attach `gltf.material_textures` to each material that has any.
pub(crate) fn read_textures(
    gltf: &Gltf,
    source: &str,
    w: &mut World,
    materials: &BTreeMap<usize, Entity>,
    resolve: &mut ImageResolver,
) -> ImageCounts {
    let json = gltf.json();
    let list = |key: &str| {
        json.get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    };
    let (materials_json, textures, images, samplers) = (
        list("materials"),
        list("textures"),
        list("images"),
        list("samplers"),
    );
    let mut counts = ImageCounts::default();
    // images by index: decoded once, or the reason it was refused
    let mut decoded: BTreeMap<usize, Result<Decoded, String>> = BTreeMap::new();
    let mut entities: BTreeMap<(usize, Usage), Entity> = BTreeMap::new();
    let mut sampled = std::collections::BTreeSet::new();
    for (&m, &material) in materials {
        let mut slots = BTreeMap::new();
        for slot in GltfSlot::ALL {
            let Some(info) = slot.info(&materials_json[m]) else {
                continue;
            };
            let Some(t) = info
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|i| textures.get(i as usize))
            else {
                continue;
            };
            // the image: `source`, else an extension's (KHR_texture_basisu, EXT_texture_webp, ...), refused by name
            let Some(i) = t.get("source").and_then(Value::as_u64).map(|i| i as usize) else {
                let by = t
                    .get("extensions")
                    .and_then(Value::as_object)
                    .map(|x| x.keys().cloned().collect::<Vec<_>>().join("+"))
                    .filter(|x| !x.is_empty())
                    .unwrap_or_else(|| "no source".to_owned());
                *counts
                    .refused
                    .entry(format!("texture whose image is not `source` ({by})"))
                    .or_default() += 1;
                continue;
            };
            let Some(image) = images.get(i) else { continue };
            sampled.insert(i);
            let pixels = decoded.entry(i).or_insert_with(|| decode(gltf, image, resolve));
            let Ok(pixels) = pixels else { continue };
            let usage = slot.usage();
            let texture = *entities.entry((i, usage)).or_insert_with(|| {
                let e = w.spawn();
                let (color_space, role, normal) = match usage {
                    Usage::Colour => (ColorSpace::Srgb, TexRole::Color, None),
                    Usage::Mask => (ColorSpace::Linear, TexRole::Mask, None),
                    Usage::Normal => (ColorSpace::Linear, TexRole::Normal, Some(NormalConvention::GLTF)),
                };
                w.insert(
                    e,
                    Texture {
                        width: pixels.width,
                        height: pixels.height,
                        format: pixels.format,
                        levels: vec![pixels.bytes.clone()],
                        color_space,
                        role,
                        normal,
                    },
                );
                let name = image
                    .get("name")
                    .and_then(Value::as_str)
                    .map_or_else(|| format!("image {i}"), str::to_owned);
                w.insert(e, Name(name));
                w.insert(e, Source(format!("{source}#/images/{i}")));
                e
            });
            let (filters, wrap) = t
                .get("sampler")
                .and_then(Value::as_u64)
                .and_then(|s| samplers.get(s as usize))
                .and_then(|s| sampler_filters(s).ok())
                .unwrap_or((Filters::default(), [Wrap::Repeat; 2]));
            slots.insert(
                slot,
                SlotTexture {
                    texture,
                    filters,
                    wrap,
                },
            );
        }
        if !slots.is_empty() {
            w.insert(material, GltfMaterialTextures(slots));
        }
    }
    for d in decoded.values() {
        match d {
            Ok(_) => counts.read += 1,
            Err(reason) => *counts.refused.entry(reason.clone()).or_default() += 1,
        }
    }
    counts.unused = images.len().saturating_sub(sampled.len());
    counts
}

/// An image's bytes (its buffer view or its `uri`, through the resolver) decoded, or why it is refused.
fn decode(gltf: &Gltf, image: &Value, resolve: &mut ImageResolver) -> Result<Decoded, String> {
    let bytes = match (
        image.get("bufferView").and_then(Value::as_u64),
        image.get("uri").and_then(Value::as_str),
    ) {
        (Some(view), _) => gltf
            .buffer_view(view as usize)
            .map_err(|e| format!("image bytes: {e}"))?
            .to_vec(),
        (None, Some(uri)) => resolve(uri).map_err(|e| format!("image file not read: {e}"))?,
        (None, None) => return Err("image with neither a bufferView nor a uri".to_owned()),
    };
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let png = read_png(&bytes).map_err(|e| format!("PNG not decoded: {e}"))?;
        let image = png.image;
        let (format, bytes) = match image.samples {
            Samples::U8(rgba) => (TexFormat::Rgba8Unorm, rgba),
            Samples::U16(rgba) => (
                TexFormat::Rgba16Unorm,
                rgba.iter().flat_map(|s| s.to_le_bytes()).collect(),
            ),
        };
        return Ok(Decoded {
            width: image.width,
            height: image.height,
            format,
            bytes,
        });
    }
    Err(if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "JPEG (no JPEG decoder in the toolkit)".to_owned()
    } else if bytes.starts_with(b"\xABKTX 20\xBB\r\n\x1A\n") {
        "KTX2 (KTX2/Basis not read)".to_owned()
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "WebP (not read)".to_owned()
    } else {
        "image of a type not recognised by its bytes".to_owned()
    })
}
