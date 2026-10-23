//! `resolve-anvil-shaders{unset-textures=white|leave,vertex-layout=none|N}`: the G-buffer programs of Anvil Materials as
//! `core.shader` entities and each Material's `core.shader_binding`, by the engine's rules (Riders Republic
//! docs/material-spec.md §2, §6.2, §11, §12.2, §15.4). The one home of those rules for the binding; the existing
//! `bake-anvil-material` still carries its own recipes until it is moved onto the binding.
//!
//! - **Program.** The Material's template permutation ([`AnvilShaders`], read with `Placer::with_shaders`) for the
//!   G-buffer pass of a plain draw: the pixel key [`gbuffer_pixel_key`] composes from the Material's flags (a
//!   Material that sets a flag the template's feature word gates is left unbound, naming it), and the vertex key of
//!   `vertex-layout=N`, the vertex layout number of the meshes drawn with it (bits 17 to 21 of the key: the mesh's, not
//!   the Material's, so the caller states it; `none` binds the pixel stage only). Each program's interface is lifted
//!   from its DXBC (the containers are stripped of their resource definitions, so its resources are its declarations').
//! - **Constants.** `cb3` is the Material's property vector: each of the template's bindings of the Material's
//!   properties at the vec4 it states (a scalar its `.x`, a vector its four, a UV transform the two rows
//!   `(su cos t, -su sin t, tu, 0)` and `(sv sin t, sv cos t, tv, 0)` where the speeds are 0, as the words of the
//!   Material's property, else the template's default for the binding). A property neither sets, and a UV transform
//!   that moves or rotates (the time term and the product order are not established), is `Unknown` with the reason.
//!   The rest of `cb3` and the other constant buffers (`cb0` per frame, `cb1` per view, `cb2` per draw) are the
//!   engine's: `Unknown`, not resolved to a role.
//! - **Textures.** The template's texture binding of shader slot `i` is `t(75 + i)`: the texture the Material's
//!   binding resolves to (record, else TextureSet slot, else template default), as a `core.texture` entity per
//!   texture and colour space (sRGB where its TextureMap's field 25 says so). A binding resolved to none is
//!   `unset-textures`: `white` gives it white (counted as an approximation: the engine's own substitute is not read),
//!   `leave` binds it `Missing`. The engine's textures (`t < 75`, the exposure `t23`, the terrain's) are `Missing`.
//!   The sampler state is not read.
//! - **Outputs** (§3, §15.4), the G-buffer's render targets: `o0.rgb` is the albedo, `o0.w` the roughness (opaque
//!   Materials: for a blended one it is the alpha); `o1.xyz` the octahedral normal and shading class byte, which no
//!   normal encoding of the vocabulary states (`Unknown(1)`); `o1.w` the metalness (shading class 1) or the occlusion
//!   (classes 3 to 5) by the class its template writes ([`shading_class`], the notes' table of the templates it covers;
//!   a template it does not cover leaves `o1.w` `Unknown(1)`); `o2` and `o3` `Unknown`.
//! - **State.** `TwoSided` is no culling; `ZTestDisabled` an always-passing depth test, with the write the Material's
//!   `ZWriteDisabledOpaque` (blend mode 0) or `ZWriteDisabledAlpha` states; `AlphaTestEnabled` an alpha test, a pixel
//!   kept where `alpha >= AlphaTestValue / 255` (the engine's division). What the Material does not state stays `None`.
//!
//! Inputs of the pixel stage are the vertex stage's outputs by label (`Previous`) where it is bound, else `Unknown`;
//! the vertex stage's are mesh streams by their semantic name.
use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::pbr::Filters;
use primase_core::shader::{
    AlphaTest, Compare, ConstantBinding, ConstantRole, Cull, DepthState, InputBinding, InputRole, Interface,
    OutputBinding, OutputRole, RenderState, ResourceId, ResourceKind, SamplerState, Shader, ShaderBinding,
    ShaderFormat, Stage, StageBinding, TextureBinding, TextureDim, TextureSource,
};
use primase_core::{ColorSpace, Component, Entity, Name, Semantic, Texture, World};

