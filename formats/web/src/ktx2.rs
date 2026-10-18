//! Optional UASTC LDR 4x4 encoding with Zstandard supercompression.
//!
//! Settings are fixed and threaded encoding is disabled. Callers provide the
//! final web-resolution pixels; this function does not silently resize them.
use basisu_c_sys::{
    BasisTextureFormat, common,
    extra::{
        BasisuEncoder, BasisuEncoderParams, SourceImage, SourceImageFormat, basisu_encoder_init,
        types::Extent3d,
    },
};
use primase_gltf::glb::Document;

use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Colour,
    Normal,
    Mask,
}

/// Largest web texture edge per role, in pixels. Each image is reduced to the
/// largest power of two not above its own size or the cap (minimum 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextureCaps {
    pub colour: u32,
    pub normal: u32,
    pub mask: u32,
}

impl Default for TextureCaps {
    fn default() -> Self {
        Self {
            colour: 512,
            normal: 512,
            mask: 256,
        }
    }
}

impl TextureCaps {
    fn get(&self, role: Role) -> u32 {
        match role {
            Role::Colour => self.colour,
            Role::Normal => self.normal,
            Role::Mask => self.mask,
        }
    }
    fn check(&self) -> Result<()> {
        for cap in [self.colour, self.normal, self.mask] {
            if cap < 4 || !cap.is_power_of_two() {
                return Err(invalid(format!(
                    "texture cap {cap} must be a power of two of at least 4"
                )));
            }
        }
        Ok(())
    }
}

impl Role {
    /// The role's name in disk cache keys.
    fn key(self) -> &'static str {
        match self {
            Role::Colour => "colour",
            Role::Normal => "normal",
            Role::Mask => "mask",
        }
    }
    fn byte_cap(self) -> usize {
        if self == Self::Mask { 98_304 } else { 360_448 }
    }
}

/// The default bound of [`TextureEncoder`]'s encodings held in memory, 512 MiB.
pub const DEFAULT_ENCODING_MEMORY: usize = 512 << 20;

/// Encodings held in memory, so a texture several tiles use is encoded once: at most `cap` bytes, the least recently
/// used dropped first (encoded again, or read from the disk cache, when it comes back; counted).
struct Memory {
    entries: BTreeMap<CacheKey, (Vec<u8>, Value, u64)>,
    /// The entries by last use.
    order: BTreeMap<u64, CacheKey>,
    tick: u64,
    bytes: usize,
    cap: usize,
    dropped: u64,
}

impl Default for Memory {
    fn default() -> Self {
        Memory {
            entries: BTreeMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            cap: DEFAULT_ENCODING_MEMORY,
            dropped: 0,
        }
    }
}

impl Memory {
    fn get(&mut self, key: &CacheKey) -> Option<(Vec<u8>, Value)> {
        let (encoded, metadata, used) = self.entries.get_mut(key)?;
        self.order.remove(used);
        self.tick += 1;
        *used = self.tick;
        self.order.insert(self.tick, key.clone());
        Some((encoded.clone(), metadata.clone()))
    }
    fn put(&mut self, key: CacheKey, encoded: Vec<u8>, metadata: Value) {
        self.tick += 1;
        self.bytes += encoded.len();
        self.order.insert(self.tick, key.clone());
        if let Some((old, _, used)) = self.entries.insert(key, (encoded, metadata, self.tick)) {
            self.bytes -= old.len();
            self.order.remove(&used);
        }
        while self.bytes > self.cap {
            let Some((_, k)) = self.order.pop_first() else {
                break;
            };
            if let Some((e, _, _)) = self.entries.remove(&k) {
                self.bytes -= e.len();
                self.dropped += 1;
            }
        }
    }
}

/// Cache final compressed images across tiles, never decoded source pixels, in memory up to a bound; and across runs
/// in `disk`, when given.
#[derive(Default)]
pub(super) struct TextureEncoder {
    memory: Memory,
    disk: Option<Arc<primase_cache::Cache>>,
    /// Encode on every disk hit as well and fail when the stored value differs (the cache's gate mode).
    verify_disk: bool,
    caps: TextureCaps,
    caps_by_name: Vec<(String, TextureCaps)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct AlphaPolicy {
    // Float bit patterns preserve each distinct consumer threshold in cache identity.
    cutoffs: BTreeSet<(u32, u32)>,
    blend: bool,
}

type CacheKey = ([u8; 32], Role, AlphaPolicy, u32);

fn invalid(message: impl ToString) -> Error {
    Error::Invalid(message.to_string())
}

fn png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    primase_image::write_png(&primase_image::Image {
        width,
        height,
        samples: primase_image::Samples::U8(rgba.to_vec()),
    })
    .map_err(invalid)
}

fn alpha_quality(encoded: &[u8], reference: &[u8], policy: &AlphaPolicy) -> Result<(bool, Value)> {
    if !policy.blend && policy.cutoffs.is_empty() {
        return Ok((true, json!(null)));
    }
    use basisu_c_sys::{
        TranscodeTargetFormat,
        extra::{BasisuTranscoder, ChannelType, SupportedTextureCompression, basisu_transcoder_init},
    };
    basisu_transcoder_init();
    let decoder = BasisuTranscoder::new(encoded, SupportedTextureCompression::empty(), ChannelType::Rgba)
        .map_err(invalid)?;
    let decoded = decoder
        .transcode(Some(TranscodeTargetFormat::RGBA32), None)
        .map_err(invalid)?;
    alpha_pixels(reference, &decoded.data[..reference.len()], policy)
}

