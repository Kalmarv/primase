//! `resolve-anvil-shaders{unset-textures=white|leave}`: the G-buffer programs of Anvil Materials as
//! `core.shader` entities and each Material's `core.shader_binding`, by the engine's rules (Riders Republic
//! docs/material-spec.md §2, §6.2, §11, §12.2, §15.4). The one home of those rules for the binding; the existing
//! `bake-anvil-material` still carries its own recipes until it is moved onto the binding.
//!
//! - **Program.** The Material's template permutation ([`AnvilShaders`], read with `Placer::with_shaders`) for the
//!   G-buffer pass of a plain draw: the pixel key [`gbuffer_key`] composes from the Material's flags (a
//!   Material that sets a flag the template's feature word gates is left unbound, naming it), and a vertex key for each
//!   vertex layout number (bits 17 to 21 of the key: the mesh's, not the Material's) of the meshes drawn with the
//!   Material, read from the data: the `kind` of every `anvil.mesh` that lists the Material among its ranges' Materials, or of
//!   every lowered mesh's primitive that draws it (its Material, or one of its variants'; `anvil.layout`), so the pass
//!   binds the same stages before `lower-anvil-mesh` and after it.
//!   A Material drawn with several layouts has a vertex stage for each, each a [`VariantKey`] (`anvil.layout.kind` = the
//!   layout) a consumer matches to the mesh it draws, refusing a mesh whose layout has no stage; a Material no mesh is
//!   drawn with has the pixel stage only (counted). Each program's interface is lifted from its DXBC (the containers are
//!   stripped of their resource definitions, so its resources are its declarations').
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
//!   Materials: for a blended one it is the alpha); `o1.xyz` the world normal, `OctahedralDiamond` (the lighting pass
//!   decodes the 9-bit diamond octahedral from the three lanes, its sign of z in lane 2's bit 0 and the shading class
//!   in the lane's upper five bits: `NormalEncoding::decode_octahedral_diamond`, material-spec §15.4; the class byte is
//!   not part of the normal); `o1.w` the metalness (shading class 1) or the occlusion
//!   (classes 3 to 5) by the class its template writes ([`shading_class`], the notes' table of the templates it covers;
//!   a template it does not cover leaves `o1.w` `Unknown(1)`); `o2` and `o3` `Unknown`.
//! - **State.** `TwoSided` is no culling; `ZTestDisabled` an always-passing depth test, with the write the Material's
//!   `ZWriteDisabledOpaque` (blend mode 0) or `ZWriteDisabledAlpha` states; `AlphaTestEnabled` an alpha test, a pixel
//!   kept where `alpha >= AlphaTestValue / 255` (the engine's division). What the Material does not state stays `None`.
//!   The template states none: a MaterialTemplate holds its programs, their keys and the parameters they read, and a
//!   pipeline-state database that is null (`PSODB`, docs/registry-named-fields.md §12: the `CompiledMaterialTemplate`
//!   field list read from the running image, null in the template examined; the shader code permutations carry a
//!   `LowestShaderConfig` and a hash, no blend, rasterizer or depth-stencil record). The engine composes the state per
//!   pass from the Material's flags (docs/shaders.md §6.2: "applied by one method that switches on six pass kinds"),
//!   which is the part mapped here. Not stated, so `None`: the blend (the notes infer source alpha over inverse source
//!   alpha for the deferred decals and do not establish it), the fill and the winding. A census of the 432 template
//!   bodies of the six forges (outside the code frames) holds this: between the last code frame and the first
//!   permutation record there are 0 bytes in 432 of 432 (no pipeline-state object), the 59 bytes after the last record
//!   are the same length in 432 of 432 (the two colours and the category), and the 135 distinct registry classes found
//!   in the heads are all material-graph nodes, lighting and shade-constant sources and the compiled-template
//!   structure; none is a pipeline-state class (`SerializedPSODB`, `PSOEntry*`, `PSOBlob`: 0 of 432), a blend,
//!   rasterizer or depth-stencil one (`OperatorBlendFloats` and its kin blend values in the graph). The graph's own
//!   bytes are not parsed whole, so this is a census of classes and gaps, not an account of every byte.
//!
//! Inputs of the pixel stage are the vertex stage's outputs by label (`Previous`) where it is bound, else `Unknown`;
//! the vertex stage's are mesh streams by their semantic name.
use std::collections::BTreeMap;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::shader::{
    AlphaTest, Compare, ConstantBinding, ConstantRole, Cull, DepthState, InputBinding, InputRole, Interface,
    NormalEncoding, NormalSpace, OutputBinding, OutputRole, RenderState, ResourceId, ResourceKind,
    SamplerState, Shader, ShaderBinding, ShaderFormat, Stage, StageBinding, SystemValue, TextureBinding,
    TextureDim, TextureSource, VariantKey, Varying,
};
use primase_core::{ColorSpace, Component, Entity, MaterialVariants, Mesh, Name, Semantic, Texture, World};

