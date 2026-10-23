//! Static source-material lowering. Unknown shader constants remain unknown and
//! are counted; this is not a reconstruction of dynamic wetness or light banks.
//! Source metadata stays attached. Existing baked PBR base/normal bindings win.
//! A metallic value without a traced source constant or map is dielectric 0,
//! never glTF's fully metallic default of 1, and every such material is counted.
mod gx;
use crate::preview::{Preview, is_emissive, preview};
use crate::{
    Result, invalid,
    materials::{Binding, Material},
    mtd::Value,
};
use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, DrawLayer, DrawOrder, Entity, MaterialUsage, Name, TexFormat, TexRole, Texture, World,
};
use primase_texcodec::TexDecoder;
use std::collections::{BTreeMap, BTreeSet};

/// Intrinsic perceptual roughness from the linear second G-buffer output's X.
/// GXLightAcc_DDirLight1 takes sqrt(gloss), applies 1 - 0.95*x, then squares
/// that value to obtain GGX alpha. Its light-dependent gloss multiplier is
/// deliberately omitted: one glTF material cannot vary roughness per light.
pub fn roughness(gloss: f32) -> Result<f32> {
    if !gloss.is_finite() || !(0.0..=1.0).contains(&gloss) {
        return Err(invalid("Sekiro PBR", 0, "gloss outside finite 0..1"));
    }
    Ok(1.0 - 0.95 * gloss.sqrt())
}

/// Metallic factor for a material with no traced source constant or map. The
/// world these shaders draw is overwhelmingly non-metal; metal must be proven.
pub(crate) const UNKNOWN_METALLIC: f32 = 0.;
pub(crate) const UNKNOWN_METALLIC_LOSS: &str = "metallic without source evidence lowered as dielectric 0";

/// The queue a decal class material is given: after the alpha-tested surfaces, before the blended ones. The decal
/// class is the source's (its `MeshDecal` shaders); the queue is the convention, as Sekiro's own pass sequence is not
/// read.
pub const DECAL_QUEUE: i32 = 2500;

/// What a source material class states about its surfaces: `water` (the tessellated water shaders) draws water,
/// `leaves` (the `M[Translucency]` shaders) vegetation, as `core.material_usage`; `decal` (the `MeshDecal` shaders)
/// draws over the surface beneath it, as `core.draw_order`'s decal layer with its conventional depth write. The other
/// classes state only their alpha mode, which is the material's.
pub fn class_usage(class: &str) -> (Option<MaterialUsage>, Option<DrawOrder>) {
    match class {
        "water" => (Some(MaterialUsage::Water), None),
        "leaves" => (Some(MaterialUsage::Foliage), None),
        "decal" => (None, Some(DrawOrder::conventional(DrawLayer::Decal, DECAL_QUEUE))),
        _ => (None, None),
    }
}

/// Each lowered source material's sidedness from the face sets drawing it (`fromsoft.face_cull`): single-sided
/// where every primitive drawing it culls back faces, two-sided where none does. A material drawn both ways is split:
/// the primitives that do not cull draw a copy named `<name>_two_sided`, two-sided, counted. A mesh drawing a lowered
/// material without `fromsoft.face_cull` is an error: sidedness is the source's, never a default.
pub fn apply_face_cull(world: &mut World, log: &mut PassLog) -> Result<()> {
    use crate::sekiro::source::FaceCull;
    // per lowered material, the (mesh, primitive) drawing it, by whether it culls
    let mut uses = BTreeMap::<Entity, [Vec<(Entity, usize)>; 2]>::new();
    for (e, mesh) in world.iter::<primase_core::Mesh>() {
        let cull = world.get::<FaceCull>(e);
        for (i, p) in mesh.primitives().iter().enumerate() {
            let Some(m) = p
                .material
                .filter(|&m| world.has::<Material>(m) && world.has::<PbrMaterial>(m))
            else {
                continue;
            };
            let c = cull.and_then(|c| c.primitives.get(i)).ok_or_else(|| {
                invalid(
                    "Sekiro sidedness",
                    i,
                    format!("{e:?} draws a FromSoft material without fromsoft.face_cull"),
                )
            })?;
            uses.entry(m).or_default()[usize::from(*c)].push((e, i));
        }
    }
    let sided = |world: &mut World, m: Entity, two: bool| {
        if let Some(p) = world.get_mut::<PbrMaterial>(m) {
            p.double_sided = two;
        }
    };
    for (m, [open, culled]) in uses {
        if open.is_empty() {
            sided(world, m, false);
            log.count("materials single-sided: their face sets cull back faces", 1);
        } else if culled.is_empty() {
            sided(world, m, true);
            log.count("materials two-sided: their face sets do not cull", 1);
        } else {
            sided(world, m, false);
            let copy = world
                .duplicate(m)
                .ok_or_else(|| invalid("Sekiro sidedness", 0, "material not alive"))?;
            sided(world, copy, true);
            let name = world.get::<Name>(m).map_or_else(String::new, |n| n.0.clone());
            world.insert(copy, Name(format!("{name}_two_sided")));
            for (e, i) in open {
                let mesh = world.get_mut::<primase_core::Mesh>(e).expect("listed mesh");
                *mesh.primitive_mut(i).expect("listed primitive").material_mut() = Some(copy);
            }
            log.count("materials split by face-set cull", 1);
        }
    }
    Ok(())
}

