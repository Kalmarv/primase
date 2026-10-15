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

/// A prototype read from a Unity mesh: the mesh, and the slice of its submeshes (first, count) when a static-batched
/// renderer draws only that part of a combined mesh; otherwise all of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnityMesh {
    pub mesh: UnityObject,
    pub submeshes: Option<(u32, u32)>,
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