use super::gbuffer_pixel_key;
use crate::components::{
    AnvilMaterial, AnvilShaders, AnvilTextureMap, MaterialBinding, MaterialFlags, MaterialValue,
};

/// What a texture binding with no texture becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsetTexture {
    /// The slot reads white, counted as an approximation.
    White,
    /// The slot is `Missing`.
    Leave,
}

/// See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolveAnvilShaders {
    unset: UnsetTexture,
    vertex_layout: Option<u32>,
}

impl ResolveAnvilShaders {
    pub fn new(unset: UnsetTexture, vertex_layout: Option<u32>) -> Self {
        ResolveAnvilShaders { unset, vertex_layout }
    }

    /// From its options (both required), through [`ResolveAnvilShaders::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let (mut unset, mut layout) = (None, None);
        for (k, v) in o {
            match k.as_str() {
                "unset-textures" => {
                    unset = Some(match v.as_str() {
                        "white" => UnsetTexture::White,
                        "leave" => UnsetTexture::Leave,
                        _ => return Err(format!("unset-textures={v} is not white or leave")),
                    });
                }
                "vertex-layout" => {
                    layout = Some(match v.as_str() {
                        "none" => None,
                        n => match n.parse::<u32>() {
                            Ok(n) if n < 32 => Some(n),
                            _ => return Err(format!("vertex-layout={v} is not none or a number below 32")),
                        },
                    });
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Self::new(
            unset.ok_or("unset-textures is required: what a texture binding with no texture reads is the program's choice")?,
            layout.ok_or("vertex-layout is required: the vertex layout number of the meshes drawn (bits 17 to 21 of the vertex key), or none")?,
        ))
    }
}

/// The shading class byte a template's G-buffer programs write (docs/material-spec.md §14, §15.4, shaders notes):
/// 1 for layout A (metalness in `o1.w`), 3 to 5 for layout B (occlusion in `o1.w`). Only the templates the notes
/// name; `None` for any other.
pub fn shading_class(template: &str) -> Option<u8> {
    match template {
        "MAD_Tex1_TerrainBlend" => Some(3),
        "MAD_Clutters_Grass" => Some(4),
        "MAD_Tex1_Leaves" | "MAD_Bark" | "MAD_Tex1_Rocks" | "MAD_FakeForest" => Some(5),
        "MAD_Fake"
        | "MAD_Fake_Sediment"
        | "MAD_Tex1"
        | "MAD_Tex1_Wind"
        | "MAD_Decals1"
        | "MAD_Tex1_Emissive"
        | "MAD_Tex1_UGE"
        | "MAD_Cliffs_TriPlanar"
        | "MAD_Tex1_UniqueNormal"
        | "MAD_Tex1_UniqueNormal_DiffuseBlend"
        | "MAD_Tex1_Graffiti_Monster"
        | "MAD_Tex1_Graffiti_Mode" => Some(1),
        t if t.starts_with("MAD_CHR_") => Some(1),
        _ => None,
    }
}

/// The fixed-function state a Material's flags state (see the module documentation).
pub fn render_state(f: &MaterialFlags) -> RenderState {
    let set = |j: usize| f.booleans.get(j).copied() == Some(1);
    let mut state = RenderState::default();
    if set(MaterialFlags::TWO_SIDED) {
        state.cull = Some(Cull::None);
    }
    if set(MaterialFlags::Z_TEST_DISABLED) {
        let write_disabled = if f.blend_mode == 0 {
            set(MaterialFlags::Z_WRITE_DISABLED_OPAQUE)
        } else {
            set(MaterialFlags::Z_WRITE_DISABLED_ALPHA)
        };
        state.depth = Some(DepthState {
            test: Compare::Always,
            write: !write_disabled,
        });
    }
    if set(MaterialFlags::ALPHA_TEST_ENABLED) {
        state.alpha_test = Some(AlphaTest {
            compare: Compare::GreaterEqual,
            reference: f32::from(f.alpha_test_value) / 255.0,
        });
    }
    state
}

/// The words of a float.
fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|v| v.to_bits()).collect()
}

