//! The JSON files of a map folder as serde records: `map.json`, `textures_index.json`, `trainer.json`,
//! `pieces.json`, `stats.json` and `ui/map_image.json`. Absent optional fields are left out.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

/// `map.json`.
#[derive(Debug, Serialize)]
pub struct MapJson<'a> {
    pub format: u32,
    pub name: &'a str,
    pub display_name: &'a str,
    pub units: &'static str,
    pub up: &'static str,
    pub forward: &'static str,
    pub generator: &'a str,
    pub materials: BTreeMap<String, MaterialJson>,
    pub surface_profiles: Map<String, Value>,
    pub streaming_distances: StreamingDistances,
    pub objects: Vec<ObjectJson<'a>>,
    pub lights: Vec<LightJson>,
    pub audio_volumes: [(); 0],
    pub audio_emitters: [(); 0],
    pub interaction_prefabs: [(); 0],
    pub trigger_effects: Vec<EffectJson>,
    pub vfx_prefabs: Vec<EffectJson>,
    pub npc_routes: [(); 0],
    pub warnings: &'a [String],
    pub travel_points: Vec<TravelPointJson>,
    pub spawn: SpawnJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawn_points: Option<Vec<SpawnJson>>,
}

#[derive(Debug, Serialize)]
pub struct StreamingDistances {
    pub near: f64,
    pub medium: f64,
    pub far: f64,
}

/// A map material: Studio's ocean, or a surface material.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MaterialJson {
    Ocean(OceanMaterial),
    Surface(Box<SurfaceMaterial>),
}

#[derive(Debug, Serialize)]
pub struct OceanMaterial {
    pub surface: &'static str,
    pub srgb: bool,
    pub domain: &'static str,
    pub shader_override: &'static str,
    pub alpha: &'static str,
}

impl OceanMaterial {
    pub const OCEAN: OceanMaterial = OceanMaterial {
        surface: "water",
        srgb: true,
        domain: "ocean",
        shader_override: "ocean",
        alpha: "opaque",
    };
}

#[derive(Debug, Serialize)]
pub struct SurfaceMaterial {
    pub surface: &'static str,
    pub srgb: bool,
    pub domain: &'static str,
    pub shader_override: &'static str,
    pub alpha_source: &'static str,
    pub alpha_cutoff: f32,
    pub cast_shadows: bool,
    pub double_sided: bool,
    pub transparent_shadow: bool,
    pub alpha: &'static str,
    pub blend_method: &'static str,
    pub roughness: f64,
    pub metallic: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_color_factor: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_color_extension: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normal_srgb: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_texture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_srgb: Option<bool>,
}

/// One object record: a rendered mesh, or collision only (`render: false`).
#[derive(Debug, Serialize)]
pub struct ObjectJson<'a> {
    pub name: &'a str,
    pub placement_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mesh: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primitive: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_geometry: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_render_origin: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collision_mesh: Option<&'a str>,
    pub transform: [f32; 12],
    #[serde(rename = "static")]
    pub is_static: bool,
    pub collision_mode: &'static str,
    pub collision_material: String,
    pub collision_material_packed: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world_transform_baked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_object: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grind_curve: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct LightJson {
    pub name: String,
    pub source_path: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub basis: Basis,
    pub color_linear: [f32; 3],
    pub energy: f32,
    pub normalize_power: bool,
    pub uses_light_nodes: bool,
    pub exposure: i32,
    pub cast_shadow: bool,
    pub use_custom_distance: bool,
    pub cutoff_distance: f32,
    pub diffuse_factor: i32,
    pub specular_factor: i32,
    pub time_of_day: i32,
    pub native_entity_authored: bool,
    pub radius: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spot_size_radians: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spot_blend: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct Basis {
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
}

/// A trigger effect or a VFX prefab.
#[derive(Debug, Serialize)]
pub struct EffectJson {
    pub name: String,
    pub blueprint: String,
    pub transform: [f32; 12],
}

#[derive(Debug, Serialize)]
pub struct TravelPointJson {
    pub name: String,
    pub source_name: String,
    pub source_path: String,
    pub source_collections: [&'static str; 1],
    pub position: [f32; 3],
    pub yaw: f32,
    pub stop_name: String,
    pub shelter: bool,
}

#[derive(Debug, Serialize)]
pub struct SpawnJson {
    pub name: String,
    pub position: [f32; 3],
    pub yaw: f64,
}

/// `textures_index.json`.
#[derive(Debug, Serialize)]
pub struct TexturesIndex<'a> {
    pub tex_cap: u32,
    pub tex_caps: &'a str,
    pub textures: Vec<TextureEntry<'a>>,
}

#[derive(Debug, Serialize)]
pub struct TextureEntry<'a> {
    pub file: String,
    pub w: u32,
    pub h: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub src_format: &'a str,
    pub bytes: u64,
    pub sources: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_variants: Option<Vec<&'a str>>,
}

