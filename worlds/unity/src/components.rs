//! The `unity.*` dialect: what a Unity scene says about an entity that the core components do not.

use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Component, Entity, JsonError, World};
use serde::{Serialize, Serializer};

use crate::shader::{PassKind, ShaderPass, SubShader};

/// Unity's frame: left-handed, +Y up, +Z forward, metres. Into the world (right-handed, +Y up, -Z north): x, y, -z,
/// so Unity's forward is north; a mirroring frame (the mesh reader reverses triangle winding).
pub const UNITY: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.0);

/// Which object of the player an entity was read from: its file (lower-case name) and path id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnityObject {
    pub file: String,
    pub path_id: i64,
}

impl Component for UnityObject {
    const NAME: &'static str = "unity.object";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A drawn mesh renderer: its GameObject's layer, its materials (`None`: a missing material), the slice of a
/// static-batched combined mesh it draws (first submesh, count), its shadow casting mode and whether it receives
/// shadows (as stored; the placement's `core.shadows` holds what they mean).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityRenderer {
    pub renderer: UnityObject,
    pub layer: u32,
    pub materials: Vec<Option<UnityObject>>,
    pub submeshes: Option<(u32, u32)>,
    pub cast_shadows: i64,
    pub receive_shadows: Option<bool>,
}

impl Component for UnityRenderer {
    const NAME: &'static str = "unity.renderer";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A prototype read from a Unity mesh: the mesh, the slice of its submeshes (first, count) when a static-batched
/// renderer draws only that part of a combined mesh (otherwise all of them), and the submesh each primitive is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnityMesh {
    pub mesh: UnityObject,
    pub submeshes: Option<(u32, u32)>,
    pub drawn: Vec<u32>,
}

impl Component for UnityMesh {
    const NAME: &'static str = "unity.mesh";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::PerPrimitive;
    /// Removed from a mapped copy (`bake-nonrigid`): it names the Unity mesh and submeshes the geometry was read
    /// from, which the mapped geometry no longer is; `core.source` keeps the identity.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Drop
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    /// The submesh each primitive is follows the rebuilt primitives.
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(
            primase_core::remap_list(&self.drawn, from).map(|drawn| UnityMesh {
                drawn,
                ..self.clone()
            }),
        )
    }

    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        if self.mesh.path_id == 0 {
            return Err("a null mesh".into());
        }
        Ok(())
    }
}

/// A texture read from a `Texture2D`: what Unity says about it beyond `core.texture` (which is absent when the pixels
/// were not decoded: `not_decoded` says why).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTexture {
    pub texture: UnityObject,
    /// Unity's TextureFormat id.
    pub format: i64,
    pub srgb: bool,
    /// TextureUsageMode.
    pub usage: i64,
    /// TextureWrapMode along u and v.
    pub wrap: [i64; 2],
    /// FilterMode.
    pub filter: i64,
    /// `m_MipBias`: added to the level of detail it is sampled at.
    pub mip_bias: f32,
    /// `m_Aniso`: the anisotropic filtering level ([`UnityTexture::anisotropic`] above 1).
    pub aniso: i64,
    pub not_decoded: Option<String>,
}

impl UnityTexture {
    /// Whether Unity filters it anisotropically (`m_Aniso` above 1): what an isotropic sampler does not honour.
    pub fn anisotropic(&self) -> bool {
        self.aniso > 1
    }
}

impl Component for UnityTexture {
    const NAME: &'static str = "unity.texture";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

// A shader program's parameters as the player records them (the reader, `crate::program`, fills them).

/// A constant-buffer field: its name, byte offset in the buffer, element type (Unity's `ShaderParamType`: 0 float, 1
/// int, 2 bool, 3 half, 4 short, 5 uint), shape (a vector of `columns` lanes, or a matrix of `rows` rows) and array
/// size (0: not an array).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Field {
    pub name: String,
    pub offset: u32,
    pub kind: i64,
    pub shape: Shape,
    pub array_size: u32,
}

/// A field's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Shape {
    Vector { columns: u32 },
    Matrix { rows: u32 },
}

/// A constant-buffer field of a struct type (Unity's instancing arrays): its name, byte offset, array size (0: not
/// an array), the struct's size in bytes and its members (offsets within the struct).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct StructField {
    pub name: String,
    pub offset: u32,
    pub array_size: u32,
    pub size: u32,
    pub members: Vec<Field>,
}

/// A constant buffer: its name, the bind slot it is bound to (`b<n>`; `None` when no binding names it), its size in
/// bytes, its fields and its struct fields (by offset).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConstantBuffer {
    pub name: String,
    pub slot: Option<u32>,
    pub size: u32,
    pub fields: Vec<Field>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub structs: Vec<StructField>,
}

/// A texture parameter: its name, bind slot (`t<n>`), the sampler slot it samples with (`None`: none of its own) and
/// dimension (Unity's `TextureDimension`: 2 a 2D texture, 3 3D, 4 a cube).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct TextureParam {
    pub name: String,
    pub slot: u32,
    pub sampler: Option<u32>,
    pub dim: i64,
}

/// The parameters the variants of a program share.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Parameters {
    pub constant_buffers: Vec<ConstantBuffer>,
    pub textures: Vec<TextureParam>,
    /// Sampler states: Unity's packed sampler value and its bind slot (`s<n>`).
    pub samplers: Vec<(u32, u32)>,
}

/// One stored variant of a stage's program: its hardware tier (the player's group), its keywords, its parameters
/// (constant-buffer fields with their offsets, texture and sampler slots) and its D3D11 code, a DXBC container
/// (Unity's record header before it removed).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityProgramVariant {
    pub tier: usize,
    /// Unity's `ShaderGpuProgramType` (15 and 16 D3D11 vertex shader model 4.0 and 5.0, 17 and 18 pixel, 19 and 20
    /// geometry, 21 hull, 22 domain).
    pub gpu_program_type: i64,
    pub keywords: Vec<String>,
    pub parameters: Parameters,
    #[serde(skip)]
    pub code: std::sync::Arc<[u8]>,
}