/// Pack a baked linear gloss plane as glTF ORM. R/B are neutral multipliers;
/// this function does not claim to have measured AO or metallic content.
pub(crate) fn orm_from_gloss(width: u32, height: u32, gloss: &[f32]) -> Result<Texture> {
    let n = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| n > 0)
        .ok_or_else(|| invalid("Sekiro PBR", 0, "invalid gloss dimensions"))?;
    if n != gloss.len() {
        return Err(invalid("Sekiro PBR", 0, "gloss plane length mismatch"));
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(
        n.checked_mul(4)
            .ok_or_else(|| invalid("Sekiro PBR", 0, "gloss size overflow"))?,
    )
    .map_err(|e| invalid("Sekiro PBR", 0, e))?;
    for &g in gloss {
        rgba.extend_from_slice(&[255, (roughness(g)? * 255.).round() as u8, 255, 255]);
    }
    Ok(Texture {
        width,
        height,
        format: TexFormat::Rgba8Unorm,
        levels: vec![rgba],
        color_space: ColorSpace::Linear,
        role: TexRole::Mask,
        normal: None,
    })
}

/// Encode linear baked emission as sRGB UNORM8 plus a global HDR strength.
/// The caller binds the texture with emissive factor [1; 3] and this strength.
/// The last value counts nonzero source texels quantized to black, not channels.
/// Source pre-exposure and vertex modulation policies belong to the bake log.
pub(crate) fn emissive_from_linear(width: u32, height: u32, rgb: &[[f32; 3]]) -> Result<(Texture, f32, u64)> {
    let n = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| n > 0 && n == rgb.len())
        .ok_or_else(|| invalid("Sekiro emission", 0, "invalid dimensions or plane length"))?;
    let mut strength = 1f32;
    for pixel in rgb {
        for &v in pixel {
            if !v.is_finite() || v < 0. {
                return Err(invalid("Sekiro emission", 0, "negative or nonfinite radiance"));
            }
            strength = strength.max(v);
        }
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(
        n.checked_mul(4)
            .ok_or_else(|| invalid("Sekiro emission", 0, "size overflow"))?,
    )
    .map_err(|e| invalid("Sekiro emission", 0, e))?;
    let mut zeroed = 0;
    for pixel in rgb {
        let encoded = pixel.map(|v| {
            (primase_image::linear_to_srgb(f64::from(v) / f64::from(strength)) * 255.).round() as u8
        });
        if pixel.iter().any(|&v| v > 0.) && encoded == [0; 3] {
            zeroed += 1;
        }
        rgba.extend_from_slice(&[encoded[0], encoded[1], encoded[2], 255]);
    }
    Ok((
        Texture {
            width,
            height,
            format: TexFormat::Rgba8Unorm,
            levels: vec![rgba],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
        strength,
        zeroed,
    ))
}

fn simple(shader: &str) -> bool {
    matches!(
        shader,
        "MetallicTex" | "MetallicValue" | "MetallicValue_MeshDecal"
    )
}
fn binding<'a>(m: &'a Material, role: &str) -> Option<&'a Binding> {
    let d = m.definition.as_ref()?;
    d.textures()
        .into_iter()
        .filter(|t| t.0.contains(role))
        .find_map(|t| m.bindings.get(t.0))
}