fn alpha_pixels(reference: &[u8], decoded: &[u8], policy: &AlphaPolicy) -> Result<(bool, Value)> {
    if reference.len() != decoded.len() || reference.is_empty() || !reference.len().is_multiple_of(4) {
        return Err(invalid("alpha comparison requires equal RGBA8 images"));
    }
    let pixels = reference.len() / 4;
    let mut histogram = [0usize; 256];
    let mut cutouts = Vec::new();
    let mut accepted = true;
    for &(cutoff, factor) in &policy.cutoffs {
        let (cutoff, factor) = (f32::from_bits(cutoff), f32::from_bits(factor));
        let flips = reference
            .as_chunks::<4>()
            .0
            .iter()
            .zip(decoded.as_chunks::<4>().0.iter())
            .filter(|(a, b)| {
                (f32::from(a[3]) / 255. * factor < cutoff) != (f32::from(b[3]) / 255. * factor < cutoff)
            })
            .count();
        accepted &= (flips as u64) * 1000 < pixels as u64;
        cutouts
            .push(json!({"cutoff":cutoff,"factor":factor,"flips":flips,"share":flips as f64/pixels as f64}));
    }
    for (a, b) in reference
        .as_chunks::<4>()
        .0
        .iter()
        .zip(decoded.as_chunks::<4>().0.iter())
    {
        histogram[a[3].abs_diff(b[3]) as usize] += 1;
    }
    let mut count = 0;
    let p99 = histogram
        .iter()
        .position(|n| {
            count += n;
            count >= (pixels * 99).div_ceil(100)
        })
        .expect("nonempty pixels");
    if policy.blend {
        accepted &= p99 <= 8;
    }
    Ok((
        accepted,
        json!({"pixels":pixels,"cutout":cutouts,"blend":policy.blend,"alpha_p99":p99,"accepted":accepted}),
    ))
}

impl TextureEncoder {
    pub(super) fn new(
        caps: TextureCaps,
        caps_by_name: Vec<(String, TextureCaps)>,
        disk: Option<Arc<primase_cache::Cache>>,
        verify_disk: bool,
    ) -> Result<Self> {
        caps.check()?;
        for (_, caps) in &caps_by_name {
            caps.check()?;
        }
        Ok(Self {
            memory: Memory::default(),
            disk,
            verify_disk,
            caps,
            caps_by_name,
        })
    }

    /// At most `bytes` of encodings held in memory ([`DEFAULT_ENCODING_MEMORY`] by default).
    pub(super) fn with_memory(mut self, bytes: usize) -> Self {
        self.memory.cap = bytes;
        self
    }

    /// Encodings dropped from memory to stay within its bound, so far.
    pub(super) fn dropped_from_memory(&self) -> u64 {
        self.memory.dropped
    }

    /// [`encode_image`] through the disk cache, when there is one.
    fn encode_cached(&self, png: &[u8], key: &CacheKey) -> Result<(Vec<u8>, Value)> {
        let (_, role, policy, cap) = key;
        let Some(disk) = &self.disk else {
            return encode_image(png, *role, policy, *cap);
        };
        let disk_key = disk_key(png, *role, policy, *cap);
        if let Some(value) = disk.get(&disk_key)
            && let Some(found) = unframe(&value)
        {
            if self.verify_disk && encode_image(png, *role, policy, *cap)? != found {
                return Err(invalid(format!(
                    "KTX2 cache entry {} differs from encoding its input again",
                    disk_key.hex()
                )));
            }
            return Ok(found);
        }
        let (encoded, metadata) = encode_image(png, *role, policy, *cap)?;
        // a value that cannot be stored is still the right value; the cache counts the failure
        let _ = disk.put(&disk_key, &frame(&encoded, &metadata)?);
        Ok((encoded, metadata))
    }

    /// The caps for an image: the first name-prefix rule that matches, else the defaults.
    fn caps_for(&self, name: Option<&str>) -> TextureCaps {
        name.and_then(|name| {
            self.caps_by_name
                .iter()
                .find(|(prefix, _)| name.starts_with(prefix.as_str()))
        })
        .map_or(self.caps, |(_, caps)| *caps)
    }