use crate::components::{
    AnvilLayout, AnvilMaterial, AnvilMesh, AnvilShaders, AnvilTextureMap, MaterialBinding, MaterialFlags,
    MaterialValue,
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
}

impl ResolveAnvilShaders {
    pub fn new(unset: UnsetTexture) -> Self {
        ResolveAnvilShaders { unset }
    }

    /// From its option (required), through [`ResolveAnvilShaders::new`].
    pub fn from_options(o: &Options) -> Result<Self, String> {
        let mut unset = None;
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
                    return Err(
                        "vertex-layout is gone: each mesh's vertex stage is bound by its own layout (anvil.layout.kind)"
                            .into(),
                    );
                }
                _ => return Err(format!("unknown option {k}")),
            }
        }
        Ok(Self::new(unset.ok_or(
            "unset-textures is required: what a texture binding with no texture reads is the program's choice",
        )?))
    }
}

/// The G-buffer pass's number, in the key's bits 22 to 29.
const GBUFFER: u32 = 1;

/// The pixel ShaderKey the engine's key composer gives the G-buffer pass of a plain draw of a Material with the flags
/// `f` of a template with the features `tables` (docs/shaders.md §6.2, read from the engine): the pass in bits 22 to
/// 29, then each Material flag sets its bit only where the template's feature word allows it:
/// - `0x40`: `TwoSided` and `FlipNormalOnBackFace` both set, and feature 12;
/// - `0x2`: `AlphaDisplayMode` 2, and feature 13;
/// - `0x4`: `LayerMaterial`, and feature 10; `0x20` also with a blend mode other than 0 (only with `0x4`);
/// - `0x10`: boolean 34 (`42452A12`), and features 3 and 4 (`T & 0x18 == 0x18`).
///
/// The draw's bits (vertex pulling, reduced material levels, the mesh's per-instance features, the eight variant
/// subsets) are those of a plain draw: none. A template whose feature bytes are not 0 or 1 is refused. (The
/// `bake-anvil-material` pass has its own [`super::gbuffer_pixel_key`], which refuses the gated flags instead.)
pub fn gbuffer_key(f: &MaterialFlags, tables: &AnvilShaders) -> Result<u32, String> {
    if f.blend_mode > 0xF || f.alpha_display_mode > 3 {
        return Err("a blend or alpha display mode wider than the engine's field".into());
    }
    let mut set = [false; 35];
    for (s, &b) in set.iter_mut().zip(&f.booleans) {
        *s = match b {
            0 => false,
            1 => true,
            _ => return Err("a boolean stored as neither 0 nor 1".into()),
        };
    }
    let feature = |n: usize| {
        tables
            .feature(n)
            .ok_or_else(|| format!("the template's shader-state node has no byte for feature {n}"))
    };
    let mut key = GBUFFER << 22;
    if set[MaterialFlags::TWO_SIDED] && set[MaterialFlags::FLIP_NORMAL_ON_BACK_FACE] && feature(12)? {
        key |= 0x40;
    }
    if f.alpha_display_mode == 2 && feature(13)? {
        key |= 0x2;
    }
    if set[MaterialFlags::LAYER_MATERIAL] && feature(10)? {
        key |= 0x4;
        if f.blend_mode != 0 {
            key |= 0x20;
        }
    }
    if set[MaterialFlags::BOOLEAN_42452A12] && feature(3)? && feature(4)? {
        key |= 0x10;
    }
    Ok(key)
}

