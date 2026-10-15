//! The core components: what every reader, pass and writer agrees on. Small and versioned; everything game- or
//! target-specific is a dialect component owned by its crate (`fromsoft.*`, `gta.*`, `skate.*`, `pbr.*`).
//!
//! Entity roles (by convention, not enforced by the store):
//! - a **prototype** has a [`Mesh`] in its own local space (plus [`Name`], [`Source`], [`Bounds`]);
//! - a **placement** has a [`Transform`] and an [`Instance`] pointing at its prototype;
//! - a **material** has the material components of some dialect (`pbr.material`, `skate.material`, ...);
//!   a [`Primitive`] refers to it by entity;
//! - a **texture** has a [`Texture`] (or only a [`TextureFile`] until a pass loads it);
//! - a prototype whose geometry is still in a file has a [`MeshFile`] (a pass loads it into a [`Mesh`]), and
//!   [`MaterialVariants`] when placements pick materials per variant.
//!
//! Geometry is in the world frame ([`crate::frame`]): metres, right-handed, +Y up, -Z north; counter-clockwise front
//! faces; UV v = 0 at the image's top row (glTF).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use crate::json::JsonError;
use crate::texformat::TexFormat;
use crate::world::{Component, Entity};

/// Display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Name(pub String);
impl Component for Name {
    const NAME: &'static str = "core.name";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// Provenance: a stable key into the source game's data (GTA archetype hash, Sekiro MSB part, asset path), for
/// debugging, reports and review links.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source(pub String);
impl Component for Source {
    const NAME: &'static str = "core.source";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// An affine 3x4 transform, column by column: the images of local +X, +Y, +Z, then the translation;
/// `world = M * local`. f32 because every format we read and write stores f32 (byte-identical round trips).
/// Shear and mirroring are allowed; [`Transform::classify`] flags them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Transform {
    pub cols: [[f32; 3]; 4],
}
impl Component for Transform {
    const NAME: &'static str = "core.transform";
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        finite(self.cols.iter().flatten().copied(), "transform")
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// What a transform does besides rotating, scaling and translating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformKind {
    /// Rotation with any per-axis scale and a translation.
    Regular,
    /// Some axis is (near) zero or the axes are (near) coplanar.
    Degenerate,
    /// Axes not orthogonal.
    Sheared,
    /// Negative determinant (and not sheared).
    Mirrored,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        cols: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0., 0., 0.]],
    };

    /// The skate-scene v1 placement rule: with `u_i = c_i / |c_i|`, degenerate when some
    /// `|c_i| < 1e-6` or `|det| <= 1e-6 |c0||c1||c2|`; sheared when some `|u_i . u_j| > 1e-4`; mirrored when `det < 0`.
    /// Computed in f64.
    pub fn classify(&self) -> TransformKind {
        let c: [[f64; 3]; 3] = std::array::from_fn(|i| self.cols[i].map(f64::from));
        let len: [f64; 3] = std::array::from_fn(|i| dot(c[i], c[i]).sqrt());
        let det = dot(c[0], cross(c[1], c[2]));
        if len.iter().any(|&l| l < 1e-6) || det.abs() <= 1e-6 * len[0] * len[1] * len[2] {
            return TransformKind::Degenerate;
        }
        let u: [[f64; 3]; 3] = std::array::from_fn(|i| c[i].map(|x| x / len[i]));
        if [(0, 1), (0, 2), (1, 2)]
            .iter()
            .any(|&(i, j)| dot(u[i], u[j]).abs() > 1e-4)
        {
            return TransformKind::Sheared;
        }
        if det < 0.0 {
            TransformKind::Mirrored
        } else {
            TransformKind::Regular
        }
    }
}

