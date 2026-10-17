//! GTA's shaders lowered to `pbr.material` and its vertex colours to glTF's meaning ([`crate::shader_class`] says what
//! each shader draws).
//!
//! - `lower-gta-pbr`: the geometry of shaders with nothing a surface material can draw is dropped (counted by
//!   reason), and so is night-only geometry unless the pass is told it is night; every other shader but terrain's
//!   (`gta-terrain-layers`) gets a `pbr.material`.
//!
//!   Specular, as GTA's shaders and deferred lights compute it: a shader's G-buffer intensity is
//!   I = `SpecularColor` x dot((s.r^2, s.g^2, s.b), `SpecularMapIntensityMask`) for its specular map s (`SpecularTex`),
//!   its power n = `Specular` x s.a, and its `Fresnel` F; the lights add I x Schlick(F0 = 1 - F, F90 = 1) x a
//!   normalised Blinn-Phong lobe of exponent [`exponent`]`(n)`. In `KHR_materials_specular` terms (dielectric F0 =
//!   0.04 x colour x factor, F90 = factor) that is factor I, colour (1 - F) / 0.04: the factor `SpecularColor`, a
//!   texture derived from the map (one per map, mask and power) holding the strength dot((s.r^2, s.g^2, s.b), mask) and
//!   the roughness of the exponent of `Specular` x s.a, per texel. A shader without `SpecularColor` writes no intensity
//!   (dry), so its specular factor is 0.
//! - `gta-vertex-colours`: vertex colour 0 kept where it fades the surface (as white with GTA's alpha), vertex colour 1
//!   kept where it weighs a layered material, both dropped elsewhere (glTF multiplies COLOR_0 into the base colour;
//!   GTA's red and green are ambient occlusion and its blue a palette column, none of them a colour).
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, Specular, TextureRef};
use primase_core::{
    ColorSpace, Entity, LayeredMaterial, Lods, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole,
    Texture, World,
};
use primase_texcodec::{Decoder, TexDecoder};

use crate::components::{GtaParam, GtaShader, GtaSources};
use crate::shader_class::{Class, ShaderClass, Skip, classify};

/// The alpha-test reference cutouts and foliage are given.
pub const CUTOFF: f32 = 0.5;
/// Water's base colour, sRGB bytes with alpha: the earlier skate exporter's.
pub const WATER: [u8; 4] = [0x30, 0x62, 0x7A, 190];
/// The base colour of a surface with no base texture, or whose texture the lookup did not find (sRGB grey 128).
pub const GREY: u8 = 128;
/// The Fresnel value of a shader without a `Fresnel` parameter: what `default`'s pixel shader writes.
pub const DEFAULT_FRESNEL: f32 = 0.98;
/// The specular map intensity mask of a shader without `SpecularMapIntensityMask`: the red channel.
pub const DEFAULT_MASK: [f32; 3] = [1.0, 0.0, 0.0];

/// The class of every shader material in `w` (an effect name hash no name in [`crate::shader_names`] gives is an
/// error).
pub fn classes(w: &World) -> Result<BTreeMap<Entity, ShaderClass>, String> {
    w.iter::<GtaShader>()
        .map(|(e, s)| {
            let effect = crate::shader_names::effect(s.name).ok_or_else(|| {
                format!(
                    "{}: shader {}: unknown effect {:#010x}",
                    s.drawable, s.index, s.name
                )
            })?;
            Ok((e, classify(effect, s.render_bucket)))
        })
        .collect()
}

/// A shader parameter by its Enhanced name.
pub fn param<'a>(s: &'a GtaShader, name: &str) -> Option<&'a GtaParam> {
    let h = primase_hash::joaat(name.as_bytes());
    s.params.iter().find(|(n, _)| *n == h).map(|(_, p)| p)
}

/// The first float of a constant buffer parameter, when it is finite.
pub fn scalar(s: &GtaShader, name: &str) -> Option<f32> {
    match param(s, name)? {
        GtaParam::Floats(f) => f.first().copied().filter(|v| v.is_finite()),
        _ => None,
    }
}

