//! `lower-shred-materials{normals=omit|rgb-up|rgb-down|ag-up|ag-down,alpha=opaque|named}`.
//!
//! Named source slots become PBR views of the preferred loaded 2D copy. Source
//! textures and every packaged copy stay untouched. Slot names alone do not prove
//! shader operations: these mappings, UV0/repeat sampling, dielectric roughness 1,
//! and unknown render state are named approximations, not native semantics.
//! See the crate README for the mapping/evidence table. Effect_Map stays unmapped.
use crate::components::{EncodedTexture, MaterialTextures, PixelProgram, ShredMaterial, SourceTexture};
use primase_core::pass::{Choice, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::pbr::{AlphaMode, PbrMaterial, Shading, Specular, TextureRef};
use primase_core::{
    ColorSpace, Component, Entity, Name, NormalConvention, TexFormat, TexRole, Texture, World,
};
use primase_texcodec::{Decoder, TexDecoder};
use rme_formats::pixel_program::PixelProgram as ProgramClass;
use std::collections::{BTreeMap, BTreeSet};

/// An explicit approximation of the source's unproved normal encoding. `ag`
/// takes X from alpha, Y from green and reconstructs positive Z. `down` inverts Y.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Normals {
    Omit,
    RgbUp,
    RgbDown,
    AgUp,
    AgDown,
}

impl Choice for Normals {
    const VALUES: &'static [(&'static str, Self)] = &[
        ("omit", Self::Omit),
        ("rgb-up", Self::RgbUp),
        ("rgb-down", Self::RgbDown),
        ("ag-up", Self::AgUp),
        ("ag-down", Self::AgDown),
    ];
}

/// Alpha state is not decoded. The caller may explicitly request the bounded,
/// named environment-family heuristic, or ignore alpha for a colour reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
    Opaque,
    Named,
}
impl Choice for Alpha {
    const VALUES: &'static [(&'static str, Self)] = &[("opaque", Self::Opaque), ("named", Self::Named)];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LowerShredMaterials {
    pub normals: Normals,
    pub alpha: Alpha,
}

pub const LOWER_SHRED_MATERIALS: PassSpec = PassSpec {
    name: "lower-shred-materials",
    options: &[
        OptionSpec::choice::<Normals>(
            "normals",
            "omit normal maps, or approximate their channel encoding and green direction; source basis remains unproved",
        ),
        OptionSpec::choice::<Alpha>(
            "alpha",
            "opaque colour reference, or approximate named environment cutout/glass parameters; no native state claim",
        ),
    ],
    make: |g| {
        Ok(Box::new(LowerShredMaterials {
            normals: g.choice("normals"),
            alpha: g.choice("alpha"),
        }))
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum View {
    Colour,
    Specular,
    Normal,
}

impl View {
    fn label(self) -> &'static str {
        match self {
            Self::Colour => "colour",
            Self::Specular => "specular",
            Self::Normal => "normal",
        }
    }
}

fn approximate(log: &mut PassLog, e: Entity, m: &ShredMaterial, why: &str) {
    log.approximate_on(e, &format!("{}: {why}", m.fragment_shader), 1);
}

fn parameter(m: &ShredMaterial, name: &str) -> Result<Option<[f32; 4]>, String> {
    let mut found = m.parameters.iter().filter(|p| p.name.as_deref() == Some(name));
    let value = found.next().map(|p| p.value);
    if found.next().is_some() {
        return Err(format!("{:?}: repeated {name} parameter is ambiguous", m.key));
    }
    if value.is_some_and(|v| v.iter().any(|x| !x.is_finite())) {
        return Err(format!("{:?}: non-finite {name}", m.key));
    }
    Ok(value)
}

fn colour(value: [f32; 4], e: Entity, m: &ShredMaterial, name: &str, log: &mut PassLog) -> [f32; 3] {
    if value[..3].iter().any(|v| !(0.0..=1.0).contains(v)) {
        approximate(log, e, m, &format!("{name} RGB clamped to PBR factor range"));
    }
    [
        value[0].clamp(0.0, 1.0),
        value[1].clamp(0.0, 1.0),
        value[2].clamp(0.0, 1.0),
    ]
}

struct Context<'a> {
    world: &'a mut World,
    log: &'a mut PassLog,
    views: BTreeMap<(Entity, View), Entity>,
    normals: Normals,
}