/// The position of the first non-finite value, as an error.
fn finite(values: impl Iterator<Item = f32>, what: &str) -> Result<(), String> {
    match values.enumerate().find(|(_, x)| !x.is_finite()) {
        Some((i, x)) => Err(format!("{what}: value {i} is {x}, not finite")),
        None => Ok(()),
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A placement of a prototype.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Instance {
    pub prototype: Entity,
    /// Selects per-placement material variants of the prototype's primitives (GTA tint rows, see
    /// [`MaterialVariants`]); 0 when it has none.
    pub variant: u32,
}
impl Component for Instance {
    const NAME: &'static str = "core.instance";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// A vertex attribute's meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Semantic {
    Position,
    Normal,
    /// xyz tangent, w the bitangent sign (glTF).
    Tangent,
    TexCoord(u8),
    Color(u8),
    Joints(u8),
    Weights(u8),
}

/// One vertex stream, decoded to the type a writer can use directly. Readers that need to keep a game's
/// quantisation (SNORM16 positions with a scale, packed normals) attach it as a dialect component and lower later.
#[derive(Clone, Debug, PartialEq)]
pub enum Stream {
    F32x2(Vec<[f32; 2]>),
    F32x3(Vec<[f32; 3]>),
    F32x4(Vec<[f32; 4]>),
    U8x4(Vec<[u8; 4]>),
    U16x4(Vec<[u16; 4]>),
}

impl Stream {
    pub fn len(&self) -> usize {
        match self {
            Stream::F32x2(v) => v.len(),
            Stream::F32x3(v) => v.len(),
            Stream::F32x4(v) => v.len(),
            Stream::U8x4(v) => v.len(),
            Stream::U16x4(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Primitive topology. Strips and fans are lowered to lists by readers (toolkit vertex helpers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Topology {
    Triangles,
    Lines,
    Points,
}

/// One drawable part of a mesh: vertex streams, optional indices, a material. Primitives are kept in the game's
/// draw order (overlay stacking depends on it).
#[derive(Clone, Debug, PartialEq)]
pub struct Primitive {
    pub topology: Topology,
    /// Every stream has the same length (the vertex count). `Position` is required.
    pub streams: BTreeMap<Semantic, Stream>,
    /// Vertex indices; `None` means sequential.
    pub indices: Option<Vec<u32>>,
    /// The material entity, if any.
    pub material: Option<Entity>,
}

impl Primitive {
    pub fn positions(&self) -> Option<&[[f32; 3]]> {
        match self.streams.get(&Semantic::Position)? {
            Stream::F32x3(v) => Some(v),
            _ => None,
        }
    }

    pub fn vertex_count(&self) -> usize {
        self.streams.get(&Semantic::Position).map_or(0, Stream::len)
    }
}

/// A prototype's geometry in its own local space.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Mesh {
    pub primitives: Vec<Primitive>,
}
impl Component for Mesh {
    const NAME: &'static str = "core.mesh";
    /// Float streams finite (glTF and every writer we have refuse NaN and infinity).
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        for (i, p) in self.primitives.iter().enumerate() {
            for (sem, st) in &p.streams {
                let what = format!("primitive {i} {sem:?}");
                match st {
                    Stream::F32x2(v) => finite(v.iter().flatten().copied(), &what)?,
                    Stream::F32x3(v) => finite(v.iter().flatten().copied(), &what)?,
                    Stream::F32x4(v) => finite(v.iter().flatten().copied(), &what)?,
                    Stream::U8x4(_) | Stream::U16x4(_) => {}
                }
            }
        }
        Ok(())
    }
    /// A summary, not the vertex data: per primitive its topology, vertex and index counts, streams and material.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let prims: Vec<Value> = self
            .primitives
            .iter()
            .map(|p| {
                json!({
                    "topology": p.topology,
                    "vertices": p.vertex_count(),
                    "indices": p.indices.as_ref().map(Vec::len),
                    "streams": p.streams.keys().collect::<Vec<_>>(),
                    "material": p.material,
                })
            })
            .collect();
        Ok(Some(json!({ "primitives": prims })))
    }
}

/// Colour space of a texture's colour channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ColorSpace {
    Srgb,
    Linear,
}

/// What a texture is used as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum TexRole {
    Color,
    Normal,
    /// Packed scalar maps (metal/rough/AO/opacity, specular, ...): always linear.
    Mask,
    Other,
}

/// Normal-map green direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum NormalGreen {
    /// OpenGL / glTF convention.
    Up,
    /// DirectX convention.
    Down,
}

/// Where a normal map's Z comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum NormalZ {
    /// Blue is Z.
    Stored,
    /// Z = sqrt(1 - x^2 - y^2); blue is ignored (or holds something else).
    Rebuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NormalConvention {
    pub green: NormalGreen,
    pub z: NormalZ,
}

/// Pixels or encoded blocks in memory.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub format: TexFormat,
    /// Mip levels, largest first; each exactly `format.level_size(w >> i, h >> i)` bytes (axes floor at 1).
    pub levels: Vec<Vec<u8>>,
    pub color_space: ColorSpace,
    pub role: TexRole,
    /// For `TexRole::Normal`.
    pub normal: Option<NormalConvention>,
}
impl Component for Texture {
    const NAME: &'static str = "core.texture";
    /// A summary, not the pixels: size, format, level count, colour space, role, normal convention.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        Ok(Some(json!({
            "width": self.width,
            "height": self.height,
            "format": format!("{:?}", self.format),
            "levels": self.levels.len(),
            "color_space": self.color_space,
            "role": self.role,
            "normal": self.normal,
        })))
    }
}

