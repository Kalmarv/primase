//! The `unity.*` dialect: what a Unity scene says about an entity that the core components do not.

use primase_core::{Component, Entity, JsonError, World};
use serde::Serialize;

use crate::player::ObjRef;

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

/// A texture property of a material: its texture entity (`None`: none set, or not in the player) and tiling (`_ST`,
/// Unity's bottom-left origin).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityTexEnv {
    pub name: String,
    pub texture: Option<Entity>,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// A material as Unity stores it: its shader's name and render state (its subshaders, as
/// [`crate::material::ShaderData`]), saved properties in stored order (colours gamma-encoded, as stored), keywords,
/// tags and render queue.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityMaterial {
    pub material: UnityObject,
    pub shader: Option<String>,
    pub shader_state: Vec<crate::material::SubShader>,
    pub textures: Vec<UnityTexEnv>,
    pub floats: Vec<(String, f32)>,
    pub ints: Vec<(String, i64)>,
    pub colors: Vec<(String, [f32; 4])>,
    pub keywords: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub render_queue: i64,
}

impl UnityMaterial {
    /// A shader render-state value: the bound material property's float where it names one the material has, else
    /// the shader's own value.
    pub fn state_value(&self, v: &crate::material::ShaderValue) -> f32 {
        v.property
            .as_deref()
            .and_then(|p| self.float(p))
            .unwrap_or(v.value)
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }

    pub fn color(&self, name: &str) -> Option<[f32; 4]> {
        self.colors.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
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
    /// 2: `shader_state`.
    const VERSION: u32 = 2;
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
/// [`crate::lower_collision`]): the entity of that `unity.collider` or `unity.terrain`.
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

/// The scene's RenderSettings as far as they are read, with the player's render pipeline (`unity.render_settings`, a
/// world resource). `lower-unity-lights` chooses `core.environment`'s sun from them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UnityRenderSettings {
    /// The light entity `m_Sun` names, when it is one of the lights read (it shines); `None` when it names none or one
    /// that does not shine.
    pub sun: Option<Entity>,
    /// The player's render pipeline; `None` when the player has no GraphicsSettings.
    pub pipeline: Option<crate::player::RenderPipeline>,
}

impl primase_core::Resource for UnityRenderSettings {
    const NAME: &'static str = "unity.render_settings";
    /// 2: `pipeline`.
    const VERSION: u32 = 2;
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