/// A texture parameter: `None` when the shader has no such parameter, `Some(None)` when it names a texture the lookup
/// did not find (or binds none).
pub fn texture(s: &GtaShader, name: &str) -> Option<Option<Entity>> {
    match param(s, name)? {
        GtaParam::Texture { texture, .. } => Some(*texture),
        _ => None,
    }
}

/// The Blinn-Phong exponent GTA's deferred lights use for a specular power `n` (a shader's `Specular`, times its
/// specular map's alpha): the G-buffer keeps n / 512 (so n stops at 512), the lights take 3n below 500 and
/// 1500 + 558 (n - 500) above.
pub fn exponent(n: f32) -> f32 {
    let n = n.clamp(0.0, 512.0);
    if n < 500.0 {
        3.0 * n
    } else {
        1500.0 + 558.0 * (n - 500.0)
    }
}

/// Perceptual roughness of a GTA specular power `n`: the microfacet alpha with the highlight of the Blinn-Phong
/// [`exponent`] e, `alpha = sqrt(2 / (e + 2))` (Walter et al. 2007), is roughness squared.
pub fn roughness(n: f32) -> f32 {
    (2.0 / (exponent(n) + 2.0)).powf(0.25)
}

/// The specular texture derived from specular map `t` (every level) under intensity mask `mask`, per texel as the
/// shader samples it (linear): A (and R) the strength dot((r^2, g^2, b), mask) clamped to 0-1 (`KHR_materials_specular`
/// reads A), G the [`roughness`] of the power `power` x a (glTF's metallic-roughness reads G; 1 without a power), B 0.
pub fn specular_texture(t: &Texture, mask: [f32; 3], power: Option<f32>) -> Result<Texture, String> {
    let linear = |c: u8| match t.color_space {
        ColorSpace::Srgb => primase_image::srgb_to_linear(f64::from(c) / 255.0) as f32,
        ColorSpace::Linear => f32::from(c) / 255.0,
    };
    let mut levels = Vec::with_capacity(t.levels.len());
    for (i, level) in t.levels.iter().enumerate() {
        let (w, h) = ((t.width >> i).max(1), (t.height >> i).max(1));
        let rgba = Decoder::default()
            .decode_rgba8(t.format, w, h, level)
            .map_err(|e| format!("specular map: {e}"))?;
        levels.push(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    let [r, g, b] = [p[0], p[1], p[2]].map(linear);
                    let v = (r * r * mask[0] + g * g * mask[1] + b * mask[2]).clamp(0.0, 1.0);
                    let strength = (v * 255.0).round() as u8;
                    let rough = power.map_or(1.0, |n| roughness(n * f32::from(p[3]) / 255.0));
                    [strength, (rough * 255.0).round() as u8, 0, strength]
                })
                .collect(),
        );
    }
    Ok(Texture {
        width: t.width,
        height: t.height,
        format: TexFormat::Rgba8Unorm,
        levels,
        color_space: ColorSpace::Linear,
        role: TexRole::Mask,
        normal: None,
    })
}

/// A material's specular map, its intensity mask and its power, to derive a texture from ([`specular_texture`]).
#[derive(Clone, Copy, Debug, PartialEq)]
struct SpecularMap {
    texture: Entity,
    mask: [f32; 3],
    power: Option<f32>,
}

/// Keeps the primitives of `mesh` (and their sources) that `keep` accepts.
pub fn retain_primitives(
    mesh: &mut Mesh,
    sources: &mut Vec<[u32; 2]>,
    mut keep: impl FnMut(&Primitive) -> bool,
) {
    let mut kept = Vec::with_capacity(mesh.primitives.len());
    let mut kept_sources = Vec::with_capacity(sources.len());
    for (p, s) in mesh.primitives.drain(..).zip(sources.drain(..)) {
        if keep(&p) {
            kept.push(p);
            kept_sources.push(s);
        }
    }
    mesh.primitives = kept;
    *sources = kept_sources;
}