/// What the Material's `cb3` bindings give, as constants of the buffer `cb3` (of `size` bytes), and what it counted:
/// each template binding's property (the Material's value, else the template's default for the binding) at its row,
/// `Unknown` where it has none or is a UV transform whose rows are not established. The rows of `cb3` no binding
/// covers are `Unknown` too, so every byte of the buffer has a role.
pub fn material_constants(
    m: &AnvilMaterial,
    cb3: ResourceId,
    size: u32,
    counts: &mut BTreeMap<String, u64>,
) -> Vec<ConstantBinding> {
    let value = |key: u32, binding: MaterialBinding| {
        m.values
            .iter()
            .find(|(k, v)| *k == key && !matches!(v, MaterialValue::Texture { .. }))
            .map(|(_, v)| v)
            .or_else(|| m.defaults.iter().find(|(b, _)| *b == binding).map(|(_, v)| v))
    };
    let mut out: Vec<ConstantBinding> = Vec::new();
    let mut push = |offset: u32, bytes: u32, role: ConstantRole, key: u32| {
        if offset.checked_add(bytes).is_some_and(|end| end <= size) {
            out.push(ConstantBinding {
                resource: cb3,
                offset,
                size: bytes,
                name: Some(format!("{key:08X}")),
                role,
            });
        }
    };
    let unknown = |why: &str, counts: &mut BTreeMap<String, u64>| {
        *counts.entry(why.to_owned()).or_default() += 1;
        ConstantRole::Unknown { why: why.to_owned() }
    };
    for &(key, binding) in &m.bindings {
        match binding {
            MaterialBinding::Scalar { vec4 } => {
                let role = match value(key, binding) {
                    Some(MaterialValue::Float(x)) => ConstantRole::Material(bits(&[*x])),
                    Some(MaterialValue::Int(i)) => ConstantRole::Material(vec![*i as u32]),
                    _ => unknown(
                        "a bound scalar property neither the Material nor its template sets",
                        counts,
                    ),
                };
                push(vec4.saturating_mul(16), 4, role, key);
            }
            MaterialBinding::Vector { vec4 } => {
                let role = match value(key, binding) {
                    Some(MaterialValue::Float4(v)) => ConstantRole::Material(bits(v)),
                    _ => unknown(
                        "a bound vector property neither the Material nor its template sets",
                        counts,
                    ),
                };
                push(vec4.saturating_mul(16), 16, role, key);
            }
            MaterialBinding::UvTransform { vec4 } => {
                let role = match value(key, binding) {
                    Some(MaterialValue::Floats(f)) => match f.as_slice() {
                        &[su, sv, tu, tv, speed_u, speed_v, t, speed_t] => {
                            if (speed_u, speed_v, speed_t) != (0.0, 0.0, 0.0) {
                                unknown(
                                    "a UV transform that moves: its time term is not established",
                                    counts,
                                )
                            } else if t != 0.0 {
                                unknown(
                                    "a UV transform that rotates: its product order is not established",
                                    counts,
                                )
                            } else {
                                ConstantRole::Material(bits(&[su, 0.0, tu, 0.0, 0.0, sv, tv, 0.0]))
                            }
                        }
                        _ => unknown("a UV transform property that is not eight floats", counts),
                    },
                    _ => unknown(
                        "a bound UV transform neither the Material nor its template sets",
                        counts,
                    ),
                };
                push(vec4.saturating_mul(16), 32, role, key);
            }
            MaterialBinding::Texture { .. } | MaterialBinding::Other => {}
        }
    }
    // every byte of the buffer has a role: the rows no binding covers are the engine's or the template's, unresolved
    out.sort_by_key(|c| c.offset);
    let mut gaps = Vec::new();
    let mut at = 0u32;
    for c in &out {
        if c.offset > at {
            gaps.push((at, c.offset - at));
        }
        at = at.max(c.offset.saturating_add(c.size));
    }
    if at < size {
        gaps.push((at, size - at));
    }
    for (offset, bytes) in gaps {
        out.push(ConstantBinding {
            resource: cb3,
            offset,
            size: bytes,
            name: None,
            role: ConstantRole::Unknown {
                why: "no template binding at these rows of cb3 (the engine's or unresolved)".into(),
            },
        });
    }
    out.sort_by_key(|c| c.offset);
    out
}