/// `trainer.json`.
#[derive(Debug, Serialize)]
pub struct TrainerJson<'a> {
    pub schema: u32,
    pub note: &'a str,
    pub spots: Vec<TrainerSpot>,
}

#[derive(Debug, Serialize)]
pub struct TrainerSpot {
    pub name: String,
    pub position: [f32; 3],
}

/// `pieces.json`: where each merged piece came from and how the overlay stack moved it.
#[derive(Debug, Serialize)]
pub struct PiecesJson<'a> {
    pub overlay_step_m: f32,
    pub pieces: Vec<PieceJson<'a>>,
}

#[derive(Debug, Serialize)]
pub struct PieceJson<'a> {
    pub mesh: &'a str,
    pub material: &'a str,
    pub pass: i32,
    pub order: i32,
    pub shift_mm: f32,
    pub tris: usize,
    pub gta: [f32; 3],
    pub drawable: &'a str,
    pub entity: [f32; 3],
    pub shader: &'a str,
    pub bucket: u8,
    pub model: i32,
    pub geometry: i32,
    pub entity_no: i32,
}

/// `ui/map_image.json`: the pause map picture's extent in Studio's frame.
#[derive(Debug, Serialize)]
pub struct MapImageJson {
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
}

/// `stats.json`: what the build did.
#[derive(Debug, Serialize)]
pub struct StatsJson<'a> {
    pub name: &'a str,
    pub scene: String,
    pub region: [f64; 4],
    pub tile_size: f32,
    pub lift_m: f32,
    pub totals: Totals,
    pub soups: Vec<SoupStats<'a>>,
    pub collision_dropped: CollisionDropped,
    pub overlay_stack: OverlayStack,
    pub texture_formats: BTreeMap<&'a str, i64>,
    /// The props with the most collision triangles, most first.
    pub prop_collision_top: Map<String, Value>,
    pub warnings: Vec<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct Totals {
    pub placements: usize,
    pub placements_degenerate: i64,
    pub placements_stubs_left_out: i64,
    pub placed_parts_stubs_left_out: i64,
    pub placements_clipped_away: i64,
    pub prototypes_placed: usize,
    pub prototypes_failed: i64,
    pub tiles: usize,
    pub object_records: usize,
    pub merged_placements: i64,
    pub merged_triangles: i64,
    pub instances: i64,
    pub instanced_triangles_drawn: i64,
    pub prototypes: usize,
    pub prototype_triangles: i64,
    pub collision_pieces: i64,
    pub collision_triangles: i64,
    pub collision_hulls: i64,
    pub materials: usize,
    pub textures: usize,
    pub textures_flattened_by_budget: i64,
    pub texture_file_bytes: u64,
    pub texture_pixels: u64,
    pub mesh_file_bytes: u64,
    pub textures_unsupported: u64,
    pub placements_with_empty_mesh: i64,
    pub lights_written: usize,
    pub lights_dropped_by_cell_cap: usize,
    pub lights_without_tangent: usize,
    pub grind_curves: i64,
    pub grind_pieces: i64,
    pub grind_length_m: f32,
    pub travel_points: usize,
    pub trainer_spots: usize,
    pub peak_mesh_bytes: i64,
    pub seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct SoupStats<'a> {
    pub file: &'a str,
    pub mode: &'static str,
    pub read: usize,
    pub cut_by_region: i64,
    pub kept_before_cleaning: i64,
}

#[derive(Debug, Serialize)]
pub struct CollisionDropped {
    pub degenerate: i64,
    pub beyond_map_edge: i64,
}

#[derive(Debug, Serialize)]
pub struct OverlayStack {
    pub step_m: f32,
    pub overlay_pieces: i64,
    pub shifted: i64,
    pub water_shifted: i64,
    pub max_shift_m: f32,
    pub order_cycles_broken: i64,
}