fn normal_binding<'a>(m: &'a Material, preview: &Preview) -> Option<&'a Binding> {
    let name = preview.normal.as_ref()?;
    m.definition
        .as_ref()?
        .textures()
        .into_iter()
        .filter(|t| t.0.contains("NormalMap"))
        .filter_map(|t| m.bindings.get(t.0))
        .find(|b| &b.texture == name)
}

/// Names required by the evidenced static policies, for one batched DDS read.
/// Layered base/normal textures must already be baked and bound by assembly.
pub fn required_textures(world: &World) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for (_, m) in world.iter::<Material>() {
        if m.skip.is_some() || m.layered.is_some() {
            continue;
        }
        let preview = preview(m);
        names.extend(preview.albedo);
        names.extend(preview.normal);
        if simple(&m.shader)
            && let Some(b) = binding(m, "MetallicMap")
        {
            names.insert(b.texture.clone());
        }
    }
    Ok(names)
}

fn read(payloads: &BTreeMap<String, impl AsRef<[u8]>>, name: &str) -> Result<Texture> {
    let bytes = payloads
        .get(name)
        .ok_or_else(|| invalid("Sekiro PBR", 0, format!("missing DDS {name}")))?;
    primase_dds::read(bytes.as_ref()).map_err(|e| invalid("Sekiro PBR DDS", 0, e))
}
fn pixels(t: &Texture) -> Result<Vec<u8>> {
    let level = t
        .levels
        .first()
        .ok_or_else(|| invalid("Sekiro PBR", 0, "missing base mip"))?;
    primase_texcodec::Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, level)
        .map_err(|e| invalid("Sekiro PBR pixels", 0, e))
}
fn spawn(world: &mut World, name: String, t: Texture) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(name));
    world.insert(e, t);
    e
}
fn texture(
    world: &mut World,
    payloads: &BTreeMap<String, impl AsRef<[u8]>>,
    cache: &mut BTreeMap<(String, bool), Entity>,
    name: &str,
    normal: bool,
) -> Result<Entity> {
    let key = (name.to_owned(), normal);
    if let Some(e) = cache.get(&key) {
        return Ok(*e);
    }
    let mut t = read(payloads, name)?;
    t.role = if normal { TexRole::Normal } else { TexRole::Color };
    t.color_space = if normal {
        ColorSpace::Linear
    } else {
        ColorSpace::Srgb
    };
    t.normal = normal.then_some(super::normals::SOURCE);
    let e = spawn(world, texture_name(name, normal), t);
    cache.insert(key, e);
    Ok(e)
}
/// A source texture's identity in a role: the stem for colour, `<stem>@normal` for a normal map. The area reader
/// names its textures the same way, so lowering reuses them instead of decoding a second copy.
fn texture_name(stem: &str, normal: bool) -> String {
    if normal {
        format!("{stem}@normal")
    } else {
        stem.to_owned()
    }
}
/// Source textures already in the world, by stem and role.
fn existing_textures(world: &World) -> BTreeMap<(String, bool), Entity> {
    let mut found = BTreeMap::new();
    for (e, t) in world.iter::<Texture>() {
        let Some(Name(name)) = world.get::<Name>(e) else {
            continue;
        };
        let key = match t.role {
            TexRole::Normal => name.strip_suffix("@normal").map(|stem| (stem.to_owned(), true)),
            TexRole::Color if !name.ends_with("@normal") => Some((name.clone(), false)),
            _ => None,
        };
        if let Some(key) = key {
            found.entry(key).or_insert(e);
        }
    }
    found
}
/// The preview colour for a material with no base texture, as sRGB bytes: water's translucent blue-grey, else
/// mid grey. A named approximation, not a source value.
fn fallback_base_color(m: &Material) -> [f32; 4] {
    let bytes: [u8; 4] = if m.class == "water" {
        [48, 98, 122, 190]
    } else {
        [128, 128, 128, 255]
    };
    let mut c = [0.; 4];
    for i in 0..3 {
        c[i] = primase_image::srgb_to_linear(f64::from(bytes[i]) / 255.) as f32;
    }
    c[3] = (f64::from(bytes[3]) / 255.) as f32;
    c
}
/// A binding's texture transform for coordinates already multiplied by `applied` (the material's tiling a geometry
/// pass applied, `[1, 1]` when none was).
fn transform(applied: [f32; 2], b: &Binding) -> Result<UvTransform> {
    if b.uv_number != 1
        || !applied.iter().all(|v| v.is_finite() && *v > 0.)
        || !b.scale.iter().all(|v| v.is_finite())
    {
        return Err(invalid("Sekiro PBR", 0, "unsupported UV set or invalid scale"));
    }
    Ok(UvTransform {
        scale: std::array::from_fn(|i| b.scale[i] / applied[i]),
        ..UvTransform::IDENTITY
    })
}