/// Calls `f` on every drawable mesh (every entity with `core.mesh` and [`GtaSources`]: prototypes and their LOD forms),
/// with its sources.
pub fn for_each_mesh(
    w: &mut World,
    mut f: impl FnMut(&mut Mesh, &mut Vec<[u32; 2]>) -> Result<(), String>,
) -> Result<(), String> {
    let holders: Vec<Entity> = w.iter::<GtaSources>().map(|(e, _)| e).collect();
    for e in holders {
        let Some(mut mesh) = w.remove::<Mesh>(e) else {
            continue;
        };
        let mut s = w.remove::<GtaSources>(e).expect("sources");
        f(&mut mesh, &mut s.sources)?;
        w.insert(e, mesh);
        w.insert(e, s);
    }
    Ok(())
}

/// `lower-gta-pbr` (see the module documentation). A material:
///
/// - base colour texture: `DiffuseTex` (else `PlateBgTex`, else `DiffuseTexture_layer0`), sRGB; without one, or when
///   the lookup did not find it, grey [`GREY`]; water: the flat colour [`WATER`];
/// - alpha: opaque for opaque surfaces, tested at [`CUTOFF`] for cutouts and foliage, blended for alpha, decals, glass
///   and water;
/// - two-sided for the shaders GTA draws from both sides (cloth);
/// - emission (`*emissive*`, night-only emission only at night): the base texture, scaled by `EmissiveMultiplier`;
/// - metallic 0 (GTA has no metalness); roughness from the specular power `Specular` ([`roughness`]), 1 for shaders
///   without one;
/// - specular (`KHR_materials_specular`): factor `SpecularColor` (0 without one), colour (1 - `Fresnel`) / 0.04, the
///   strength and the roughness per texel (the map's alpha scaling the power) in one texture derived from `SpecularTex`
///   ([`specular_texture`]), its specular and metallic-roughness texture; none for water.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LowerGtaPbr {
    /// The world is lowered for night: night-only geometry kept and night emission on.
    pub night: bool,
}