/// The register of a resource of class `class` in `interface`, with its id.
fn by_register(
    interface: &Interface,
    class: primase_core::shader::D3dClass,
) -> Vec<(u32, &primase_core::shader::Resource)> {
    interface
        .resources
        .iter()
        .filter_map(|r| {
            let b = r.provenance.d3d?;
            (b.class == class).then_some((b.register, r))
        })
        .collect()
}

/// A stage's mesh-stream semantic by its signature name.
fn stream(name: &str, index: u32) -> Option<Semantic> {
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

/// What resolving a Material needs besides itself, from the pass: texture entities by (texture, sRGB), spawned on
/// first use.
struct Textures {
    spawned: BTreeMap<(Entity, bool), Entity>,
}

impl Textures {
    /// The entity to bind for the TextureMap entity `e`: itself where its colour space is the sRGB-ness its
    /// TextureMap states, else a copy in that colour space. `None` where `e` holds no `core.texture`.
    fn entity(&mut self, world: &mut World, e: Entity) -> Option<Entity> {
        let srgb = world.get::<AnvilTextureMap>(e).is_some_and(|t| t.field_25 == 1);
        let have = world.get::<Texture>(e)?;
        let wanted = if srgb {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        };
        if have.color_space == wanted {
            return Some(e);
        }
        if let Some(&c) = self.spawned.get(&(e, srgb)) {
            return Some(c);
        }
        let mut copy = have.clone();
        copy.color_space = wanted;
        let name = world.get::<Name>(e).map_or_else(String::new, |n| n.0.clone());
        let c = world.spawn();
        world.insert(c, Name(if srgb { format!("{name}@srgb") } else { name }));
        world.insert(c, copy);
        self.spawned.insert((e, srgb), c);
        Some(c)
    }
}

impl Pass for ResolveAnvilShaders {
    fn name(&self) -> &'static str {
        "resolve-anvil-shaders"
    }
    fn options(&self) -> Options {
        Options::from([
            (
                "unset-textures".to_owned(),
                match self.unset {
                    UnsetTexture::White => "white",
                    UnsetTexture::Leave => "leave",
                }
                .to_owned(),
            ),
            (
                "vertex-layout".to_owned(),
                self.vertex_layout.map_or("none".to_owned(), |n| n.to_string()),
            ),
        ])
    }
    fn requires(&self) -> &[&'static str] {
        &[AnvilMaterial::NAME, AnvilShaders::NAME]
    }
    fn provides(&self) -> &[&'static str] {
        &[Shader::NAME, ShaderBinding::NAME, Texture::NAME, Name::NAME]
    }
    fn approximates(&self) -> &'static str {
        "a texture binding with no texture reads white (unset-textures=white)"
    }
    fn run(&self, world: &mut World, log: &mut PassLog) -> Result<(), String> {
        let materials: Vec<(Entity, AnvilMaterial)> = world
            .iter::<AnvilMaterial>()
            .map(|(e, m)| (e, m.clone()))
            .collect();
        let mut programs: BTreeMap<(Entity, bool, u32), Result<Entity, String>> = BTreeMap::new();
        let mut textures = Textures {
            spawned: BTreeMap::new(),
        };
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        let bump = |what: &str, n: u64, counts: &mut BTreeMap<String, u64>| {
            *counts.entry(what.to_owned()).or_default() += n;
        };
        'materials: for (material, m) in materials {
            let Some(tables) = m.shaders else {
                bump(
                    "materials without their template's shader permutations read",
                    1,
                    &mut counts,
                );
                continue;
            };
            let template = m.template_name.clone().unwrap_or_default();
            let pixel_key = match gbuffer_pixel_key(&m.flags) {
                Ok(k) => k,
                Err(why) => {
                    bump(&format!("materials left unbound: {why}"), 1, &mut counts);
                    continue;
                }
            };
            let vertex_key = self.vertex_layout.map(|n| pixel_key | (n << 17));
            let mut stages = Vec::new();
            let mut pixel_shader = None;
            for (is_vertex, key) in [(false, Some(pixel_key)), (true, vertex_key)] {
                let Some(key) = key else { continue };
                let entry = programs.entry((tables, is_vertex, key));
                let found = match entry {
                    std::collections::btree_map::Entry::Occupied(o) => o.get().clone(),
                    std::collections::btree_map::Entry::Vacant(v) => {
                        let bytes = world.get::<AnvilShaders>(tables).and_then(|t| {
                            if is_vertex { t.vertex(key) } else { t.pixel(key) }.map(<[u8]>::to_vec)
                        });
                        let made = match bytes {
                            None => Err(format!(
                                "the template has no {} permutation for the G-buffer key",
                                if is_vertex { "vertex" } else { "pixel" }
                            )),
                            Some(bytes) => match primase_dxbc::lift(&bytes) {
                                Err(e) => Err(format!(
                                    "a {} program the lifter refuses: {}",
                                    if is_vertex { "vertex" } else { "pixel" },
                                    primase_dxbc::refusal_reason(&e)
                                )),
                                Ok(program) => {
                                    let e = world.spawn();
                                    world.insert(
                                        e,
                                        Name(format!(
                                            "{template}-{key:06x}.{}",
                                            if is_vertex { "vs" } else { "ps" }
                                        )),
                                    );
                                    world.insert(
                                        e,
                                        Shader {
                                            format: ShaderFormat::Dxbc,
                                            bytecode: bytes,
                                            interface: program.interface,
                                        },
                                    );
                                    Ok(e)
                                }
                            },
                        };
                        v.insert(made.clone());
                        made
                    }
                };
                match found {
                    Ok(e) => {
                        if is_vertex {
                            stages.push((Stage::Vertex, e));
                        } else {
                            pixel_shader = Some(e);
                        }
                    }
                    Err(why) if !is_vertex => {
                        bump(&format!("materials left unbound: {why}"), 1, &mut counts);
                        continue 'materials;
                    }
                    Err(why) => bump(
                        &format!("materials whose vertex stage is left unbound: {why}"),
                        1,
                        &mut counts,
                    ),
                }
            }
            if let Some(p) = pixel_shader {
                stages.push((Stage::Pixel, p));
            }
            let vertex_bound = stages.iter().any(|(s, _)| *s == Stage::Vertex);
            let mut bound = Vec::new();
            for (stage, shader) in stages {
                let interface = world
                    .get::<Shader>(shader)
                    .map(|s| s.interface.clone())
                    .ok_or("a resolved shader entity without its shader")?;
                bound.push(self.stage_binding(
                    world,
                    &mut textures,
                    &m,
                    &template,
                    stage,
                    shader,
                    &interface,
                    vertex_bound,
                    &mut counts,
                ));
            }
            bound.sort_by_key(|s| s.stage);
            world.insert(
                material,
                ShaderBinding {
                    stages: bound,
                    state: render_state(&m.flags),
                },
            );
            bump("materials bound", 1, &mut counts);
        }
        for (what, n) in counts {
            if what.starts_with("texture bindings with no texture, bound white") {
                log.approximate(&what, n);
            } else {
                log.count(&what, n);
            }
        }
        log.count(
            "shader programs",
            programs.values().filter(|r| r.is_ok()).count() as u64,
        );
        Ok(())
    }
}