    pub(super) fn apply(&mut self, input: Document) -> Result<Document> {
        let Some(textures) = input.json.get("textures").and_then(Value::as_array).cloned() else {
            return Ok(input);
        };
        let Document {
            json: mut doc,
            bin: source,
        } = input;
        if source.is_empty() {
            return Err(invalid("textured GLB has no BIN"));
        }
        let source = &source[..];
        let mut roles = BTreeMap::<usize, Role>::new();
        // A texture shared between different roles needs separate transfer/mip semantics.
        // Split each material binding deterministically before assigning image sources.
        let mut texture_variants = BTreeMap::<(usize, Role), usize>::new();
        let mut new_textures = Vec::new();
        let mut alpha_policies = BTreeMap::<usize, AlphaPolicy>::new();
        if let Some(materials) = doc["materials"].as_array_mut() {
            for material in materials {
                let alpha_mode = material["alphaMode"].as_str().unwrap_or("OPAQUE").to_owned();
                let factor = material["pbrMetallicRoughness"]["baseColorFactor"][3]
                    .as_f64()
                    .unwrap_or(1.) as f32;
                let cutoff = material["alphaCutoff"].as_f64().unwrap_or(0.5) as f32;
                // PRIMASE_layered_material version 2: every mask a weight channel samples is linear data
                let mut masks = Vec::new();
                let channels = "/extensions/PRIMASE_layered_material/weights/channels";
                for (i, c) in material
                    .pointer(channels)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    for (form, parts) in [
                        ("source", &[""][..]),
                        ("lerp", &["/a", "/b", "/t"][..]),
                        ("threshold", &["/v", "/m"][..]),
                    ] {
                        if c.get(form).is_some() {
                            for part in parts {
                                masks.push(format!("{channels}/{i}/{form}{part}/mask/texture/index"));
                            }
                        }
                    }
                }
                for (pointer, role) in [
                    ("/pbrMetallicRoughness/baseColorTexture/index", Role::Colour),
                    ("/emissiveTexture/index", Role::Colour),
                    ("/normalTexture/index", Role::Normal),
                    ("/pbrMetallicRoughness/metallicRoughnessTexture/index", Role::Mask),
                    ("/occlusionTexture/index", Role::Mask),
                    // KHR_materials_specular: the strength in A (linear), the colour sRGB
                    (
                        "/extensions/KHR_materials_specular/specularTexture/index",
                        Role::Mask,
                    ),
                    (
                        "/extensions/KHR_materials_specular/specularColorTexture/index",
                        Role::Colour,
                    ),
                ]
                .map(|(p, r)| (p.to_owned(), r))
                .into_iter()
                .chain(masks.into_iter().map(|p| (p, Role::Mask)))
                {
                    let pointer = pointer.as_str();
                    if let Some(binding) = material.pointer_mut(pointer) {
                        let original = binding.as_u64().ok_or_else(|| invalid("texture index"))? as usize;
                        let index = *texture_variants.entry((original, role)).or_insert_with(|| {
                            let n = new_textures.len();
                            new_textures.push(textures[original].clone());
                            roles.insert(n, role);
                            n
                        });
                        if pointer == "/pbrMetallicRoughness/baseColorTexture/index" {
                            let policy = alpha_policies.entry(index).or_default();
                            policy.blend |= alpha_mode == "BLEND";
                            if alpha_mode == "MASK" && factor > 0. {
                                policy.cutoffs.insert((cutoff.to_bits(), factor.to_bits()));
                            }
                        }
                        *binding = json!(index);
                    }
                }
            }
        }
        let old_images = doc["images"]
            .as_array()
            .ok_or_else(|| invalid("texture images missing"))?
            .clone();
        let image_views: BTreeSet<_> = old_images
            .iter()
            .filter_map(|i| i["bufferView"].as_u64())
            .map(|n| n as usize)
            .collect();
        let old_views = doc["bufferViews"]
            .as_array()
            .ok_or_else(|| invalid("buffer views missing"))?
            .clone();
        let mut bin = Vec::new();
        let mut views = Vec::new();
        let mut remap = BTreeMap::new();
        for (i, view) in old_views.iter().enumerate() {
            if image_views.contains(&i) {
                continue;
            }
            let mut view = view.clone();
            let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
            let len = view["byteLength"]
                .as_u64()
                .ok_or_else(|| invalid("buffer length"))? as usize;
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            view["byteOffset"] = json!(bin.len());
            bin.extend_from_slice(
                source
                    .get(start..start + len)
                    .ok_or_else(|| invalid("buffer range"))?,
            );
            remap.insert(i, views.len());
            views.push(view);
        }
        if let Some(accessors) = doc["accessors"].as_array_mut() {
            for accessor in accessors {
                if let Some(view) = accessor.get_mut("bufferView") {
                    *view = json!(remap[&(view.as_u64().ok_or_else(|| invalid("accessor view"))? as usize)]);
                }
            }
        }
        let mut images = Vec::new();
        let mut image_variants = BTreeMap::new();
        for (t, texture) in new_textures.iter_mut().enumerate() {
            let original = texture["source"]
                .as_u64()
                .ok_or_else(|| invalid("texture source"))? as usize;
            let role = roles[&t];
            let alpha_policy = alpha_policies.get(&t).cloned().unwrap_or_default();
            let key = (original, role, alpha_policy.clone());
            let image_index = if let Some(&i) = image_variants.get(&key) {
                i
            } else {
                let image = &old_images[original];
                let view = &old_views[image["bufferView"]
                    .as_u64()
                    .ok_or_else(|| invalid("image view"))? as usize];
                let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
                let len = view["byteLength"]
                    .as_u64()
                    .ok_or_else(|| invalid("image bytes"))? as usize;
                let png = source
                    .get(start..start + len)
                    .ok_or_else(|| invalid("image range"))?;
                if image["extras"]["primase.role"] == "data" {
                    // numbers a shader reads exactly: kept as stored, whole and lossless, whatever slot binds them
                    while !bin.len().is_multiple_of(4) {
                        bin.push(0);
                    }
                    let index = images.len();
                    let mut image = image.clone();
                    image["bufferView"] = json!(views.len());
                    image["extras"]["primase.web_texture"] =
                        json!({"role": "data", "codec": "PNG_AS_STORED"});
                    views.push(json!({"buffer":0,"byteOffset":bin.len(),"byteLength":png.len()}));
                    bin.extend_from_slice(png);
                    images.push(image);
                    image_variants.insert(key, index);
                    texture["source"] = json!(index);
                    continue;
                }
                let digest: [u8; 32] = Sha256::digest(png).into();
                let cap = self.caps_for(image["name"].as_str()).get(role);
                let cache_key = (digest, role, alpha_policy, cap);
                let (encoded, metadata) = match self.memory.get(&cache_key) {
                    Some(held) => held,
                    None => {
                        let (encoded, metadata) = self.encode_cached(png, &cache_key)?;
                        self.memory
                            .put(cache_key.clone(), encoded.clone(), metadata.clone());
                        (encoded, metadata)
                    }
                };
                while !bin.len().is_multiple_of(4) {
                    bin.push(0);
                }
                let index = images.len();
                let mut image = image.clone();
                image["mimeType"] = json!(if metadata["codec"] == "PNG_RGBA8" {
                    "image/png"
                } else {
                    "image/ktx2"
                });
                image["bufferView"] = json!(views.len());
                image["extras"]["primase.web_texture"] = metadata.clone();
                views.push(json!({"buffer":0,"byteOffset":bin.len(),"byteLength":encoded.len()}));
                bin.extend_from_slice(&encoded);
                images.push(image);
                image_variants.insert(key, index);
                index
            };
            if images[image_index]["mimeType"] == "image/png" {
                texture["source"] = json!(image_index);
            } else {
                texture
                    .as_object_mut()
                    .expect("writer texture object")
                    .remove("source");
                texture["extensions"]["KHR_texture_basisu"] = json!({"source":image_index});
            }
        }
        doc["textures"] = json!(new_textures);
        doc["images"] = json!(images);
        doc["bufferViews"] = json!(views);
        doc["buffers"][0]["byteLength"] = json!(bin.len());
        if images.iter().any(|i| i["mimeType"] == "image/ktx2") {
            for field in ["extensionsUsed", "extensionsRequired"] {
                if doc.get(field).is_none() {
                    doc[field] = json!([]);
                }
                let extensions = doc[field].as_array_mut().expect("writer extensions array");
                if !extensions.iter().any(|e| e == "KHR_texture_basisu") {
                    extensions.push(json!("KHR_texture_basisu"));
                }
            }
        }
        Ok(Document::new(doc, bin))
    }
}

