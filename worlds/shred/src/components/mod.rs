//! Source identities survive frame conversion; no material semantics are inferred from IDs.
use primase_core::{Component, Geometry, Linear, Transform, world::Linearity};
use serde::Serialize;
mod skin;
pub use skin::{SourceBone, SourceSkeleton, SourceSkin, SourceSkinVertex};
mod vertex_inputs;
pub use vertex_inputs::{VertexInput, VertexInputs};

mod collision;
mod level_params;
mod material;
pub use collision::{CollisionSurface, CollisionSurfaces, DynamicShape, MOTION_FIXED, RigidBody};
pub use level_params::{LevelParams, ScriptCall, ScriptValue};
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

/// One SCP spline as authored, beside a core `Curve` of its points (`kind` there: `rail_candidate`, `guide`,
/// `ai_path`, `nav_path` or `short_path`; the section type says which, and `flags >> 16` is that type).
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

/// One SCP spot camera record (section type 3) as authored: the entity's transform is the record's first matrix, the
/// frame it places; `target` is the second, in the same (requested) frame. Reading them as a camera and the point it
/// looks at comes from the spot names and the frame pair; the engine function that uses them was not traced.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpotCamera {
    pub pak: String,
    pub quartet: Vec<u8>,
    pub section: usize,
    pub index: u32,
    pub id: u32,
    /// sdbm of a database spot name as written.
    pub name: u32,
    pub parent: u32,
    /// An f32 that is 53.638 in every record of the game.
    pub value: f32,
    /// 1 in every record of the game.
    pub flag: u32,
    pub target: Transform,
}
component!(SpotCamera, "shred.spot_camera");

/// One SCP locator record (section type 5) as authored: the entity's transform is the record's matrix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Locator {
    pub pak: String,
    pub quartet: Vec<u8>,
    pub section: usize,
    pub index: u32,
    /// sdbm of the class name (`RMLocator`).
    pub class: u32,
    pub name: u32,
    /// sdbm of the kind name (`Waypoint` for the one record there is).
    pub kind: u32,
}
component!(Locator, "shred.locator");

/// One SCP occluder record (section type b) as authored: the entity's transform is the record's matrix. Nothing
/// draws or collides with it; the engine's Lua binding `set_occluder_enabled` is the only consumer found.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Occluder {
    pub pak: String,
    pub quartet: Vec<u8>,
    pub section: usize,
    pub index: u32,
    /// sdbm of the occluder's name: `RMOccluder` or `RMOccluder<N>` for 276 of the 341 on the disc.
    pub name: u32,
    /// The record's flags as stored: the low half is the outline's shape (`0x0800`: four segments, `0x1801`:
    /// twelve), the high half is unknown.
    pub flags: u32,
    /// The outline's segments as stored (not mapped by the requested frame).
    pub outline: Vec<[[f32; 3]; 2]>,
}
component!(Occluder, "shred.occluder");
