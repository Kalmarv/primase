//! `resolve-unity-shaders{tier=N,keywords=A+B|none,colour-space=linear|gamma}`: the programs of the pass each Unity
//! material draws with (`unity.programs`, [`crate::program::read_programs`]) as `core.shader` entities and each
//! material's `core.shader_binding`, by Unity's rules. The one home of those rules for the binding; `bind` still serves
//! the CPU evaluation until it is moved onto the binding.
//!
//! - **Variants.** The draw's keyword set is the material's keywords and `keywords` (the renderer's: the light, shadow,
//!   lightmap and probe keywords Unity sets per draw); with hardware tier `tier`, each stage's variant is the one
//!   whose keywords are the draw's restricted to those the stage uses ([`UnityPrograms::draw_variants`]). A material
//!   with none for a stage's variants is counted and the stage left out (no fragment variant: the material is left
//!   unbound).
//! - **Constants.** Each constant-buffer field the variant's parameters name, at its byte offset: a float or int
//!   property of the material by the field's name; a colour property (`colour-space=linear`: converted from its
//!   gamma-encoded storage to linear, as Unity sets colours in a linear-colour-space player; `gamma`: as stored); a
//!   texture's `<name>_ST` tiling (scale x, y, offset x, y); Unity's own matrices and camera position as the roles of
//!   [`builtin`]; anything else `Unknown` with the reason, struct (instancing) fields included. Every byte of a
//!   buffer the program reads has a role.
//! - **Textures.** A texture parameter binds the texture the material's property of that name holds; a property the
//!   material leaves unset takes the shader's declared default: `white` is white, any other (`bump`, `black`...)
//!   `Missing`, counted by name (Unity's built-in textures' values are not read). The engine's textures (a lightmap,
//!   the camera's depth) the program reads and the parameters do not name are `Missing`. The sampler state is Unity's
//!   packed value ([`crate::components::Parameters::samplers`]): not decoded.
//! - **Outputs.** The fragment stage's first target is the finished colour of a forward pass; the others `Unknown`.
//! - **State.** The pass's blend factors where they map to the vocabulary (`SrcAlphaSaturate` does not: counted),
//!   `Blend One Zero` being no blending, and its culling; its depth write is stored without its depth test, so no
//!   depth state is stated.
use std::collections::BTreeMap;

use primase_core::shader::{
    Blend, BlendEquation, BlendFactor, BlendOp, ConstantBinding, ConstantRole, Cull, D3dClass, InputBinding,
    InputRole, Interface, ObjectParameter, OutputBinding, OutputRole, RenderState, SamplerState, Shader,
    ShaderBinding, ShaderFormat, Stage, StageBinding, TextureBinding, TextureSource, ViewParameter,
};
use primase_core::{Component, Entity, Name, Semantic, Texture, World};

use crate::components::{Parameters, Shape, UnityMaterial, UnityProgramVariant, UnityPrograms, UnityTexEnv};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolveUnityShaders {
    tier: usize,
    keywords: Vec<String>,
    linear: bool,
}

impl ResolveUnityShaders {
    pub fn new(tier: usize, keywords: Vec<String>, linear: bool) -> Self {
        ResolveUnityShaders {
            tier,
            keywords,
            linear,
        }
    }