/// A texture whose data is still in a file (PNG or DDS); a pass loads it into a [`Texture`]. Lets readers describe
/// thousands of textures without holding their pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextureFile {
    pub path: PathBuf,
}
impl Component for TextureFile {
    const NAME: &'static str = "core.texture_file";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// A prototype whose geometry is still in a file (a GLB); a pass loads it into a [`Mesh`]. Present only while the
/// prototype's geometry is exactly that file's mesh: a pass that changes the geometry removes it, so a writer may
/// copy the file instead of encoding the mesh again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MeshFile {
    pub path: PathBuf,
    /// Which mesh of the file (glTF `meshes[mesh]`).
    pub mesh: u32,
    /// One entry per primitive of that mesh, in order: the material the reader already knows for it (skate-scene:
    /// `parts[i].material`). The load pass checks the primitive count against it and fills [`Primitive::material`].
    pub materials: Vec<Option<Entity>>,
}
impl Component for MeshFile {
    const NAME: &'static str = "core.mesh_file";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// Per-placement material choices of a prototype: one map per primitive, variant -> material, overriding the
/// primitive's material for placements whose [`Instance::variant`] matches (GTA tint rows).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct MaterialVariants(pub Vec<BTreeMap<u32, Entity>>);
impl Component for MaterialVariants {
    const NAME: &'static str = "core.material_variants";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// Axis-aligned box in the entity's own space (local for prototypes, world for soups and placements).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}
impl Component for Bounds {
    const NAME: &'static str = "core.bounds";
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        finite(self.min.iter().chain(&self.max).copied(), "bounds")
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// A streaming or processing tile, `(col, row)` in the ground frame of whoever tiled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Tile {
    pub col: i32,
    pub row: i32,
}
impl Component for Tile {
    const NAME: &'static str = "core.tile";
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(cols: [[f32; 3]; 4]) -> Transform {
        Transform { cols }
    }

    #[test]
    fn json_views() {
        let mut w = crate::World::new();
        let mat = w.spawn();
        let p = w.spawn();
        w.insert(
            p,
            MeshFile {
                path: "prototypes/a.glb".into(),
                mesh: 1,
                materials: vec![Some(mat), None],
            },
        );
        w.insert(
            p,
            MaterialVariants(vec![BTreeMap::new(), BTreeMap::from([(3, mat)])]),
        );
        w.insert(p, Name("a".into()));
        w.insert(
            p,
            Mesh {
                primitives: vec![Primitive {
                    topology: Topology::Triangles,
                    streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.; 3]; 3]))]),
                    indices: None,
                    material: Some(mat),
                }],
            },
        );
        let got: Vec<(&str, Option<Value>)> = w
            .components_json(p)
            .unwrap()
            .into_iter()
            .map(|c| (c.name, c.value))
            .collect();
        let names: Vec<&str> = got.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "core.material_variants",
                "core.mesh",
                "core.mesh_file",
                "core.name"
            ]
        );
        assert_eq!(got[0].1, Some(json!([{}, {"3": 0}])));
        assert_eq!(
            got[1].1,
            Some(
                json!({"primitives": [{"topology": "Triangles", "vertices": 3, "indices": null,
                "streams": ["Position"], "material": 0}]})
            )
        );
        assert_eq!(
            got[2].1,
            Some(json!({"path": "prototypes/a.glb", "mesh": 1, "materials": [0, null]}))
        );
        assert_eq!(got[3].1, Some(json!("a")));
    }

    #[test]
    fn the_verifier_refuses_non_finite_geometry() {
        let mut w = crate::World::new();
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        w.insert(
            e,
            Bounds {
                min: [0.0; 3],
                max: [1.0, f32::INFINITY, 1.0],
            },
        );
        let mut m = Transform::IDENTITY;
        m.cols[3][2] = f32::NAN;
        let e2 = w.spawn();
        w.insert(e2, m);
        w.insert(
            e2,
            Mesh {
                primitives: vec![Primitive {
                    topology: Topology::Triangles,
                    streams: BTreeMap::from([(
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0.0, f32::NEG_INFINITY]]),
                    )]),
                    indices: None,
                    material: None,
                }],
            },
        );
        let got: Vec<(&str, Entity, String)> = w.verify_components();
        assert_eq!(
            got,
            [
                ("core.bounds", e, "bounds: value 4 is inf, not finite".to_owned()),
                (
                    "core.mesh",
                    e2,
                    "primitive 0 TexCoord(0): value 1 is -inf, not finite".to_owned()
                ),
                (
                    "core.transform",
                    e2,
                    "transform: value 11 is NaN, not finite".to_owned()
                ),
            ]
        );
        // and the JSON view refuses to show the NaN rather than writing null
        assert!(
            w.components_json(e2)
                .unwrap_err()
                .to_string()
                .contains("core.transform on e1")
        );
    }

    #[test]
    fn classify_follows_the_scene_interchange_rule() {
        assert_eq!(Transform::IDENTITY.classify(), TransformKind::Regular);
        assert_eq!(
            t([[2., 0., 0.], [0., 0., 3.], [0., -1., 0.], [5., 5., 5.]]).classify(),
            TransformKind::Regular
        );
        assert_eq!(
            t([[-1., 0., 0.], [0., 1., 0.], [0., 0., 1.], [0.; 3]]).classify(),
            TransformKind::Mirrored
        );
        assert_eq!(
            t([[1., 0., 0.], [0.1, 1., 0.], [0., 0., 1.], [0.; 3]]).classify(),
            TransformKind::Sheared
        );
        assert_eq!(
            t([[1., 0., 0.], [0., 1., 0.], [0., 0., 0.], [0.; 3]]).classify(),
            TransformKind::Degenerate
        );
    }
}