/// What the disk cache keys KTX2 output by, besides the inputs: bumped by hand when the output's meaning changes in a
/// way the sources below do not show.
const CACHE_VERSION: u32 = 1;

/// External crates whose code computes the output (the UASTC encoder and transcoder, PNG and its deflate stack), as
/// they are in Cargo.lock; `encoders_match_the_lock` keeps this list true.
const ENCODERS: &str = "adler2@2.0.1 basisu_c_sys@0.9.1 bitflags@2.13.2 cfg-if@1.0.5 crc32fast@1.5.2 fdeflate@0.3.7 \
flate2@1.1.10 miniz_oxide@0.8.9 miniz_oxide@0.9.1 png@0.18.1 simd-adler32@0.3.10 zlib-rs@0.6.8";

/// The hash of the code that computes KTX2 output: this module and primase-image.
fn source_hash() -> &'static [u8; 32] {
    static HASH: OnceLock<[u8; 32]> = OnceLock::new();
    HASH.get_or_init(|| {
        let mut files = vec![("web/ktx2.rs", include_bytes!("ktx2.rs").as_slice())];
        files.extend(primase_image::CACHE_SOURCES.iter().copied());
        primase_cache::source_hash(&files)
    })
}

/// The disk cache key of one image's web form: everything [`encode_image`] reads, and what computes it.
fn disk_key(png: &[u8], role: Role, policy: &AlphaPolicy, cap: u32) -> primase_cache::Key {
    let mut k = primase_cache::KeyBuilder::new("web.ktx2")
        .u32("version", CACHE_VERSION)
        .bytes("sources", source_hash())
        .str("encoders", ENCODERS)
        .bytes("png", png)
        .str("role", role.key())
        .u32("cap", cap)
        .bool("blend", policy.blend)
        .u64("cutoffs", policy.cutoffs.len() as u64);
    for (cutoff, factor) in &policy.cutoffs {
        k = k.u32("cutoff", *cutoff).u32("factor", *factor);
    }
    k.finish()
}

/// A cached value: the metadata JSON's length (u64 LE), the JSON, the encoded image.
fn frame(encoded: &[u8], metadata: &Value) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(metadata)?;
    let mut out = Vec::with_capacity(8 + json.len() + encoded.len());
    out.extend_from_slice(&(json.len() as u64).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(encoded);
    Ok(out)
}

/// [`frame`] undone; `None` for bytes it did not make (recomputed, never trusted).
fn unframe(value: &[u8]) -> Option<(Vec<u8>, Value)> {
    let n = u64::from_le_bytes(value.get(..8)?.try_into().ok()?);
    let end = 8usize.checked_add(usize::try_from(n).ok()?)?;
    let metadata = serde_json::from_slice(value.get(8..end)?).ok()?;
    Some((value[end..].to_vec(), metadata))
}

/// One image's web form: the PNG decoded, reduced to the role's cap, encoded to UASTC (halving until it fits the
/// role's byte cap) or kept as PNG where the encoding would lose too much alpha, with the metadata the tile records.
/// Pure: the same PNG, role, policy and cap give the same bytes (what the disk cache relies on).
fn encode_image(png: &[u8], role: Role, policy: &AlphaPolicy, cap: u32) -> Result<(Vec<u8>, Value)> {
    let image = primase_image::read_png(png).map_err(invalid)?.image;
    let original_size = [image.width, image.height];
    let mut width = image.width;
    let mut height = image.height;
    let primase_image::Samples::U8(mut rgba) = image.samples else {
        return Err(invalid("web KTX2 requires RGBA8"));
    };
    let dimension = |v: u32| (1 << v.ilog2()).clamp(4, cap);
    let (w, h) = (dimension(width), dimension(height));
    let mut near_zero = 0;
    if width != w || height != h {
        if role == Role::Normal {
            let filtered = primase_image::resize_normals(
                &primase_image::Image {
                    width,
                    height,
                    samples: primase_image::Samples::U8(rgba),
                },
                w,
                h,
            )
            .map_err(invalid)?;
            near_zero = filtered.near_zero_to_flat;
            let primase_image::Samples::U8(pixels) = filtered.image.samples else {
                unreachable!()
            };
            (width, height, rgba) = (w, h, pixels);
        } else {
            (width, height, rgba) = resize(width, height, &rgba, role, w, h);
        }
    }
    let mut alpha_report = json!(null);
    let mut png_output = role == Role::Normal;
    let encoded = if png_output {
        png_rgba(width, height, &rgba)?
    } else {
        let compressed = loop {
            let encoded = encode_rgba8(width, height, &rgba, role == Role::Colour)?;
            if encoded.len() <= role.byte_cap() {
                break encoded;
            }
            if width == 4 && height == 4 {
                return Err(invalid("KTX2 exceeds byte cap"));
            }
            (width, height, rgba) = resize(
                width,
                height,
                &rgba,
                role,
                (width / 2).max(4),
                (height / 2).max(4),
            );
        };
        if role == Role::Colour {
            let (accepted, report) = alpha_quality(&compressed, &rgba, policy)?;
            alpha_report = report;
            png_output = !accepted;
        }
        if png_output {
            png_rgba(width, height, &rgba)?
        } else {
            compressed
        }
    };
    let metadata = json!({"source_dimensions":original_size,"dimensions":[width,height],"role":format!("{role:?}").to_lowercase(),"max_dimension":cap,"max_bytes":if png_output { None } else { Some(role.byte_cap()) },"bytes":encoded.len(),"codec":if png_output { "PNG_RGBA8" } else { "UASTC_LDR_4x4_ZSTD" },"near_zero_cap_average_to_flat":near_zero,"alpha_quality":alpha_report});
    Ok((encoded, metadata))
}