/// What [`lower`] does with a layered material no pass baked (one without PBR bindings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layers {
    /// Leave it as source data (its `fromsoft.material`, layered definition and layer inputs), unlowered, counted:
    /// for a later bake or a writer that blends layers.
    Preserve,
    /// Lower it from its selected main-layer preview textures, a counted approximation.
    Preview,
}

/// Lower static source materials to core PBR without serializing through a
/// consumer dialect. A missing required DDS is an error. Baked variants must
/// carry PBR base/normal bindings; these are never replaced by source layers.
/// Approximation units are material entities unless a counter says pixels.
///
/// `layers` says what happens to a layered material no pass baked; `None` refuses one (the program must choose).
/// Materials of a skipped source class ([`Material::skip`]) are not lowered, counted. Every normal map's shader
/// basis is checked against [`super::normals::consumer_basis`]; an assumed or unclassified basis is counted.
pub fn lower(
    world: &mut World,
    payloads: &BTreeMap<String, impl AsRef<[u8]>>,
    layers: Option<Layers>,
) -> Result<PassLog> {
    lower_materials(world, payloads, layers, None).map(|(log, _)| log)
}

/// [`lower`] for the source materials some mesh primitive draws; the others (the slots of meshes a geometry pass
/// left out or recorded apart) stay as source data, counted. Also returns the layered materials lowered from their
/// main-layer preview (`layers=preview`).
pub fn lower_drawn(
    world: &mut World,
    payloads: &BTreeMap<String, impl AsRef<[u8]>>,
    layers: Option<Layers>,
) -> Result<(PassLog, BTreeSet<Entity>)> {
    let drawn: BTreeSet<Entity> = world
        .iter::<primase_core::Mesh>()
        .flat_map(|(_, m)| m.primitives().iter().filter_map(|p| p.material))
        .collect();
    let undrawn = world
        .iter::<Material>()
        .filter(|(e, _)| !drawn.contains(e))
        .count();
    let (mut log, previewed) = lower_materials(world, payloads, layers, Some(&drawn))?;
    if undrawn > 0 {
        log.count("source materials no primitive draws, not lowered", undrawn as u64);
    }
    Ok((log, previewed))
}

