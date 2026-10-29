//! Static level colour draws through the stored D3/Render / Render / P0 programs.
//! ShaderRender.UseMaterial supplies the glTF factors and samplers; Scene3D.UseShading supplies
//! the baked switches. Dynamic shadows are absent in this static scene (no board or actors).
//! Runtime light, shadow and outline values without a core role remain named unknowns.
use crate::components::StuntboostEffect;
use crate::components::{
    StuntboostDirectLight, StuntboostMaterial, StuntboostNode, StuntboostPart, StuntboostRole,
    StuntboostTextureRef,
};
use iridia_interface::{
    AddressMode, Interface, MatrixLines, ResourceKind, SamplerState, ShaderFormat, Stage,
};
use primase_core::pass::{OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_core::shader::*;
use primase_core::{Component, Entity, Instance, Mesh, Name, Semantic, World};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default)]
pub struct ResolveStuntboostShaders {
    pub gamma: Option<f32>,
    pub texture_msaa: Option<bool>,
}
pub const RESOLVE_STUNTBOOST_SHADERS: PassSpec = PassSpec {
    name: "resolve-stuntboost-shaders",
    options: &[
        OptionSpec::optional(
            "gamma",
            "1",
            "Game gamma; runtime graphics settings may override the constructor value. State the captured frame's setting for comparison.",
            "GraphicsOptions game-code constructor default: 1",
        ),
        OptionSpec::optional(
            "texture-msaa",
            "true",
            "Game texture supersampling switch; runtime graphics quality may override the constructor value. State the captured frame's setting for comparison.",
            "GraphicsOptions game-code constructor default: true",
        ),
    ],
    make: |g| {
        let gamma = g
            .optional("gamma")
            .map(|s| {
                s.parse::<f32>()
                    .map_err(|_| "gamma must be a finite positive number".to_owned())
            })
            .transpose()?;
        if gamma.is_some_and(|v| !v.is_finite() || v <= 0. || !v.recip().is_finite()) {
            return Err("gamma must have a finite positive reciprocal".into());
        }
        let texture_msaa = g
            .optional("texture-msaa")
            .map(|s| {
                s.parse::<bool>()
                    .map_err(|_| "texture-msaa must be true or false".to_owned())
            })
            .transpose()?;
        Ok(Box::new(ResolveStuntboostShaders { gamma, texture_msaa }))
    },
};
impl ResolveStuntboostShaders {
    fn setting(&self, name: &str) -> Option<ConstantRole> {
        let (value, caller) = match name {
            "GammaInverse" => (self.gamma.unwrap_or(1.).recip(), self.gamma.is_some()),
            "TextureMsaaEnabled" => (
                if self.texture_msaa.unwrap_or(true) { 1. } else { 0. },
                self.texture_msaa.is_some(),
            ),
            _ => return None,
        };
        Some(default(
            &[value],
            if caller {
                "stated by the caller"
            } else {
                "GraphicsOptions game-code constructor default"
            },
        ))
    }
}
fn default(value: &[f32], why: &str) -> ConstantRole {
    ConstantRole::Default {
        words: value.iter().map(|f| f.to_bits()).collect(),
        why: why.into(),
    }
}
fn constant(
    name: &str,
    m: &StuntboostMaterial,
    source: &StuntboostEffect,
    color: bool,
    normal: bool,
) -> ConstantRole {
    let scale = source.metres_per_unit;
    if matches!(
        name,
        "FogStart" | "FogLength" | "FogColor" | "ShadowColor" | "AmbientLightInShadow"
    ) && let Some(value) = source.shading.get(name).filter(|v| !v.is_empty())
    {
        let parsed: Option<Vec<f32>> = if matches!(name, "FogColor" | "ShadowColor") {
            let mut colour = if value.starts_with('(') {
                let values = value
                    .trim_matches(['(', ')'])
                    .split(',')
                    .take(4)
                    .map(|v| {
                        let f = v.trim().parse::<f32>().unwrap_or(1.);
                        (f.is_finite() && (0.0..=1.0).contains(&f))
                            .then_some(((f * 255.) as u8) as f32 / 255.)
                    })
                    .collect::<Option<Vec<_>>>();
                values.map(|mut c| {
                    while c.len() < 4 {
                        c.push(1.)
                    }
                    c
                })
            } else {
                let hex = value.trim_start_matches('#');
                if hex.len() == 6 {
                    u32::from_str_radix(hex, 16).ok().map(|n| {
                        vec![
                            ((n >> 16) & 255) as f32 / 255.,
                            ((n >> 8) & 255) as f32 / 255.,
                            (n & 255) as f32 / 255.,
                            1.,
                        ]
                    })
                } else {
                    None
                }
            };
            if name == "ShadowColor"
                && let Some(c) = &mut colour
            {
                c.truncate(3);
            }
            colour
        } else {
            value.parse::<f32>().ok().filter(|f| f.is_finite()).map(|f| {
                vec![if matches!(name, "FogStart" | "FogLength") {
                    f * scale as f32
                } else {
                    f
                }]
            })
        };
        return parsed
            .map(|v| default(&v, "Level.ReloadShading: Content/Other/Levels table override"))
            .unwrap_or_else(|| ConstantRole::Unknown {
                why: format!("{name}: unsupported level table value {value}"),
            });
    }
    match name {
        "World" => ConstantRole::Object(ObjectParameter::World),
        "ViewProjection" => ConstantRole::View(ViewParameter::ViewProjection),
        "CamPos" => ConstantRole::View(ViewParameter::CameraPosition),
        "ClipNear" => ConstantRole::View(ViewParameter::NearPlane),
        "ClipFar" => ConstantRole::View(ViewParameter::FarPlane),
        "Color" => ConstantRole::Material(m.base_color.map(f32::to_bits).to_vec()),
        "MetallicFactor"
            if m.other
                .get("pbrMetallicRoughness")
                .and_then(|p| p.get("metallicFactor"))
                .is_none() =>
        {
            default(
                &[0.5],
                "GlobalGame.CreateShaderRender: factor remains 0.5 when UseMaterial has no explicit metallic factor",
            )
        }
        "RoughnessFactor"
            if m.other
                .get("pbrMetallicRoughness")
                .and_then(|p| p.get("roughnessFactor"))
                .is_none() =>
        {
            default(
                &[0.5],
                "GlobalGame.CreateShaderRender: factor remains 0.5 when UseMaterial has no explicit roughness factor",
            )
        }
        "MetallicFactor" => ConstantRole::Material(vec![m.metallic.to_bits()]),
        "RoughnessFactor" => ConstantRole::Material(vec![m.roughness.to_bits()]),
        "AlbedoMapEnabled" => default(
            &[if m.base_color_texture.is_some() { 1. } else { 0. }],
            "ShaderRender.UseMaterial enables a present base colour map",
        ),
        "VertexColorEnabled" => default(
            &[if color { 1. } else { 0. }],
            "ShaderRender.UseVertexDeclaration: colour stream presence",
        ),
        "VertexNormalEnabled" => default(
            &[if normal { 1. } else { 0. }],
            "ShaderRender.UseVertexDeclaration: normal stream presence",
        ),
        "GammaInverse" => default(&[1.], "GlobalGame shader construction: gamma defaults to 1"),
        "VertexShiftEnabled" => default(&[1.], "GlobalGame.CreateShaderRender: vertexShiftEnabled is true"),
        "FogStart" => default(
            &[(100. * scale) as f32],
            "LevelShadingDefault.FogStart, converted with the level units",
        ),
        "FogLength" => default(
            &[(2000. * scale) as f32],
            "LevelShadingDefault.FogLength, converted with the level units",
        ),
        "FogColor" => default(&[0.8, 0.8, 0.6, 1.], "GlobalGame.DefaultFogColor"),
        "DynamicObject" | "DiffuseLightingEnabled" => default(&[0.], "Scene3D.UseShading(baked: true)"),
        "ShadowsEnabled" => default(
            &[0.],
            "Scene3D.UseShading: static scene has no dynamic actor shadow draws",
        ),
        "InstancedEnabled"
        | "SkinnedEnabled"
        | "NormalMapEnabled"
        | "OrmMapEnabled"
        | "EmissiveMapEnabled"
        | "InverseHullOutlines"
        | "False" => default(
            &[0.],
            "ShaderRender construction: disabled for this static base colour draw",
        ),
        "ShadowColor" => default(&[0.; 3], "LevelShadingDefault.ShadowColor"),
        "AmbientLightInShadow" => default(&[0.5], "LevelShadingDefault.AmbientLightInShadow"),
        _ => ConstantRole::Unknown {
            why: format!("{name}: frame, object or graphics setting not resolved from the static level"),
        },
    }
}
fn texture(t: &StuntboostTextureRef) -> TextureSource {
    let wrap = |w| {
        Some(match w {
            primase_core::pbr::Wrap::Repeat => AddressMode::Repeat,
            primase_core::pbr::Wrap::Clamp => AddressMode::Clamp,
            primase_core::pbr::Wrap::Mirror => AddressMode::Mirror,
        })
    };
    TextureSource::Texture {
        texture: t.texture,
        sampler: SamplerState {
            filters: t.filters,
            wrap: [wrap(t.wrap[0]), wrap(t.wrap[1]), None],
            ..Default::default()
        },
    }
}
#[derive(Clone, Copy)]
struct DrawBinding<'a> {
    settings: &'a ResolveStuntboostShaders,
    m: &'a StuntboostMaterial,
    direct: Option<&'a StuntboostDirectLight>,
    source: &'a StuntboostEffect,
    color: bool,
    normal: bool,
    core_colour: bool,
}
fn binding(
    shader: Entity,
    interface: &Interface,
    log: &mut PassLog,
    samplers: &[monogame_formats::mgfx::Sampler],
    draw: &DrawBinding<'_>,
) -> StageBinding {
    let DrawBinding {
        settings,
        m,
        direct,
        source,
        color,
        normal,
        core_colour,
    } = *draw;
    let mut b = StageBinding::new(interface.stage, shader);
    for v in &interface.inputs {
        let role = if interface.stage == Stage::Pixel {
            InputRole::Previous
        } else {
            match v.label.name.as_str() {
                "POSITION" => InputRole::Stream(Attribute::Position),
                "NORMAL" => InputRole::Stream(Attribute::Normal),
                "COLOR" if color && !core_colour => InputRole::Unknown {why:"Color0 comes from the game's _COLOR_SHIFT attribute, retained in the source dialect but not lowered to a core colour stream".into()},
                "COLOR" => InputRole::Stream(Attribute::Color(0)),
                "TEXCOORD" => InputRole::Stream(Attribute::TexCoord(v.label.index as u8)),
                _ => InputRole::Unknown {
                    why: format!("{}: static vertex stream not resolved", v.label.name),
                },
            }
        };
        if let InputRole::Unknown { why } = &role {
            log.count(&format!("unresolved input: {why}"), 1);
        }
        b.inputs.push(InputBinding {
            label: v.label.clone(),
            role,
        });
    }
    for r in &interface.resources {
        match &r.kind {
            ResourceKind::ConstantBuffer { layout } => {
                for c in &layout.constants {
                    let Some(size) = c.ty.size() else { continue };
                    let role = settings
                        .setting(&c.name)
                        .unwrap_or_else(|| constant(&c.name, m, source, color, normal));
                    if let ConstantRole::Unknown { why } = &role {
                        log.count(&format!("unresolved constant: {why}"), 1);
                    }
                    let lines = role.is_matrix().then_some(MatrixLines::Rows);
                    b.constants.push(ConstantBinding {
                        resource: r.id,
                        offset: c.offset,
                        size,
                        name: Some(c.name.clone()),
                        role,
                        lines,
                    });
                }
            }
            ResourceKind::Texture { .. } => {
                let name = r.provenance.name.as_str();
                let mut source = match name {
                    "AlbedoTex" => m.base_color_texture.as_ref().map(texture),
                    "DiffuseTex" => direct.map(|d| texture(&d.texture)),
                    "LightDepthTex" => Some(TextureSource::Frame(FrameTexture::ShadowMap)),
                    _ => None,
                }
                .unwrap_or_else(|| {
                    log.count(
                        &format!("unbound texture: {name}: ShaderRender has no texture for this material"),
                        1,
                    );
                    TextureSource::Missing {
                        why: format!("{name}: ShaderRender binds no texture for this material"),
                    }
                });
                // UseMaterial binds the specular/transmission sampler to slot zero, even
                // though its image is DiffuseTex. Preserve that source-side override.
                if name == "AlbedoTex"
                    && let (Some(direct), TextureSource::Texture { sampler, .. }) = (direct, &mut source)
                    && let TextureSource::Texture {
                        sampler: override_sampler,
                        ..
                    } = texture(&direct.texture)
                {
                    *sampler = override_sampler;
                }
                if let Some(state) = r
                    .provenance
                    .d3d
                    .and_then(|d| samplers.iter().find(|s| u32::from(s.texture_slot) == d.register))
                    .and_then(|s| s.state)
                {
                    let address = |mode| match mode {
                        0 => Some(AddressMode::Repeat),
                        1 => Some(AddressMode::Clamp),
                        2 => Some(AddressMode::Mirror),
                        3 => Some(AddressMode::Border),
                        _ => None,
                    };
                    if let TextureSource::Texture { sampler, .. } = &mut source {
                        let filter = match state.filter {
                            0 | 2 => Some(iridia_interface::Filter::Linear),
                            1 => Some(iridia_interface::Filter::Nearest),
                            _ => None,
                        };
                        *sampler = SamplerState {
                            wrap: state.address.map(address),
                            filters: iridia_interface::Filters {
                                min: filter,
                                mag: filter,
                                mip: Some(if state.filter == 1 {
                                    iridia_interface::MipFilter::Nearest
                                } else {
                                    iridia_interface::MipFilter::Linear
                                }),
                            },
                            max_anisotropy: Some(if state.filter == 2 {
                                u8::try_from(state.max_anisotropy).unwrap_or(1).max(1)
                            } else {
                                1
                            }),
                            mip_lod_bias: Some(state.mip_bias),
                            border: Some(state.border.map(|v| f32::from(v) / 255.)),
                            ..Default::default()
                        };
                        if state.max_mip_level != 0 {
                            log.count("MGFX nonzero MaxMipLevel not carried", 1);
                        }
                    } else {
                        log.count(
                            "MGFX sampler state on an unavailable frame texture is not carried",
                            1,
                        );
                    }
                }
                b.textures.push(TextureBinding {
                    resource: r.id,
                    sampler: interface
                        .resources
                        .iter()
                        .find(|s| {
                            matches!(s.kind, ResourceKind::Sampler { .. })
                                && s.provenance.d3d.map(|d| d.register)
                                    == r.provenance.d3d.map(|d| d.register)
                        })
                        .map(|s| s.id),
                    source,
                });
            }
            _ => {}
        }
    }
    if interface.stage == Stage::Pixel {
        for v in &interface.outputs {
            b.outputs.push(OutputBinding {
                label: v.label.clone(),
                lanes: (1u8 << v.value.width.min(4)) - 1,
                role: OutputRole::Colour,
            });
        }
    }
    b
}
impl Pass for ResolveStuntboostShaders {
    fn name(&self) -> &'static str {
        "resolve-stuntboost-shaders"
    }
    fn options(&self) -> Options {
        let mut options = Options::new();
        if let Some(gamma) = self.gamma {
            options.insert("gamma".into(), gamma.to_string());
        }
        if let Some(msaa) = self.texture_msaa {
            options.insert("texture-msaa".into(), msaa.to_string());
        }
        options
    }
    fn requires(&self) -> &[&'static str] {
        &[StuntboostMaterial::NAME, StuntboostEffect::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[
            Shader::NAME,
            LiftedShader::NAME,
            ShaderBinding::NAME,
            ShaderSpecialisations::NAME,
            MaterialPrograms::NAME,
            ProgramFacts::NAME,
            Name::NAME,
        ]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if self
            .gamma
            .is_some_and(|v| !v.is_finite() || v <= 0. || !v.recip().is_finite())
        {
            return Err("gamma must have a finite positive reciprocal".into());
        }
        log.count(
            &format!(
                "gamma={} ({})",
                self.gamma.unwrap_or(1.),
                if self.gamma.is_some() {
                    "stated by the caller"
                } else {
                    "game-code constructor default"
                }
            ),
            1,
        );
        log.count(
            &format!(
                "texture-msaa={} ({})",
                self.texture_msaa.unwrap_or(true),
                if self.texture_msaa.is_some() {
                    "stated by the caller"
                } else {
                    "game-code constructor default"
                }
            ),
            1,
        );
        let source = w
            .iter::<StuntboostEffect>()
            .next()
            .map(|(_, e)| e.clone())
            .ok_or("effect content not read")?;
        let mut uses: BTreeMap<Entity, BTreeSet<(bool, bool, bool, bool)>> = BTreeMap::new();
        let mut skinned = BTreeSet::new();
        for (e, n) in w.iter::<StuntboostNode>() {
            let Some(instance) = w.get::<Instance>(e) else {
                continue;
            };
            let Some(mesh) = w.get::<Mesh>(instance.prototype) else {
                continue;
            };
            let sky = w
                .get::<StuntboostPart>(n.part)
                .is_some_and(|p| p.role == StuntboostRole::Skybox);
            for (index, p) in mesh.primitives().iter().enumerate() {
                if let Some(m) = p.material {
                    if p.streams.contains_key(&Semantic::Joints(0))
                        || p.streams.contains_key(&Semantic::Weights(0))
                    {
                        skinned.insert(m);
                    }
                    let custom_colour = w
                        .get::<crate::components::StuntboostAttributes>(instance.prototype)
                        .and_then(|a| a.0.get(index))
                        .is_some_and(|a| a.keys().any(|name| name.starts_with("_COLOR")));
                    uses.entry(m).or_default().insert((
                        sky,
                        p.streams.contains_key(&Semantic::Color(0)) || custom_colour,
                        p.streams.contains_key(&Semantic::Normal),
                        p.streams.contains_key(&Semantic::Color(0)),
                    ));
                }
            }
        }
        let baked = w
            .iter::<StuntboostNode>()
            .any(|(_, n)| n.name.calls().any(|c| c.name == "SetBakedLighting"));
        let mut programs = Vec::new();
        if let Some(bytes) = &source.bytes {
            let effect = monogame_formats::mgfx::Effect::parse(bytes).map_err(|e| e.to_string())?;
            let tech = effect
                .techniques
                .iter()
                .find(|t| t.name == "Render")
                .ok_or("Render technique absent")?;
            let [pass] = tech.passes.as_slice() else {
                return Err("Render technique does not have exactly one pass".into());
            };
            for index in [pass.vertex_shader, pass.pixel_shader].into_iter().flatten() {
                let program =
                    primase_monogame::programs::program(&effect, index).map_err(|e| e.to_string())?;
                let e = w.spawn();
                w.insert(
                    e,
                    Shader {
                        format: ShaderFormat::Glsl,
                        bytecode: effect.shaders[index].code.to_vec(),
                        interface: program.interface.clone(),
                    },
                );
                w.insert(e, LiftedShader(Arc::new(program.clone())));
                w.insert(
                    e,
                    Name(format!("D3/Render: Render/{} {:?}", pass.name, program.stage)),
                );
                programs.push((e, program, effect.shaders[index].samplers.clone()));
            }
        }
        let copies: Vec<_> = w
            .iter::<crate::components::StuntboostColourDraw>()
            .map(|(e, draw)| (e, draw.multiplier))
            .collect();
        for (e, alpha) in copies {
            let why = format!(
                "Transparent({alpha}) sets ReceiveShadows=true; frame shadow map and shadow matrices are not resolved"
            );
            w.insert(
                e,
                MaterialPrograms(vec![ProgramFacts::unavailable(
                    "D3/Render Render/P0",
                    Stage::Pixel,
                    None,
                    &why,
                )]),
            );
            log.omit(&format!("materials unresolved: {why}"), 1);
        }
        let materials: Vec<_> = w
            .iter::<StuntboostMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        for (e, m) in materials {
            let why = if programs.len() != 2 {
                Some(source.missing.as_deref().unwrap_or("effect stages absent"))
            } else if m.name.starts_with("C_") {
                Some("collision material has no colour draw")
            } else if !baked {
                Some("level does not state SetBakedLighting; dynamic lighting not resolved")
            } else if uses.get(&e).is_some_and(|u| u.iter().any(|u| u.0)) {
                Some("skybox uses D3/Skybox with camera translation removed; not resolved")
            } else if skinned.contains(&e) {
                Some("skinned vertex declaration: joint palette and draw transforms are not resolved")
            } else if uses.get(&e).is_none_or(|u| u.len() != 1) {
                Some("no unique static vertex declaration for material")
            } else if m
                .other
                .get("pbrMetallicRoughness")
                .and_then(|p| p.get("baseColorTexture"))
                .is_some()
                && m.base_color_texture.is_none()
            {
                Some("base colour map is declared but its image is unavailable")
            } else if m.other.contains_key("metallicRoughnessTexture")
                || m.other.contains_key("normalTexture")
                || m.other.contains_key("emissiveTexture")
            {
                Some("additional material maps not read")
            } else {
                None
            };
            if let Some(why) = why {
                w.insert(
                    e,
                    MaterialPrograms(vec![ProgramFacts::unavailable(
                        "D3/Render",
                        Stage::Pixel,
                        None,
                        why,
                    )]),
                );
                log.omit(&format!("materials unresolved: {why}"), 1);
                continue;
            }
            let (_, color, normal, core_colour) = *uses[&e].first().ok_or("empty vertex declaration set")?;
            let direct = w.get::<StuntboostDirectLight>(e).cloned();
            let draw = DrawBinding {
                settings: self,
                m: &m,
                direct: direct.as_ref(),
                source: &source,
                color,
                normal,
                core_colour,
            };
            let mut stages = Vec::new();
            let mut facts = Vec::new();
            for (shader, p, samplers) in &programs {
                let b = binding(*shader, &p.interface, log, samplers, &draw);
                facts.push(
                    ProgramFacts::lifted(
                        "D3/Render Render/P0",
                        p.stage,
                        p.provenance.source_hash.clone(),
                        None,
                        &p.interface,
                    )
                    .bound(*shader, &b.outputs),
                );
                stages.push(b);
            }
            let binding = ShaderBinding {
                stages,
                state: Default::default(),
            };
            let specialised = ShaderSpecialisations::of(w, &binding);
            for stage in &specialised.0 {
                if let Some(why) = &stage.refusal {
                    log.count(&format!("stage specialisation refused: {why}"), 1);
                }
                log.count(
                    "resources removed by stated feature words",
                    stage.removed.len() as u64,
                );
            }
            w.insert(e, specialised);
            w.insert(e, binding);
            w.insert(e, MaterialPrograms(facts));
            log.count("materials resolved to D3/Render Render/P0", 1);
            log.count("render state not resolved from the frame draw scope", 1);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn material() -> StuntboostMaterial {
        StuntboostMaterial {
            name: "bake_mat_0".into(),
            base_color: [0.2, 0.4, 0.6, 0.8],
            base_color_texture: None,
            metallic: 0.3,
            roughness: 0.7,
            emissive: [0.; 3],
            double_sided: false,
            alpha_mode: "OPAQUE".into(),
            alpha_cutoff: None,
            shading: primase_core::pbr::Shading::Lit,
            other: Default::default(),
        }
    }
    #[test]
    fn material_words_and_level_table_overrides_follow_the_game_setters() {
        let m = material();
        let mut source = StuntboostEffect {
            bytes: None,
            missing: None,
            metres_per_unit: 0.01,
            shading: Default::default(),
        };
        assert_eq!(
            constant("Color", &m, &source, false, true),
            ConstantRole::Material(m.base_color.map(f32::to_bits).to_vec())
        );
        source.shading.insert("FogStart".into(), "50".into());
        assert_eq!(
            constant("FogStart", &m, &source, false, true),
            default(&[0.5], "Level.ReloadShading: Content/Other/Levels table override")
        );
        assert!(matches!(
            constant("not a game parameter", &m, &source, false, true),
            ConstantRole::Unknown { .. }
        ));
    }
    #[test]
    fn omitted_factors_retain_constructor_values_and_csv_colours_are_bytes() {
        let mut m = material();
        let mut source = StuntboostEffect {
            bytes: None,
            missing: None,
            metres_per_unit: 1.,
            shading: Default::default(),
        };
        for name in ["MetallicFactor", "RoughnessFactor"] {
            let ConstantRole::Default { words, .. } = constant(name, &m, &source, false, true) else {
                panic!("omitted factor has no constructor value")
            };
            assert_eq!(words, vec![0.5f32.to_bits()]);
        }
        m.other.insert(
            "pbrMetallicRoughness".into(),
            serde_json::json!({"metallicFactor":0.3,"roughnessFactor":0.7}),
        );
        assert_eq!(
            constant("MetallicFactor", &m, &source, false, true),
            ConstantRole::Material(vec![0.3f32.to_bits()])
        );
        source.shading.insert("FogColor".into(), "(0.5, 0.1, 1.0)".into());
        let ConstantRole::Default { words, .. } = constant("FogColor", &m, &source, false, true) else {
            panic!("table colour is not stated")
        };
        assert_eq!(words, [127f32 / 255., 25f32 / 255., 1., 1.].map(f32::to_bits));
    }
    #[test]
    fn settings_retain_their_origin_and_gamma_is_inverted() {
        let defaults = ResolveStuntboostShaders::default();
        assert_eq!(
            defaults.setting("TextureMsaaEnabled"),
            Some(default(&[1.], "GraphicsOptions game-code constructor default"))
        );
        let caller = ResolveStuntboostShaders {
            gamma: Some(2.),
            texture_msaa: Some(false),
        };
        assert_eq!(
            caller.setting("GammaInverse"),
            Some(default(&[0.5], "stated by the caller"))
        );
        assert_eq!(
            caller.setting("TextureMsaaEnabled"),
            Some(default(&[0.], "stated by the caller"))
        );
    }
}