// Integer-bin averaging includes every source texel, including odd edge rows.
// Colour filtering is linear-light; normal vectors are renormalized afterward.
fn resize(width: u32, height: u32, rgba: &[u8], role: Role, w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0f64; 4];
            let mut count = 0.;
            for sy in y * height / h..((y + 1) * height / h).max(y * height / h + 1) {
                for sx in x * width / w..((x + 1) * width / w).max(x * width / w + 1) {
                    for c in 0..4 {
                        let mut v = rgba[((sy * width + sx) * 4) as usize + c] as f64 / 255.;
                        if role == Role::Colour && c < 3 {
                            v = primase_image::srgb_to_linear(v);
                        }
                        sum[c] += v;
                    }
                    count += 1.;
                }
            }
            for v in &mut sum {
                *v /= count;
            }
            for (c, mut v) in sum.into_iter().enumerate() {
                if role == Role::Colour && c < 3 {
                    v = primase_image::linear_to_srgb(v);
                }
                out.push((v * 255.).round().clamp(0., 255.) as u8);
            }
        }
    }
    (w, h, out)
}

/// Encode a single RGBA8 image with a complete, wrapping mip chain.
/// Colour images use sRGB; masks and normals must pass `srgb = false`.
pub fn encode_rgba8(width: u32, height: u32, rgba: &[u8], srgb: bool) -> Result<Vec<u8>> {
    let size = width.checked_mul(height).and_then(|n| n.checked_mul(4));
    if width == 0
        || height == 0
        || !width.is_multiple_of(4)
        || !height.is_multiple_of(4)
        || size.map(u64::from) != Some(rgba.len() as u64)
    {
        return Err(Error::Invalid(
            "glTF KTX2 requires tightly packed RGBA8 with dimensions divisible by four".into(),
        ));
    }
    basisu_encoder_init();
    let mut encoder = BasisuEncoder::new();
    encoder
        .set_image(SourceImage {
            data: rgba,
            format: SourceImageFormat::Rgba8,
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        })
        .map_err(|e| Error::Invalid(format!("KTX2 input: {e}")))?;
    let format = BasisTextureFormat::UastcLdr4x4;
    let mut params = if srgb {
        BasisuEncoderParams::new_with_srgb_defaults(format)
    } else {
        BasisuEncoderParams::new_with_linear_defaults(format)
    }
    .with_removed_flags(common::BU_COMP_FLAGS_THREADED)
    .with_flags(common::BU_COMP_FLAGS_GEN_MIPS_WRAP);
    params.quality_level = 100; // Native UASTC quality 100 disables RDO.
    let mut encoded = encoder
        .compress(params)
        .map_err(|e| Error::Invalid(format!("KTX2 encoding: {e}")))?;
    // KHR_texture_basisu requires unspecified primaries for non-colour data.
    // The native encoder's DFD template always defaults to BT709.
    let dfd = u32::from_le_bytes(
        encoded
            .get(48..52)
            .ok_or_else(|| invalid("KTX2 DFD offset"))?
            .try_into()
            .expect("four bytes"),
    ) as usize;
    *encoded
        .get_mut(dfd + 13)
        .ok_or_else(|| invalid("KTX2 DFD primaries"))? = u8::from(srgb);
    Ok(encoded)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// One PNG bound as colour, mask and normal: three encodings.
    fn three_roles() -> Vec<u8> {
        let pixels: Vec<u8> = (0..64 * 32 * 4).map(|i| (i * 37 % 251) as u8).collect();
        let png = primase_image::write_png(&primase_image::Image {
            width: 64,
            height: 32,
            samples: primase_image::Samples::U8(pixels),
        })
        .unwrap();
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":png.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png"}],"textures":[{"source":0}],
            "materials":[{"alphaMode":"MASK","pbrMetallicRoughness":{"baseColorTexture":{"index":0},"metallicRoughnessTexture":{"index":0}},"normalTexture":{"index":0}}]});
        primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &png).unwrap()
    }

    #[test]
    #[allow(
        clippy::disallowed_methods,
        clippy::disallowed_types,
        reason = "test I/O: damages a cache entry"
    )]
    fn disk_cache_gives_the_bytes_encoding_gives() {
        let dir = std::env::temp_dir().join(format!("primase-ktx2-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let input = three_roles();
        let plain = glb(&mut TextureEncoder::default(), input.clone()).unwrap();
        let run = || {
            let cache = Arc::new(primase_cache::Cache::open(&dir, 1 << 30).unwrap());
            let mut encoder =
                TextureEncoder::new(TextureCaps::default(), Vec::new(), Some(cache.clone()), true).unwrap();
            (glb(&mut encoder, input.clone()).unwrap(), cache.stats())
        };
        let (cold, stats) = run();
        assert_eq!(cold, plain);
        assert_eq!((stats.hits, stats.misses, stats.written), (0, 3, 3));
        let (warm, stats) = run();
        assert_eq!(warm, plain);
        assert_eq!((stats.hits, stats.misses), (3, 0));
        // a damaged entry is recomputed, not used
        let shard = std::fs::read_dir(dir.join("v1"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let entry = std::fs::read_dir(shard).unwrap().next().unwrap().unwrap().path();
        let mut bytes = std::fs::read(&entry).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x40;
        std::fs::write(&entry, bytes).unwrap();
        let (repaired, stats) = run();
        assert_eq!(repaired, plain);
        assert_eq!((stats.hits, stats.corrupt, stats.written), (2, 1, 1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A memory bound smaller than one encoding keeps nothing between tiles: the same bytes, each image encoded
    /// again, the drops counted; the default bound keeps them.
    #[test]
    fn a_memory_bound_changes_no_byte() {
        let input = three_roles();
        let mut held = TextureEncoder::default();
        let mut bounded = TextureEncoder::default().with_memory(1);
        for _ in 0..2 {
            assert_eq!(
                glb(&mut bounded, input.clone()).unwrap(),
                glb(&mut held, input.clone()).unwrap()
            );
        }
        assert_eq!(held.dropped_from_memory(), 0);
        // three encodings (colour, mask, normal), dropped as made, made again on the second pass
        assert_eq!(bounded.dropped_from_memory(), 6);
    }

    #[test]
    fn disk_keys_follow_every_input() {
        let policy = AlphaPolicy::default();
        let k = disk_key(b"png", Role::Colour, &policy, 512);
        let blend = AlphaPolicy {
            blend: true,
            ..AlphaPolicy::default()
        };
        let cut = AlphaPolicy {
            cutoffs: BTreeSet::from([(0.5f32.to_bits(), 1f32.to_bits())]),
            ..AlphaPolicy::default()
        };
        for other in [
            disk_key(b"pnh", Role::Colour, &policy, 512),
            disk_key(b"png", Role::Mask, &policy, 512),
            disk_key(b"png", Role::Colour, &policy, 256),
            disk_key(b"png", Role::Colour, &blend, 512),
            disk_key(b"png", Role::Colour, &cut, 512),
        ] {
            assert_ne!(other, k);
        }
    }

    /// [`ENCODERS`] is basisu_c_sys and png with everything png depends on, at their Cargo.lock versions.
    #[test]
    #[allow(clippy::disallowed_methods, reason = "test I/O: reads Cargo.lock")]
    fn encoders_match_the_lock() {
        let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
        let mut packages = BTreeMap::<String, (String, Vec<String>)>::new();
        for block in lock.split("[[package]]").skip(1) {
            let field = |name: &str| {
                block
                    .lines()
                    .find_map(|l| l.strip_prefix(&format!("{name} = \"")))
                    .map(|v| v.trim_end_matches('"').to_owned())
            };
            let (Some(name), Some(version)) = (field("name"), field("version")) else {
                continue;
            };
            let deps = block
                .split_once("dependencies = [")
                .map(|(_, rest)| rest.split(']').next().unwrap())
                .unwrap_or("")
                .split(',')
                .map(|d| d.trim().trim_matches('"').to_owned())
                .filter(|d| !d.is_empty())
                .collect();
            packages.insert(format!("{name}@{version}"), (name, deps));
        }
        // a dependency is `name` or `name version` (when several versions are locked)
        let resolve = |dep: &str| -> String {
            match dep.split_once(' ') {
                Some((n, v)) => format!("{n}@{v}"),
                None => packages.iter().find(|(_, (n, _))| n == dep).unwrap().0.clone(),
            }
        };
        let mut want = BTreeSet::from([resolve("basisu_c_sys")]);
        let mut todo = vec![resolve("png")];
        while let Some(p) = todo.pop() {
            if want.insert(p.clone()) {
                todo.extend(packages[&p].1.iter().map(|d| resolve(d)));
            }
        }
        let have: BTreeSet<String> = ENCODERS.split_whitespace().map(str::to_owned).collect();
        assert_eq!(have, want);
    }

    #[test]
    fn data_images_are_kept_as_stored() {
        let pixels = [200, 180, 160, 17].repeat(4 * 4);
        let png = primase_image::write_png(&primase_image::Image {
            width: 4,
            height: 4,
            samples: primase_image::Samples::U8(pixels),
        })
        .unwrap();
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":png.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png","extras":{"primase.role":"data"}}],"textures":[{"source":0}],
            "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}]});
        let bytes = primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &png).unwrap();
        let output = glb(&mut TextureEncoder::default(), bytes).unwrap();
        let chunks = primase_gltf::glb::parse(&output).unwrap();
        let doc: Value = serde_json::from_slice(chunks.json).unwrap();
        let t = doc["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["index"]
            .as_u64()
            .unwrap() as usize;
        let image = doc["textures"][t]["source"].as_u64().unwrap() as usize;
        assert_eq!(doc["images"][image]["mimeType"], "image/png");
        assert_eq!(
            doc["images"][image]["extras"]["primase.web_texture"]["role"],
            "data"
        );
        let view = &doc["bufferViews"][doc["images"][image]["bufferView"].as_u64().unwrap() as usize];
        let start = view["byteOffset"].as_u64().unwrap() as usize;
        let bin = chunks.bin.unwrap();
        assert_eq!(&bin[start..start + png.len()], &png[..], "the stored bytes");
        assert!(
            doc.get("extensionsUsed").is_none_or(|u| !u
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e == "KHR_texture_basisu"))
        );
    }

    /// The encoder over GLB bytes, as a tile's stages see them.
    fn glb(encoder: &mut TextureEncoder, bytes: Vec<u8>) -> Result<Vec<u8>> {
        encoder
            .apply(Document::parse(&bytes)?)?
            .to_glb()
            .map_err(Into::into)
    }

    #[test]
    fn specular_textures_are_a_mask_and_a_colour() {
        let pixels = [200, 180, 160, 17].repeat(4 * 4);
        let png = primase_image::write_png(&primase_image::Image {
            width: 4,
            height: 4,
            samples: primase_image::Samples::U8(pixels),
        })
        .unwrap();
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":png.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png"}],"textures":[{"source":0}],
            "materials":[{"extensions":{"KHR_materials_specular":{
                "specularTexture":{"index":0},"specularColorTexture":{"index":0}}}}]});
        let bytes = primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &png).unwrap();
        let output = glb(&mut TextureEncoder::default(), bytes).unwrap();
        let chunks = primase_gltf::glb::parse(&output).unwrap();
        let doc: Value = serde_json::from_slice(chunks.json).unwrap();
        let ext = &doc["materials"][0]["extensions"]["KHR_materials_specular"];
        let role = |slot: &str| {
            let t = ext[slot]["index"].as_u64().unwrap() as usize;
            let image = doc["textures"][t]["extensions"]["KHR_texture_basisu"]["source"]
                .as_u64()
                .unwrap() as usize;
            doc["images"][image]["extras"]["primase.web_texture"]["role"].clone()
        };
        assert_eq!(role("specularTexture"), "mask");
        assert_eq!(role("specularColorTexture"), "colour");
    }

    #[test]
    fn layer_channel_masks_are_masks() {
        let pixels = [200, 180, 160, 17].repeat(4 * 4);
        let png = primase_image::write_png(&primase_image::Image {
            width: 4,
            height: 4,
            samples: primase_image::Samples::U8(pixels),
        })
        .unwrap();
        let mask = json!({"mask":{"texture":{"index":0},"channels":[1,0,0,0],"bias":0}});
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":png.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png"}],"textures":[{"source":0}],
            "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}},
                "extensions":{"PRIMASE_layered_material":{"version":2,"weights":{"attribute":"COLOR_1",
                "mapping":"identity","channels":[{"source":mask},{"threshold":{"v":{"vertex":{"attribute":"COLOR_0",
                "channel":0}},"m":mask,"k":[0,1,1,0]}}]},"layers":[{"material":0},{"material":0}]}}}]});
        let bytes = primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &png).unwrap();
        let output = glb(&mut TextureEncoder::default(), bytes).unwrap();
        let chunks = primase_gltf::glb::parse(&output).unwrap();
        let doc: Value = serde_json::from_slice(chunks.json).unwrap();
        let role = |pointer: &str| {
            let t = doc["materials"][0].pointer(pointer).unwrap().as_u64().unwrap() as usize;
            let image = doc["textures"][t]["extensions"]["KHR_texture_basisu"]["source"]
                .as_u64()
                .unwrap() as usize;
            doc["images"][image]["extras"]["primase.web_texture"]["role"].clone()
        };
        let channels = "/extensions/PRIMASE_layered_material/weights/channels";
        assert_eq!(role("/pbrMetallicRoughness/baseColorTexture/index"), "colour");
        assert_eq!(role(&format!("{channels}/0/source/mask/texture/index")), "mask");
        assert_eq!(
            role(&format!("{channels}/1/threshold/m/mask/texture/index")),
            "mask"
        );
    }

    #[test]
    fn shared_image_roles_caps_and_geometry_preservation() {
        let pixels = [128, 128, 255, 255].repeat(513 * 3);
        let png = primase_image::write_png(&primase_image::Image {
            width: 513,
            height: 3,
            samples: primase_image::Samples::U8(pixels),
        })
        .unwrap();
        let mut bin = vec![0u8; 12];
        bin.extend_from_slice(&png);
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":bin.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":12},{"buffer":0,"byteOffset":12,"byteLength":png.len()}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":1,"type":"VEC3"}],
            "images":[{"bufferView":1,"mimeType":"image/png"}],"textures":[{"source":0}],
            "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0},"metallicRoughnessTexture":{"index":0}},"normalTexture":{"index":0}}]});
        let bytes = primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &bin).unwrap();
        let mut encoder = TextureEncoder::default();
        let output = glb(&mut encoder, bytes.clone()).unwrap();
        assert_eq!(output, glb(&mut encoder, bytes).unwrap());
        assert_eq!(encoder.memory.entries.len(), 3);
        let chunks = primase_gltf::glb::parse(&output).unwrap();
        let doc: Value = serde_json::from_slice(chunks.json).unwrap();
        assert_eq!(doc["images"].as_array().unwrap().len(), 3);
        assert_eq!(&chunks.bin.unwrap()[..12], &[0; 12]);
        for (i, image) in doc["images"].as_array().unwrap().iter().enumerate() {
            let view = &doc["bufferViews"][image["bufferView"].as_u64().unwrap() as usize];
            let start = view["byteOffset"].as_u64().unwrap() as usize;
            let len = view["byteLength"].as_u64().unwrap() as usize;
            let bytes = &chunks.bin.unwrap()[start..start + len];
            if i == 1 {
                assert_eq!(image["mimeType"], "image/png");
                assert_eq!(doc["textures"][i]["source"], i);
                let png = primase_image::read_png(bytes).unwrap().image;
                assert_eq!((png.width, png.height), (512, 4));
                assert_eq!(
                    image["extras"]["primase.web_texture"]["near_zero_cap_average_to_flat"],
                    0
                );
                continue;
            }
            let read = |n| u32::from_le_bytes(bytes[n..n + 4].try_into().unwrap());
            assert_eq!(read(20), if i == 2 { 256 } else { 512 });
            assert_eq!(read(24), 4);
            assert_eq!(read(44), 2); // Zstandard
            let dfd = read(48) as usize;
            assert_eq!(bytes[dfd + 13], u8::from(i == 0));
            assert_eq!(bytes[dfd + 14], if i == 0 { 2 } else { 1 });
            let decoder = basisu::Transcoder::new(bytes).unwrap();
            let pixels = decoder
                .transcode(0, basisu::TargetFormat::Rgba32, basisu::DecodeFlags::NONE)
                .unwrap();
            assert_eq!(pixels.len(), (read(20) * read(24) * 4) as usize);
            assert!(pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        }
    }

    #[test]
    fn name_prefix_caps_apply_only_to_matching_images() {
        // The same pixels under two names: the cap is part of the cache identity.
        let png = png_rgba(64, 64, &[90, 120, 200, 255].repeat(64 * 64)).unwrap();
        let mut bin = png.clone();
        bin.extend_from_slice(&png);
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":bin.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()},{"buffer":0,"byteOffset":png.len(),"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png","name":"bake_wall"},{"bufferView":1,"mimeType":"image/png","name":"wall"}],
            "textures":[{"source":0},{"source":1}],
            "materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}},"normalTexture":{"index":0}},{"pbrMetallicRoughness":{"baseColorTexture":{"index":1}}}]});
        let input = primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &bin).unwrap();
        let small = TextureCaps {
            colour: 16,
            normal: 8,
            mask: 4,
        };
        let mut encoder =
            TextureEncoder::new(TextureCaps::default(), vec![("bake_".into(), small)], None, false).unwrap();
        let output = glb(&mut encoder, input).unwrap();
        let doc: Value = serde_json::from_slice(primase_gltf::glb::parse(&output).unwrap().json).unwrap();
        let sizes: Vec<_> = doc["images"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let t = &i["extras"]["primase.web_texture"];
                (
                    i["name"].as_str().unwrap().to_owned(),
                    t["role"].as_str().unwrap().to_owned(),
                    t["dimensions"].clone(),
                    t["max_dimension"].clone(),
                )
            })
            .collect();
        assert_eq!(
            sizes,
            [
                ("bake_wall".into(), "colour".into(), json!([16, 16]), json!(16)),
                ("bake_wall".into(), "normal".into(), json!([8, 8]), json!(8)),
                ("wall".into(), "colour".into(), json!([64, 64]), json!(512)),
            ]
        );
        assert_eq!(encoder.memory.entries.len(), 3);
        assert!(
            TextureEncoder::new(
                TextureCaps {
                    colour: 300,
                    ..TextureCaps::default()
                },
                Vec::new(),
                None,
                false
            )
            .is_err()
        );
        assert!(
            TextureEncoder::new(
                TextureCaps::default(),
                vec![("x".into(), TextureCaps { mask: 2, ..small })],
                None,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn rejected_blend_uses_png_and_native_alpha_matches_independent_decoder() {
        let mut seed = 17u32;
        let pixels: Vec<u8> = (0..16 * 16 * 4)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let encoded = encode_rgba8(16, 16, &pixels, true).unwrap();
        let independent = basisu::Transcoder::new(&encoded)
            .unwrap()
            .transcode(0, basisu::TargetFormat::Rgba32, basisu::DecodeFlags::NONE)
            .unwrap();
        let policy = AlphaPolicy {
            blend: true,
            ..AlphaPolicy::default()
        };
        let expected = alpha_pixels(&pixels, &independent, &policy).unwrap();
        assert!(!expected.0, "fixture must exceed blend alpha bound");
        assert_eq!(alpha_quality(&encoded, &pixels, &policy).unwrap(), expected);
        let png = png_rgba(16, 16, &pixels).unwrap();
        let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":png.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":png.len()}],
            "images":[{"bufferView":0,"mimeType":"image/png"}],"textures":[{"source":0}],
            "materials":[{"alphaMode":"BLEND","pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}]});
        let input = primase_gltf::glb::assemble(&serde_json::to_vec(&doc).unwrap(), &png).unwrap();
        let output = glb(&mut TextureEncoder::default(), input).unwrap();
        let chunks = primase_gltf::glb::parse(&output).unwrap();
        let doc: Value = serde_json::from_slice(chunks.json).unwrap();
        assert_eq!(doc["images"][0]["mimeType"], "image/png");
        assert_eq!(doc["textures"][0]["source"], 0);
        assert!(doc.get("extensionsRequired").is_none());
        let len = doc["bufferViews"][0]["byteLength"].as_u64().unwrap() as usize;
        assert_eq!(
            primase_image::read_png(&chunks.bin.unwrap()[..len])
                .unwrap()
                .image
                .samples,
            primase_image::Samples::U8(pixels)
        );
    }

    #[test]
    fn alpha_acceptance_uses_cutoff_crossings_and_blend_percentiles() {
        let reference = [128, 128, 128, 128].repeat(1000);
        let mut decoded = reference.clone();
        decoded[3] = 127;
        let mask = AlphaPolicy {
            cutoffs: BTreeSet::from([(0.5f32.to_bits(), 1f32.to_bits())]),
            blend: false,
        };
        // Exactly 0.1% is refused: policy requires strictly below 0.1%.
        let (accepted, report) = alpha_pixels(&reference, &decoded, &mask).unwrap();
        assert!(!accepted);
        assert_eq!(report["cutout"][0]["flips"], 1);
        assert!(alpha_pixels(&reference, &reference, &mask).unwrap().0);
        let blend = AlphaPolicy {
            blend: true,
            ..AlphaPolicy::default()
        };
        assert!(
            alpha_pixels(&reference, &[128, 128, 128, 120].repeat(1000), &blend)
                .unwrap()
                .0
        );
        assert!(
            !alpha_pixels(&reference, &[128, 128, 128, 119].repeat(1000), &blend)
                .unwrap()
                .0
        );
        assert!(
            alpha_pixels(&reference, &decoded, &AlphaPolicy::default())
                .unwrap()
                .0
        );
    }
}