impl Context<'_> {
    fn texture(
        &mut self,
        e: Entity,
        m: &ShredMaterial,
        slot: u32,
        view: View,
    ) -> Result<Option<TextureRef>, String> {
        let bindings = self.world.get::<MaterialTextures>(e).ok_or_else(|| {
            format!(
                "{:?}: material texture graph missing; attach material inputs before lowering",
                m.key
            )
        })?;
        let matches: Vec<_> = bindings.0.iter().filter(|b| b.slot == slot).collect();
        if matches.len() != 1 {
            return Err(format!(
                "{:?}: slot {slot} has {} graph bindings",
                m.key,
                matches.len()
            ));
        }
        let Some(source) = matches[0].texture else {
            approximate(
                self.log,
                e,
                m,
                &format!("slot {slot} source path unavailable; texture omitted"),
            );
            return Ok(None);
        };
        let source = self
            .world
            .get::<SourceTexture>(source)
            .ok_or_else(|| format!("{:?}: slot {slot} lacks source texture", m.key))?;
        let Some(preferred) = source.preferred else {
            approximate(
                self.log,
                e,
                m,
                &format!("slot {slot} has no pixel-bearing copy; texture omitted"),
            );
            return Ok(None);
        };
        let copy = source
            .copies
            .get(preferred)
            .ok_or_else(|| format!("{:?}: invalid preferred texture copy", m.key))?;
        if copy.faces.len() != 1 || copy.header.get(63) == Some(&1) {
            approximate(
                self.log,
                e,
                m,
                &format!("slot {slot} cube or absent faces cannot become a PBR 2D map"),
            );
            return Ok(None);
        }
        let face = copy.faces[0];
        if self.world.get::<EncodedTexture>(face).is_some() {
            approximate(
                self.log,
                e,
                m,
                &format!("slot {slot} encoded volume outside PBR 2D maps"),
            );
            return Ok(None);
        }
        if let Some(&texture) = self.views.get(&(face, view)) {
            return Ok(Some(TextureRef::new(texture)));
        }
        let mut texture = self.world.get::<Texture>(face).cloned().ok_or_else(|| {
            format!(
                "{:?}: slot {slot} pixels not loaded; run reader.load before material lowering",
                m.key
            )
        })?;
        let path = source.path.clone();
        match view {
            View::Colour | View::Specular => {
                texture.color_space = ColorSpace::Srgb;
                texture.role = TexRole::Color;
                texture.normal = None;
            }
            View::Normal => {
                if self.normals != Normals::RgbUp {
                    let decoder = Decoder::default();
                    for (i, bytes) in texture.levels.iter_mut().enumerate() {
                        let width = (texture.width >> i).max(1);
                        let height = (texture.height >> i).max(1);
                        let mut rgba = decoder
                            .decode_rgba8(texture.format, width, height, bytes)
                            .map_err(|e| e.to_string())?;
                        for p in rgba.as_chunks_mut::<4>().0 {
                            if matches!(self.normals, Normals::AgUp | Normals::AgDown) {
                                let x = f32::from(p[3]) / 127.5 - 1.0;
                                let y = f32::from(p[1]) / 127.5 - 1.0;
                                p[0] = p[3];
                                p[2] = ((1.0 - x * x - y * y).max(0.0).sqrt() * 127.5 + 127.5)
                                    .round_ties_even() as u8;
                            }
                            if matches!(self.normals, Normals::RgbDown | Normals::AgDown) {
                                p[1] = 255 - p[1];
                            }
                            p[3] = 255;
                        }
                        *bytes = rgba;
                    }
                    texture.format = TexFormat::Rgba8Unorm;
                }
                texture.color_space = ColorSpace::Linear;
                texture.role = TexRole::Normal;
                texture.normal = Some(NormalConvention::GLTF);
            }
        }
        let entity = self.world.spawn();
        self.world.insert(entity, texture);
        self.world
            .insert(entity, Name(format!("{path}@shred-{}", view.label())));
        self.views.insert((face, view), entity);
        self.log.count("PBR texture views (source copies unchanged)", 1);
        Ok(Some(TextureRef::new(entity)))
    }
}