fn lower_materials(
    world: &mut World,
    payloads: &BTreeMap<String, impl AsRef<[u8]>>,
    layers: Option<Layers>,
    only: Option<&BTreeSet<Entity>>,
) -> Result<(PassLog, BTreeSet<Entity>)> {
    let sources: Vec<_> = world
        .iter::<Material>()
        .filter(|(e, _)| only.is_none_or(|o| o.contains(e)))
        .map(|(e, m)| (e, m.clone()))
        .collect();
    let mut log = PassLog::default();
    let mut previews = BTreeSet::new();
    let mut cache = existing_textures(world);
    for (e, m) in sources {
        if m.skip.is_some() {
            log.count("materials of a skipped source class not lowered", 1);
            continue;
        }
        let existing = world.get::<PbrMaterial>(e).cloned();
        let unbaked = m.layered.is_some() && existing.is_none();
        if unbaked {
            match layers {
                None => {
                    return Err(invalid(
                        "Sekiro PBR",
                        0,
                        "a layered material no pass baked: choose layers=preserve or layers=preview",
                    ));
                }
                Some(Layers::Preserve) => {
                    log.count("layered materials kept as source data (layers=preserve)", 1);
                    continue;
                }
                Some(Layers::Preview) => log.approximate(
                    "layered material lowered from its main-layer preview; layers not baked",
                    1,
                ),
            }
        }
        let pv = preview(&m);
        let previewed = unbaked;
        if previewed {
            previews.insert(e);
        }
        let baked = m.layered.is_some() && !previewed;
        let applied = world.get::<super::UvTiling>(e).map_or([1.; 2], |t| t.0);
        if pv.normal.is_some() {
            match super::normals::consumer_basis(&m.shader) {
                Ok(true) => {}
                Ok(false) => {
                    log.approximate("normal maps of shaders whose basis is assumed, not observed", 1)
                }
                Err(_) => log.approximate("normal maps of unclassified shaders: basis assumed", 1),
            }
        }
        // Only a fresh material lacks evidence; an existing factor came from a prior pass.
        let fresh = existing.is_none();
        let mut p = existing.unwrap_or(PbrMaterial {
            metallic: UNKNOWN_METALLIC,
            ..PbrMaterial::default()
        });
        let mut metal_map_used = false;
        let scalar = if !baked && p.metallic_roughness_texture.is_none() {
            gx::metallic(&m, &mut log)?
        } else {
            None
        };
        if let Some(value) = scalar {
            p.metallic = value;
            log.approximate("dielectric F0 0.03 represented by glTF 0.04", 1);
        }
        if !baked {
            if p.base_color_texture.is_none()
                && let Some(name) = &pv.albedo
            {
                p.base_color_texture = Some(TextureRef::new(texture(
                    world, payloads, &mut cache, name, false,
                )?));
            } else if fresh && pv.albedo.is_none() {
                p.base_color = fallback_base_color(&m);
                log.approximate("missing base texture: preview fallback colour", 1);
            }
            if p.normal_texture.is_none()
                && let Some(name) = &pv.normal
            {
                let mut r = TextureRef::new(texture(world, payloads, &mut cache, name, true)?);
                if let Some(b) = normal_binding(&m, &pv) {
                    r.transform = Some(transform(applied, b)?);
                }
                p.normal_texture = Some(r);
            }
            p.alpha_mode = match m.class.as_str() {
                "leaves" | "cutout" => AlphaMode::Mask { cutoff: m.cutoff },
                "alpha" | "decal" | "water" => AlphaMode::Blend,
                _ => AlphaMode::Opaque,
            };
        }
        if baked {
            log.approximate("layered PBR channels require shader bake outputs", 1);
        } else if simple(&m.shader) {
            if p.metallic_roughness_texture.is_some() {
                // Existing channels (including this pass's output) are authoritative.
                // Repacking would apply the already-normalized factors a second time.
                log.count("existing ORM textures preserved", 1);
            } else if let Some(name) = &pv.normal {
                let normal = read(payloads, name)?;
                let metal = binding(&m, "MetallicMap");
                let mut metal_sampler = None;
                if let Some(b) = metal {
                    let t = read(payloads, &b.texture)?;
                    metal_sampler = Some(crate::layer_bake::BaseMipBilinearWrap::new(
                        t.width,
                        t.height,
                        &pixels(&t)?,
                        false,
                    )?);
                }
                let normal_binding = normal_binding(&m, &pv)
                    .ok_or_else(|| invalid("Sekiro PBR", 0, "normal binding absent"))?;
                let nt = transform(applied, normal_binding)?;
                let mt = metal.map(|b| transform(applied, b)).transpose()?;
                if nt.scale.contains(&0.) {
                    return Err(invalid("Sekiro PBR", 0, "zero normal scale"));
                }
                let t = primase_image::map_rgba8_levels(
                    &normal,
                    0..1,
                    primase_image::TextureMetadata {
                        color_space: ColorSpace::Linear,
                        role: TexRole::Mask,
                        normal: None,
                    },
                    |px| {
                        let uv = [
                            (px.x as f32 + 0.5) / px.width as f32,
                            (px.y as f32 + 0.5) / px.height as f32,
                        ];
                        let metallic = if let (Some(s), Some(mt)) = (&metal_sampler, mt) {
                            s.sample(std::array::from_fn(|j| uv[j] * mt.scale[j] / nt.scale[j]))
                                .map_err(|e| primase_image::Error::Invalid(e.to_string()))?[0]
                        } else if scalar.is_some() {
                            1. // Preserve the source scalar exactly in the material factor.
                        } else {
                            p.metallic
                        };
                        let roughness = roughness(px.rgba[2] as f32 / 255.)
                            .map_err(|e| primase_image::Error::Invalid(e.to_string()))?;
                        Ok([
                            255,
                            (roughness * 255.).round() as u8,
                            (metallic * 255.).round() as u8,
                            255,
                        ])
                    },
                )
                .map_err(|e| invalid("Sekiro PBR ORM", 0, e))?;
                // named for the normal map and the material it was packed for: its identity, not its entity
                let te = spawn(world, format!("{}_orm_{}", name, m.identity_digest()?), t);
                let mut r = TextureRef::new(te);
                r.transform = Some(nt);
                p.metallic_roughness_texture = Some(r);
                p.roughness = 1.;
                p.metallic = if metal.is_none() { scalar.unwrap_or(1.) } else { 1. };
                log.count("roughness textures", 1);
                log.approximate("per-light gloss correction omitted", 1);
                log.approximate("roughness quantized to UNORM8", 1);
                if metal.is_some() {
                    metal_map_used = true;
                    log.count("metallic textures", 1);
                    log.approximate("dielectric F0 0.03 represented by glTF 0.04", 1);
                    log.approximate("metal resampled at normal base-mip texel centers", 1);
                }
            } else {
                log.approximate("missing normal gloss; retained target roughness", 1);
            }
        } else {
            log.approximate("unclassified shader PBR; retained target roughness", 1);
        }
        if !baked && fresh && scalar.is_none() && !metal_map_used {
            log.approximate(UNKNOWN_METALLIC_LOSS, 1);
        }
        if m.definition.is_none() {
            log.approximate("missing material definition", 1);
        }
        if p.emissive_texture.is_some() {
            log.count("existing emission textures preserved", 1);
        } else if is_emissive(&m, &pv) || m.shader.contains("Emissive") || pv.emissive.is_some() {
            log.approximate("source emission without proven radiance; retained dialect", 1);
        }
        // A static parameter may exist only in a subset of shader permutations;
        // retaining all definitions lets later policies establish its consumer.
        if m.definition
            .as_ref()
            .and_then(crate::materials::Definition::mtd)
            .is_some_and(|d| {
                d.parameters
                    .iter()
                    .any(|v| matches!(&v.value,Value::Float(x) if x.iter().any(|f|!f.is_finite())))
            })
        {
            return Err(invalid("Sekiro PBR", 0, "nonfinite source parameter"));
        }
        if !baked {
            world.insert(e, Name(pv.albedo.clone().unwrap_or_else(|| m.mtd.clone())));
        }
        world.insert(e, p);
        log.count("PBR materials", 1);
        let (usage, order) = class_usage(&m.class);
        if let Some(usage) = usage {
            world.insert(e, usage);
            log.count(&format!("materials of class {} given their usage", m.class), 1);
        }
        if let Some(order) = order {
            world.insert(e, order);
            log.count(
                &format!("materials of class {} given their draw order", m.class),
                1,
            );
        }
    }
    Ok((log, previews))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn emission_preserves_hdr_and_counts_dim_texels_lost_to_quantization() {
        let (t, strength, zeroed) =
            emissive_from_linear(4, 1, &[[4., 1., 0.], [0.; 3], [0.00001; 3], [0.00001, 1., 0.]]).unwrap();
        assert_eq!(strength, 4.);
        assert_eq!(zeroed, 1);
        assert_eq!(
            t.levels[0],
            [255, 137, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 137, 0, 255]
        );
        assert_eq!(t.color_space, ColorSpace::Srgb);
        let (_, strength, zeroed) = emissive_from_linear(1, 1, &[[0.; 3]]).unwrap();
        assert_eq!((strength, zeroed), (1., 0));
        let (t, strength, _) = emissive_from_linear(1, 1, &[[f32::MAX, 0., 0.]]).unwrap();
        assert_eq!(strength, f32::MAX);
        assert_eq!(t.levels[0], [255, 0, 0, 255]);
        for v in [-0.01, f32::NAN, f32::INFINITY] {
            assert!(emissive_from_linear(1, 1, &[[v; 3]]).is_err());
        }
        assert!(emissive_from_linear(0, 1, &[]).is_err());
        assert!(emissive_from_linear(2, 2, &[[0.; 3]; 3]).is_err());
    }
    #[test]
    fn gloss_pack_has_orm_channels_and_rejects_invalid_planes() {
        let t = orm_from_gloss(3, 1, &[0., 0.25, 1.]).unwrap();
        assert_eq!(
            t.levels[0],
            [255, 255, 255, 255, 255, 134, 255, 255, 255, 13, 255, 255]
        );
        assert_eq!(t.role, TexRole::Mask);
        assert_eq!(t.color_space, ColorSpace::Linear);
        assert!(orm_from_gloss(2, 2, &[0.; 3]).is_err());
        assert!(orm_from_gloss(0, 1, &[]).is_err());
        assert!(orm_from_gloss(1, 1, &[f32::NAN]).is_err());
    }
    #[test]
    fn unbaked_layers_use_the_readers_preview_textures_only_when_asked() {
        let mut world = World::new();
        let mut m = crate::materials::resolve(
            &crate::flver::Material {
                name: "fixture".into(),
                mtd: "fixture.mtd".into(),
                gx: vec![],
                textures: vec![],
            },
            None,
            |_| false,
        );
        m.bindings.insert(
            "Texture2D_8_AlbedoMap".into(),
            crate::materials::Binding {
                texture: "main_a".into(),
                scale: [1.; 2],
                uv_number: 1,
                address: [None; 2],
                available: true,
            },
        );
        m.layered = Some(crate::materials::LayeredMaterial {
            mtd: m.mtd.clone(),
            definition: crate::mtd::MaterialDefinition {
                shader: "M[Multiple].spx".into(),
                description: String::new(),
                parameters: vec![],
                textures: vec![],
                block_lengths: vec![],
            },
            bindings: BTreeMap::new(),
        });
        let reader = spawn(
            &mut world,
            "main_a".into(),
            Texture {
                width: 1,
                height: 1,
                format: TexFormat::Rgba8Unorm,
                levels: vec![vec![1, 2, 3, 255]],
                color_space: ColorSpace::Srgb,
                role: TexRole::Color,
                normal: None,
            },
        );
        let e = world.spawn();
        world.insert(e, m);
        assert!(lower(&mut world.clone(), &BTreeMap::<String, Vec<u8>>::new(), None).is_err());
        let mut preserved = world.clone();
        let log = lower(
            &mut preserved,
            &BTreeMap::<String, Vec<u8>>::new(),
            Some(Layers::Preserve),
        )
        .unwrap();
        assert!(preserved.get::<PbrMaterial>(e).is_none());
        assert_eq!(
            log.counts["layered materials kept as source data (layers=preserve)"],
            1
        );
        assert!(log.approximations.is_empty());
        let log = lower(
            &mut world,
            &BTreeMap::<String, Vec<u8>>::new(),
            Some(Layers::Preview),
        )
        .unwrap();
        let p = world.get::<PbrMaterial>(e).unwrap();
        assert_eq!(p.base_color_texture.unwrap().texture, reader);
        assert_eq!(p.metallic, 0.);
        assert_eq!(
            log.approximations["layered material lowered from its main-layer preview; layers not baked"],
            1
        );
        assert_eq!(
            world.iter::<Texture>().count(),
            1,
            "no second decode of the reader's texture"
        );
    }
    #[test]
    fn gloss_matches_deferred_ggx_alpha() {
        assert_eq!(roughness(0.).unwrap(), 1.);
        assert!((roughness(1.).unwrap() - 0.05).abs() < 1e-6);
        assert!((roughness(0.25).unwrap() - 0.525).abs() < 1e-6);
        for bad in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            assert!(roughness(bad).is_err());
        }
    }
}