/// The compiled programs of the pass a material draws with, as the player stores them
/// ([`crate::program::read_programs`]): its shader's name, the pass (subshader and pass index in the shader's parsed
/// form, the draw pass under the player's pipeline), every stored variant with D3D11 code of each stage the pass has
/// (`variants`: the fragment stage's; `vertex`, `geometry`, `hull`, `domain`: empty where the pass has none), and the
/// default the shader declares for each texture property (`m_PropInfo`: Unity's built-in texture name, "white", "bump"
/// and so on). A stage's variants carry only the keywords that stage's code uses, so a vertex variant pairs with a
/// fragment variant of the same tier by the keywords they share. Which variant a lowering evaluates is its caller's
/// choice ([`UnityPrograms::variant`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityPrograms {
    pub shader: String,
    pub subshader: usize,
    pub pass: usize,
    pub texture_defaults: Vec<(String, String)>,
    /// The fragment stage's variants.
    pub variants: Vec<UnityProgramVariant>,
    pub vertex: Vec<UnityProgramVariant>,
    pub geometry: Vec<UnityProgramVariant>,
    pub hull: Vec<UnityProgramVariant>,
    pub domain: Vec<UnityProgramVariant>,
}

impl UnityPrograms {
    /// The variant a material with keywords `material` draws with under the renderer state the caller states: its
    /// global keywords `global` (the light, shadow, lightmap and probe keywords Unity sets per draw) and hardware tier
    /// `tier`. Its keywords are exactly `global` and those of the material's keywords the pass's variants use (a
    /// keyword no variant uses is not compiled into any); `None` when the pass stores no such variant.
    pub fn variant(
        &self,
        material: &[String],
        global: &[String],
        tier: usize,
    ) -> Option<&UnityProgramVariant> {
        let used = |k: &String| self.variants.iter().any(|v| v.keywords.contains(k));
        let mut wanted: Vec<&String> = global.iter().chain(material.iter().filter(|k| used(k))).collect();
        wanted.sort();
        wanted.dedup();
        self.variants.iter().find(|v| {
            let mut have: Vec<&String> = v.keywords.iter().collect();
            have.sort();
            v.tier == tier && have == wanted
        })
    }
}

/// The variant of each stage a draw uses ([`UnityPrograms::draw_variants`]); `None` for a stage the pass has no
/// variants of, or none of for the draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawVariants<'a> {
    pub vertex: Option<&'a UnityProgramVariant>,
    pub fragment: Option<&'a UnityProgramVariant>,
    pub geometry: Option<&'a UnityProgramVariant>,
    pub hull: Option<&'a UnityProgramVariant>,
    pub domain: Option<&'a UnityProgramVariant>,
    /// The keywords the draw has beyond the ones it was given: those every stored variant of a stage has in the tier
    /// ([`UnityPrograms::draw_variants`]), sorted.
    pub implied: Vec<&'a String>,
}

impl UnityPrograms {
    /// The keywords every stored variant of some stage has in the tier `tier`, over the stages that have variants
    /// there. A player stores a stage's variants only for the keyword sets a draw can have, so a draw has these.
    fn required_keywords(&self, tier: usize) -> std::collections::BTreeSet<&String> {
        let mut required = std::collections::BTreeSet::new();
        for list in [
            &self.variants,
            &self.vertex,
            &self.geometry,
            &self.hull,
            &self.domain,
        ] {
            let mut at_tier = list.iter().filter(|v| v.tier == tier);
            let Some(first) = at_tier.next() else { continue };
            let mut always: std::collections::BTreeSet<&String> = first.keywords.iter().collect();
            for v in at_tier {
                always.retain(|k| v.keywords.contains(k));
            }
            required.extend(always);
        }
        required
    }
}

impl UnityPrograms {
    /// The variant of each stage a draw with the keywords `material` and `global` (their union is the draw's keyword
    /// set, with the keywords [`UnityPrograms::required_keywords`] names: `implied`) and hardware tier `tier` uses.
    /// Unity compiles each stage for the keyword sets that stage uses, so per stage the draw's keywords are restricted
    /// to those some variant of the stage has, and the variant is the one with exactly those, in that tier
    /// (`tests/program_pairing.rs` checks on every pass of two players that a fragment variant and a vertex variant so
    /// selected exist together: a fragment variant's keywords alone do not determine the vertex variant). Where a
    /// stage has the same keywords and tier in two shader models, the fragment stage takes the first stored and the
    /// others the model of its (a vertex program two types below the pixel program's), else the first.
    pub fn draw_variants<'a>(
        &'a self,
        material: &[String],
        global: &[String],
        tier: usize,
    ) -> DrawVariants<'a> {
        let mut draw: std::collections::BTreeSet<&String> = material.iter().chain(global).collect();
        let implied: Vec<&String> = self
            .required_keywords(tier)
            .into_iter()
            .filter(|k| !draw.contains(k))
            .collect();
        draw.extend(implied.iter().copied());
        let pick = |list: &'a [UnityProgramVariant], model: Option<i64>| -> Option<&'a UnityProgramVariant> {
            let used: std::collections::BTreeSet<&String> = list.iter().flat_map(|v| &v.keywords).collect();
            let wanted: std::collections::BTreeSet<&String> = draw.intersection(&used).copied().collect();
            let mut found = list.iter().filter(|v| {
                v.tier == tier && v.keywords.iter().collect::<std::collections::BTreeSet<_>>() == wanted
            });
            match model {
                Some(m) => {
                    let all: Vec<_> = found.collect();
                    all.iter()
                        .copied()
                        .find(|v| v.gpu_program_type == m)
                        .or_else(|| all.first().copied())
                }
                None => found.next(),
            }
        };
        let fragment = pick(&self.variants, None);
        DrawVariants {
            fragment,
            vertex: pick(&self.vertex, fragment.map(|f| f.gpu_program_type - 2)),
            geometry: pick(&self.geometry, None),
            hull: pick(&self.hull, None),
            domain: pick(&self.domain, None),
            implied,
        }
    }
}

impl Component for UnityPrograms {
    const NAME: &'static str = "unity.programs";
    /// 2: the vertex, geometry, hull and domain stages beside the fragment's. 3: each variant's GPU program type.
    const VERSION: u32 = 3;
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn payload_bytes(&self) -> u64 {
        [
            &self.variants,
            &self.vertex,
            &self.geometry,
            &self.hull,
            &self.domain,
        ]
        .into_iter()
        .flatten()
        .map(|v| v.code.len() as u64)
        .sum()
    }
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Invariant
    }
}

/// A number a material stores, in the dialect's JSON view: a JSON number, or `"inf"`, `"-inf"` or `"nan"` for the
/// non-finite values a shader property may store as a sentinel (JSON has none; the dialect keeps what was stored).
struct Stored(f32);

impl Serialize for Stored {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            x if x.is_finite() => s.serialize_f32(x),
            x if x.is_nan() => s.serialize_str("nan"),
            x if x > 0.0 => s.serialize_str("inf"),
            _ => s.serialize_str("-inf"),
        }
    }
}

