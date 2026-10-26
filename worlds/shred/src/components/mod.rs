//! Source identities survive frame conversion; no material semantics are inferred from IDs.
use primase_core::{Component, Geometry, Linear, world::Linearity};
use serde::Serialize;

/// A model ID is scoped to its PAK and quartet, never globally deduplicated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MeshId {
    pub pak: String,
    pub quartet: Vec<u8>,
    pub id: u32,
}

/// Global material reference. Bank interpretation and material content are separate reader work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MaterialId(pub u32);

/// Geometry that the reader could identify but could not decode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RefusedMesh {
    pub offset: usize,
    pub reason: String,
}

macro_rules! component {
    ($ty:ty, $name:literal) => {
        impl Component for $ty {
            const NAME: &'static str = $name;
            const GEOMETRY: Geometry = Geometry::None;
            fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
                Linearity::Invariant
            }
            fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
                primase_core::json::to_value(self).map(Some)
            }
        }
    };
}
component!(MeshId, "shred.mesh_id");
component!(MaterialId, "shred.material_id");
component!(RefusedMesh, "shred.refused_mesh");

/// Deferred prototype dependencies, retained before its geometry is loaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelDependencies(pub Vec<primase_core::Entity>);
impl Component for ModelDependencies {
    const NAME: &'static str = "shred.model_dependencies";
    const GEOMETRY: Geometry = Geometry::None;
    fn apply_linear(&self, _: &Linear) -> Linearity<Self> {
        Linearity::Invariant
    }
    fn references(&self, out: &mut Vec<primase_core::Entity>) {
        out.extend(&self.0);
    }
    fn to_json(&self) -> Result<Option<serde_json::Value>, primase_core::json::JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}