/// The vertex key of the pair a pixel key `pixel` belongs to, for vertex layout `layout`: the Material-flag bits (1 to
/// 5, and 0) occur in the pixel stage's keys only, the layout number (bits 17 to 21) in the vertex stage's only.
pub fn gbuffer_vertex_key(pixel: u32, layout: u32) -> u32 {
    (pixel & !0x3F) | (layout << 17)
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
        Options::from([(
            "unset-textures".to_owned(),
            match self.unset {
                UnsetTexture::White => "white",
                UnsetTexture::Leave => "leave",
            }
            .to_owned(),
        )])
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
        // the vertex layout number (`anvil.layout.kind`) of every mesh drawn with each Material
        let mut layouts_of: BTreeMap<Entity, std::collections::BTreeSet<u8>> = BTreeMap::new();
        for (_, mesh) in world.iter::<AnvilMesh>() {
            for material in mesh.material_entities.iter().flatten() {
                layouts_of.entry(*material).or_default().insert(mesh.kind);
            }
        }
        // a mesh `lower-anvil-mesh` has lowered: each primitive's layout, for the Material it draws with and the ones its
        // variants draw it with
        for (e, layout) in world.iter::<AnvilLayout>() {
            let Some(mesh) = world.get::<Mesh>(e) else {
                continue;
            };
            let variants = world.get::<MaterialVariants>(e);
            for (i, (primitive, &kind)) in mesh.primitives().iter().zip(&layout.kind).enumerate() {
                let drawn = primitive.material.into_iter().chain(
                    variants
                        .and_then(|v| v.0.get(i))
                        .into_iter()
                        .flat_map(|v| v.values().copied()),
                );
                for material in drawn {
                    layouts_of.entry(material).or_default().insert(kind);
                }
            }
        }
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
            let pixel_key = match world.get::<AnvilShaders>(tables).map_or_else(
                || Err("the template's shader tables are gone".to_owned()),
                |t| gbuffer_key(&m.flags, t),
            ) {
                Ok(k) => k,
                Err(why) => {
                    bump(&format!("materials left unbound: {why}"), 1, &mut counts);
                    continue;
                }
            };
            // the vertex layouts of the meshes drawn with this material (bits 17 to 21 of the vertex key, from the data)
            let layouts = layouts_of.get(&material).cloned().unwrap_or_default();
            if layouts.is_empty() {
                bump(
                    "materials no mesh is drawn with: bound with the pixel stage only",
                    1,
                    &mut counts,
                );
            }
            let pixel = match program(world, &mut programs, &template, tables, false, pixel_key) {
                Ok(e) => e,
                Err(why) => {
                    bump(&format!("materials left unbound: {why}"), 1, &mut counts);
                    continue 'materials;
                }
            };
            let mut stages: Vec<(Stage, Option<VariantKey>, Entity)> = Vec::new();
            for kind in layouts {
                if kind >= 32 {
                    bump(
                        &format!(
                            "meshes whose vertex layout number {kind} is past the key's five bits: no vertex stage bound"
                        ),
                        1,
                        &mut counts,
                    );
                    continue;
                }
                let key = gbuffer_vertex_key(pixel_key, u32::from(kind));
                match program(world, &mut programs, &template, tables, true, key) {
                    Ok(e) => stages.push((
                        Stage::Vertex,
                        Some(VariantKey {
                            field: MESH_LAYOUT_FIELD.to_owned(),
                            value: u32::from(kind),
                        }),
                        e,
                    )),
                    Err(why) => bump(
                        &format!("vertex layouts whose vertex stage is left unbound: {why}"),
                        1,
                        &mut counts,
                    ),
                }
            }
            let vertex_bound = !stages.is_empty();
            stages.push((Stage::Pixel, None, pixel));
            let mut bound = Vec::new();
            for (stage, variant, shader) in stages {
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
                    variant,
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

/// The mesh-side field a vertex stage's variant key names: the vertex layout number of the mesh (bits 17 to 21 of its
/// vertex key), kept per primitive of a lowered mesh ([`AnvilLayout`]).
pub const MESH_LAYOUT_FIELD: &str = "anvil.layout.kind";

/// The `core.shader` entity of the template's `key` permutation of one stage, made once and kept in `programs`.
fn program(
    world: &mut World,
    programs: &mut BTreeMap<(Entity, bool, u32), Result<Entity, String>>,
    template: &str,
    tables: Entity,
    is_vertex: bool,
    key: u32,
) -> Result<Entity, String> {
    if let Some(found) = programs.get(&(tables, is_vertex, key)) {
        return found.clone();
    }
    let stage = if is_vertex { "vertex" } else { "pixel" };
    let bytes = world
        .get::<AnvilShaders>(tables)
        .and_then(|t| if is_vertex { t.vertex(key) } else { t.pixel(key) }.map(<[u8]>::to_vec));
    let made = match bytes {
        None => Err(format!(
            "the template has no {stage} permutation for the G-buffer key"
        )),
        Some(bytes) => match primase_dxbc::lift(&bytes) {
            Err(e) => Err(format!("a {stage} program the lifter refuses: {}", e.group())),
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
    programs.insert((tables, is_vertex, key), made.clone());
    made
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
        variant: Option<VariantKey>,
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
                        why: "the vertex stage is not bound (no mesh is drawn with the material)".into(),
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
                            sampler: SamplerState::default(),
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
            outputs = output_bindings(interface, class, m.flags.blend_mode == 0);
            if class.is_none() {
                bump(
                    "pixel stages of a template whose shading class is not in the notes' table (o1.w Unknown)",
                    counts,
                );
            }
        }
        StageBinding {
            stage,
            variant,
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
        assert!(ResolveAnvilShaders::from_options(&o(&[("unset-textures", "white")])).is_ok());
        // the vertex layout is no option: each mesh's own decides
        let gone =
            ResolveAnvilShaders::from_options(&o(&[("unset-textures", "white"), ("vertex-layout", "2")]));
        assert!(gone.unwrap_err().contains("vertex-layout is gone"));
        let p = ResolveAnvilShaders::from_options(&o(&[("unset-textures", "leave")])).unwrap();
        assert_eq!(p, ResolveAnvilShaders::new(UnsetTexture::Leave));
        assert!(!p.options().contains_key("vertex-layout"));
    }
}

/// The render target a pixel output writes: a lifted program states it as the system value `SV_Target<n>`, a
/// program described by semantic as the label's index; any other system value is not a colour output.
fn target_of(v: &Varying) -> Option<u32> {
    match v.system {
        None => Some(v.label.index),
        Some(SystemValue::Target(n)) => Some(n),
        Some(_) => None,
    }
}

/// The G-buffer roles of a pixel program's targets: target 0 is albedo (rgb) and roughness (a, opaque materials
/// only), target 1 the unnamed rgb and the metallic or occlusion lane of the shading class; any other lane is Unknown.
fn output_bindings(interface: &Interface, class: Option<u8>, opaque: bool) -> Vec<OutputBinding> {
    let mut outputs = Vec::new();
    for (v, target) in interface.outputs.iter().filter_map(|v| Some((v, target_of(v)?))) {
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
        match target {
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
                role(
                    0b0111,
                    OutputRole::Normal {
                        encoding: NormalEncoding::OctahedralDiamond,
                        space: NormalSpace::World,
                    },
                );
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
    outputs
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "hand-worked fixtures")]
mod key_tests {
    use super::*;

    /// A template whose feature node has the given features set (byte 0, the object's, then one byte per feature).
    fn template(features: &[usize]) -> AnvilShaders {
        let mut node = vec![1u8; 1];
        node.resize(54, 0);
        for &n in features {
            node[n + 1] = 1;
        }
        AnvilShaders {
            pixel: Vec::new(),
            vertex: Vec::new(),
            programs: Vec::new(),
            feature_node: node,
        }
    }

    fn flags(set: &[usize], blend: u32, alpha_display: u32) -> MaterialFlags {
        let mut booleans = [0u8; 35];
        for &j in set {
            booleans[j] = 1;
        }
        MaterialFlags {
            blend_mode: blend,
            alpha_display_mode: alpha_display,
            alpha_test_value: 0,
            booleans,
        }
    }

    /// Each gated bit, with the gate open and shut (docs/shaders.md §6.2's table, worked by hand).
    #[test]
    fn a_flag_sets_its_key_bit_only_where_the_templates_feature_allows_it() {
        let plain = 1 << 22;
        let both_sides = [MaterialFlags::TWO_SIDED, MaterialFlags::FLIP_NORMAL_ON_BACK_FACE];
        // TwoSided alone never; with FlipNormalOnBackFace, 0x40 only through feature 12
        assert_eq!(
            gbuffer_key(&flags(&[MaterialFlags::TWO_SIDED], 0, 0), &template(&[12])),
            Ok(plain)
        );
        assert_eq!(gbuffer_key(&flags(&both_sides, 0, 0), &template(&[])), Ok(plain));
        assert_eq!(
            gbuffer_key(&flags(&both_sides, 0, 0), &template(&[12])),
            Ok(plain | 0x40)
        );
        // AlphaDisplayMode 2: 0x2 through feature 13
        assert_eq!(gbuffer_key(&flags(&[], 0, 2), &template(&[])), Ok(plain));
        assert_eq!(gbuffer_key(&flags(&[], 0, 2), &template(&[13])), Ok(plain | 0x2));
        assert_eq!(gbuffer_key(&flags(&[], 0, 1), &template(&[13])), Ok(plain));
        // LayerMaterial: 0x4 through feature 10, with 0x20 beside it for a blend mode
        let layer = [MaterialFlags::LAYER_MATERIAL];
        assert_eq!(gbuffer_key(&flags(&layer, 0, 0), &template(&[])), Ok(plain));
        assert_eq!(
            gbuffer_key(&flags(&layer, 0, 0), &template(&[10])),
            Ok(plain | 0x4)
        );
        assert_eq!(
            gbuffer_key(&flags(&layer, 3, 0), &template(&[10])),
            Ok(plain | 0x24)
        );
        assert_eq!(gbuffer_key(&flags(&layer, 3, 0), &template(&[])), Ok(plain));
        // boolean 42452A12: 0x10 only with features 3 and 4 both
        let b = [MaterialFlags::BOOLEAN_42452A12];
        assert_eq!(gbuffer_key(&flags(&b, 0, 0), &template(&[3])), Ok(plain));
        assert_eq!(
            gbuffer_key(&flags(&b, 0, 0), &template(&[3, 4])),
            Ok(plain | 0x10)
        );
        // everything at once
        let all = [
            MaterialFlags::TWO_SIDED,
            MaterialFlags::FLIP_NORMAL_ON_BACK_FACE,
            MaterialFlags::LAYER_MATERIAL,
            MaterialFlags::BOOLEAN_42452A12,
        ];
        assert_eq!(
            gbuffer_key(&flags(&all, 1, 2), &template(&[3, 4, 10, 12, 13])),
            Ok(plain | 0x40 | 0x2 | 0x24 | 0x10)
        );
    }

    #[test]
    fn a_stored_value_the_engine_does_not_read_is_refused() {
        assert!(gbuffer_key(&flags(&[], 16, 0), &template(&[])).is_err());
        assert!(gbuffer_key(&flags(&[], 0, 4), &template(&[])).is_err());
        let mut f = flags(&[], 0, 0);
        f.booleans[7] = 2;
        assert!(gbuffer_key(&f, &template(&[])).is_err());
        // a feature the node has no byte for, where a flag asks for it
        let mut t = template(&[]);
        t.feature_node.truncate(10);
        let both = flags(
            &[MaterialFlags::TWO_SIDED, MaterialFlags::FLIP_NORMAL_ON_BACK_FACE],
            0,
            0,
        );
        assert!(gbuffer_key(&both, &t).unwrap_err().contains("feature 12"));
        // and one it does not ask for is not read
        assert_eq!(gbuffer_key(&flags(&[], 0, 0), &t), Ok(1 << 22));
        // a feature byte of 2 or 5 is a set feature (feature 15's byte is 2 on 82 templates), as 1 is
        let mut t = template(&[]);
        t.feature_node[13] = 5;
        assert_eq!(gbuffer_key(&both, &t), Ok((1 << 22) | 0x40));
    }

    /// The vertex stage's key keeps the shared bits (pass, 0x40, the feature flags) and takes the layout number, and
    /// drops the Material-flag bits only the pixel stage's keys have.
    #[test]
    fn the_vertex_key_drops_the_pixel_stages_flag_bits_and_takes_the_layout() {
        assert_eq!(gbuffer_vertex_key(0x40_0000, 0), 0x40_0000);
        assert_eq!(
            gbuffer_vertex_key(0x40_0000 | 0x40 | 0x24 | 0x2, 3),
            0x40_0000 | 0x40 | (3 << 17)
        );
        assert_eq!(
            gbuffer_vertex_key(0x40_0000 | 0x10 | 0x1, 11),
            0x40_0000 | (11 << 17)
        );
    }

    /// The D3D behaviour fixture of the DXBC lifter (our own HLSL, `ps_5_0`): five colour targets, which a lifted
    /// program states as the system values `SV_Target0..4` (not as semantics), as every program this pass binds.
    const LIFTED: &[u8] = include_bytes!("../../../../toolkit/dxbc/tests/fixtures/modes.dxbc");

    /// A lifted pixel program's outputs are system values, and the roles come all the same: target 0 is albedo and
    /// roughness, target 1 the unnamed rgb and the lane its shading class names, the rest Unknown by number.
    #[test]
    fn a_lifted_pixel_programs_targets_have_the_gbuffer_roles() {
        let program = primase_dxbc::lift(LIFTED).unwrap();
        assert!(
            program
                .interface
                .outputs
                .iter()
                .all(|v| matches!(v.system, Some(SystemValue::Target(_))))
        );
        let roles: Vec<(u32, u8, OutputRole)> = output_bindings(&program.interface, Some(1), true)
            .into_iter()
            .map(|o| {
                let target = program
                    .interface
                    .outputs
                    .iter()
                    .position(|v| v.label == o.label)
                    .unwrap();
                (u32::try_from(target).unwrap(), o.lanes, o.role)
            })
            .collect();
        assert_eq!(
            roles,
            [
                (0, 0b0111, OutputRole::Albedo),
                (0, 0b1000, OutputRole::Roughness),
                (
                    1,
                    0b0111,
                    OutputRole::Normal {
                        encoding: NormalEncoding::OctahedralDiamond,
                        space: NormalSpace::World
                    }
                ),
                (1, 0b1000, OutputRole::Metallic),
                (2, 0b1111, OutputRole::Unknown(2)),
                (3, 0b1111, OutputRole::Unknown(3)),
                (4, 0b1111, OutputRole::Unknown(4)),
            ]
        );
        // a depth output is not a colour target
        let mut with_depth = program.interface.clone();
        with_depth.outputs[0].system = Some(SystemValue::Depth);
        assert_eq!(output_bindings(&with_depth, Some(1), true).len(), 5);
    }
}