fn stored_pair<S: Serializer>(v: &[f32; 2], s: S) -> Result<S::Ok, S::Error> {
    [Stored(v[0]), Stored(v[1])].serialize(s)
}

fn stored_floats<S: Serializer>(v: &[(String, f32)], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(v.iter().map(|(k, x)| (k, Stored(*x))))
}

fn stored_colors<S: Serializer>(v: &[(String, [f32; 4])], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(v.iter().map(|(k, c)| (k, c.map(Stored))))
}

/// A texture property of a material: whether it names an object (`referenced`: a texture is set), its texture entity
/// (`None` when none is set, or what it names was not read as a texture: another class), and tiling (`_ST`, Unity's
/// bottom-left origin), as stored (non-finite numbers included). An unset property (`referenced` false) samples the
/// default its shader declares; a referenced one without a texture entity, or whose entity has no pixels, is a
/// source the world does not hold.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTexEnv {
    pub name: String,
    pub referenced: bool,
    pub texture: Option<Entity>,
    #[serde(serialize_with = "stored_pair")]
    pub scale: [f32; 2],
    #[serde(serialize_with = "stored_pair")]
    pub offset: [f32; 2],
}

impl UnityTexEnv {
    /// The tiling, when both its scale and its offset are finite: a non-finite one is no tiling a lowering can use.
    pub fn finite_tiling(&self) -> Option<([f32; 2], [f32; 2])> {
        self.scale
            .iter()
            .chain(&self.offset)
            .all(|x| x.is_finite())
            .then_some((self.scale, self.offset))
    }
}

/// The pass of a subshader that draws the surface under `pipeline`: the first with its own state (not a `UsePass` or
/// `GrabPass`) among the passes the pipeline draws objects with, by `LightMode` (compared ignoring case: Unity stores
/// some upper-cased).
/// - URP: absent, `SRPDefaultUnlit`, `UniversalForward` or `UniversalForwardOnly` (the shader tags of URP's
///   `DrawObjectsPass`).
/// - Built-in forward: the base pass, `ForwardBase`, which draws the surface once with the main light; additive
///   `ForwardAdd` passes only add further lights over it. Without one, the first unlit pass (`LightMode` absent or
///   `Always`), else the first vertex-lit one (`Vertex`, `VertexLMRGBM`, `VertexLM`), which Unity draws for shaders
///   without forward passes.
///
/// `None` under another pipeline or when no pass qualifies.
pub fn draw_pass<'a>(sub: &'a SubShader, pipeline: &RenderPipeline) -> Option<&'a ShaderPass> {
    let own = || sub.passes.iter().filter(|p| p.kind == PassKind::Normal);
    let mode_in = |p: &ShaderPass, modes: &[&str]| match &p.light_mode {
        None => modes.contains(&""),
        Some(l) => modes.iter().any(|m| l.eq_ignore_ascii_case(m)),
    };
    match pipeline {
        RenderPipeline::Universal => own().find(|p| {
            mode_in(
                p,
                &["", "SRPDefaultUnlit", "UniversalForward", "UniversalForwardOnly"],
            )
        }),
        RenderPipeline::BuiltIn => own()
            .find(|p| mode_in(p, &["ForwardBase"]))
            .or_else(|| own().find(|p| mode_in(p, &["", "Always"])))
            .or_else(|| own().find(|p| mode_in(p, &["Vertex", "VertexLMRGBM", "VertexLM"]))),
        RenderPipeline::Other(_) => None,
    }
}

/// A shader `Queue` tag's value: `Background` 1000, `Geometry` 2000, `AlphaTest` 2450, `GeometryLast` 2500,
/// `Transparent` 3000, `Overlay` 4000, each optionally `+n` or `-n`, or a number (ShaderLab's queue syntax); `None`
/// for anything else.
pub fn queue_tag(v: &str) -> Option<i32> {
    let v = v.trim();
    if let Ok(n) = v.parse() {
        return Some(n);
    }
    let split = v.find(['+', '-']).unwrap_or(v.len());
    let base: i32 = match v[..split].trim().to_ascii_lowercase().as_str() {
        "background" => 1000,
        "geometry" => 2000,
        "alphatest" => 2450,
        "geometrylast" => 2500,
        "transparent" => 3000,
        "overlay" => 4000,
        _ => return None,
    };
    let offset: i32 = match &v[split..] {
        "" => 0,
        o => o.trim().parse().ok()?,
    };
    base.checked_add(offset)
}

/// A material as Unity stores it: its shader's name and render state (its subshaders, as
/// [`crate::material::ShaderData`]), saved properties in stored order (colours gamma-encoded, as stored; a non-finite
/// number a shader property stores as a sentinel kept as stored, [`UnityMaterial::float`] and [`UnityMaterial::color`]
/// giving it to no lowering), keywords, tags and render queue.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityMaterial {
    pub material: UnityObject,
    pub shader: Option<String>,
    pub shader_state: Vec<crate::shader::SubShader>,
    pub textures: Vec<UnityTexEnv>,
    #[serde(serialize_with = "stored_floats")]
    pub floats: Vec<(String, f32)>,
    pub ints: Vec<(String, i64)>,
    #[serde(serialize_with = "stored_colors")]
    pub colors: Vec<(String, [f32; 4])>,
    pub keywords: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub render_queue: i64,
}

