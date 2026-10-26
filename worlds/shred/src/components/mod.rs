//! Source identities survive frame conversion; no material semantics are inferred from IDs.
use primase_core::{Component, Geometry, Linear, world::Linearity};
use serde::Serialize;
mod skin;
pub use skin::{SourceBone, SourceSkeleton, SourceSkin, SourceSkinVertex};
mod vertex_inputs;
pub use vertex_inputs::{VertexInput, VertexInputs};

mod collision;
mod material;
pub use collision::{CollisionFlags, DynamicShape, MOTION_FIXED, RigidBody};
pub use material::{
    BankEvidence, EncodedTexture, MaterialKey, MaterialNamespace, MaterialParameter, MaterialResolution,
    MaterialSampler, MaterialTextures, SamplerNameSource, SamplerTexture, ShredMaterial, SourceTexture,
    TextureCopy, TextureResidency,
};

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

/// One SCP spot chunk as authored. The entity's transform is the chunk's own world matrix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Spot {
    pub pak: String,
    pub quartet: Vec<u8>,
    /// The chunk's index within its SCP's spot chunks, in section order.
    pub index: u32,
    /// sdbm of the class name; `class_name` when the name is proven (`scp_tables::CLASS_NAMES`).
    pub class: u32,
    pub class_name: Option<String>,
    /// `Rectangle`: a centred unit box in the chunk's frame; `Grid` otherwise (hash kept when neither).
    pub shape: u32,
    pub shape_name: Option<String>,
    /// sdbm of the chunk's name; DB spot names match it (as written or lower-cased).
    pub name: u32,
    pub id: u32,
    pub parent: u32,
    pub pair: [u32; 2],
    /// Index of the node of the same SCP whose id is `id`.
    pub node: Option<u32>,
}
component!(Spot, "shred.spot");

/// One SCP spline as authored, beside a core `Curve` of its points (`kind` there: `ai_path`, `guide` or
/// `rail_candidate`; the section type says which).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spline {
    pub pak: String,
    pub quartet: Vec<u8>,
    /// The section's flags and offset, and the spline's index in it.
    pub flags: u32,
    pub section: usize,
    pub index: u32,
    /// sdbm of the spline's name (`core.name` when a name is known to hash to it).
    pub id: u32,
    pub parent: u32,
    pub counts: [u32; 2],
    pub words: [u32; 2],
    /// A `MAIN.DB` `surfaces.id` for rail candidates, 0 for the rest.
    pub surface: u32,
    pub matrix: [f32; 16],
    /// The stored per-point values, increasing from zero (not arc length).
    pub params: Vec<f32>,
}
component!(Spline, "shred.spline");

/// One SCP light record as authored: the entity's transform is the record's matrix. Not a core light: the units,
/// range, cone and shining axis of the game's lights are not established.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SceneLight {
    pub pak: String,
    pub quartet: Vec<u8>,
    pub section: usize,
    pub index: u32,
    pub kind: u32,
    pub rgba: [f32; 4],
    pub name: u32,
    pub params: [f32; 3],
    pub links: [u32; 4],
    pub tail_words: [u32; 2],
    pub buffer: [u8; 16],
}
component!(SceneLight, "shred.light");
