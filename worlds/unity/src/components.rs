//! The `unity.*` dialect: what a Unity scene says about an entity that the core components do not.

use primase_core::{Component, Entity, JsonError, World};
use serde::{Serialize, Serializer};

use crate::material::{PassKind, ShaderPass, SubShader};
use crate::read::player::{ObjRef, RenderPipeline};

/// Which object of the player an entity was read from: its file (lower-case name) and path id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnityObject {
    pub file: String,
    pub path_id: i64,
}

impl From<&ObjRef> for UnityObject {
    fn from(r: &ObjRef) -> Self {
        UnityObject {
            file: r.file.clone(),
            path_id: r.path_id,
        }
    }
}

impl Component for UnityObject {
    const NAME: &'static str = "unity.object";
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
    /// Removed from a mapped copy (`bake-nonrigid`): it names the Unity mesh and submeshes the geometry was read
    /// from, which the mapped geometry no longer is; `core.source` keeps the identity.
    fn apply_linear(&self, _l: &primase_core::Linear) -> primase_core::Linearity<Self> {
        primase_core::Linearity::Drop
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
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
    pub not_decoded: Option<String>,
}

impl Component for UnityTexture {
    const NAME: &'static str = "unity.texture";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
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

/// A texture property of a material: its texture entity (`None`: none set, or not in the player) and tiling (`_ST`,
/// Unity's bottom-left origin), as stored (non-finite numbers included).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTexEnv {
    pub name: String,
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
    pub shader_state: Vec<crate::material::SubShader>,
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

    /// A pass's blend factors (source, destination) as `BlendMode` values ([`crate::material::blend_mode`]), each the
    /// bound property's value where it has one ([`Self::state_value`]); `None` when one is not a `BlendMode`.
    pub fn blend_factors(&self, pass: &ShaderPass) -> Option<(u8, u8)> {
        let mode = |v: &crate::material::ShaderValue| {
            let x = self.state_value(v);
            (x.fract() == 0.0 && (0.0..=f32::from(crate::material::blend_mode::LAST)).contains(&x))
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
    pub fn state_value(&self, v: &crate::material::ShaderValue) -> f32 {
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
    /// 2: `shader_state`; 3: each shader pass's `kind`.
    const VERSION: u32 = 3;
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

    fn verify(&self, _e: Entity, _w: &World) -> Result<(), String> {
        let n = self.resolution as usize;
        if self.heights.len() != n * n {
            return Err(format!("{} heights for resolution {n}", self.heights.len()));
        }
        if self.holes.as_ref().is_some_and(|h| h.len() != (n - 1) * (n - 1)) {
            return Err("holes are not one per cell".into());
        }
        Ok(())
    }
}

/// On the entity of a collision shape lowered apart from its collider (`unity.collider_of`, see
/// [`crate::passes::lower_collision`]): the entity of that `unity.collider` or `unity.terrain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UnityColliderOf(pub Entity);

impl Component for UnityColliderOf {
    const NAME: &'static str = "unity.collider_of";
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
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A scene's ambient lighting as RenderSettings stores it: `mode` (`m_AmbientMode`: 0 skybox, 1 trilight, a gradient
/// of the sky, equator and ground colours, 3 flat, the sky colour alone, 4 custom), the three colours as stored, and
/// `intensity` (`m_AmbientIntensity`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityAmbient {
    pub mode: i64,
    pub sky: [f32; 4],
    pub equator: [f32; 4],
    pub ground: [f32; 4],
    pub intensity: f32,
}

/// The scene's RenderSettings as far as they are read, with the player's render pipeline (`unity.render_settings`, a
/// world resource). `lower-unity-lights` chooses `core.environment`'s sun from them.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UnityRenderSettings {
    /// The light entity `m_Sun` names, when it is one of the lights read (it shines); `None` when it names none or one
    /// that does not shine.
    pub sun: Option<Entity>,
    /// The player's render pipeline; `None` when the player has no GraphicsSettings.
    pub pipeline: Option<crate::read::player::RenderPipeline>,
    /// Its ambient lighting; `None` when the scene has no RenderSettings or they store no ambient mode.
    pub ambient: Option<UnityAmbient>,
}

impl primase_core::Resource for UnityRenderSettings {
    const NAME: &'static str = "unity.render_settings";
    /// 2: `pipeline`; 3: `ambient`.
    const VERSION: u32 = 3;
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
    use crate::material::fixture;

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
        use crate::material::PassKind;
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