impl UnityMaterial {
    /// The subshader `pipeline` draws with: Unity takes the first subshader the pipeline supports, and a subshader
    /// with a `RenderPipeline` tag only under the pipeline it names. URP: the first tagged `UniversalPipeline` or
    /// untagged; built-in: the first untagged. `None` under another pipeline, whose tag is not known. (Hardware
    /// support and `Shader.maximumLOD` also skip subshaders; neither is read, so the first is taken.)
    pub fn subshader(&self, pipeline: &RenderPipeline) -> Option<&SubShader> {
        let wanted = match pipeline {
            RenderPipeline::Universal => Some("UniversalPipeline"),
            RenderPipeline::BuiltIn => None,
            RenderPipeline::Other(_) => return None,
        };
        self.shader_state.iter().find(|s| {
            s.tags
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("RenderPipeline"))
                .is_none_or(|(_, v)| wanted == Some(v.as_str()))
        })
    }

    /// A pass's blend factors (source, destination) as `BlendMode` values ([`crate::shader::blend_mode`]), each the
    /// bound property's value where it has one ([`Self::state_value`]); `None` when one is not a `BlendMode`.
    pub fn blend_factors(&self, pass: &ShaderPass) -> Option<(u8, u8)> {
        let mode = |v: &crate::shader::ShaderValue| {
            let x = self.state_value(v);
            (x.fract() == 0.0 && (0.0..=f32::from(crate::shader::blend_mode::LAST)).contains(&x))
                .then_some(x as u8)
        };
        Some((mode(&pass.src_blend)?, mode(&pass.dst_blend)?))
    }

    /// The pass that draws the material's surface under `pipeline` ([`draw_pass`] in its [`Self::subshader`]).
    pub fn draw_pass(&self, pipeline: &RenderPipeline) -> Option<&ShaderPass> {
        draw_pass(self.subshader(pipeline)?, pipeline)
    }

    /// The render queue it draws in under `pipeline`: its own (`m_CustomRenderQueue`), else its subshader's `Queue`
    /// tag ([`queue_tag`]; no subshader when the pipeline is not known), else Geometry (2000); and which of the three
    /// said so.
    pub fn render_queue(&self, pipeline: Option<&RenderPipeline>) -> (i32, &'static str) {
        if self.render_queue >= 0 {
            return (
                i32::try_from(self.render_queue).unwrap_or(i32::MAX),
                "the material's render queue",
            );
        }
        let tag = pipeline
            .and_then(|p| self.subshader(p))
            .and_then(|s| s.tags.iter().find(|(k, _)| k.eq_ignore_ascii_case("Queue")))
            .and_then(|(_, v)| queue_tag(v));
        match tag {
            Some(q) => (q, "the shader's Queue tag"),
            None => (2000, "Geometry (no queue stated)"),
        }
    }

    /// A shader render-state value: the bound material property's float where it names one the material has, else
    /// the shader's own value.
    pub fn state_value(&self, v: &crate::shader::ShaderValue) -> f32 {
        v.property
            .as_deref()
            .and_then(|p| self.float(p))
            .unwrap_or(v.value)
    }

    /// A float property's value, unless it is not finite (a sentinel no lowering can use: read as unset, and
    /// [`UnityMaterial::non_finite`] counts it).
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
            .filter(|v| v.is_finite())
    }

    /// A colour property's value, unless a channel of it is not finite (as [`UnityMaterial::float`]).
    pub fn color(&self, name: &str) -> Option<[f32; 4]> {
        self.colors
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
            .filter(|v| v.iter().all(|x| x.is_finite()))
    }

    /// The float and colour properties holding a non-finite number, as stored.
    pub fn non_finite(&self) -> usize {
        self.floats.iter().filter(|(_, v)| !v.is_finite()).count()
            + self
                .colors
                .iter()
                .filter(|(_, c)| c.iter().any(|x| !x.is_finite()))
                .count()
    }

    /// A texture property with a texture set.
    pub fn texture(&self, name: &str) -> Option<&UnityTexEnv> {
        self.textures
            .iter()
            .find(|t| t.name == name && t.texture.is_some())
    }

    pub fn keyword(&self, k: &str) -> bool {
        self.keywords.iter().any(|x| x == k)
    }
}

impl Component for UnityMaterial {
    const NAME: &'static str = "unity.material";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: `shader_state`; 3: each shader pass's `kind`; 4: each texture property's `referenced`.
    const VERSION: u32 = 4;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }

    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.textures.iter().filter_map(|t| t.texture));
    }

    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        for t in &self.textures {
            if let Some(e) = t.texture
                && !w.has::<UnityTexture>(e)
            {
                return Err(format!(
                    "texture property {} names an entity that is not a texture",
                    t.name
                ));
            }
        }
        Ok(())
    }
}

/// A MonoBehaviour as stored: its script (namespace and class) and the script's serialized fields, kept as bytes
/// (a stripped build has no type tree for them).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityBehaviour {
    pub behaviour: UnityObject,
    pub script: String,
    pub enabled: bool,
    #[serde(skip)]
    pub data: Vec<u8>,
}

/// The MonoBehaviours of a GameObject the world carries without understanding them.
#[derive(Clone, Debug, PartialEq)]
pub struct UnityBehaviours(pub Vec<UnityBehaviour>);

impl Component for UnityBehaviours {
    const NAME: &'static str = "unity.behaviours";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// Scripts and sizes, not the bytes.
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        Ok(Some(serde_json::Value::Array(
            self.0
                .iter()
                .map(|b| {
                    serde_json::json!({"behaviour": [b.behaviour.file, b.behaviour.path_id], "script": b.script,
                        "enabled": b.enabled, "bytes": b.data.len()})
                })
                .collect(),
        )))
    }
}

/// A terrain layer: its textures (entities) and how the terrain shader uses them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTerrainLayer {
    pub layer: UnityObject,
    pub diffuse: Option<Entity>,
    pub normal: Option<Entity>,
    pub mask: Option<Entity>,
    pub tile_size: [f32; 2],
    pub tile_offset: [f32; 2],
    pub specular: [f32; 4],
    pub metallic: f32,
    pub smoothness: f32,
    pub normal_scale: f32,
    /// Min and max, RGBA.
    pub diffuse_remap: [[f32; 4]; 2],
    pub mask_remap: [[f32; 4]; 2],
}

impl Component for UnityTerrainLayer {
    const NAME: &'static str = "unity.terrain_layer";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }

    fn references(&self, out: &mut Vec<Entity>) {
        out.extend([self.diffuse, self.normal, self.mask].into_iter().flatten());
    }
}

/// A terrain's collider as Unity stores it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTerrainCollider {
    pub collider: UnityObject,
    pub enabled: bool,
    /// Its TerrainData when it is not the drawn one.
    pub other_data: Option<UnityObject>,
    pub physic_material: Option<String>,
    pub tree_colliders: bool,
}

/// A tree a terrain places (see [`crate::terrain::TreeInstance`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTree {
    pub position: [f32; 3],
    pub width_scale: f32,
    pub height_scale: f32,
    pub rotation: f32,
    pub color: u32,
    pub prototype: usize,
}

/// The highest stored height sample: 0..=32766 is 0..1 of the terrain's height (`TerrainData.GetHeight`).
pub const MAX_HEIGHT: i16 = 32766;

