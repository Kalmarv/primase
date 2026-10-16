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

use crate::{Error, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

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
    fn byte_cap(self) -> usize {
        if self == Self::Mask { 98_304 } else { 360_448 }
    }
}

/// Cache final compressed images across tiles, never decoded source pixels.
#[derive(Default)]
pub(super) struct TextureEncoder {
    cache: BTreeMap<CacheKey, (Vec<u8>, Value)>,
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
    pub(super) fn new(caps: TextureCaps, caps_by_name: Vec<(String, TextureCaps)>) -> Result<Self> {
        caps.check()?;
        for (_, caps) in &caps_by_name {
            caps.check()?;
        }
        Ok(Self {
            cache: BTreeMap::new(),
            caps,
            caps_by_name,
        })
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

    pub(super) fn apply(&mut self, bytes: Vec<u8>) -> Result<Vec<u8>> {
        let chunks = primase_gltf::glb::parse(&bytes)?;
        let mut doc: Value = serde_json::from_slice(chunks.json)?;
        let Some(textures) = doc.get("textures").and_then(Value::as_array).cloned() else {
            return Ok(bytes);
        };
        let source = chunks.bin.ok_or_else(|| invalid("textured GLB has no BIN"))?;
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
                for (pointer, role) in [
                    ("/pbrMetallicRoughness/baseColorTexture/index", Role::Colour),
                    ("/emissiveTexture/index", Role::Colour),
                    ("/normalTexture/index", Role::Normal),
                    ("/pbrMetallicRoughness/metallicRoughnessTexture/index", Role::Mask),
                    ("/occlusionTexture/index", Role::Mask),
                ] {
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
                let digest: [u8; 32] = Sha256::digest(png).into();
                let cap = self.caps_for(image["name"].as_str()).get(role);
                let cache_key = (digest, role, alpha_policy, cap);
                if let std::collections::btree_map::Entry::Vacant(entry) = self.cache.entry(cache_key.clone())
                {
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
                            let (accepted, report) = alpha_quality(&compressed, &rgba, &cache_key.2)?;
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
                    entry.insert((encoded, metadata));
                }
                let (encoded, metadata) = &self.cache[&cache_key];
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
                bin.extend_from_slice(encoded);
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
        Ok(primase_gltf::glb::assemble(&serde_json::to_vec(&doc)?, &bin)?)
    }
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
        let output = encoder.apply(bytes.clone()).unwrap();
        assert_eq!(output, encoder.apply(bytes).unwrap());
        assert_eq!(encoder.cache.len(), 3);
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
        let mut encoder = TextureEncoder::new(TextureCaps::default(), vec![("bake_".into(), small)]).unwrap();
        let output = encoder.apply(input).unwrap();
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
        assert_eq!(encoder.cache.len(), 3);
        assert!(
            TextureEncoder::new(
                TextureCaps {
                    colour: 300,
                    ..TextureCaps::default()
                },
                Vec::new()
            )
            .is_err()
        );
        assert!(
            TextureEncoder::new(
                TextureCaps::default(),
                vec![("x".into(), TextureCaps { mask: 2, ..small })]
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
        let output = TextureEncoder::default().apply(input).unwrap();
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
