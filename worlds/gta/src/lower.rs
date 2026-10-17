//! GTA's shaders lowered to `pbr.material` and its vertex colours to glTF's meaning ([`crate::shader_class`] says what
//! each shader draws).
//!
//! - `lower-gta-pbr`: the geometry of shaders with nothing a surface material can draw is dropped (counted by
//!   reason), and so is night-only geometry unless the pass is told it is night; every other shader but terrain's
//!   (`gta-terrain-layers`) gets a `pbr.material`.
//! - `gta-vertex-colours`: vertex colour 0 kept where it fades the surface (as white with GTA's alpha), vertex colour 1
//!   kept where it weighs a layered material, both dropped elsewhere (glTF multiplies COLOR_0 into the base colour;
//!   GTA's red and green are ambient occlusion and its blue a palette column, none of them a colour).
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, LayeredMaterial, Lods, Mesh, Primitive, Semantic, Stream, TexRole, Texture, World,
};

use crate::components::{GtaParam, GtaShader, GtaSources};
use crate::shader_class::{Class, ShaderClass, Skip, classify};

/// The alpha-test reference cutouts and foliage are given.
pub const CUTOFF: f32 = 0.5;
/// Water's base colour, sRGB bytes with alpha: the earlier skate exporter's.
pub const WATER: [u8; 4] = [0x30, 0x62, 0x7A, 190];
/// The base colour of a surface with no base texture, or whose texture the lookup did not find (sRGB grey 128).
pub const GREY: u8 = 128;

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

/// Perceptual roughness of a Blinn-Phong specular power `n`: the microfacet alpha with the same highlight,
/// `alpha = sqrt(2 / (n + 2))` (Walter et al. 2007), is roughness squared.
pub fn roughness(power: f32) -> f32 {
    (2.0 / (power.max(0.0) + 2.0)).powf(0.25)
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
///   without one.
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
        "alpha tested at 0.5 (the game's reference is a render state); specular intensity not carried (metal-rough \
         has no specular factor); normal and specular maps not lowered yet; water a blended flat colour"
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
        for (&e, c) in &classes {
            if dropped.contains_key(&e) {
                continue;
            }
            if c.class == Class::Terrain {
                log.count("terrain materials left to gta-terrain-layers", 1);
                continue;
            }
            let s = w.get::<GtaShader>(e).expect("shader");
            let m = self.material(s, c, log);
            colour.extend(m.base_color_texture.map(|t| t.texture));
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
    fn material(&self, s: &GtaShader, c: &ShaderClass, log: &mut PassLog) -> PbrMaterial {
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
        for (name, what) in [
            ("BumpTex", "normal maps not lowered yet"),
            ("SpecularTex", "specular maps not lowered yet"),
        ] {
            if matches!(texture(s, name), Some(Some(_))) {
                log.approximate(what, 1);
            }
        }
        m
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
    use super::roughness;

    #[test]
    fn roughness_of_a_specular_power() {
        assert_eq!(roughness(0.0), 1.0);
        assert!((roughness(30.0) - 0.5).abs() < 1e-6);
        assert!(roughness(512.0) < roughness(100.0));
    }
}