/// What a heightmap must be to be read: `resolution` x `resolution` samples (at least 2 x 2: one cell), each
/// 0..=[`MAX_HEIGHT`], and, where it has holes, one per cell ((resolution - 1)^2).
pub fn check_heightmap(
    resolution: u32,
    heights: &[i16],
    holes: Option<&[u8]>,
) -> std::result::Result<(), String> {
    if resolution < 2 {
        return Err(format!(
            "resolution {resolution}: a heightmap has at least 2 x 2 samples"
        ));
    }
    let n = resolution as usize;
    let samples = n.checked_mul(n).ok_or("the heightmap's sample count overflows")?;
    if heights.len() != samples {
        return Err(format!("{} heights for resolution {resolution}", heights.len()));
    }
    if let Some((i, h)) = heights
        .iter()
        .enumerate()
        .find(|(_, h)| !(0..=MAX_HEIGHT).contains(*h))
    {
        return Err(format!(
            "height sample {i} (row {}, column {}) is {h}, outside 0..={MAX_HEIGHT}",
            i / n,
            i % n
        ));
    }
    let cells = (n - 1) * (n - 1);
    match holes {
        Some(h) if h.len() != cells => Err(format!("{} holes for {cells} cells", h.len())),
        _ => Ok(()),
    }
}

/// A terrain as Unity stores it, on the entity that places it: its heightmap (row z, column x, 0..=32766 for 0..1 of
/// `scale.y`), holes (one byte per cell, 0 a hole), splat layers and alphamaps (texture entities, rows top first),
/// material, collider and trees (their prototypes' prefabs).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTerrain {
    pub terrain: UnityObject,
    pub data: UnityObject,
    pub resolution: u32,
    pub scale: [f32; 3],
    #[serde(skip)]
    pub heights: Vec<i16>,
    #[serde(skip)]
    pub holes: Option<Vec<u8>>,
    pub layers: Vec<Option<Entity>>,
    pub alphamaps: Vec<Option<Entity>>,
    pub alphamap_resolution: u32,
    pub material: Option<Entity>,
    pub draw_heightmap: bool,
    pub draw_trees: bool,
    pub collider: Option<UnityTerrainCollider>,
    pub tree_prototypes: Vec<Option<UnityObject>>,
    #[serde(skip)]
    pub trees: Vec<UnityTree>,
}

impl Component for UnityTerrain {
    const NAME: &'static str = "unity.terrain";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// The fields, with counts in place of the heights, holes and trees.
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        let mut v = primase_core::json::to_value(self)?;
        if let serde_json::Value::Object(o) = &mut v {
            o.insert("heights".into(), serde_json::json!(self.heights.len()));
            o.insert(
                "holes".into(),
                serde_json::json!(
                    self.holes
                        .as_ref()
                        .map_or(0, |h| h.iter().filter(|&&x| x == 0).count())
                ),
            );
            o.insert("trees".into(), serde_json::json!(self.trees.len()));
        }
        Ok(Some(v))
    }

    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.layers.iter().chain(&self.alphamaps).flatten());
        out.extend(self.material);
    }

    /// Its heightmap as the reader requires it ([`check_heightmap`]).
    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        check_heightmap(self.resolution, &self.heights, self.holes.as_deref())
    }
}

/// On the entity of a collision shape lowered apart from its collider (`unity.collider_of`, see
/// [`crate::passes::lower_collision`]): the entity of that `unity.collider` or `unity.terrain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UnityColliderOf(pub Entity);

impl Component for UnityColliderOf {
    const NAME: &'static str = "unity.collider_of";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }

    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.0);
    }

    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        if w.has::<UnityCollider>(self.0) || w.has::<UnityTerrain>(self.0) {
            Ok(())
        } else {
            Err("the entity it names has no unity.collider or unity.terrain".into())
        }
    }
}

/// A renderer placed by a terrain's tree: the terrain's entity and the tree's index in its `unity.terrain` trees.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnityTreeInstance {
    pub terrain: Entity,
    pub tree: usize,
}

impl Component for UnityTreeInstance {
    const NAME: &'static str = "unity.tree_instance";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }

    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.terrain);
    }

    fn verify(&self, _e: Entity, w: &World) -> Result<(), String> {
        match w.get::<UnityTerrain>(self.terrain) {
            Some(t) if self.tree < t.trees.len() => Ok(()),
            Some(_) => Err(format!("tree {} past the terrain's trees", self.tree)),
            None => Err("the terrain entity has no unity.terrain".into()),
        }
    }
}

/// A light as Unity stores it, on an entity placed by its Transform (Unity's light shines along its local +Z: the
/// world's -Z after the frame conversion). See [`crate::scene::LightComponent`] for the fields.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityLight {
    pub light: UnityObject,
    pub kind: i64,
    pub color: [f32; 4],
    pub intensity: f32,
    pub range: f32,
    pub spot_angle: f32,
    pub inner_spot_angle: f32,
    pub shadows: i64,
    pub color_temperature: Option<f32>,
    pub bounce_intensity: f32,
    pub light_unit: Option<i64>,
    pub lux_at_distance: Option<f32>,
    pub area_size: [f32; 2],
    pub render_mode: i64,
    pub baked: bool,
    pub culling_mask: u32,
    pub cookie: Option<UnityObject>,
}

impl Component for UnityLight {
    const NAME: &'static str = "unity.light";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: `baked`.
    const VERSION: u32 = 2;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A camera as stored, on an entity placed by its Transform: the layers it draws (`culling_mask`, bit n for layer n)
/// and the texture it renders into (`None`: the screen). `unity-drawn` leaves out renderers on layers no screen camera
/// draws ([`crate::drawn`]).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityCamera {
    pub camera: UnityObject,
    pub culling_mask: u32,
    pub target_texture: Option<UnityObject>,
}

impl Component for UnityCamera {
    const NAME: &'static str = "unity.camera";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A scene's ambient lighting as RenderSettings stores it: `mode` (`m_AmbientMode`: 0 skybox, 1 trilight, a gradient
/// of the sky, equator and ground colours, 3 flat, the sky colour alone, 4 custom), the three colours as stored,
/// `intensity` (`m_AmbientIntensity`) and the ambient probe as stored (`m_AmbientProbe`, Unity's SphericalHarmonicsL2:
/// nine coefficients per channel, red 0..8, green 9..17, blue 18..26; `None` when the RenderSettings store none). The
/// probe is what the editor last generated: all zeros when it never did, and not necessarily from the colours stored
/// beside it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityAmbient {
    pub mode: i64,
    pub sky: [f32; 4],
    pub equator: [f32; 4],
    pub ground: [f32; 4],
    pub intensity: f32,
    pub probe: Option<[f32; 27]>,
}

/// The scene's fog as RenderSettings stores it: whether it is on (`m_Fog`), its `mode` (`m_FogMode`: 1 linear, 2
/// exponential, 3 exponential squared), its colour, `density` (exponential modes) and the linear mode's `start` and
/// `end`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityFog {
    pub enabled: bool,
    pub mode: i64,
    pub color: [f32; 4],
    pub density: f32,
    pub start: f32,
    pub end: f32,
}

