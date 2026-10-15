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
/// static-batched combined mesh it draws (first submesh, count), and its shadow casting mode.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityRenderer {
    pub renderer: UnityObject,
    pub layer: u32,
    pub materials: Vec<Option<UnityObject>>,
    pub submeshes: Option<(u32, u32)>,
    pub cast_shadows: i64,
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

/// A material as Unity stores it: its shader's name and saved properties in stored order (colours gamma-encoded, as
/// stored), keywords, tags and render queue.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnityMaterial {
    pub material: UnityObject,
    pub shader: Option<String>,
    pub textures: Vec<UnityTexEnv>,
    pub floats: Vec<(String, f32)>,
    pub ints: Vec<(String, i64)>,
    pub colors: Vec<(String, [f32; 4])>,
    pub keywords: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub render_queue: i64,
}

impl UnityMaterial {
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
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
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

/// A renderer the world leaves out, and why (`procedural geometry: Shapes/Line 2D Opaque`): it keeps its transform,
/// `unity.renderer` and the GameObject's `unity.behaviours`, but no instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnityLeftOut(pub String);

impl Component for UnityLeftOut {
    const NAME: &'static str = "unity.left_out";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
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
    pub culling_mask: u32,
    pub cookie: Option<UnityObject>,
}

impl Component for UnityLight {
    const NAME: &'static str = "unity.light";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
