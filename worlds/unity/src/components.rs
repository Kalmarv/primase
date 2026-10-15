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