/// The lighting and shadow settings of a Universal Render Pipeline asset as stored (`UniversalRenderPipelineAsset`,
/// asset version 13; [`crate::read::pipeline`] holds the verified layout): the light modes as `LightRenderingMode`
/// numbers them (0 disabled, 1 per pixel, 2 per vertex), the shadow cascade count (1 to 4), the settings that decide
/// which of a pass's keywords the pipeline sets.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityUrpSettings {
    pub main_light_mode: i64,
    pub main_light_shadows: bool,
    pub additional_lights_mode: i64,
    pub additional_light_shadows: bool,
    pub reflection_probe_blending: bool,
    pub reflection_probe_box_projection: bool,
    pub reflection_probe_atlas: bool,
    pub shadow_distance: f32,
    pub shadow_cascades: u8,
    pub soft_shadows: bool,
    pub mixed_lighting: bool,
    pub light_cookies: bool,
    pub light_layers: bool,
    pub lod_cross_fade: bool,
    /// `ShEvalMode`: 0 auto, 1 per vertex, 2 mixed, 3 per pixel.
    pub sh_eval_mode: i64,
    /// `LightProbeSystem`: 0 legacy light probes, 1 probe volumes.
    pub light_probe_system: i64,
}

/// The shadow settings of a quality level (`QualitySettings`): `shadows` 0 for none, 1 for hard shadows only and 2 for
/// all, the distance shadows are drawn to, the built-in pipeline's cascade count.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityShadowQuality {
    pub shadows: i64,
    pub distance: f32,
    pub cascades: i64,
}

/// The script class of Universal Render Pipeline's asset (`m_Namespace.m_ClassName` of its MonoScript).
pub const URP_ASSET: &str = "UnityEngine.Rendering.Universal.UniversalRenderPipelineAsset";

/// The render pipeline a player draws with: GraphicsSettings' default pipeline asset (`m_CustomRenderPipeline`), or a
/// quality level's own (`customRenderPipeline`) where it sets one. Which quality level runs is the user's choice, so
/// the pipeline is known only when every level gives the same kind.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub enum RenderPipeline {
    /// No pipeline asset for any quality level: Unity's built-in renderer.
    BuiltIn,
    /// A Universal Render Pipeline asset ([`URP_ASSET`]) for every quality level.
    Universal,
    /// Anything else, or a mix: each level's asset script class (`built-in` for none), sorted, each once.
    Other(Vec<String>),
}

/// The scene's RenderSettings as far as they are read, with the player's render pipeline (`unity.render_settings`, a
/// world resource). `lower-unity-lights` chooses `core.environment`'s sun from them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UnityRenderSettings {
    /// The light entity `m_Sun` names, when it is one of the lights read (it shines); `None` when it names none or one
    /// that does not shine.
    pub sun: Option<Entity>,
    /// The player's render pipeline; `None` when the player has no GraphicsSettings.
    pub pipeline: Option<RenderPipeline>,
    /// Its ambient lighting; `None` when the scene has no RenderSettings or they store no ambient mode.
    pub ambient: Option<UnityAmbient>,
    /// Its fog; `None` when the scene has no RenderSettings or they store no fog.
    pub fog: Option<UnityFog>,
    /// The settings of the Universal Render Pipeline asset the read quality level draws with; `None` for another
    /// pipeline, or where the levels differ and none was named.
    pub urp: Option<UnityUrpSettings>,
    /// The shadow settings of the quality level the scene was read for (every level's where they agree and none was
    /// named); `None` where the levels differ and none was named, or the player has none.
    pub shadow_quality: Option<UnityShadowQuality>,
}

impl primase_core::Resource for UnityRenderSettings {
    const NAME: &'static str = "unity.render_settings";
    /// 2: `pipeline`; 3: `ambient`; 4: the ambient's `probe`; 5: `fog`, `urp` and `shadow_quality`.
    const VERSION: u32 = 5;
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.sun);
    }
    fn retain_references(&mut self, keep: &dyn Fn(Entity) -> bool) {
        if self.sun.is_some_and(|e| !keep(e)) {
            self.sun = None;
        }
    }
}

/// A mesh collider's source geometry as stored (Unity's frame): every submesh's triangles, base vertex applied.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ColliderMesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// A collider's shape as Unity stores it, in its GameObject's space (Unity's frame, metres before scale).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum UnityColliderShape {
    Box {
        center: [f32; 3],
        size: [f32; 3],
    },
    Sphere {
        center: [f32; 3],
        radius: f32,
    },
    /// `direction`: the axis the height runs along (0 x, 1 y, 2 z); the height includes the caps.
    Capsule {
        center: [f32; 3],
        radius: f32,
        height: f32,
        direction: usize,
    },
    Mesh {
        mesh: Option<UnityObject>,
        convex: bool,
        #[serde(skip)]
        geometry: Option<std::sync::Arc<ColliderMesh>>,
    },
}

/// A PhysicMaterial: name, frictions, bounciness and their combine modes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityPhysicMaterial {
    pub name: String,
    pub dynamic_friction: f32,
    pub static_friction: f32,
    pub bounciness: f32,
    pub friction_combine: i64,
    pub bounce_combine: i64,
}

/// A collider as Unity stores it, on an entity placed by its Transform: its shape, whether it is a trigger (triggers
/// detect, they do not collide), its physic material and the Rigidbody on it or an ancestor (`Some(true)`: kinematic,
/// it moves only when scripted). Whether its GameObject is active and it is enabled is the entity's state
/// (`core.condition`). `lowered`: `lower-unity-collision` has made its collision (false as read).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityCollider {
    pub collider: UnityObject,
    pub layer: u32,
    pub shape: UnityColliderShape,
    pub trigger: bool,
    pub physic_material: Option<UnityPhysicMaterial>,
    pub rigidbody: Option<bool>,
    pub lowered: bool,
}

impl UnityCollider {
    /// Whether it is collision geometry when it exists: not a trigger, and not moved by physics (no Rigidbody, or
    /// kinematic).
    pub fn collides(&self) -> bool {
        !self.trigger && self.rigidbody != Some(false)
    }
}