impl LowerGtaPbr {
    pub fn from_options(o: &Options) -> Result<LowerGtaPbr, String> {
        let mut night = false;
        for (k, v) in o.iter() {
            match k.as_str() {
                "night" => night = v.parse().map_err(|_| format!("night: not true or false: {v}"))?,
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(LowerGtaPbr { night })
    }
}

impl Pass for LowerGtaPbr {
    fn name(&self) -> &'static str {
        "lower-gta-pbr"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        if self.night {
            o.insert("night".into(), "true".into());
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.shader", "gta.sources"]
    }
    fn provides(&self) -> &[&'static str] {
        &["pbr.material"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.sources", "core.texture"]
    }
    /// The meshes left with no primitive.
    fn removes(&self) -> &[&'static str] {
        &["core.mesh"]
    }
    fn approximates(&self) -> &'static str {
        "alpha tested at 0.5 (the game's reference is a render state); the Blinn-Phong lobe as GGX of the same \
         highlight, dry (no wetness); normal maps not lowered yet; water a \
         blended flat colour"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let classes = classes(w)?;
        let dropped: BTreeMap<Entity, Skip> = classes
            .iter()
            .filter_map(|(e, c)| c.skip.map(|s| (*e, s)))
            .filter(|(_, s)| !(self.night && *s == Skip::NightOnlyGeometry))
            .collect();
        for_each_mesh(w, |mesh, sources| {
            retain_primitives(mesh, sources, |p| {
                let skip = p.material.and_then(|m| dropped.get(&m));
                if let Some(s) = skip {
                    log.count(&format!("primitives dropped: {}", s.reason()), 1);
                }
                skip.is_none()
            });
            Ok(())
        })?;
        // a mesh whose every primitive was dropped draws nothing: no mesh (a prototype's own mesh stays while it has
        // other forms, which core.lods needs it for)
        let emptied: Vec<Entity> = w
            .iter::<Mesh>()
            .filter(|(e, m)| m.primitives.is_empty() && w.has::<GtaSources>(*e))
            .map(|(e, _)| e)
            .filter(|e| w.get::<Lods>(*e).is_none_or(|l| l.forms.is_empty()))
            .collect();
        log.count(
            "meshes left without primitives, removed (draw nothing)",
            emptied.len() as u64,
        );
        for e in emptied {
            w.remove::<Mesh>(e);
        }
        let mut colour: BTreeSet<Entity> = BTreeSet::new();
        let mut lowered = Vec::new();
        // per specular map, mask and power (bits), its derived texture
        let mut derived: BTreeMap<(Entity, [u32; 3], Option<u32>), Option<Entity>> = BTreeMap::new();
        for (&e, c) in &classes {
            if dropped.contains_key(&e) {
                continue;
            }
            if c.class == Class::Terrain {
                log.count("terrain materials left to gta-terrain-layers", 1);
                continue;
            }
            let s = w.get::<GtaShader>(e).expect("shader");
            let (mut m, map) = self.material(s, c, log);
            colour.extend(m.base_color_texture.map(|t| t.texture));
            if let Some(SpecularMap {
                texture: t,
                mask,
                power,
            }) = map
            {
                let key = (t, mask.map(f32::to_bits), power.map(f32::to_bits));
                let texture = match derived.get(&key) {
                    Some(d) => *d,
                    None => {
                        let d = match w.get::<Texture>(t) {
                            Some(tex) => {
                                let d = specular_texture(tex, mask, power)?;
                                let name = w.get::<Name>(t).map(|n| format!("{}_specular", n.0));
                                let de = w.spawn();
                                w.insert(de, d);
                                if let Some(n) = name {
                                    w.insert(de, Name(n));
                                }
                                log.count("specular textures derived (strength A, roughness G)", 1);
                                Some(de)
                            }
                            None => {
                                log.count("specular maps not loaded: intensity unmasked", 1);
                                None
                            }
                        };
                        derived.insert(key, d);
                        d
                    }
                };
                if let Some(d) = texture {
                    m.specular.as_mut().expect("specular").texture = Some(TextureRef::new(d));
                    if power.is_some() {
                        // the roughness is the texture's (the factor multiplies it)
                        m.metallic_roughness_texture = Some(TextureRef::new(d));
                        m.roughness = 1.0;
                    }
                }
            }
            lowered.push((e, m));
        }
        log.count("materials", lowered.len() as u64);
        for (e, m) in lowered {
            w.insert(e, m);
        }
        for t in colour {
            if let Some(tex) = w.get_mut::<Texture>(t)
                && (tex.role, tex.color_space) != (TexRole::Color, ColorSpace::Srgb)
            {
                tex.role = TexRole::Color;
                tex.color_space = ColorSpace::Srgb;
                log.count("base colour textures marked sRGB colour", 1);
            }
        }
        Ok(())
    }
}

impl LowerGtaPbr {
    /// The material, and its specular map, intensity mask and power when it has a map to derive a texture from.
    fn material(
        &self,
        s: &GtaShader,
        c: &ShaderClass,
        log: &mut PassLog,
    ) -> (PbrMaterial, Option<SpecularMap>) {
        let srgb = |b: u8| primase_image::srgb_to_linear(f64::from(b) / 255.0) as f32;
        let mut m = PbrMaterial {
            metallic: 0.0,
            alpha_mode: match c.class {
                Class::Opaque | Class::Terrain => AlphaMode::Opaque,
                Class::Cutout | Class::Foliage => AlphaMode::Mask { cutoff: CUTOFF },
                Class::Alpha | Class::Decal | Class::Glass | Class::Water => AlphaMode::Blend,
            },
            double_sided: c.two_sided,
            ..PbrMaterial::default()
        };
        if c.class == Class::Water {
            m.base_color = [
                srgb(WATER[0]),
                srgb(WATER[1]),
                srgb(WATER[2]),
                f32::from(WATER[3]) / 255.0,
            ];
            log.approximate("water drawn as a blended flat colour", 1);
        } else {
            let base = ["DiffuseTex", "PlateBgTex", "DiffuseTexture_layer0"]
                .iter()
                .find_map(|n| texture(s, n));
            match base {
                Some(Some(t)) => m.base_color_texture = Some(TextureRef::new(t)),
                Some(None) => {
                    log.count("base texture not found: grey", 1);
                    m.base_color = [srgb(GREY), srgb(GREY), srgb(GREY), 1.0];
                }
                None => {
                    log.count("no base texture: grey", 1);
                    m.base_color = [srgb(GREY), srgb(GREY), srgb(GREY), 1.0];
                }
            }
        }
        m.roughness = match scalar(s, "Specular") {
            Some(n) => roughness(n),
            None => {
                log.count("no specular power: roughness 1", 1);
                1.0
            }
        };
        if c.emissive && (!c.night_emissive || self.night) {
            m.emissive = [1.0; 3];
            m.emissive_texture = m.base_color_texture;
            m.emissive_strength = scalar(s, "EmissiveMultiplier")
                .filter(|v| *v >= 0.0)
                .unwrap_or(1.0);
            log.count("emissive materials", 1);
        } else if c.night_emissive {
            log.count("night emission off (day)", 1);
        }
        let mut map = None;
        if c.class != Class::Water {
            let fresnel = scalar(s, "Fresnel")
                .filter(|f| (0.0..=1.0).contains(f))
                .unwrap_or(DEFAULT_FRESNEL);
            let factor = match scalar(s, "SpecularColor") {
                None => {
                    log.count("no specular intensity: specular factor 0", 1);
                    0.0
                }
                Some(i) => {
                    if i > 1.0 {
                        log.approximate("specular intensity above 1 clamped to 1", 1);
                    }
                    match texture(s, "SpecularTex") {
                        Some(Some(t)) => {
                            let mask = match param(s, "SpecularMapIntensityMask") {
                                Some(GtaParam::Floats(f))
                                    if f.len() >= 3 && f[..3].iter().all(|v| v.is_finite()) =>
                                {
                                    [f[0], f[1], f[2]]
                                }
                                _ => {
                                    log.count("specular maps without an intensity mask: red", 1);
                                    DEFAULT_MASK
                                }
                            };
                            map = Some(SpecularMap {
                                texture: t,
                                mask,
                                power: scalar(s, "Specular"),
                            });
                        }
                        Some(None) => log.count("specular map not found: intensity unmasked", 1),
                        None => {}
                    }
                    i.clamp(0.0, 1.0)
                }
            };
            m.specular = Some(Specular {
                factor,
                texture: None,
                color: [(1.0 - fresnel) / 0.04; 3],
                color_texture: None,
            });
        }
        if matches!(texture(s, "BumpTex"), Some(Some(_))) {
            log.approximate("normal maps not lowered yet", 1);
        }
        (m, map)
    }
}

/// `gta-vertex-colours` (see the module documentation): on a primitive whose shader fades by vertex alpha
/// ([`ShaderClass::vertex_alpha`]) and is blended (alpha, decal, glass), with some vertex alpha below 255, COLOR_0
/// becomes (255, 255, 255, alpha); elsewhere it is dropped, except where `gta-tint` wrote the tint into it (kept,
/// its alpha only where it fades). COLOR_1 stays on primitives drawn with a layered material
/// (its layer weights) and is dropped elsewhere. Terrain primitives whose material is neither layered nor lowered are
/// left as they are (`gta-terrain-layers` has not run). Counted: primitives whose COLOR_0 red or green (GTA's per-vertex ambient
/// occlusion) is not all 255, which no PBR channel carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GtaVertexColours;

impl Pass for GtaVertexColours {
    fn name(&self) -> &'static str {
        "gta-vertex-colours"
    }
    fn requires(&self) -> &[&'static str] {
        &["gta.shader", "gta.sources"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["core.mesh", "gta.sources"]
    }
    fn approximates(&self) -> &'static str {
        "per-vertex ambient occlusion (vertex colour 0 red and green) not carried"
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let classes = classes(w)?;
        let layered: BTreeSet<Entity> = w.iter::<LayeredMaterial>().map(|(e, _)| e).collect();
        let lowered: BTreeSet<Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
        let tinted: BTreeSet<Entity> = w
            .resource::<crate::tint::GtaTinted>()
            .map_or(BTreeSet::new(), |t| t.0.clone());
        for_each_mesh(w, |mesh, _| {
            for p in &mut mesh.primitives {
                let Some(m) = p.material else { continue };
                let Some(c) = classes.get(&m) else { continue };
                if c.class == Class::Terrain && !layered.contains(&m) && !lowered.contains(&m) {
                    log.count("terrain primitives left (gta-terrain-layers has not run)", 1);
                    continue;
                }
                match p.streams.remove(&Semantic::Color(0)) {
                    Some(Stream::U8x4(v)) => {
                        if v.iter().any(|c| c[0] != 255 || c[1] != 255) {
                            log.approximate("primitives with per-vertex ambient occlusion not carried", 1);
                        }
                        let fades = c.vertex_alpha
                            && matches!(c.class, Class::Alpha | Class::Decal | Class::Glass)
                            && v.iter().any(|c| c[3] < 255);
                        if fades {
                            let white = v.iter().map(|c| [255, 255, 255, c[3]]).collect();
                            p.streams.insert(Semantic::Color(0), Stream::U8x4(white));
                            log.count("COLOR_0 kept as vertex alpha", 1);
                        } else {
                            log.count("COLOR_0 dropped", 1);
                        }
                    }
                    // gta-tint's colour: kept, with the alpha only where it fades the surface
                    Some(Stream::U16x4(mut v)) if tinted.contains(&m) => {
                        let fades = c.vertex_alpha
                            && matches!(c.class, Class::Alpha | Class::Decal | Class::Glass)
                            && v.iter().any(|c| c[3] < 65535);
                        if !fades {
                            for c in &mut v {
                                c[3] = 65535;
                            }
                        }
                        p.streams.insert(Semantic::Color(0), Stream::U16x4(v));
                        log.count("COLOR_0 kept as the tint", 1);
                    }
                    Some(_) => return Err("COLOR_0 not a u8 x 4 stream".into()),
                    None => {}
                }
                if !layered.contains(&m) && p.streams.remove(&Semantic::Color(1)).is_some() {
                    log.count("COLOR_1 dropped", 1);
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{exponent, roughness, specular_texture};
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};

    #[test]
    fn roughness_of_a_specular_power() {
        assert_eq!(roughness(0.0), 1.0);
        // exponent 30: alpha sqrt(2 / 32) = 0.25
        assert_eq!(exponent(10.0), 30.0);
        assert!((roughness(10.0) - 0.5).abs() < 1e-6);
        assert_eq!(exponent(501.0), 2058.0);
        assert_eq!(exponent(9000.0), exponent(512.0));
        assert!(roughness(512.0) < roughness(100.0));
    }

    #[test]
    fn specular_texture_squares_red_and_green_and_scales_the_power_by_alpha() {
        let t = Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![255, 0, 0, 7, 128, 128, 255, 9], vec![51, 0, 0, 0]],
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        };
        let s = specular_texture(&t, [1.0, 0.0, 0.0], None).unwrap();
        assert_eq!(
            (s.format, s.role, s.color_space),
            (TexFormat::Rgba8Unorm, TexRole::Mask, ColorSpace::Linear)
        );
        // strength in R and A, roughness 1 without a power in G, B 0; 128/255 squared is 0.252: 64
        assert_eq!(s.levels[0], vec![255, 255, 0, 255, 64, 255, 0, 64]);
        // 51/255 = 0.2, squared 0.04: 10
        assert_eq!(s.levels[1], vec![10, 255, 0, 10]);
        let s = specular_texture(&t, [0.0, 0.5, 0.5], None).unwrap();
        // 0.5 * 0.252 + 0.5 * 1 = 0.626: 160
        assert_eq!((s.levels[0][4], s.levels[0][7]), (160, 160));
        // power 10 x alpha: alpha 255 (level 1's 0 aside) gives exponent 30, roughness 0.5
        let mut opaque = t.clone();
        opaque.levels[0] = vec![255, 0, 0, 255, 0, 0, 0, 0];
        let s = specular_texture(&opaque, [1.0, 0.0, 0.0], Some(10.0)).unwrap();
        assert_eq!(s.levels[0][1], 128);
        // alpha 0: power 0, roughness 1
        assert_eq!(s.levels[0][5], 255);
    }
}