    /// From its options (all required), through [`ResolveUnityShaders::new`].
    pub fn from_options(o: &primase_core::pass::Options) -> Result<Self, String> {
        let (mut tier, mut keywords, mut linear) = (None, None, None);
        for (k, v) in o {
            match k.as_str() {
                "tier" => {
                    tier = Some(
                        v.parse::<usize>()
                            .map_err(|_| format!("tier={v} is not a number"))?,
                    )
                }
                "keywords" => {
                    keywords = Some(if v == "none" {
                        Vec::new()
                    } else {
                        v.split('+').map(str::to_owned).collect()
                    });
                }
                "colour-space" => {
                    linear = Some(match v.as_str() {
                        "linear" => true,
                        "gamma" => false,
                        _ => return Err(format!("colour-space={v} is not linear or gamma")),
                    });
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Self::new(
            tier.ok_or("tier is required: the hardware tier the variants are chosen for")?,
            keywords.ok_or("keywords is required: the renderer's keywords joined by +, or none")?,
            linear.ok_or(
                "colour-space is required: whether the player sets colour properties linear or as stored",
            )?,
        ))
    }
}

/// What Unity's built-in constants are, by field name: the matrices and the camera position the roles of the
/// vocabulary name.
pub fn builtin(name: &str) -> Option<ConstantRole> {
    Some(match name {
        "unity_ObjectToWorld" => ConstantRole::Object(ObjectParameter::World),
        "unity_WorldToObject" => ConstantRole::Object(ObjectParameter::InverseWorld),
        "unity_MatrixV" => ConstantRole::View(ViewParameter::View),
        "unity_MatrixInvV" => ConstantRole::View(ViewParameter::InverseView),
        "unity_MatrixVP" => ConstantRole::View(ViewParameter::ViewProjection),
        "glstate_matrix_projection" => ConstantRole::View(ViewParameter::Projection),
        "_WorldSpaceCameraPos" => ConstantRole::View(ViewParameter::CameraPosition),
        _ => return None,
    })
}

/// A field's bytes under D3D packing: a vector its columns, a matrix four per row, each array element but the last
/// padded to whole registers.
fn field_bytes(shape: Shape, array_size: u32) -> u32 {
    let one = match shape {
        Shape::Vector { columns } => columns,
        Shape::Matrix { rows } => rows * 4,
    };
    4 * match array_size {
        0 => one,
        n => (n - 1) * one.div_ceil(4) * 4 + one,
    }
}

fn words(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// The material's value for a constant-buffer field, if it has one.
fn material_value(m: &UnityMaterial, name: &str, linear: bool) -> Option<Vec<u32>> {
    if let Some(v) = m.float(name) {
        return Some(words(&[v]));
    }
    if let Some(c) = m.color(name) {
        let l = |x: f32| primase_image::srgb_to_linear(f64::from(x)) as f32;
        return Some(words(&if linear {
            [l(c[0]), l(c[1]), l(c[2]), c[3]]
        } else {
            c
        }));
    }
    if let Some((_, i)) = m.ints.iter().find(|(k, _)| k == name) {
        return Some(words(&[*i as f32]));
    }
    let base = name.strip_suffix("_ST")?;
    let (scale, offset) = m.textures.iter().find(|t| t.name == base)?.finite_tiling()?;
    Some(words(&[scale[0], scale[1], offset[0], offset[1]]))
}

/// The constants of the constant buffer `resource` (of `size` bytes, register `slot`) from the variant's parameters:
/// each named field and struct field, then `Unknown` over every byte left.
pub fn buffer_constants(
    parameters: &Parameters,
    slot: u32,
    resource: primase_core::shader::ResourceId,
    size: u32,
    m: &UnityMaterial,
    linear: bool,
    counts: &mut BTreeMap<String, u64>,
) -> Vec<ConstantBinding> {
    let mut out: Vec<ConstantBinding> = Vec::new();
    let mut bump = |what: &str| *counts.entry(what.to_owned()).or_default() += 1;
    if let Some(cb) = parameters
        .constant_buffers
        .iter()
        .find(|cb| cb.slot == Some(slot))
    {
        for f in &cb.fields {
            let bytes = field_bytes(f.shape, f.array_size);
            let role = if let Some(mut w) = material_value(m, &f.name, linear) {
                w.truncate(bytes as usize / 4);
                bump("constants set from the material's properties");
                ConstantRole::Material(w)
            } else if let Some(role) = builtin(&f.name) {
                bump("constants that are Unity's built-in matrices and camera position");
                role
            } else {
                bump(
                    "constants no role is resolved for (not a material property, not in the built-in table)",
                );
                ConstantRole::Unknown {
                    why: "not a material property of this name and not a built-in this pass maps".into(),
                }
            };
            out.push(ConstantBinding {
                resource,
                offset: f.offset,
                size: bytes,
                name: Some(f.name.clone()),
                role,
            });
        }
        for s in &cb.structs {
            bump("struct (instancing) constants, Unknown");
            out.push(ConstantBinding {
                resource,
                offset: s.offset,
                size: s.size.saturating_mul(s.array_size.max(1)),
                name: Some(s.name.clone()),
                role: ConstantRole::Unknown {
                    why: "a struct field (an instancing array): not laid out".into(),
                },
            });
        }
    }
    // only what fits the buffer the program declares
    out.retain(|c| c.size > 0 && c.offset.checked_add(c.size).is_some_and(|end| end <= size));
    out.sort_by_key(|c| (c.offset, c.size));
    // overlapping declarations (a field and a struct's member over the same bytes) keep the first
    let mut kept: Vec<ConstantBinding> = Vec::new();
    for c in out {
        if kept.last().is_none_or(|l| c.offset >= l.offset + l.size) {
            kept.push(c);
        }
    }
    let mut gaps = Vec::new();
    let mut at = 0;
    for c in &kept {
        if c.offset > at {
            gaps.push((at, c.offset - at));
        }
        at = c.offset + c.size;
    }
    if at < size {
        gaps.push((at, size - at));
    }
    for (offset, bytes) in gaps {
        kept.push(ConstantBinding {
            resource,
            offset,
            size: bytes,
            name: None,
            role: ConstantRole::Unknown {
                why: "bytes of the buffer no stored field covers".into(),
            },
        });
    }
    kept.sort_by_key(|c| c.offset);
    kept
}

/// A shader's declared default texture (`white`...) as a colour, where Unity's name is one this pass values.
fn default_colour(name: &str) -> Option<[f32; 4]> {
    (name == "white").then_some([1.0; 4])
}

/// Unity's `BlendMode` as the vocabulary's factor; `None` for `SrcAlphaSaturate`.
fn blend_factor(mode: u8) -> Option<BlendFactor> {
    Some(match mode {
        0 => BlendFactor::Zero,
        1 => BlendFactor::One,
        2 => BlendFactor::DstColour,
        3 => BlendFactor::SrcColour,
        4 => BlendFactor::OneMinusDstColour,
        5 => BlendFactor::SrcAlpha,
        6 => BlendFactor::OneMinusSrcColour,
        7 => BlendFactor::DstAlpha,
        8 => BlendFactor::OneMinusDstAlpha,
        10 => BlendFactor::OneMinusSrcAlpha,
        _ => return None,
    })
}

/// The render state the material's draw pass states (see the module documentation), and whether its blend factors
/// had one that does not map.
pub fn render_state(m: &UnityMaterial, programs: &UnityPrograms) -> (RenderState, bool) {
    let mut state = RenderState::default();
    let mut unmapped = false;
    let Some(pass) = m
        .shader_state
        .get(programs.subshader)
        .and_then(|s| s.passes.get(programs.pass))
    else {
        return (state, false);
    };
    if let Some((src, dst)) = m.blend_factors(pass) {
        match (blend_factor(src), blend_factor(dst)) {
            (Some(BlendFactor::One), Some(BlendFactor::Zero)) => {}
            (Some(source), Some(destination)) => {
                let eq = BlendEquation {
                    source,
                    destination,
                    op: BlendOp::Add,
                };
                state.blend = Some(Blend {
                    colour: eq,
                    alpha: eq,
                });
            }
            _ => unmapped = true,
        }
    }
    let cull = m.state_value(&pass.cull);
    state.cull = match cull as i64 {
        0 => Some(Cull::None),
        1 => Some(Cull::Front),
        2 => Some(Cull::Back),
        _ => None,
    };
    (state, unmapped)
}

fn semantic(name: &str, index: u32) -> Option<Semantic> {
    let n = u8::try_from(index).ok()?;
    match name.to_ascii_uppercase().as_str() {
        "POSITION" => Some(Semantic::Position),
        "NORMAL" => Some(Semantic::Normal),
        "TANGENT" => Some(Semantic::Tangent),
        "COLOR" => Some(Semantic::Color(n)),
        "TEXCOORD" => Some(Semantic::TexCoord(n)),
        _ => None,
    }
}

impl primase_core::pass::Pass for ResolveUnityShaders {
    fn name(&self) -> &'static str {
        "resolve-unity-shaders"
    }
    fn options(&self) -> primase_core::pass::Options {
        primase_core::pass::Options::from([
            ("tier".to_owned(), self.tier.to_string()),
            (
                "keywords".to_owned(),
                if self.keywords.is_empty() {
                    "none".to_owned()
                } else {
                    self.keywords.join("+")
                },
            ),
            (
                "colour-space".to_owned(),
                if self.linear { "linear" } else { "gamma" }.to_owned(),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[UnityMaterial::NAME, UnityPrograms::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Shader::NAME, ShaderBinding::NAME, Name::NAME]
    }
    fn run(&self, world: &mut World, log: &mut primase_core::pass::PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, UnityMaterial, UnityPrograms)> = world
            .iter::<UnityMaterial>()
            .filter_map(|(e, m)| Some((e, m.clone(), world.get::<UnityPrograms>(e)?.clone())))
            .collect();
        let without: u64 = world
            .iter::<UnityMaterial>()
            .filter(|(e, _)| !world.has::<UnityPrograms>(*e))
            .count() as u64;
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        counts.insert(
            "materials without unity.programs (no draw pass, shader object or D3D11 code)".into(),
            without,
        );
        let mut shaders: BTreeMap<std::sync::Arc<[u8]>, Result<Entity, String>> = BTreeMap::new();
        for (material, m, programs) in materials {
            let chosen = programs.draw_variants(&m.keywords, &self.keywords, self.tier);
            let Some(fragment) = chosen.fragment else {
                *counts
                    .entry("materials left unbound: no fragment variant for these keywords and tier".into())
                    .or_default() += 1;
                continue;
            };
            let mut bound = Vec::new();
            for (stage, variant, lists) in [
                (Stage::Vertex, chosen.vertex, !programs.vertex.is_empty()),
                (Stage::Pixel, Some(fragment), true),
            ] {
                let Some(variant) = variant else {
                    if lists {
                        *counts
                            .entry("stages left unbound: no variant for these keywords and tier".into())
                            .or_default() += 1;
                    }
                    continue;
                };
                if !shaders.contains_key(&variant.code) {
                    let made = match primase_dxbc::lift(&variant.code) {
                        Err(e) => Err(format!(
                            "a program the lifter refuses: {}",
                            primase_dxbc::refusal_reason(&e)
                        )),
                        Ok(program) => {
                            let e = world.spawn();
                            world.insert(e, Name(format!("{}-{:?}", programs.shader, stage)));
                            world.insert(
                                e,
                                Shader {
                                    format: ShaderFormat::Dxbc,
                                    bytecode: variant.code.to_vec(),
                                    interface: program.interface,
                                },
                            );
                            Ok(e)
                        }
                    };
                    shaders.insert(variant.code.clone(), made);
                }
                let made = shaders[&variant.code].clone();
                match made {
                    Ok(shader) => {
                        let interface = world
                            .get::<Shader>(shader)
                            .map(|s| s.interface.clone())
                            .ok_or("a resolved shader without its shader")?;
                        bound.push(self.stage_binding(
                            &m,
                            variant,
                            &programs.texture_defaults,
                            stage,
                            shader,
                            &interface,
                            chosen.vertex.is_some(),
                            &mut counts,
                        ));
                    }
                    Err(why) if stage == Stage::Pixel => {
                        *counts
                            .entry(format!("materials left unbound: {why}"))
                            .or_default() += 1;
                        bound.clear();
                        break;
                    }
                    Err(why) => {
                        *counts
                            .entry(format!("vertex stages left unbound: {why}"))
                            .or_default() += 1;
                    }
                }
            }
            if bound.iter().all(|s: &StageBinding| s.stage != Stage::Pixel) {
                continue;
            }
            // textures: bound per stage binding by name, entities checked against the world
            for stage in &mut bound {
                for t in &mut stage.textures {
                    if let TextureSource::Texture { texture, .. } = &t.source
                        && !(world.has::<Texture>(*texture)
                            || world.has::<primase_core::TextureFile>(*texture))
                    {
                        t.source = TextureSource::Missing;
                        *counts
                            .entry("texture bindings of an entity with no texture, left Missing".into())
                            .or_default() += 1;
                    }
                }
            }
            let (state, unmapped) = render_state(&m, &programs);
            if unmapped {
                *counts
                    .entry("passes whose blend factors do not map to the vocabulary (SrcAlphaSaturate): blending unstated".into())
                    .or_default() += 1;
            }
            world.insert(material, ShaderBinding { stages: bound, state });
            *counts.entry("materials bound".into()).or_default() += 1;
        }
        for (what, n) in counts {
            if n > 0 {
                log.count(&what, n);
            }
        }
        log.count(
            "shader programs",
            shaders.values().filter(|r| r.is_ok()).count() as u64,
        );
        Ok(())
    }
}

impl ResolveUnityShaders {
    #[allow(clippy::too_many_arguments)]
    fn stage_binding(
        &self,
        m: &UnityMaterial,
        variant: &UnityProgramVariant,
        defaults: &[(String, String)],
        stage: Stage,
        shader: Entity,
        interface: &Interface,
        vertex_bound: bool,
        counts: &mut BTreeMap<String, u64>,
    ) -> StageBinding {
        let inputs = interface
            .inputs
            .iter()
            .filter(|v| v.system.is_none())
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: match stage {
                    Stage::Vertex => semantic(&v.label.name, v.label.index).map_or_else(
                        || InputRole::Unknown {
                            why: format!(
                                "vertex input {} is not a mesh semantic this pass maps",
                                v.label.name
                            ),
                        },
                        InputRole::Stream,
                    ),
                    Stage::Pixel if vertex_bound => InputRole::Previous,
                    _ => InputRole::Unknown {
                        why: "the vertex stage is not bound".into(),
                    },
                },
            })
            .collect();
        let mut constants = Vec::new();
        let mut textures = Vec::new();
        for r in &interface.resources {
            let Some(b) = r.provenance.d3d else { continue };
            match (&r.kind, b.class) {
                (primase_core::shader::ResourceKind::ConstantBuffer { layout }, D3dClass::ConstantBuffer) => {
                    constants.extend(buffer_constants(
                        &variant.parameters,
                        b.register,
                        r.id,
                        layout.size,
                        m,
                        self.linear,
                        counts,
                    ));
                }
                (primase_core::shader::ResourceKind::Texture { .. }, D3dClass::ShaderResource) => {
                    let named = variant.parameters.textures.iter().find(|t| t.slot == b.register);
                    let source = match named {
                        None => {
                            *counts
                                .entry(
                                    "texture slots the parameters do not name (the engine's), left Missing"
                                        .into(),
                                )
                                .or_default() += 1;
                            TextureSource::Missing
                        }
                        Some(t) => resolve_texture(m, defaults, &t.name, counts),
                    };
                    textures.push(TextureBinding {
                        resource: r.id,
                        sampler: None,
                        source,
                    });
                }
                _ => {}
            }
        }
        let mut outputs = Vec::new();
        if stage == Stage::Pixel {
            for v in interface.outputs.iter().filter(|v| v.system.is_none()) {
                let full: u8 = ((1u16 << v.value.width) - 1) as u8;
                outputs.push(OutputBinding {
                    label: v.label.clone(),
                    lanes: full,
                    role: if v.label.index == 0 {
                        OutputRole::Colour
                    } else {
                        OutputRole::Unknown(v.label.index)
                    },
                });
            }
        }
        StageBinding {
            stage,
            shader,
            inputs,
            constants,
            textures,
            outputs,
        }
    }
}

/// What a texture parameter named `name` samples: the property's texture, else the shader's default.
fn resolve_texture(
    m: &UnityMaterial,
    defaults: &[(String, String)],
    name: &str,
    counts: &mut BTreeMap<String, u64>,
) -> TextureSource {
    let mut bump = |what: String| *counts.entry(what).or_default() += 1;
    let Some(UnityTexEnv {
        referenced, texture, ..
    }) = m.textures.iter().find(|t| t.name == name)
    else {
        bump("texture parameters the material has no property of, left Missing".into());
        return TextureSource::Missing;
    };
    match (referenced, texture) {
        (true, Some(texture)) => {
            bump("texture parameters bound to the property's texture".into());
            TextureSource::Texture {
                texture: *texture,
                sampler: SamplerState::default(),
            }
        }
        (true, None) => {
            bump("texture properties naming a texture the world does not hold, left Missing".into());
            TextureSource::Missing
        }
        (false, _) => match defaults.iter().find(|(p, _)| p == name) {
            Some((_, d)) => match default_colour(d) {
                Some(value) => {
                    bump("unset texture properties, bound the shader's default (white)".into());
                    TextureSource::Default {
                        value,
                        why: format!("unset; the shader's declared default is \"{d}\""),
                    }
                }
                None => {
                    bump(format!(
                        "unset texture properties whose shader default \"{d}\" is not valued, left Missing"
                    ));
                    TextureSource::Missing
                }
            },
            None => {
                bump("unset texture properties whose shader declares no default, left Missing".into());
                TextureSource::Missing
            }
        },
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "hand-worked fixtures"
)]
mod tests {
    use super::*;
    use crate::components::{ConstantBuffer, Field, StructField, TextureParam, UnityObject};
    use crate::shader::fixture;
    use primase_core::shader::ResourceId;

    fn material() -> UnityMaterial {
        UnityMaterial {
            material: UnityObject {
                file: "level1".into(),
                path_id: 1,
            },
            shader: Some("S".into()),
            shader_state: Vec::new(),
            textures: vec![
                UnityTexEnv {
                    name: "_MainTex".into(),
                    referenced: true,
                    texture: None,
                    scale: [2.0, 3.0],
                    offset: [0.5, 0.25],
                },
                UnityTexEnv {
                    name: "_BumpMap".into(),
                    referenced: false,
                    texture: None,
                    scale: [1.0; 2],
                    offset: [0.0; 2],
                },
            ],
            floats: vec![("_Cutoff".into(), 0.25)],
            ints: vec![("_Mode".into(), 2)],
            colors: vec![("_BaseColor".into(), [0.5, 1.0, 0.0, 0.75])],
            keywords: Vec::new(),
            tags: Vec::new(),
            render_queue: -1,
        }
    }

    fn field(name: &str, offset: u32, shape: Shape) -> Field {
        Field {
            name: name.into(),
            offset,
            kind: 0,
            shape,
            array_size: 0,
        }
    }

    /// A buffer at register 1 of 192 bytes: a colour, a float, a matrix, a tiling, an unknown vector and a struct
    /// array of one 16-byte element, with bytes left over at 20 to 32, 112 to 128 and 160 to 192.
    fn parameters() -> Parameters {
        Parameters {
            constant_buffers: vec![ConstantBuffer {
                name: "UnityPerMaterial".into(),
                slot: Some(1),
                size: 192,
                fields: vec![
                    field("_BaseColor", 0, Shape::Vector { columns: 4 }),
                    field("_Cutoff", 16, Shape::Vector { columns: 1 }),
                    field("unity_ObjectToWorld", 32, Shape::Matrix { rows: 4 }),
                    field("_MainTex_ST", 96, Shape::Vector { columns: 4 }),
                    field("_Mystery", 128, Shape::Vector { columns: 4 }),
                ],
                structs: vec![StructField {
                    name: "Props".into(),
                    offset: 144,
                    array_size: 1,
                    size: 16,
                    members: Vec::new(),
                }],
            }],
            textures: vec![TextureParam {
                name: "_MainTex".into(),
                slot: 0,
                sampler: None,
                dim: 2,
            }],
            samplers: Vec::new(),
        }
    }

    #[test]
    fn fields_take_the_materials_properties_builtins_or_unknown_and_every_byte_has_a_role() {
        let mut counts = BTreeMap::new();
        let c = buffer_constants(
            &parameters(),
            1,
            ResourceId(7),
            192,
            &material(),
            true,
            &mut counts,
        );
        let at = |offset: u32| c.iter().find(|c| c.offset == offset).unwrap();
        // a colour, gamma-encoded as stored, set linear (alpha unchanged): 0.5 -> 0.21404114, 1 -> 1, 0 -> 0
        let ConstantRole::Material(w) = &at(0).role else {
            panic!("{:?}", at(0))
        };
        let v: Vec<f32> = w.iter().map(|x| f32::from_bits(*x)).collect();
        assert!(
            (v[0] - 0.214_041_14).abs() < 1e-6 && v[1] == 1.0 && v[2] == 0.0 && v[3] == 0.75,
            "{v:?}"
        );
        assert_eq!(at(16).role, ConstantRole::Material(vec![0.25f32.to_bits()]));
        assert_eq!(at(32).role, ConstantRole::Object(ObjectParameter::World));
        assert_eq!(at(32).size, 64);
        assert_eq!(at(96).role, ConstantRole::Material(words(&[2.0, 3.0, 0.5, 0.25])));
        assert!(
            matches!(at(128).role, ConstantRole::Unknown { .. })
                && at(128).name.as_deref() == Some("_Mystery")
        );
        assert_eq!((at(144).name.as_deref(), at(144).size), (Some("Props"), 16));
        // no byte twice, none left out
        let mut covered = [false; 192];
        for b in &c {
            for byte in b.offset..b.offset + b.size {
                assert!(!covered[byte as usize], "byte {byte} twice");
                covered[byte as usize] = true;
            }
        }
        assert!(covered.iter().all(|&x| x));
        // as stored, not converted, in a gamma player
        let g = buffer_constants(
            &parameters(),
            1,
            ResourceId(7),
            192,
            &material(),
            false,
            &mut BTreeMap::new(),
        );
        assert_eq!(
            g.iter().find(|c| c.offset == 0).unwrap().role,
            ConstantRole::Material(words(&[0.5, 1.0, 0.0, 0.75]))
        );
        // a buffer the parameters do not name is one Unknown
        let other = buffer_constants(
            &parameters(),
            5,
            ResourceId(8),
            32,
            &material(),
            true,
            &mut BTreeMap::new(),
        );
        assert_eq!(other.len(), 1);
        assert_eq!((other[0].offset, other[0].size), (0, 32));
    }

    #[test]
    fn the_builtin_table_names_the_matrices_and_the_camera() {
        assert_eq!(
            builtin("unity_MatrixVP"),
            Some(ConstantRole::View(ViewParameter::ViewProjection))
        );
        assert_eq!(
            builtin("_WorldSpaceCameraPos"),
            Some(ConstantRole::View(ViewParameter::CameraPosition))
        );
        assert_eq!(builtin("_Time"), None);
    }

    #[test]
    fn a_texture_parameter_samples_the_property_else_the_shaders_default() {
        let m = material();
        let defaults = vec![
            ("_BumpMap".to_owned(), "bump".to_owned()),
            ("_Other".to_owned(), "white".to_owned()),
        ];
        let mut counts = BTreeMap::new();
        // referenced, but the world holds no texture for it
        assert_eq!(
            resolve_texture(&m, &defaults, "_MainTex", &mut counts),
            TextureSource::Missing
        );
        // unset, with a default this pass does not value
        assert_eq!(
            resolve_texture(&m, &defaults, "_BumpMap", &mut counts),
            TextureSource::Missing
        );
        assert!(counts.keys().any(|k| k.contains("\"bump\" is not valued")));
        // a property the material does not have
        assert_eq!(
            resolve_texture(&m, &defaults, "_Other", &mut counts),
            TextureSource::Missing
        );
        let mut unset = material();
        unset.textures.push(UnityTexEnv {
            name: "_Other".into(),
            referenced: false,
            texture: None,
            scale: [1.0; 2],
            offset: [0.0; 2],
        });
        assert!(matches!(
            resolve_texture(&unset, &defaults, "_Other", &mut counts),
            TextureSource::Default { value, .. } if value == [1.0; 4]
        ));
    }

    #[test]
    fn state_is_the_passes_blend_and_cull() {
        let mut m = material();
        let programs = |subshader, pass| UnityPrograms {
            shader: "S".into(),
            subshader,
            pass,
            texture_defaults: Vec::new(),
            variants: Vec::new(),
            vertex: Vec::new(),
            geometry: Vec::new(),
            hull: Vec::new(),
            domain: Vec::new(),
        };
        // alpha blending, culling back faces
        m.shader_state = vec![fixture::sub(&[], vec![fixture::own(None, 5.0, 10.0)])];
        let (s, unmapped) = render_state(&m, &programs(0, 0));
        let eq = BlendEquation {
            source: BlendFactor::SrcAlpha,
            destination: BlendFactor::OneMinusSrcAlpha,
            op: BlendOp::Add,
        };
        assert_eq!(
            (s.blend, s.cull, unmapped),
            (
                Some(Blend {
                    colour: eq,
                    alpha: eq
                }),
                Some(Cull::Back),
                false
            )
        );
        // One Zero is no blending; SrcAlphaSaturate is unstated and counted
        m.shader_state = vec![fixture::sub(
            &[],
            vec![fixture::own(None, 1.0, 0.0), fixture::own(None, 9.0, 0.0)],
        )];
        assert_eq!(render_state(&m, &programs(0, 0)).0.blend, None);
        let (s, unmapped) = render_state(&m, &programs(0, 1));
        assert_eq!((s.blend, unmapped), (None, true));
        assert_eq!(render_state(&m, &programs(3, 0)).0, RenderState::default());
    }

    #[test]
    fn options_are_required_and_checked() {
        let o = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<primase_core::pass::Options>()
        };
        assert!(ResolveUnityShaders::from_options(&o(&[("tier", "1")])).is_err());
        assert!(
            ResolveUnityShaders::from_options(&o(&[
                ("tier", "x"),
                ("keywords", "none"),
                ("colour-space", "linear")
            ]))
            .is_err()
        );
        let p = ResolveUnityShaders::from_options(&o(&[
            ("tier", "1"),
            ("keywords", "A+B"),
            ("colour-space", "gamma"),
        ]))
        .unwrap();
        assert_eq!(
            p,
            ResolveUnityShaders::new(1, vec!["A".into(), "B".into()], false)
        );
        assert_eq!(
            <ResolveUnityShaders as primase_core::pass::Pass>::options(&p)["keywords"],
            "A+B"
        );
    }
}