impl Component for UnityCollider {
    const NAME: &'static str = "unity.collider";
    const GEOMETRY: primase_core::Geometry = primase_core::Geometry::None;
    /// 2: `active` left to the entity's state; 3: `lowered`.
    const VERSION: u32 = 3;
    /// Removed when its entity's local space is mapped (`bake-nonrigid`), once nothing is lost with it: its shape is the
    /// collider as stored, relative to the stored transform, and no longer describes the entity once the transform's
    /// non-rigid part is baked away. A collider that collides is dropped only after `lower-unity-collision` has made
    /// its collision (`lowered`), which carries the geometry and is mapped; before that it is unsupported, so the
    /// entity keeps its transform. One that does not collide (a trigger, one moved by physics) makes no collision and
    /// is dropped.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        if self.lowered || !self.collides() {
            primase_core::Linearity::Drop
        } else {
            primase_core::Linearity::Unsupported
        }
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shader::fixture;

    fn variant(tier: usize, model: i64, keywords: &[&str], code: u8) -> UnityProgramVariant {
        UnityProgramVariant {
            tier,
            gpu_program_type: model,
            keywords: keywords.iter().map(|k| (*k).to_owned()).collect(),
            parameters: Parameters::default(),
            code: std::sync::Arc::from(vec![code]),
        }
    }

    /// A draw's keywords select each stage's variant by the keywords that stage uses: the vertex stage reads
    /// `LOD_FADE_PERCENTAGE` (and the fragment stage never does), so the fragment variant for `FOG` is paired with the
    /// vertex variant of `FOG` and `LOD_FADE_PERCENTAGE`; a keyword neither stage reads has no effect; the fragment's
    /// shader model decides between two stored models of one vertex variant.
    #[test]
    fn a_draws_keywords_select_each_stages_variant_by_what_the_stage_uses() {
        let programs = UnityPrograms {
            shader: "S".into(),
            subshader: 0,
            pass: 0,
            texture_defaults: Vec::new(),
            variants: vec![
                variant(1, 18, &[], 1),
                variant(1, 18, &["FOG"], 2),
                variant(2, 18, &[], 3),
            ],
            vertex: vec![
                variant(1, 15, &["LOD_FADE_PERCENTAGE"], 10),
                variant(1, 16, &["LOD_FADE_PERCENTAGE"], 11),
                variant(1, 16, &["LOD_FADE_PERCENTAGE", "FOG"], 12),
                variant(2, 16, &["LOD_FADE_PERCENTAGE"], 13),
            ],
            geometry: Vec::new(),
            hull: Vec::new(),
            domain: Vec::new(),
        };
        let s = |k: &[&str]| k.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
        let code = |v: Option<&UnityProgramVariant>| v.map(|v| v.code[0]);
        let d = programs.draw_variants(&s(&["FOG", "UNREAD"]), &s(&["LOD_FADE_PERCENTAGE"]), 1);
        assert_eq!((code(d.fragment), code(d.vertex)), (Some(2), Some(12)));
        assert_eq!(
            (code(d.geometry), code(d.hull), code(d.domain)),
            (None, None, None)
        );
        // no fog: the fragment variant without keywords; its model 18 takes the vertex model 16
        let d = programs.draw_variants(&[], &s(&["LOD_FADE_PERCENTAGE"]), 1);
        assert_eq!((code(d.fragment), code(d.vertex)), (Some(1), Some(11)));
        // another tier, and a tier that has none
        let d = programs.draw_variants(&[], &s(&["LOD_FADE_PERCENTAGE"]), 2);
        assert_eq!((code(d.fragment), code(d.vertex)), (Some(3), Some(13)));
        assert_eq!(programs.draw_variants(&[], &[], 0), DrawVariants::default());
        // the keyword every vertex variant of the tier has is the draw's though nobody states it (the player stores no
        // vertex variant without it): implied, and named
        let d = programs.draw_variants(&[], &[], 1);
        assert_eq!((code(d.fragment), code(d.vertex)), (Some(1), Some(11)));
        assert_eq!(d.implied, [&"LOD_FADE_PERCENTAGE".to_owned()]);
        // stated, it is not implied again
        let d = programs.draw_variants(&[], &s(&["LOD_FADE_PERCENTAGE"]), 1);
        assert!(d.implied.is_empty());
    }

    /// The submesh each primitive is follows a rebuild of the primitive list (one split in two, one dropped).
    #[test]
    fn a_meshs_submeshes_follow_its_primitives() {
        let m = UnityMesh {
            mesh: UnityObject {
                file: "level1".into(),
                path_id: 7,
            },
            submeshes: Some((2, 3)),
            drawn: vec![2, 3, 4],
        };
        let r = m.remap_primitives(&[0, 0, 2]).unwrap().unwrap();
        assert_eq!(r.drawn, [2, 2, 4]);
        assert_eq!((r.mesh, r.submeshes), (m.mesh.clone(), m.submeshes));
        assert!(m.remap_primitives(&[3]).unwrap().is_err());
    }

    #[test]
    fn queue_tags_read_as_shaderlab_writes_them() {
        for (tag, q) in [
            ("Geometry", Some(2000)),
            ("AlphaTest", Some(2450)),
            ("Transparent", Some(3000)),
            ("Transparent-1", Some(2999)),
            ("Geometry+50", Some(2050)),
            ("geometrylast", Some(2500)),
            ("Overlay", Some(4000)),
            ("Background", Some(1000)),
            ("2100", Some(2100)),
            ("Sky", None),
            ("Geometry+x", None),
        ] {
            assert_eq!(queue_tag(tag), q, "{tag}");
        }
    }

    /// A non-finite number a material stores is kept: its JSON view writes it as a token, and the accessors give it to
    /// no lowering (unset), counted by `non_finite`.
    #[test]
    fn non_finite_material_numbers_are_kept_as_stored() {
        let mut m = material(2000, Vec::new());
        m.floats = vec![("_Glossiness".into(), 0.5), ("_Fade".into(), f32::INFINITY)];
        m.colors = vec![("_CameraFadeParams".into(), [0.0, f32::INFINITY, 0.0, 1.0])];
        m.textures = vec![UnityTexEnv {
            name: "_MainTex".into(),
            referenced: false,
            texture: None,
            scale: [f32::NAN, 1.0],
            offset: [0.0, 0.0],
        }];
        let json = m.to_json().expect("a faithful view").expect("a value");
        assert_eq!(json["floats"][1][1], "inf");
        assert_eq!(json["floats"][0][1], 0.5);
        assert_eq!(json["colors"][0][1][1], "inf");
        assert_eq!(json["textures"][0]["scale"][0], "nan");
        assert_eq!(m.float("_Fade"), None);
        assert_eq!(m.float("_Glossiness"), Some(0.5));
        assert_eq!(m.color("_CameraFadeParams"), None);
        assert_eq!(m.non_finite(), 2);
        assert_eq!(m.textures[0].finite_tiling(), None);
    }

    fn material(render_queue: i64, subshaders: Vec<SubShader>) -> UnityMaterial {
        UnityMaterial {
            material: UnityObject {
                file: "sharedassets1.assets".into(),
                path_id: 1,
            },
            shader: Some("Shader Graphs/Example".into()),
            shader_state: subshaders,
            textures: Vec::new(),
            floats: Vec::new(),
            ints: Vec::new(),
            colors: Vec::new(),
            keywords: Vec::new(),
            tags: Vec::new(),
            render_queue,
        }
    }

    fn sub(tags: &[(&str, &str)]) -> SubShader {
        fixture::sub(tags, vec![fixture::own(None, 1.0, 0.0)])
    }

    #[test]
    fn a_materials_queue_is_its_own_else_its_subshaders_tag_else_geometry() {
        let urp = sub(&[("RenderPipeline", "UniversalPipeline"), ("QUEUE", "Transparent")]);
        let hdrp = sub(&[("RenderPipeline", "HDRenderPipeline"), ("QUEUE", "Overlay")]);
        let builtin = sub(&[("QUEUE", "AlphaTest")]);
        let u = Some(&RenderPipeline::Universal);
        let b = Some(&RenderPipeline::BuiltIn);
        // its own queue wins (URP writes _QueueOffset into it: the water's 2050)
        assert_eq!(material(2050, vec![urp.clone()]).render_queue(u).0, 2050);
        // -1: the tag of the subshader the pipeline takes, skipping another pipeline's
        assert_eq!(
            material(-1, vec![hdrp.clone(), urp.clone()]).render_queue(u).0,
            3000
        );
        assert_eq!(
            material(-1, vec![urp.clone(), builtin.clone()]).render_queue(b).0,
            2450
        );
        // no subshader of the pipeline with a tag, none at all, or no pipeline known: Geometry
        assert_eq!(material(-1, vec![hdrp]).render_queue(u).0, 2000);
        assert_eq!(material(-1, Vec::new()).render_queue(u).0, 2000);
        assert_eq!(material(-1, vec![urp]).render_queue(None).0, 2000);
        let other = RenderPipeline::Other(vec!["HDRenderPipelineAsset".into()]);
        assert_eq!(material(-1, vec![builtin]).render_queue(Some(&other)).0, 2000);
    }

    /// Each pipeline takes the first subshader it supports: URP its own or an untagged one, built-in an untagged one,
    /// another pipeline none (its tag is not known).
    #[test]
    fn each_pipeline_takes_the_first_subshader_it_supports() {
        let urp = sub(&[("RenderPipeline", "UniversalPipeline"), ("QUEUE", "1")]);
        let untagged = sub(&[("QUEUE", "2")]);
        let m = material(-1, vec![urp, untagged]);
        let queue = |p| m.subshader(&p).map(|s| s.tags[s.tags.len() - 1].1.clone());
        assert_eq!(queue(RenderPipeline::Universal).as_deref(), Some("1"));
        assert_eq!(queue(RenderPipeline::BuiltIn).as_deref(), Some("2"));
        assert_eq!(queue(RenderPipeline::Other(Vec::new())), None);
    }

    /// The draw pass: built-in's base pass even when a GrabPass, a UsePass, an unlit or a per-light pass comes first
    /// (light modes compared ignoring case); without one the first unlit pass, else a vertex-lit one; URP's first
    /// forward pass, not its shadow caster; none under another pipeline.
    #[test]
    fn the_draw_pass_is_the_pipelines_surface_pass() {
        use crate::shader::PassKind;
        let b = RenderPipeline::BuiltIn;
        let u = RenderPipeline::Universal;
        let picked = |passes: Vec<ShaderPass>, p: &RenderPipeline| {
            let s = fixture::sub(&[], passes);
            draw_pass(&s, p).cloned()
        };
        let one_zero = || (fixture::value(1.0, None), fixture::value(0.0, None));
        let (s, d) = one_zero();
        let grab = fixture::pass(PassKind::Grab, None, s, d);
        let (s, d) = one_zero();
        let used = fixture::pass(
            PassKind::Use("Legacy Shaders/VertexLit/SHADOWCASTER".into()),
            None,
            s,
            d,
        );
        let unlit = fixture::own(None, 1.0, 0.0);
        let add = fixture::own(Some("FORWARDADD"), 1.0, 1.0);
        let base = fixture::own(Some("FORWARDBASE"), 5.0, 10.0);
        assert_eq!(
            picked(
                vec![
                    grab.clone(),
                    used.clone(),
                    unlit.clone(),
                    add.clone(),
                    base.clone()
                ],
                &b
            ),
            Some(base.clone())
        );
        assert_eq!(
            picked(vec![grab.clone(), add.clone(), unlit.clone()], &b),
            Some(unlit.clone())
        );
        let always = fixture::own(Some("Always"), 5.0, 1.0);
        assert_eq!(picked(vec![always.clone(), unlit.clone()], &b), Some(always));
        let vertex = fixture::own(Some("Vertex"), 5.0, 10.0);
        assert_eq!(picked(vec![add.clone(), vertex.clone()], &b), Some(vertex));
        assert_eq!(picked(vec![grab, used, add], &b), None);
        let shadow = fixture::own(Some("ShadowCaster"), 1.0, 0.0);
        let forward = fixture::own(Some("UniversalForward"), 5.0, 10.0);
        assert_eq!(picked(vec![shadow.clone(), forward.clone()], &u), Some(forward));
        assert_eq!(picked(vec![shadow, base], &u), None);
        assert_eq!(picked(vec![unlit], &RenderPipeline::Other(Vec::new())), None);
    }

    /// Bound factors take the material's value, else the shader's; one that is not a `BlendMode` gives none.
    #[test]
    fn blend_factors_take_the_bound_properties() {
        let mut m = material(-1, vec![fixture::standard()]);
        let base = m.draw_pass(&RenderPipeline::BuiltIn).unwrap().clone();
        assert_eq!(base.light_mode.as_deref(), Some("FORWARDBASE"));
        assert_eq!(m.blend_factors(&base), Some((1, 1)));
        m.floats = vec![("_SrcBlend".into(), 5.0), ("_DstBlend".into(), 10.0)];
        assert_eq!(m.blend_factors(&base), Some((5, 10)));
        m.floats[1].1 = 10.5;
        assert_eq!(m.blend_factors(&base), None);
        m.floats[1].1 = 11.0;
        assert_eq!(m.blend_factors(&base), None);
    }
}