impl ResolveAnvilShaders {
    #[allow(clippy::too_many_arguments)]
    fn stage_binding(
        &self,
        world: &mut World,
        textures: &mut Textures,
        m: &AnvilMaterial,
        template: &str,
        stage: Stage,
        shader: Entity,
        interface: &Interface,
        vertex_bound: bool,
        counts: &mut BTreeMap<String, u64>,
    ) -> StageBinding {
        let bump = |what: &str, counts: &mut BTreeMap<String, u64>| {
            *counts.entry(what.to_owned()).or_default() += 1;
        };
        // inputs
        let inputs = interface
            .inputs
            .iter()
            .filter(|v| v.system.is_none())
            .map(|v| InputBinding {
                label: v.label.clone(),
                role: match stage {
                    Stage::Vertex => match stream(&v.label.name, v.label.index) {
                        Some(s) => InputRole::Stream(s),
                        None => InputRole::Unknown {
                            why: format!(
                                "vertex input {} is not a mesh semantic this pass maps",
                                v.label.name
                            ),
                        },
                    },
                    Stage::Pixel if vertex_bound => InputRole::Previous,
                    Stage::Pixel => InputRole::Unknown {
                        why: "the vertex stage is not bound (vertex-layout=none)".into(),
                    },
                    _ => InputRole::Unknown {
                        why: "a stage this pass does not bind".into(),
                    },
                },
            })
            .collect();
        // constants: cb3 the Material's, the rest the engine's
        let mut constants = Vec::new();
        for (register, r) in by_register(interface, primase_core::shader::D3dClass::ConstantBuffer) {
            let ResourceKind::ConstantBuffer { layout } = &r.kind else {
                continue;
            };
            if register == 3 {
                constants.extend(material_constants(m, r.id, layout.size, counts));
            } else if layout.size > 0 {
                constants.push(ConstantBinding {
                    resource: r.id,
                    offset: 0,
                    size: layout.size,
                    name: None,
                    role: ConstantRole::Unknown {
                        why: format!("cb{register} is the engine's per-frame, per-view or per-draw constants: no role resolved"),
                    },
                });
            }
        }
        // textures
        let mut bindings = Vec::new();
        for (register, r) in by_register(interface, primase_core::shader::D3dClass::ShaderResource) {
            let ResourceKind::Texture { dim, .. } = r.kind else {
                continue;
            };
            let slot = register.checked_sub(75);
            let bound = slot.and_then(|s| m.textures.iter().find(|t| t.shader_slot == s));
            let source = match (bound, dim) {
                (None, _) => {
                    bump("texture slots that are the engine's, left Missing", counts);
                    TextureSource::Missing
                }
                (Some(_), d) if d != TextureDim::D2 => {
                    bump(
                        "texture bindings of a dimension other than 2D, left Missing",
                        counts,
                    );
                    TextureSource::Missing
                }
                (Some(b), _) => match b.texture.and_then(|e| textures.entity(world, e)) {
                    Some(texture) => {
                        bump("texture bindings bound", counts);
                        TextureSource::Texture {
                            texture,
                            sampler: SamplerState {
                                wrap: [None; 3],
                                filters: Filters::default(),
                            },
                        }
                    }
                    None => match self.unset {
                        UnsetTexture::White => {
                            bump(
                                "texture bindings with no texture, bound white (unset-textures=white)",
                                counts,
                            );
                            TextureSource::Default {
                                value: [1.0; 4],
                                why: "no texture bound: white (unset-textures=white)".into(),
                            }
                        }
                        UnsetTexture::Leave => {
                            bump("texture bindings with no texture, left Missing", counts);
                            TextureSource::Missing
                        }
                    },
                },
            };
            bindings.push(TextureBinding {
                resource: r.id,
                sampler: None,
                source,
            });
        }
        // outputs: the G-buffer's targets
        let mut outputs = Vec::new();
        if stage == Stage::Pixel {
            let class = shading_class(template);
            let opaque = m.flags.blend_mode == 0;
            for v in interface.outputs.iter().filter(|v| v.system.is_none()) {
                let full: u8 = (1u16 << v.value.width).saturating_sub(1) as u8;
                let mut role = |lanes: u8, role: OutputRole| {
                    let lanes = lanes & full;
                    if lanes != 0 {
                        outputs.push(OutputBinding {
                            label: v.label.clone(),
                            lanes,
                            role,
                        });
                    }
                };
                match v.label.index {
                    0 => {
                        role(0b0111, OutputRole::Albedo);
                        role(
                            0b1000,
                            if opaque {
                                OutputRole::Roughness
                            } else {
                                OutputRole::Unknown(0)
                            },
                        );
                    }
                    1 => {
                        role(0b0111, OutputRole::Unknown(1));
                        role(
                            0b1000,
                            match class {
                                Some(1) => OutputRole::Metallic,
                                Some(3..=5) => OutputRole::Occlusion,
                                _ => OutputRole::Unknown(1),
                            },
                        );
                    }
                    n => role(0b1111, OutputRole::Unknown(n)),
                }
            }
            if class.is_none() {
                bump(
                    "pixel stages of a template whose shading class is not in the notes' table (o1.w Unknown)",
                    counts,
                );
            }
        }
        StageBinding {
            stage,
            shader,
            inputs,
            constants,
            textures: bindings,
            outputs,
        }
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
    use primase_core::frame::SourceFrame;

    fn material(
        bindings: Vec<(u32, MaterialBinding)>,
        values: Vec<(u32, MaterialValue)>,
        defaults: Vec<(MaterialBinding, MaterialValue)>,
    ) -> AnvilMaterial {
        AnvilMaterial {
            index_id: 1,
            template: None,
            template_name: Some("MAD_Tex1".into()),
            bindings,
            values,
            texture_set: Vec::new(),
            textures: Vec::new(),
            defaults,
            frame: SourceFrame::WORLD,
            flags: MaterialFlags {
                blend_mode: 0,
                alpha_display_mode: 0,
                alpha_test_value: 0,
                booleans: [0; 35],
            },
            shaders: None,
        }
    }

    /// A scalar at row 1, a vector at row 2, a UV transform at rows 4 and 5 of a 6-row buffer: the Material sets the
    /// scalar, the template defaults the vector, nothing sets the transform. Every byte of the buffer has a role.
    #[test]
    fn cb3_rows_follow_the_templates_bindings_and_leave_nothing_unroled() {
        let m = material(
            vec![
                (0xA, MaterialBinding::Scalar { vec4: 1 }),
                (0xB, MaterialBinding::Vector { vec4: 2 }),
                (0xC, MaterialBinding::UvTransform { vec4: 4 }),
            ],
            vec![(0xA, MaterialValue::Float(0.25))],
            vec![(
                MaterialBinding::Vector { vec4: 2 },
                MaterialValue::Float4([1.0, 2.0, 3.0, 4.0]),
            )],
        );
        let mut counts = BTreeMap::new();
        let c = material_constants(&m, ResourceId(0), 6 * 16, &mut counts);
        let at = |offset: u32| c.iter().find(|c| c.offset == offset).unwrap();
        assert_eq!(at(16).role, ConstantRole::Material(vec![0.25f32.to_bits()]));
        assert_eq!((at(16).size, at(16).name.as_deref()), (4, Some("0000000A")));
        assert_eq!(
            at(32).role,
            ConstantRole::Material(bits(&[1.0, 2.0, 3.0, 4.0])),
            "the template's default where the Material sets none"
        );
        assert!(
            matches!(at(64).role, ConstantRole::Unknown { .. }),
            "a transform nothing sets"
        );
        assert_eq!(at(64).size, 32);
        // the bytes between and around (row 0, the rest of row 1, row 3) are Unknown gaps: the whole buffer is covered
        let mut covered = [false; 96];
        for b in &c {
            for byte in b.offset..b.offset + b.size {
                assert!(!covered[byte as usize], "byte {byte} twice");
                covered[byte as usize] = true;
            }
        }
        assert!(covered.iter().all(|&x| x));
        assert_eq!(
            counts["a bound UV transform neither the Material nor its template sets"],
            1
        );
    }

    /// A static UV transform's rows, and the two forms whose rows are not established.
    #[test]
    fn uv_transform_rows_are_exact_only_where_the_notes_show_them() {
        let transform = |floats: Vec<f32>| {
            let m = material(
                vec![(1, MaterialBinding::UvTransform { vec4: 0 })],
                vec![(1, MaterialValue::Floats(floats))],
                Vec::new(),
            );
            material_constants(&m, ResourceId(0), 48, &mut BTreeMap::new())[0]
                .role
                .clone()
        };
        // su 2, sv 3, tu 0.5, tv -1: rows (2, 0, 0.5, 0) and (0, 3, -1, 0)
        assert_eq!(
            transform(vec![2.0, 3.0, 0.5, -1.0, 0.0, 0.0, 0.0, 0.0]),
            ConstantRole::Material(bits(&[2.0, 0.0, 0.5, 0.0, 0.0, 3.0, -1.0, 0.0]))
        );
        for moving in [
            vec![1.0, 1.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.0],
            vec![1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.3, 0.0],
            vec![1.0, 1.0, 0.0],
        ] {
            assert!(matches!(transform(moving), ConstantRole::Unknown { .. }));
        }
    }

    #[test]
    fn state_is_what_the_flags_state() {
        let mut f = material(Vec::new(), Vec::new(), Vec::new()).flags;
        assert_eq!(render_state(&f), RenderState::default());
        f.booleans[MaterialFlags::TWO_SIDED] = 1;
        f.booleans[MaterialFlags::Z_TEST_DISABLED] = 1;
        f.booleans[MaterialFlags::Z_WRITE_DISABLED_OPAQUE] = 1;
        f.booleans[MaterialFlags::ALPHA_TEST_ENABLED] = 1;
        f.alpha_test_value = 51;
        let s = render_state(&f);
        assert_eq!(s.cull, Some(Cull::None));
        assert_eq!(
            s.depth,
            Some(DepthState {
                test: Compare::Always,
                write: false
            })
        );
        assert_eq!(
            s.alpha_test,
            Some(AlphaTest {
                compare: Compare::GreaterEqual,
                reference: 0.2
            })
        );
        // a blended Material's depth write follows the alpha flag
        f.blend_mode = 2;
        assert!(render_state(&f).depth.unwrap().write);
        f.booleans[MaterialFlags::Z_WRITE_DISABLED_ALPHA] = 1;
        assert!(!render_state(&f).depth.unwrap().write);
    }

    #[test]
    fn the_shading_class_is_the_notes_table_only() {
        assert_eq!(shading_class("MAD_Tex1_Rocks"), Some(5));
        assert_eq!(shading_class("MAD_Tex1_TerrainBlend"), Some(3));
        assert_eq!(shading_class("MAD_Clutters_Grass"), Some(4));
        assert_eq!(shading_class("MAD_Tex1"), Some(1));
        assert_eq!(shading_class("MAD_CHR_5Zones_Details"), Some(1));
        assert_eq!(shading_class("MAD_Road"), None);
    }

    #[test]
    fn options_are_required_and_checked() {
        let o = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<Options>()
        };
        assert!(ResolveAnvilShaders::from_options(&o(&[])).is_err());
        assert!(ResolveAnvilShaders::from_options(&o(&[("unset-textures", "white")])).is_err());
        assert!(
            ResolveAnvilShaders::from_options(&o(&[("unset-textures", "white"), ("vertex-layout", "40")]))
                .is_err()
        );
        let p = ResolveAnvilShaders::from_options(&o(&[("unset-textures", "leave"), ("vertex-layout", "2")]))
            .unwrap();
        assert_eq!(p, ResolveAnvilShaders::new(UnsetTexture::Leave, Some(2)));
        assert_eq!(p.options()["vertex-layout"], "2");
    }
}