impl Pass for LowerShredMaterials {
    fn name(&self) -> &'static str {
        "lower-shred-materials"
    }
    fn options(&self) -> Options {
        Options::from([
            ("normals".into(), self.normals.text().into()),
            ("alpha".into(), self.alpha.text().into()),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[ShredMaterial::NAME]
    }
    fn may_lack_inputs(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[PbrMaterial::NAME, Texture::NAME, Name::NAME]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[ShredMaterial::NAME]
    }
    fn approximates(&self) -> &'static str {
        "named slot and constant mappings to PBR; colour transfer, normal encoding/basis, UV0/repeat, rough dielectric response, alpha and culling without native state semantics; unmapped shader operations omitted; a material whose pixel program (microcode) only scales its colour texture by uniforms is unlit"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<_> = world
            .iter::<ShredMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut ctx = Context {
            world,
            log,
            views: BTreeMap::new(),
            normals: self.normals,
        };
        for (e, m) in materials {
            if ctx.world.get::<PbrMaterial>(e).is_some() {
                ctx.log.count("existing PBR materials kept", 1);
                continue;
            }
            let program = ctx.world.get::<PixelProgram>(e).cloned();
            let mut pbr = PbrMaterial {
                metallic: 0.0,
                roughness: 1.0,
                ..PbrMaterial::default()
            };
            let mut used = BTreeSet::new();
            let family = m.fragment_shader.rsplit(['\\', '/']).next().unwrap_or("");
            let water = family == "env_water";
            let tint = if water {
                Some("baseWaterColor")
            } else if family == "env_glass" {
                Some("GlassColor")
            } else if family.starts_with("char_") {
                Some("BaseColor")
            } else if family == "env_solid" {
                Some("SolidColor")
            } else if family.starts_with("design_") {
                Some("PrimaryColor")
            } else {
                None
            };
            if let Some(tint) = tint
                && let Some(value) = parameter(&m, tint)?
            {
                pbr.base_color[..3].copy_from_slice(&colour(value, e, &m, tint, ctx.log));
                used.insert(tint.to_owned());
                approximate(
                    ctx.log,
                    e,
                    &m,
                    &format!("{tint} RGB as linear base factor; stored w is not opacity"),
                );
                if tint == "GlassColor" && self.alpha == Alpha::Named {
                    if !(0.0..=1.0).contains(&value[3]) {
                        approximate(ctx.log, e, &m, "GlassColor alpha clamped to PBR range");
                    }
                    pbr.base_color[3] = value[3].clamp(0.0, 1.0);
                    pbr.alpha_mode = AlphaMode::Blend;
                    approximate(
                        ctx.log,
                        e,
                        &m,
                        "GlassColor.w as blended alpha; refraction/Fresnel omitted",
                    );
                }
            }
            let mut names = BTreeSet::new();
            for sampler in &m.samplers {
                if sampler.texture.is_empty() {
                    ctx.log.count("empty source sampler bindings", 1);
                    continue;
                }
                let Some(name) = sampler.name.as_deref() else {
                    approximate(
                        ctx.log,
                        e,
                        &m,
                        &format!(
                            "unnamed sampler {:08x} slot {} omitted from PBR",
                            sampler.hash, sampler.slot
                        ),
                    );
                    continue;
                };
                if !names.insert(name) {
                    return Err(format!("{:?}: repeated {name} sampler is ambiguous", m.key));
                }
                let view = match name {
                    "Color_Map" if !water => View::Colour,
                    "Normal_Map" if self.normals != Normals::Omit => View::Normal,
                    "Spec_Map" => View::Specular,
                    "Normal_Map" => {
                        approximate(ctx.log, e, &m, "Normal_Map omitted by requested normals=omit");
                        continue;
                    }
                    _ => {
                        approximate(
                            ctx.log,
                            e,
                            &m,
                            &format!("{name} slot {} omitted; operation not established", sampler.slot),
                        );
                        continue;
                    }
                };
                let texture = ctx.texture(e, &m, sampler.slot, view)?;
                if let Some(texture) = texture {
                    match view {
                        View::Colour => pbr.base_color_texture = Some(texture),
                        View::Normal => pbr.normal_texture = Some(texture),
                        View::Specular => {
                            pbr.specular = Some(Specular {
                                color_texture: Some(texture),
                                ..Specular::default()
                            })
                        }
                    }
                    approximate(
                        ctx.log,
                        e,
                        &m,
                        &format!(
                            "{name} as PBR {} with UV0/repeat and unstated filters",
                            view.label()
                        ),
                    );
                    if view == View::Normal {
                        approximate(
                            ctx.log,
                            e,
                            &m,
                            &format!(
                                "normal encoding {} and tangent basis assumed",
                                self.normals.text()
                            ),
                        );
                    } else {
                        approximate(ctx.log, e, &m, &format!("{name} RGB interpreted as sRGB"));
                    }
                }
            }
            // Presence alone is insufficient: checked effect/character programs
            // use this name without a sampled-alpha kill. This is an opt-in
            // heuristic for the seven environment families in the material bank.
            let cutout = matches!(
                family,
                "env_base"
                    | "env_base_specular"
                    | "env_base_detail"
                    | "env_base_detail_specular"
                    | "env_tree_frond"
                    | "env_tree_leafmesh"
                    | "env_uvanim"
            );
            if self.alpha == Alpha::Named
                && cutout
                && let Some(value) = parameter(&m, "Alpha_test_ref")?
            {
                if value[0] < 0.0 {
                    return Err(format!("{:?}: negative Alpha_test_ref", m.key));
                }
                pbr.alpha_mode = AlphaMode::Mask { cutoff: value[0] };
                used.insert("Alpha_test_ref".into());
                approximate(
                    ctx.log,
                    e,
                    &m,
                    "Alpha_test_ref.x as alpha >= cutoff by requested environment heuristic; state enable/comparison unproved",
                );
            } else if !matches!(pbr.alpha_mode, AlphaMode::Blend) {
                approximate(
                    ctx.log,
                    e,
                    &m,
                    "opaque alpha without decoded blend/test enable state",
                );
            }
            if let Some(ProgramClass::ScaledTexture { unit, uniforms }) = program.as_deref() {
                let colour_slot = m
                    .samplers
                    .iter()
                    .find(|s| s.name.as_deref() == Some("Color_Map"))
                    .map(|s| s.slot);
                if colour_slot == Some(*unit) && pbr.base_color_texture.is_some() {
                    // The program's output is texture unit `unit` times its uniforms and nothing else: no light is read.
                    let mut tint = [1.0; 3];
                    for name in uniforms {
                        match parameter(&m, name)? {
                            Some(value) => {
                                let value = colour(value, e, &m, name, ctx.log);
                                for (t, v) in tint.iter_mut().zip(value) {
                                    *t *= v;
                                }
                                used.insert(name.clone());
                            }
                            None => approximate(
                                ctx.log,
                                e,
                                &m,
                                &format!("uniform {name} has no material value; 1 used"),
                            ),
                        }
                    }
                    pbr.base_color[..3].copy_from_slice(&tint);
                    pbr.shading = Shading::Unlit;
                    approximate(
                        ctx.log,
                        e,
                        &m,
                        &format!(
                            "pixel program only scales texture unit {unit} by {uniforms:?} (microcode): unlit, uniform RGB as base factor"
                        ),
                    );
                } else {
                    approximate(
                        ctx.log,
                        e,
                        &m,
                        &format!(
                            "pixel program only scales texture unit {unit}, but Color_Map is not that mapped slot; kept lit"
                        ),
                    );
                }
            }
            if pbr.shading == Shading::Unlit {
                approximate(
                    ctx.log,
                    e,
                    &m,
                    "single-sided culling and fog without native state",
                );
            } else {
                approximate(
                    ctx.log,
                    e,
                    &m,
                    "lit dielectric PBR, metallic 0, roughness 1 and single-sided culling without native lighting/state",
                );
            }
            if pbr.base_color_texture.is_none() {
                approximate(ctx.log, e, &m, "base colour has no mapped texture; factor only");
            }
            for p in &m.parameters {
                if !p.name.as_ref().is_some_and(|name| used.contains(name)) {
                    let name = p.name.clone().unwrap_or_else(|| format!("hash {:08x}", p.hash));
                    approximate(ctx.log, e, &m, &format!("parameter {name} omitted from PBR"));
                }
            }
            ctx.world.insert(e, pbr);
            ctx.log.count("SHRED materials lowered", 1);
        }
        Ok(())
    }
}
