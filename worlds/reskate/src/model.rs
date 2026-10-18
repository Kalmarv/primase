//! What the map writer works on, in its z-up frame (east, north, up): prototypes made of parts, and placements.

use crate::material::VClass;
use primase_meshops::vector::Vec3;

/// A run of triangles of a prototype sharing one material choice.
#[derive(Clone, Debug, Default)]
pub struct SrcPart {
    pub class: Option<VClass>,
    pub emissive: bool,
    /// Map material id; `None`: chosen per placement (the variant resolver).
    pub material: Option<usize>,
    /// The scene part it came from (prototype index, part index), for the variant resolver.
    pub def: Option<(usize, usize)>,
    /// Blended triangles over too many texture repeats: alpha tested instead (UV-repeat rule).
    pub cutout: bool,
    pub shader: String,
    pub bucket: u8,
    pub model: i32,
    pub geom: i32,
    pub pos: Vec<f32>,
    pub nrm: Vec<f32>,
    pub uv: Vec<f32>,
    pub idx: Vec<u32>,
    /// Per-vertex blend weight of a `vertex_blend` part (empty: none).
    pub blend: Vec<f32>,
}

impl SrcPart {
    pub fn nverts(&self) -> usize {
        self.pos.len() / 3
    }
}

/// How a prototype's collision is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CollisionMode {
    #[default]
    TriangleMesh,
    Water,
    Hull,
}

/// A prototype: its parts in local space, in draw order.
#[derive(Clone, Debug, Default)]
pub struct SrcMesh {
    /// The scene's prototype key: identity within one scene (one part) only.
    pub hash: u32,
    /// What the prototype's parts hold ([`content_key`]): its identity across parts, with the materials a variant row
    /// draws it with ([`crate::map`]'s prototype key).
    pub content: [u8; 16],
    pub name: String,
    pub parts: Vec<SrcPart>,
    pub min: Vec3,
    pub max: Vec3,
    pub tris: i64,
    pub verts: i64,
    pub is_tree: bool,
    pub has_variants: bool,
    pub bytes: i64,
    pub col_pos: Option<Vec<f32>>,
    pub col_idx: Option<Vec<u32>>,
    pub col_surf: Vec<u8>,
    pub col_mode: CollisionMode,
    pub col_ground: bool,
}

/// One placement, in the writer's frame.
#[derive(Clone, Debug, Default)]
pub struct Placement {
    pub proto: u32,
    pub variant: u32,
    pub pos: Vec3,
    pub ex: Vec3,
    pub ey: Vec3,
    pub ez: Vec3,
    pub radius: f32,
    pub tx: i32,
    pub ty: i32,
    pub outside: bool,
    pub no_collision: bool,
    pub no_instance: bool,
    pub part_off: Option<Vec<bool>>,
    /// Index in the scene's placement list (identity for the split-by-tile expansion).
    pub index: usize,
    /// A split-by-tile whole (expanded later).
    pub split_whole: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ProtoInfo {
    pub id: usize,
    pub name: String,
    pub file: String,
    pub tris: i64,
    pub verts: i64,
    pub parts: usize,
    pub count: i64,
    pub tree: bool,
}

#[derive(Clone, Debug)]
pub struct TileOut {
    pub x: i32,
    pub y: i32,
    pub tris: i64,
    pub verts: i64,
    pub parts: usize,
    pub instances: i64,
    pub entities: i64,
    pub mesh: Option<String>,
    pub zmin: f32,
    pub zmax: f32,
}

impl TileOut {
    pub fn new(x: i32, y: i32) -> TileOut {
        TileOut {
            x,
            y,
            tris: 0,
            verts: 0,
            parts: 0,
            instances: 0,
            entities: 0,
            mesh: None,
            zmin: f32::MAX,
            zmax: f32::MIN,
        }
    }
}

/// A convex hull written whole as one collision object (world positions in the writer's frame).
#[derive(Clone, Debug)]
pub struct Hull {
    pub name: String,
    pub pos: Vec<f32>,
    pub idx: Vec<u32>,
    pub surface: usize,
    pub ground: bool,
}

/// One object record of map.json.
#[derive(Clone, Debug)]
pub struct MapObject {
    pub name: String,
    pub mesh: Option<String>,
    pub material: Option<String>,
    pub collision_mesh: Option<String>,
    pub shared_origin: Option<[f32; 3]>,
    pub primitive: usize,
    pub transform: [f32; 12],
    pub render: bool,
    pub collision_mode: &'static str,
    pub collision_packed: i32,
    pub grind_source: Option<String>,
}

impl MapObject {
    /// Studio's water collision material.
    pub const WATER_PACKED: i32 = 0x00F0_07A0;

    pub fn new(name: String, transform: [f32; 12]) -> MapObject {
        MapObject {
            name,
            mesh: None,
            material: None,
            collision_mesh: None,
            shared_origin: None,
            primitive: 0,
            transform,
            render: true,
            collision_mode: "none",
            collision_packed: 32,
            grind_source: None,
        }
    }

    /// Studio's 12 numbers for a z-up basis and position: conv(v) = (x, z, -y).
    pub fn rows(ex: Vec3, ey: Vec3, ez: Vec3, p: Vec3) -> [f32; 12] {
        [
            ex.x, ex.z, -ex.y, ez.x, ez.z, -ez.y, -ey.x, -ey.z, ey.y, p.x, p.z, -p.y,
        ]
    }

    pub fn translation(x: f32, y: f32, z: f32) -> [f32; 12] {
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, x, z, -y]
    }
}

/// SHA-256 (first 128 bits) of a prototype's parts in draw order: every field that reaches the output. Each part's
/// class, emissive and cutout flags, shader, bucket, model and geometry numbers, its fixed material (as `Some(id)`/`None`;
/// the map key adds the material names), then its positions, normals, UVs, blend weights (as f32 bits) and indices,
/// each string and stream preceded by its length. Not `def`, the scene's own index. Two prototypes with this key draw
/// the same thing given the same materials.
pub fn content_key(parts: &[SrcPart]) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update((parts.len() as u64).to_le_bytes());
    for p in parts {
        h.update([
            p.class.map_or(0xFF, |c| c as u8),
            u8::from(p.emissive),
            u8::from(p.cutout),
            u8::from(p.material.is_some()),
            p.bucket,
        ]);
        h.update((p.shader.len() as u64).to_le_bytes());
        h.update(p.shader.as_bytes());
        h.update(p.model.to_le_bytes());
        h.update(p.geom.to_le_bytes());
        for stream in [&p.pos, &p.nrm, &p.uv, &p.blend] {
            h.update((stream.len() as u64).to_le_bytes());
            for x in stream.iter() {
                h.update(x.to_bits().to_le_bytes());
            }
        }
        h.update((p.idx.len() as u64).to_le_bytes());
        for i in &p.idx {
            h.update(i.to_le_bytes());
        }
    }
    let d = h.finalize();
    let mut k = [0u8; 16];
    k.copy_from_slice(&d[..16]);
    k
}
