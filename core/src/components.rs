//! The core components: what every reader, pass and writer agrees on. Small and versioned; everything game- or
//! target-specific is a dialect component owned by its crate (`<dialect>.*`, `pbr.*`).
//!
//! Entity roles (by convention, not enforced by the store):
//! - a **prototype** has a [`Mesh`] in its own local space (plus [`Name`], [`Source`]);
//! - a **placement** has a [`Transform`] and an [`Instance`] pointing at its prototype;
//! - a **material** has the material components of some dialect (`pbr.material`, a destination's `material`, ...);
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
use crate::vector::{cross, dot};
use crate::world::{Component, Entity};

/// Display name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Name(pub String);
impl Component for Name {
    const NAME: &'static str = "core.name";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// Provenance: a stable key into the source game's data (an archetype hash, a map part's name, an asset path), for
/// debugging, reports and review links.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source(pub String);
impl Component for Source {
    const NAME: &'static str = "core.source";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// An affine 3x4 transform, column by column: the images of local +X, +Y, +Z, then the translation;
/// `world = M * local`. f32 because every format we read and write stores f32 (byte-identical round trips).
/// A transform composed of several (a hierarchy, an entity placed in another's space, a tile offset) is computed in
/// f64 from the stored f32 values and each entry rounded once to f32, never rounded step by step.
/// Shear and mirroring are allowed; [`Transform::classify`] flags them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Transform {
    pub cols: [[f32; 3]; 4],
}
impl Component for Transform {
    const NAME: &'static str = "core.transform";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// The placement itself, which a pass baking its non-rigid part replaces: not mapped.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        finite(self.cols.iter().flatten().copied(), "transform")
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

/// A basis axis shorter than this, or a determinant under this fraction of the axes' lengths' product, makes a transform
/// [`TransformKind::Degenerate`]. Fixed, not an option: every consumer that classifies a placement (and
/// `primase_meshops::transform`, which applies it in f32) means the same by it.
pub const DEGENERATE_BASIS: f64 = 1e-6;
/// Two axes whose unit vectors' dot product exceeds this are not orthogonal: the transform is [`TransformKind::Sheared`].
/// Fixed, as [`DEGENERATE_BASIS`] is.
pub const SHEAR_TOLERANCE: f64 = 1e-4;

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

    /// The determinant of the linear part, in f64: negative for a mirroring transform, which reverses the winding of
    /// triangles mapped through it.
    pub fn determinant(&self) -> f64 {
        let c: [[f64; 3]; 3] = std::array::from_fn(|i| self.cols[i].map(f64::from));
        dot(c[0], cross(c[1], c[2]))
    }

    /// A rigid transform at `position` whose local -Z points along `direction` (as a light or camera shines) and whose
    /// +Y is as near world +Y as possible (+Z when `direction` is vertical); `None` for a zero or non-finite
    /// direction. A point light or round spot cone looks the same under any roll, so no roll is taken from the source.
    pub fn facing(position: [f32; 3], direction: [f32; 3]) -> Option<Transform> {
        let unit = |v: [f32; 3]| {
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            (n > 0.0 && n.is_finite()).then(|| v.map(|x| x / n))
        };
        let cross = |a: [f32; 3], b: [f32; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let z = unit(direction.map(|x| -x))?;
        let x = unit(cross([0.0, 1.0, 0.0], z)).or_else(|| unit(cross([0.0, 0.0, 1.0], z)))?;
        let y = cross(z, x);
        Some(Transform {
            cols: [x, y, z, position],
        })
    }

    /// Point `p` through the transform, `M p + t`, computed in f64 and rounded once to f32.
    pub fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| {
            (f64::from(self.cols[3][i])
                + (0..3)
                    .map(|j| f64::from(self.cols[j][i]) * f64::from(p[j]))
                    .sum::<f64>()) as f32
        })
    }

    /// A 4x4 matrix given column by column (each column x, y, z, w) as the affine transform it is: refused unless its
    /// last row (the columns' w) is 0, 0, 0, 1, each within `tolerance`, the one a format states (0: exactly). The
    /// one rule every reader of a 4x4 matrix holds it to; a format that stores three rows is affine by its storage.
    pub fn from_columns4(cols: [[f32; 4]; 4], tolerance: f32) -> Result<Transform, String> {
        let last = cols.map(|c| c[3]);
        // written so that a NaN entry is refused
        if !last
            .iter()
            .zip([0.0, 0.0, 0.0, 1.0])
            .all(|(&w, want)| (w - want).abs() <= tolerance)
        {
            return Err(format!(
                "a matrix whose last row is {last:?}, not 0 0 0 1 (not affine)"
            ));
        }
        Ok(Transform {
            cols: cols.map(|c| [c[0], c[1], c[2]]),
        })
    }

    /// `self` after `inner` (`world = self (inner local)`): the transform of an entity placed by `inner` in a space
    /// `self` places, computed in f64 from the stored values and each entry rounded once to f32. A chain of several
    /// rounds at each link; a chain whose intermediate rounding matters is composed in f64 by its owner.
    pub fn compose(&self, inner: &Transform) -> Transform {
        let o = self.cols.map(|c| c.map(f64::from));
        let linear = |v: [f32; 3]| -> [f64; 3] {
            let v = v.map(f64::from);
            std::array::from_fn(|r| o[0][r] * v[0] + o[1][r] * v[1] + o[2][r] * v[2])
        };
        let [x, y, z, t] = inner.cols.map(linear);
        Transform {
            cols: [
                x.map(|v| v as f32),
                y.map(|v| v as f32),
                z.map(|v| v as f32),
                std::array::from_fn(|r| (t[r] + o[3][r]) as f32),
            ],
        }
    }

    /// What the basis does besides rotating, scaling and translating, the one meaning every reader, writer and mesh operation
    /// that classifies a placement shares: with `u_i = c_i / |c_i|`, degenerate when some `|c_i| <` [`DEGENERATE_BASIS`] or
    /// `|det| <=` [`DEGENERATE_BASIS`] `|c0||c1||c2|` (or the determinant is not a number); sheared when some `|u_i . u_j| >` [`SHEAR_TOLERANCE`]; mirrored when
    /// `det < 0`. Computed in f64.
    pub fn classify(&self) -> TransformKind {
        let c: [[f64; 3]; 3] = std::array::from_fn(|i| self.cols[i].map(f64::from));
        let len: [f64; 3] = std::array::from_fn(|i| dot(c[i], c[i]).sqrt());
        let det = dot(c[0], cross(c[1], c[2]));
        if degenerate(&c, det) {
            return TransformKind::Degenerate;
        }
        let u: [[f64; 3]; 3] = std::array::from_fn(|i| c[i].map(|x| x / len[i]));
        if [(0, 1), (0, 2), (1, 2)]
            .iter()
            .any(|&(i, j)| dot(u[i], u[j]).abs() > SHEAR_TOLERANCE)
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

/// Whether the basis with columns `c` and determinant `det` is degenerate ([`Transform::classify`]): some axis
/// shorter than [`DEGENERATE_BASIS`], or `|det| <=` [`DEGENERATE_BASIS`] `|c0||c1||c2|`, or a determinant that isn't
/// a number.
fn degenerate(c: &[[f64; 3]; 3], det: f64) -> bool {
    let len: [f64; 3] = std::array::from_fn(|i| dot(c[i], c[i]).sqrt());
    det.is_nan()
        || len.iter().any(|&l| l < DEGENERATE_BASIS)
        || det.abs() <= DEGENERATE_BASIS * len[0] * len[1] * len[2]
}

/// The position of the first non-finite value, as an error.
fn finite(values: impl Iterator<Item = f32>, what: &str) -> Result<(), String> {
    match values.enumerate().find(|(_, x)| !x.is_finite()) {
        Some((i, x)) => Err(format!("{what}: value {i} is {x}, not finite")),
        None => Ok(()),
    }
}

/// A linear map of an entity's local space (a transform's 3x3 part), computed in f64 from the f32 entries and rounded
/// once per value: what [`crate::Component::apply_linear`] maps local-space geometry by.
///
/// Points go through `M`; normals through `M^-T` with the determinant's sign kept (they stay on the side they were
/// on) and directions through `M`, both then of unit length (zero stays zero); a tangent's w (the bitangent sign) and
/// a triangle's winding flip when the determinant is negative ([`Linear::mirrors`]). A singular map
/// ([`Linear::singular`]) turns no normal or tangent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Linear {
    c: [[f64; 3]; 3],
    /// Columns of the inverse transpose times the determinant: `[c1 x c2, c2 x c0, c0 x c1]`.
    cof: [[f64; 3]; 3],
    det: f64,
}

impl Linear {
    /// The 3x3 part of `t`.
    pub fn of(t: &Transform) -> Linear {
        Linear::new(std::array::from_fn(|i| t.cols[i]))
    }

    /// The map with these columns (images of local +X, +Y, +Z).
    pub fn new(cols: [[f32; 3]; 3]) -> Linear {
        Linear::of_f64(cols.map(|col| col.map(f64::from)))
    }

    /// The map with these columns held in f64: a part composed in f64 (a chain of bone transforms) taken as it is,
    /// not rounded to f32 first.
    pub fn of_f64(c: [[f64; 3]; 3]) -> Linear {
        let cof = [cross(c[1], c[2]), cross(c[2], c[0]), cross(c[0], c[1])];
        let det = dot(c[0], cof[0]);
        Linear { c, cof, det }
    }

    /// x negated: the mirror a mirroring placement keeps out of its transform, `M = M' * MIRROR_X`.
    pub fn mirror_x() -> Linear {
        Linear::new([[-1., 0., 0.], [0., 1., 0.], [0., 0., 1.]])
    }

    pub fn determinant(&self) -> f64 {
        self.det
    }

    /// Whether the map has no inverse to turn normals and tangents by: its basis is degenerate, as
    /// [`Transform::classify`] says of a transform with this linear part (or its determinant is not finite). The one
    /// rule [`Linear::normal`], [`Linear::tangent`] and [`Linear::inverse_columns`] answer `None` by.
    pub fn singular(&self) -> bool {
        !self.det.is_finite() || degenerate(&self.c, self.det)
    }

    /// The columns of `M^-1` (the transpose of the cofactors over the determinant); `None` for a singular map
    /// ([`Linear::singular`]).
    pub fn inverse_columns(&self) -> Option<[[f64; 3]; 3]> {
        if self.singular() {
            return None;
        }
        // row i of M^-1 is cof[i] / det (c1 x c2 dotted with c0 is det, with c1 and c2 it is 0): column j holds
        // entry j of each
        Some(std::array::from_fn(|j| {
            std::array::from_fn(|r| self.cof[r][j] / self.det)
        }))
    }

    /// Whether it reverses orientation (negative determinant): triangles must be reversed and tangent w negated.
    pub fn mirrors(&self) -> bool {
        self.det < 0.0
    }

    /// The uniform scale `s` when the map is a rotation or mirror times `s` (columns orthogonal and of one length, to
    /// a relative 1e-6): a sphere stays a sphere of `s` times its radius. `None` for any other map.
    pub fn similarity(&self) -> Option<f64> {
        let len: [f64; 3] = self.c.map(|v| dot(v, v).sqrt());
        let s = len[0];
        let tol = 1e-6 * s;
        let orthogonal = [(0, 1), (0, 2), (1, 2)]
            .iter()
            .all(|&(i, j)| dot(self.c[i], self.c[j]).abs() <= tol * s);
        (s > 0.0 && orthogonal && len.iter().all(|l| (l - s).abs() <= tol)).then_some(s)
    }

    /// For each local axis, the world axis it maps onto, when every column has exactly one nonzero entry (a signed,
    /// scaled permutation): an axis-aligned box stays one. `None` for any other map.
    pub fn axis_map(&self) -> Option<[usize; 3]> {
        let mut out = [0; 3];
        for (k, col) in self.c.iter().enumerate() {
            let nonzero: Vec<usize> = (0..3).filter(|&r| col[r] != 0.0).collect();
            match nonzero[..] {
                [r] => out[k] = r,
                _ => return None,
            }
        }
        Some(out)
    }

    fn apply(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|r| m[0][r] * v[0] + m[1][r] * v[1] + m[2][r] * v[2])
    }

    /// `M p`.
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        Self::apply(&self.c, p.map(f64::from)).map(|x| x as f32)
    }

    /// `M^-T n` times the determinant's sign, of unit length (zero stays zero); `None` under a singular map
    /// ([`Linear::singular`]), which turns no normal.
    pub fn normal(&self, n: [f32; 3]) -> Option<[f32; 3]> {
        if self.singular() {
            return None;
        }
        let v = Self::apply(&self.cof, n.map(f64::from)).map(|x| x * self.det.signum());
        Some(unit(v))
    }

    /// `M t` of unit length (zero stays zero), w negated under a mirror; `None` under a singular map
    /// ([`Linear::singular`]).
    pub fn tangent(&self, t: [f32; 4]) -> Option<[f32; 4]> {
        if self.singular() {
            return None;
        }
        let v = unit(Self::apply(&self.c, [t[0], t[1], t[2]].map(f64::from)));
        let w = if self.mirrors() { -t[3] } else { t[3] };
        Some([v[0], v[1], v[2], w])
    }
}

/// `v` of unit length as f32 ([`crate::vector::unit`]); a vector with no direction stays zero.
fn unit(v: [f64; 3]) -> [f32; 3] {
    crate::vector::unit(v).map_or([0.0; 3], |u| u.map(|x| x as f32))
}

/// A placement of a prototype.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Instance {
    pub prototype: Entity,
    /// Selects per-placement material variants of the prototype's primitives (one source's tint rows, see
    /// [`MaterialVariants`]); 0 when it has none.
    pub variant: u32,
}
impl Component for Instance {
    const NAME: &'static str = "core.instance";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.push(self.prototype);
    }
    /// A placement has its transform ([`Transform`] on the same entity).
    fn verify(&self, e: Entity, w: &crate::World) -> Result<(), String> {
        if w.has::<Transform>(e) {
            Ok(())
        } else {
            Err("a placement without a core.transform".into())
        }
    }
}

/// How a placed entity takes part in shadows as a caster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum ShadowCast {
    Off,
    On,
    /// Casts a shadow but is not drawn (a shadow proxy).
    Only,
}

/// The shadows of a drawn entity: a placement, a prototype (for each of its placements that states none of its own:
/// a placement's own overrides it), or an entity drawing its own mesh. The one place shadow casting is stated: a
/// source's fact where it has one (a reader), else the program's choice (`set-shadow-casting`). `cast: None` casts
/// (the default); `receive: None` is the renderer's default. Renderer settings (map sizes, bias, cascades) are not
/// facts of the scene and stay out of core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Shadows {
    pub cast: Option<ShadowCast>,
    pub receive: Option<bool>,
}
impl Component for Shadows {
    const NAME: &'static str = "core.shadows";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// 2: `cast: None` casts (it was the renderer's default); materials no longer state casting.
    const VERSION: u32 = 2;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// On something drawn: a placement or an entity with its own mesh.
    fn verify(&self, e: Entity, w: &crate::World) -> Result<(), String> {
        if w.has::<Instance>(e) || w.has::<Mesh>(e) {
            Ok(())
        } else {
            Err("shadows on an entity that draws nothing (no core.instance or core.mesh)".into())
        }
    }
}

/// A vertex attribute's meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Semantic {
    Position,
    Normal,
    /// Tangent set: xyz tangent, w the bitangent sign. Set 0 is glTF's standard TANGENT.
    Tangent(u8),
    /// u right, v down: (0, 0) is the image's top-left corner (glTF). Readers of bottom-left conventions store 1 - v.
    TexCoord(u8),
    Color(u8),
    Joints(u8),
    Weights(u8),
    /// Opaque per-vertex shader data, bound by the game's input index and passed through untransformed.
    /// Geometry passes must copy it through; a pass that cannot preserve it must refuse ShaderInput by name.
    /// glTF carries these application attributes as `_SHADERINPUTn`.
    ShaderInput(u8),
}

/// A shader input reads a vertex attribute by the same meaning a mesh stream has.
impl From<Semantic> for crate::shader::Attribute {
    fn from(s: Semantic) -> Self {
        use crate::shader::Attribute as A;
        match s {
            Semantic::Position => A::Position,
            Semantic::Normal => A::Normal,
            Semantic::Tangent(n) => A::Tangent(n),
            Semantic::TexCoord(n) => A::TexCoord(n),
            Semantic::Color(n) => A::Color(n),
            Semantic::Joints(n) => A::Joints(n),
            Semantic::Weights(n) => A::Weights(n),
            Semantic::ShaderInput(n) => A::ShaderInput(n),
        }
    }
}

impl From<crate::shader::Attribute> for Semantic {
    fn from(a: crate::shader::Attribute) -> Self {
        use crate::shader::Attribute as A;
        match a {
            A::Position => Semantic::Position,
            A::Normal => Semantic::Normal,
            A::Tangent(n) => Semantic::Tangent(n),
            A::TexCoord(n) => Semantic::TexCoord(n),
            A::Color(n) => Semantic::Color(n),
            A::Joints(n) => Semantic::Joints(n),
            A::Weights(n) => Semantic::Weights(n),
            A::ShaderInput(n) => Semantic::ShaderInput(n),
        }
    }
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

    /// The stream's data in bytes: its length times its element size.
    pub fn byte_len(&self) -> u64 {
        let element = match self {
            Stream::F32x2(_) => 8,
            Stream::F32x3(_) => 12,
            Stream::F32x4(_) => 16,
            Stream::U8x4(_) => 4,
            Stream::U16x4(_) => 8,
        };
        self.len() as u64 * element
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

/// One primitive of a [`Mesh`], edited in place: it reads as a [`Primitive`] and changes field by field (its streams,
/// indices, topology and material), but never as a whole, so a primitive can't be replaced by another or swapped
/// with one, and the list keeps its count and order. Per-primitive components describe the primitive in its place;
/// a pass that rebuilds the list uses [`crate::World::rebuild_primitives`] or [`crate::World::edit_mesh`], which
/// carry them along.
///
/// No slice to swap, sort or reverse:
/// ```compile_fail
/// let mut mesh = primase_core::Mesh::default();
/// mesh.primitives_mut().swap(0, 1);
/// ```
/// and no primitive replaced by another:
/// ```compile_fail
/// let mut mesh = primase_core::Mesh::default();
/// let other = mesh.primitives()[0].clone();
/// *mesh.primitive_mut(0).unwrap() = other;
/// ```
#[derive(Debug)]
pub struct PrimitiveMut<'a>(&'a mut Primitive);

impl std::ops::Deref for PrimitiveMut<'_> {
    type Target = Primitive;
    fn deref(&self) -> &Primitive {
        self.0
    }
}

impl PrimitiveMut<'_> {
    pub fn topology_mut(&mut self) -> &mut Topology {
        &mut self.0.topology
    }
    pub fn streams_mut(&mut self) -> &mut BTreeMap<Semantic, Stream> {
        &mut self.0.streams
    }
    pub fn indices_mut(&mut self) -> &mut Option<Vec<u32>> {
        &mut self.0.indices
    }
    pub fn material_mut(&mut self) -> &mut Option<Entity> {
        &mut self.0.material
    }
    /// Every field at once, for an edit that changes several (indices against the streams, say).
    pub fn fields(&mut self) -> PrimitiveFields<'_> {
        let Primitive {
            topology,
            streams,
            indices,
            material,
        } = self.0;
        PrimitiveFields {
            topology,
            streams,
            indices,
            material,
        }
    }
}

/// A primitive's fields, each editable ([`PrimitiveMut::fields`]).
#[derive(Debug)]
pub struct PrimitiveFields<'a> {
    pub topology: &'a mut Topology,
    pub streams: &'a mut BTreeMap<Semantic, Stream>,
    pub indices: &'a mut Option<Vec<u32>>,
    pub material: &'a mut Option<Entity>,
}

/// A prototype's geometry in its own local space.
///
/// Its primitive list changes only through [`crate::World::rebuild_primitives`] and [`crate::World::edit_mesh`], which
/// keep every per-primitive component (material variants, dialect provenance) in step: a mesh is built whole
/// ([`Mesh::new`]), read ([`Mesh::primitives`]) and edited one primitive at a time, field by field
/// ([`Mesh::primitives_mut`], [`Mesh::primitive_mut`]: a [`PrimitiveMut`] edits a primitive's streams, indices,
/// topology and material, and cannot replace, reorder, add or remove primitives, so per-primitive components stay with
/// the primitive they describe).
///
/// Each mesh built with [`Mesh::new`] has an identity of its own, which a clone and an edit in place keep, so the
/// conservation diagnostics ([`crate::ledger`]) tell a mesh edited in place from one replaced by another. Equality and
/// `Debug` ignore it; nothing writes it.
#[derive(Clone)]
pub struct Mesh {
    primitives: Vec<Primitive>,
    id: u64,
}
static NEXT_MESH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
impl Mesh {
    pub fn new(primitives: Vec<Primitive>) -> Self {
        Mesh {
            primitives,
            id: NEXT_MESH.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        }
    }
    /// This mesh's identity (see [`Mesh`]).
    pub(crate) fn id(&self) -> u64 {
        self.id
    }
    /// Give this mesh `id` (a mesh rebuilt in place stays the mesh it was: [`crate::World::edit_mesh`]).
    pub(crate) fn keep_id(&mut self, id: u64) {
        self.id = id;
    }
    pub fn primitives(&self) -> &[Primitive] {
        &self.primitives
    }
    /// The primitives, each editable field by field in place (see [`PrimitiveMut`]).
    pub fn primitives_mut(&mut self) -> impl ExactSizeIterator<Item = PrimitiveMut<'_>> {
        self.primitives.iter_mut().map(PrimitiveMut)
    }
    /// Primitive `i`, editable field by field in place; `None` past the last.
    pub fn primitive_mut(&mut self, i: usize) -> Option<PrimitiveMut<'_>> {
        self.primitives.get_mut(i).map(PrimitiveMut)
    }
    pub fn into_primitives(self) -> Vec<Primitive> {
        self.primitives
    }
}
impl Default for Mesh {
    fn default() -> Self {
        Mesh::new(Vec::new())
    }
}
impl PartialEq for Mesh {
    fn eq(&self, other: &Self) -> bool {
        self.primitives == other.primitives
    }
}
impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mesh")
            .field("primitives", &self.primitives)
            .finish()
    }
}
impl Component for Mesh {
    const NAME: &'static str = "core.mesh";
    /// Version 2 gives tangent streams a set index, serialized like TexCoord(n).
    const VERSION: u32 = 2;
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// Positions, normals and tangents mapped ([`crate::Linear`]); triangles reversed under a mirror. A mesh with
    /// normals or tangents cannot follow a singular map ([`crate::Linear::singular`]): unsupported.
    fn apply_linear(&self, l: &crate::Linear) -> crate::world::Linearity<Self> {
        let mut m = self.clone();
        for p in &mut m.primitives {
            for (sem, st) in &mut p.streams {
                match (sem, st) {
                    (Semantic::Position, Stream::F32x3(v)) => v.iter_mut().for_each(|x| *x = l.point(*x)),
                    // a singular map turns no normal or tangent: the mesh cannot follow it
                    (Semantic::Normal | Semantic::Tangent(_), _) if l.singular() => {
                        return crate::world::Linearity::Unsupported;
                    }
                    (Semantic::Normal, Stream::F32x3(v)) => {
                        v.iter_mut()
                            .for_each(|x| *x = l.normal(*x).expect("not singular"));
                    }
                    (Semantic::Tangent(_), Stream::F32x4(v)) => {
                        v.iter_mut()
                            .for_each(|x| *x = l.tangent(*x).expect("not singular"));
                    }
                    (Semantic::Position | Semantic::Normal | Semantic::Tangent(_), _) => {
                        return crate::world::Linearity::Unsupported;
                    }
                    _ => {}
                }
            }
            if l.mirrors() && p.topology == Topology::Triangles {
                let n = p.vertex_count() as u32;
                for t in p
                    .indices
                    .get_or_insert_with(|| (0..n).collect())
                    .as_chunks_mut::<3>()
                    .0
                {
                    t.swap(1, 2);
                }
            }
        }
        crate::world::Linearity::Applied(m)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.primitives.iter().filter_map(|p| p.material));
    }
    /// Every stream's bytes and 4 bytes per index.
    fn payload_bytes(&self) -> u64 {
        self.primitives
            .iter()
            .map(|p| {
                p.streams.values().map(Stream::byte_len).sum::<u64>()
                    + p.indices.as_ref().map_or(0, |i| i.len() as u64 * 4)
            })
            .sum()
    }
    /// Each primitive whole: a position stream of points, every stream as long as it (one value per vertex), indices
    /// within it, whole triangles or lines; and float streams finite (glTF and every writer we have refuse NaN and
    /// infinity). Joint and weight streams mean a skin: only on an entity with a `core.skin`, which checks them.
    fn verify(&self, e: Entity, w: &crate::World) -> Result<(), String> {
        if !w.has::<crate::Skin>(e)
            && let Some(i) = self.primitives.iter().position(crate::skin::has_skin_streams)
        {
            return Err(format!(
                "primitive {i}: joint or weight streams without a core.skin"
            ));
        }
        for (i, p) in self.primitives.iter().enumerate() {
            let n = match p.streams.get(&Semantic::Position) {
                Some(Stream::F32x3(v)) => v.len(),
                Some(_) => return Err(format!("primitive {i}: positions not 3 floats")),
                None => return Err(format!("primitive {i}: no positions")),
            };
            if let Some((sem, st)) = p.streams.iter().find(|(_, st)| st.len() != n) {
                return Err(format!(
                    "primitive {i}: {sem:?} has {} values for {n} vertices",
                    st.len()
                ));
            }
            let corners = match &p.indices {
                Some(idx) => {
                    if let Some(bad) = idx.iter().find(|&&v| usize::try_from(v).map_or(true, |v| v >= n)) {
                        return Err(format!("primitive {i}: index {bad} of {n} vertices"));
                    }
                    idx.len()
                }
                None => n,
            };
            let per = match p.topology {
                Topology::Triangles => 3,
                Topology::Lines => 2,
                Topology::Points => 1,
            };
            if corners % per != 0 {
                return Err(format!(
                    "primitive {i}: {corners} corners are not whole {:?}",
                    p.topology
                ));
            }
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
    /// Numbers or indices a shader reads exactly (a palette indexed per vertex, a lookup table): any filtering,
    /// resizing or lossy coding changes what it means. The reader sets it from how the material binds the texture
    /// (a point sampler without mips, or the slot's documented use); passes and writers that change texels keep it
    /// whole and lossless.
    Data,
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

/// Which channel holds a normal map's X.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize)]
pub enum NormalX {
    /// Red is X.
    #[default]
    Red,
    /// Alpha is X, green Y (DXT5nm, "BC3n"); red and blue are unused.
    Alpha,
}

/// Ordering of the two tangent-plane coordinates, relative to a supplied
/// tangent T and its bitangent B = cross(N,T)*w. This describes the source
/// frame, not how a consumer regenerates a frame from texture coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize)]
pub enum NormalBasis {
    /// Red/X follows T; green follows B.
    #[default]
    TangentBitangent,
    /// Red/X follows B; green follows T.
    BitangentTangent,
}

/// How a normal map's channels encode its tangent-space normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct NormalConvention {
    pub green: NormalGreen,
    pub z: NormalZ,
    pub x: NormalX,
    pub basis: NormalBasis,
}

impl NormalConvention {
    /// glTF's: X in red, green up, Z stored.
    pub const GLTF: NormalConvention = NormalConvention {
        green: NormalGreen::Up,
        z: NormalZ::Stored,
        x: NormalX::Red,
        basis: NormalBasis::TangentBitangent,
    };
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
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// 3: the normal convention gained tangent/bitangent basis ordering.
    const VERSION: u32 = 3;
    /// Every level's bytes.
    fn payload_bytes(&self) -> u64 {
        self.levels.iter().map(|l| l.len() as u64).sum()
    }
    /// A size, at least one level and no more than the size has, each exactly its format's size for its dimensions.
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err(format!("{} x {} pixels", self.width, self.height));
        }
        let most = self.width.max(self.height).ilog2() + 1;
        if self.levels.is_empty() || self.levels.len() > most as usize {
            return Err(format!(
                "{} levels for {} x {} (1 to {most})",
                self.levels.len(),
                self.width,
                self.height
            ));
        }
        for (i, level) in self.levels.iter().enumerate() {
            let shift = u32::try_from(i).unwrap_or(u32::MAX);
            let (w, h) = (
                self.width.checked_shr(shift).unwrap_or(0).max(1),
                self.height.checked_shr(shift).unwrap_or(0).max(1),
            );
            match self.format.checked_level_size(w, h) {
                Some(size) if size == level.len() => {}
                Some(size) => {
                    return Err(format!(
                        "level {i} ({w} x {h} {:?}): {} bytes, not {size}",
                        self.format,
                        level.len()
                    ));
                }
                None => return Err(format!("level {i}: {w} x {h} doesn't fit in memory")),
            }
        }
        Ok(())
    }
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

/// What a [`Light`] is: glTF's `KHR_lights_punctual` types. A spot's cone angles are from its axis, in radians, the
/// inner within the outer (full intensity inside the inner, falling to none at the outer).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum LightKind {
    Point,
    Spot { inner_cone: f32, outer_cone: f32 },
    Directional,
}

/// A punctual light, exactly `KHR_lights_punctual`'s: placed by its entity's [`Transform`] and shining along that
/// transform's local -Z (a point light shines everywhere); `color` linear RGB; `intensity` in candela for point and spot
/// lights, in lux for a directional one; `range` the distance where it stops (`None`: unlimited, an inverse-square
/// falloff to infinity); `radius` the size of the emitter, the radius of the sphere (or disc, for a spot) the light
/// leaves from, in metres (`None`: a point, glTF's punctual light, which has no size); `specular` the colour and
/// intensity of the light's highlights where the source states its own (`None`: the light's own colour and intensity,
/// as glTF's punctual light, which has one). A game's own light model stays in its dialect; this is what a writer emits.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Light {
    pub kind: LightKind,
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: Option<f32>,
    /// Casts shadows; `None`: unknown (the renderer's default).
    pub cast_shadows: Option<bool>,
    /// The emitter's radius, metres; `None`: a point.
    pub radius: Option<f32>,
    /// The highlights' own colour and intensity; `None`: the light's [`Light::color`] and [`Light::intensity`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub specular: Option<LightSpecular>,
}

/// The colour and intensity a [`Light`]'s specular highlights take where the source states them apart from its diffuse
/// light: linear RGB, and an intensity in the light's own unit (candela for a point or spot light, lux for a directional
/// one). A light whose source states no diffuse light at all has intensity 0 and its highlights here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct LightSpecular {
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Component for Light {
    const NAME: &'static str = "core.light";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// 2: `cast_shadows`; 3: `radius`; 4: `specular`.
    const VERSION: u32 = 4;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    /// Finite, non-negative colour and intensity (the specular's too), a positive range, a finite non-negative radius,
    /// and a spot's cones within 0 <= inner <= outer <= pi / 2.
    fn verify(&self, _e: Entity, _w: &crate::World) -> Result<(), String> {
        finite(self.color.iter().copied().chain([self.intensity]), "light")?;
        if self.color.iter().any(|&c| c < 0.0) || self.intensity < 0.0 {
            return Err("a negative colour or intensity".into());
        }
        if let Some(s) = &self.specular {
            finite(s.color.iter().copied().chain([s.intensity]), "light specular")?;
            if s.color.iter().any(|&c| c < 0.0) || s.intensity < 0.0 {
                return Err("a negative specular colour or intensity".into());
            }
        }
        if self.range.is_some_and(|r| !(r.is_finite() && r > 0.0)) {
            return Err("a range that is not positive".into());
        }
        if self.radius.is_some_and(|r| !(r.is_finite() && r >= 0.0)) {
            return Err("a radius that is not finite and non-negative".into());
        }
        if let LightKind::Spot {
            inner_cone,
            outer_cone,
        } = self.kind
            && !(0.0 <= inner_cone && inner_cone <= outer_cone && outer_cone <= std::f32::consts::FRAC_PI_2)
        {
            return Err(format!(
                "spot cones {inner_cone} and {outer_cone} not 0 <= inner <= outer <= pi / 2"
            ));
        }
        Ok(())
    }
}

/// Collision geometry: a triangle soup in the entity's own space, each triangle made of a surface.
///
/// Placed like a [`Mesh`]: on a prototype, by its [`Instance`]s; on an entity with a [`Transform`] that is not a
/// prototype, by that transform. A prototype may collide without drawing (no mesh) and draw without colliding. A
/// triangle's front is counter-clockwise (the world's convention, as a mesh's); soups need not be closed or connected.
/// `surfaces` holds one index per triangle into the world's [`Surfaces`]. What a collider was (a box, a capsule, a
/// heightfield) stays in the reader's dialect; this is the shape a consumer collides with.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Collision {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub surfaces: Vec<u16>,
}

impl Collision {
    /// Append another soup, its positions mapped by `f`, every triangle made of `surface`.
    pub fn extend(
        &mut self,
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
        surface: u16,
        f: impl Fn([f32; 3]) -> [f32; 3],
    ) {
        let base = self.positions.len() as u32;
        self.positions.extend(positions.iter().map(|&p| f(p)));
        self.triangles
            .extend(triangles.iter().map(|t| t.map(|i| i + base)));
        self.surfaces
            .extend(std::iter::repeat_n(surface, triangles.len()));
    }
}

impl Component for Collision {
    const NAME: &'static str = "core.collision";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// Positions mapped; triangles reversed under a mirror.
    fn apply_linear(&self, l: &crate::Linear) -> crate::world::Linearity<Self> {
        let mut c = self.clone();
        c.positions.iter_mut().for_each(|p| *p = l.point(*p));
        if l.mirrors() {
            c.triangles.iter_mut().for_each(|t| t.swap(1, 2));
        }
        crate::world::Linearity::Applied(c)
    }
    /// 12 bytes per position, 12 per triangle and 2 per surface index.
    fn payload_bytes(&self) -> u64 {
        (self.positions.len() as u64 + self.triangles.len() as u64) * 12 + self.surfaces.len() as u64 * 2
    }
    /// One surface per triangle, indices in range, positions finite, surfaces known to the world's [`Surfaces`].
    fn verify(&self, _e: Entity, w: &crate::World) -> Result<(), String> {
        if self.surfaces.len() != self.triangles.len() {
            return Err(format!(
                "{} surfaces for {} triangles",
                self.surfaces.len(),
                self.triangles.len()
            ));
        }
        let n = self.positions.len() as u32;
        if let Some(t) = self.triangles.iter().find(|t| t.iter().any(|&i| i >= n)) {
            return Err(format!("triangle {t:?} past the {n} positions"));
        }
        finite(self.positions.iter().flatten().copied(), "collision positions")?;
        let known = w.resource::<Surfaces>().map_or(0, |s| s.0.len());
        if let Some(&s) = self.surfaces.iter().find(|&&s| usize::from(s) >= known) {
            return Err(format!("surface {s} past the world's {known} surfaces"));
        }
        Ok(())
    }
    /// A summary: counts, and triangles per surface index.
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        let mut per: BTreeMap<u16, u64> = BTreeMap::new();
        for &s in &self.surfaces {
            *per.entry(s).or_default() += 1;
        }
        Ok(Some(json!({
            "positions": self.positions.len(),
            "triangles": self.triangles.len(),
            "surfaces": per,
        })))
    }
}

/// A collision surface: what a triangle is made of. `name` is the source's own (a game's physic material, "Snow",
/// "CONCRETE_POTHOLE"); `class` what it is generically, set by the source's lowering pass from a named table (a
/// reader leaves it `None`). Writers map classes onto their target's surfaces; a game's properties for it (friction,
/// sound) stay in its dialect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Surface {
    pub name: String,
    /// `None`: not classified (a writer that needs a class refuses it or counts it by a named policy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<SurfaceClass>,
}

impl Surface {
    /// A surface by its source name, not classified.
    pub fn new(name: impl Into<String>) -> Surface {
        Surface {
            name: name.into(),
            class: None,
        }
    }
}

/// What a surface is made of, generically (its feel, sound and grip), for every source and target: each source's
/// lowering pass maps its own names here and each writer maps these to its target, never one game's names to
/// another's. `Other`: classified, and no class fits.
///
/// Closed on purpose: writers match it without a wildcard, so adding a class (with the evidence that a source has
/// such surfaces and a target treats them differently, its line in docs/SPEC.md and a `core.surfaces` VERSION bump)
/// fails to compile until every writer maps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceClass {
    Concrete,
    Asphalt,
    Stone,
    Brick,
    Tile,
    Metal,
    Wood,
    Glass,
    Plastic,
    Rubber,
    Fabric,
    Grass,
    Dirt,
    Gravel,
    Sand,
    Mud,
    Snow,
    Ice,
    /// Swimmable or wading surfaces.
    Water,
    Other,
}

impl SurfaceClass {
    pub const ALL: [SurfaceClass; 20] = [
        SurfaceClass::Concrete,
        SurfaceClass::Asphalt,
        SurfaceClass::Stone,
        SurfaceClass::Brick,
        SurfaceClass::Tile,
        SurfaceClass::Metal,
        SurfaceClass::Wood,
        SurfaceClass::Glass,
        SurfaceClass::Plastic,
        SurfaceClass::Rubber,
        SurfaceClass::Fabric,
        SurfaceClass::Grass,
        SurfaceClass::Dirt,
        SurfaceClass::Gravel,
        SurfaceClass::Sand,
        SurfaceClass::Mud,
        SurfaceClass::Snow,
        SurfaceClass::Ice,
        SurfaceClass::Water,
        SurfaceClass::Other,
    ];

    /// The lower-case word JSON shows.
    pub fn as_str(self) -> &'static str {
        match self {
            SurfaceClass::Concrete => "concrete",
            SurfaceClass::Asphalt => "asphalt",
            SurfaceClass::Stone => "stone",
            SurfaceClass::Brick => "brick",
            SurfaceClass::Tile => "tile",
            SurfaceClass::Metal => "metal",
            SurfaceClass::Wood => "wood",
            SurfaceClass::Glass => "glass",
            SurfaceClass::Plastic => "plastic",
            SurfaceClass::Rubber => "rubber",
            SurfaceClass::Fabric => "fabric",
            SurfaceClass::Grass => "grass",
            SurfaceClass::Dirt => "dirt",
            SurfaceClass::Gravel => "gravel",
            SurfaceClass::Sand => "sand",
            SurfaceClass::Mud => "mud",
            SurfaceClass::Snow => "snow",
            SurfaceClass::Ice => "ice",
            SurfaceClass::Water => "water",
            SurfaceClass::Other => "other",
        }
    }

    pub fn parse(word: &str) -> Option<SurfaceClass> {
        SurfaceClass::ALL.into_iter().find(|c| c.as_str() == word)
    }
}

/// The world's collision surfaces, indexed by [`Collision::surfaces`].
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct Surfaces(pub Vec<Surface>);

/// A surface index cannot be represented by collision's u16 surface IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("surface index {index} exceeds u16")]
pub struct SurfaceIndexError {
    pub index: usize,
}

impl Surfaces {
    /// The index of the surface named `name`, added when new.
    /// An unrepresentable index is an error and leaves the table unchanged.
    pub fn index(&mut self, name: &str) -> Result<u16, SurfaceIndexError> {
        let index = self.0.iter().position(|s| s.name == name).unwrap_or(self.0.len());
        let id = u16::try_from(index).map_err(|_| SurfaceIndexError { index })?;
        if index == self.0.len() {
            self.0.push(Surface::new(name));
        }
        Ok(id)
    }
}

impl crate::world::Resource for Surfaces {
    const NAME: &'static str = "core.surfaces";
    /// 2: each surface's optional `class`.
    const VERSION: u32 = 2;
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
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
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
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
    /// One entry per primitive of that mesh, in order: the material the reader already knows for it (a consumer's
    /// `parts[i].material`). The load pass checks the primitive count against it and fills [`Primitive::material`].
    pub materials: Vec<Option<Entity>>,
}
impl Component for MeshFile {
    const NAME: &'static str = "core.mesh_file";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// The geometry is no longer the file's.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Drop
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.materials.iter().flatten());
    }
}

/// Per-placement material choices of a prototype: one map per primitive, variant -> material, overriding the
/// primitive's material for placements whose [`Instance::variant`] matches (one source's tint rows).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct MaterialVariants(pub Vec<BTreeMap<u32, Entity>>);
impl Component for MaterialVariants {
    const NAME: &'static str = "core.material_variants";
    const GEOMETRY: crate::Geometry = crate::Geometry::PerPrimitive;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(self.0.iter().flat_map(|m| m.values().copied()));
    }
    fn remap_primitives(&self, from: &[usize]) -> Option<Result<Self, String>> {
        Some(crate::world::remap_list(&self.0, from).map(MaterialVariants))
    }
}

/// A streaming or processing tile, `(col, row)` in the ground frame of whoever tiled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Tile {
    pub col: i32,
    pub row: i32,
}

/// A point or a tile size that has no tile.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
#[error(
    "no tile at ground point {ground:?} on a grid of {size} metres: the point and the size must be finite, the size \
     positive and the tile within i32"
)]
pub struct TileError {
    pub ground: [f32; 2],
    pub size: f32,
}

impl Tile {
    /// The tile of a `size`-metre grid from the origin holding the ground point `g` (`[east, north]`):
    /// `(floor(east / size), floor(north / size))`, in f32. The one rule for every grid that has to agree on where
    /// content stands: a writer's tiles, a partition aligned to them. A point or size that is not finite, a size that
    /// is not positive, or a tile outside i32 is an error, never a saturated or zero tile.
    pub fn of(g: [f32; 2], size: f32) -> Result<Tile, TileError> {
        let bad = TileError { ground: g, size };
        if !(size.is_finite() && size > 0.0) {
            return Err(bad);
        }
        let axis = |v: f32| {
            let t = (v / size).floor();
            (t.is_finite() && t >= i32::MIN as f32 && t < i32::MAX as f32).then_some(t as i32)
        };
        match (axis(g[0]), axis(g[1])) {
            (Some(col), Some(row)) => Ok(Tile { col, row }),
            _ => Err(bad),
        }
    }
}

impl Component for Tile {
    const NAME: &'static str = "core.tile";
    const GEOMETRY: crate::Geometry = crate::Geometry::None;
    /// No local-space geometry.
    fn apply_linear(&self, _l: &crate::Linear) -> crate::world::Linearity<Self> {
        crate::world::Linearity::Invariant
    }
    fn to_json(&self) -> Result<Option<Value>, JsonError> {
        crate::json::to_value(self).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile is the floor of the point over the size; a point or size that has none is an error, never a saturated tile.
    #[test]
    fn a_tile_of_a_point_that_has_none_is_an_error() {
        assert_eq!(Tile::of([-0.5, 399.9], 200.0), Ok(Tile { col: -1, row: 1 }));
        for (g, size) in [
            ([f32::NAN, 0.0], 200.0),
            ([0.0, f32::INFINITY], 200.0),
            ([1.0, 1.0], 0.0),
            ([1.0, 1.0], -5.0),
            ([1.0, 1.0], f32::NAN),
            ([1e30, 0.0], 200.0),
        ] {
            let e = Tile::of(g, size).unwrap_err();
            assert_eq!(
                (e.ground.map(f32::is_nan), e.size.is_nan()),
                (g.map(f32::is_nan), size.is_nan())
            );
        }
    }

    #[test]
    fn payload_bytes_count_streams_indices_levels_and_soups() {
        // 3 positions (12 bytes each), 3 normals (12), 3 uvs (8), 3 indices (4): 36 + 36 + 24 + 12 = 108
        let streams = BTreeMap::from([
            (Semantic::Position, Stream::F32x3(vec![[0.0; 3]; 3])),
            (Semantic::Normal, Stream::F32x3(vec![[0.0, 0.0, 1.0]; 3])),
            (Semantic::TexCoord(0), Stream::F32x2(vec![[0.0; 2]; 3])),
        ]);
        let mesh = Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: Some(vec![0, 1, 2]),
            material: None,
        }]);
        assert_eq!(mesh.payload_bytes(), 108);
        // a 4 x 4 RGBA8 texture with its 2 x 2 and 1 x 1 levels: 64 + 16 + 4
        let texture = Texture {
            width: 4,
            height: 4,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![0; 64], vec![0; 16], vec![0; 4]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        };
        assert_eq!(texture.payload_bytes(), 84);
        // 3 positions and 1 triangle (12 bytes each), 1 surface index (2)
        let soup = Collision {
            positions: vec![[0.0; 3]; 3],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0],
        };
        assert_eq!(soup.payload_bytes(), 50);
        let mut w = crate::World::new();
        let (a, b) = (w.spawn(), w.spawn());
        w.insert(a, mesh.clone());
        w.insert(b, mesh);
        w.insert(a, texture);
        w.insert(a, Name("small".into()));
        w.insert_opaque(
            b,
            "game.blob",
            crate::Opaque {
                version: 1,
                data: crate::OpaqueData::Bytes(vec![0; 7]),
            },
        )
        .unwrap();
        assert_eq!(
            w.payload_bytes(),
            BTreeMap::from([("core.mesh", 216), ("core.texture", 84), ("game.blob", 7)])
        );
    }

    fn t(cols: [[f32; 3]; 4]) -> Transform {
        Transform { cols }
    }

    #[test]
    fn surface_indices_check_the_u16_boundary_without_mutating_on_error() {
        let mut surfaces = Surfaces((0..u16::MAX).map(|i| Surface::new(i.to_string())).collect());
        assert_eq!(surfaces.index("last"), Ok(u16::MAX));
        assert_eq!(surfaces.index("0"), Ok(0));
        assert_eq!(surfaces.index("last"), Ok(u16::MAX));
        let before = surfaces.clone();
        assert_eq!(
            surfaces.index("overflow"),
            Err(SurfaceIndexError { index: 65536 })
        );
        assert_eq!(surfaces, before);
        // The public vector can also contain an existing out-of-range entry.
        surfaces.0.push(Surface::new("outside"));
        assert_eq!(surfaces.index("outside"), Err(SurfaceIndexError { index: 65536 }));
        assert_eq!(surfaces.index("0"), Ok(0));
    }

    #[test]
    fn surface_classes_are_lower_case_words_shown_only_when_set() {
        for c in SurfaceClass::ALL {
            assert_eq!(SurfaceClass::parse(c.as_str()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), json!(c.as_str()));
        }
        assert_eq!(SurfaceClass::parse("Concrete"), None);
        let mut surfaces = Surfaces::default();
        surfaces.index("Snow").unwrap();
        surfaces.0.push(Surface {
            name: "CONCRETE_POTHOLE".into(),
            class: Some(SurfaceClass::Concrete),
        });
        assert_eq!(
            crate::world::Resource::to_json(&surfaces).unwrap(),
            Some(json!([{"name": "Snow"}, {"name": "CONCRETE_POTHOLE", "class": "concrete"}]))
        );
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
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(vec![[0.; 3]; 3]))]),
                indices: None,
                material: Some(mat),
            }]),
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

    /// A mesh, texture or placement that is whole is accepted; one whose parts don't fit together is refused,
    /// naming what; an unloaded (file-backed) or collision-only prototype is fine.
    #[test]
    fn the_verifier_refuses_structurally_broken_data() {
        let tri = |streams: BTreeMap<Semantic, Stream>, indices: Option<Vec<u32>>, topology| Primitive {
            topology,
            streams,
            indices,
            material: None,
        };
        let pos = |n: usize| (Semantic::Position, Stream::F32x3(vec![[0.0; 3]; n]));
        let mesh_err = |p: Primitive| {
            let mut w = crate::World::new();
            let e = w.spawn();
            w.insert(e, Mesh::new(vec![p]));
            w.verify_components()
                .into_iter()
                .map(|(_, _, m)| m)
                .collect::<Vec<_>>()
        };
        assert!(
            mesh_err(tri(
                BTreeMap::from([pos(3)]),
                Some(vec![0, 1, 2]),
                Topology::Triangles
            ))
            .is_empty()
        );
        assert!(mesh_err(tri(BTreeMap::from([pos(4)]), Some(vec![0, 1]), Topology::Lines)).is_empty());
        for (p, words) in [
            (tri(BTreeMap::new(), None, Topology::Triangles), "no positions"),
            (
                tri(
                    BTreeMap::from([(Semantic::Position, Stream::F32x2(vec![[0.0; 2]; 3]))]),
                    None,
                    Topology::Triangles,
                ),
                "not 3 floats",
            ),
            (
                tri(
                    BTreeMap::from([pos(3), (Semantic::Normal, Stream::F32x3(vec![[0.0; 3]; 2]))]),
                    None,
                    Topology::Triangles,
                ),
                "2 values for 3 vertices",
            ),
            (
                tri(BTreeMap::from([pos(3)]), Some(vec![0, 1, 3]), Topology::Triangles),
                "index 3 of 3",
            ),
            (
                tri(
                    BTreeMap::from([pos(3)]),
                    Some(vec![0, 1, 2, 0]),
                    Topology::Triangles,
                ),
                "4 corners",
            ),
            (tri(BTreeMap::from([pos(3)]), None, Topology::Lines), "3 corners"),
        ] {
            let got = mesh_err(p);
            assert!(got.len() == 1 && got[0].contains(words), "{words}: {got:?}");
        }
        let tex = |width, height, levels: Vec<usize>| {
            let mut w = crate::World::new();
            let e = w.spawn();
            w.insert(
                e,
                Texture {
                    width,
                    height,
                    format: TexFormat::Rgba8Unorm,
                    levels: levels.into_iter().map(|n| vec![0; n]).collect(),
                    color_space: ColorSpace::Srgb,
                    role: TexRole::Color,
                    normal: None,
                },
            );
            w.verify_components()
                .into_iter()
                .map(|(_, _, m)| m)
                .collect::<Vec<_>>()
        };
        assert!(tex(4, 2, vec![32, 8, 4]).is_empty());
        assert!(tex(0, 2, vec![0])[0].contains("0 x 2"));
        assert!(tex(4, 4, vec![])[0].contains("0 levels"));
        assert!(tex(4, 4, vec![64, 16, 4, 4])[0].contains("4 levels"));
        assert!(tex(4, 4, vec![64, 15])[0].contains("level 1"));
        assert!(tex(u32::MAX, u32::MAX, vec![0])[0].contains("doesn't fit"));
        // a placement needs its transform; an unloaded and a collision-only prototype are whole as they are
        let mut w = crate::World::new();
        let (file, collision) = (w.spawn(), w.spawn());
        w.insert(
            file,
            MeshFile {
                path: "m.glb".into(),
                mesh: 0,
                materials: vec![None],
            },
        );
        w.insert(collision, Name("collision only".into()));
        let placed = w.spawn();
        w.insert(
            placed,
            Instance {
                prototype: file,
                variant: 0,
            },
        );
        let bare = w.spawn();
        w.insert(
            bare,
            Instance {
                prototype: collision,
                variant: 0,
            },
        );
        w.insert(placed, Transform::IDENTITY);
        let tf = w.spawn();
        w.insert(tf, TextureFile { path: "t.png".into() });
        assert_eq!(
            w.verify_components(),
            [(
                "core.instance",
                bare,
                "a placement without a core.transform".to_owned()
            )]
        );
    }

    #[test]
    fn the_verifier_refuses_non_finite_geometry() {
        let mut w = crate::World::new();
        let e = w.spawn();
        w.insert(e, Transform::IDENTITY);
        let mut m = Transform::IDENTITY;
        m.cols[3][2] = f32::NAN;
        let e2 = w.spawn();
        w.insert(e2, m);
        w.insert(
            e2,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([
                    (Semantic::Position, Stream::F32x3(vec![[0.0; 3]; 3])),
                    (
                        Semantic::TexCoord(0),
                        Stream::F32x2(vec![[0.0, f32::NEG_INFINITY], [0.0; 2], [0.0; 2]]),
                    ),
                ]),
                indices: None,
                material: None,
            }]),
        );
        let got: Vec<(&str, Entity, String)> = w.verify_components();
        assert_eq!(
            got,
            [
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
    /// A 4x4 matrix is affine when its last row is 0 0 0 1 within the format's tolerance (0: exactly); a NaN there
    /// is refused whatever the tolerance.
    #[test]
    fn a_four_by_four_is_affine_by_its_last_row() {
        let m = [
            [1., 0., 0., 0.],
            [0., 2., 0., 0.],
            [0., 0., 3., 0.],
            [4., 5., 6., 1.],
        ];
        let t = Transform::from_columns4(m, 0.0).unwrap();
        assert_eq!(t.cols, [[1., 0., 0.], [0., 2., 0.], [0., 0., 3.], [4., 5., 6.]]);
        let mut noisy = m;
        noisy[0][3] = 5e-5;
        assert!(Transform::from_columns4(noisy, 0.0).is_err());
        assert!(Transform::from_columns4(noisy, 1e-4).is_ok());
        let mut projective = m;
        projective[3][3] = 2.0;
        let e = Transform::from_columns4(projective, 1e-4).unwrap_err();
        assert!(e.contains("not affine"), "{e}");
        let mut nan = m;
        nan[2][3] = f32::NAN;
        assert!(Transform::from_columns4(nan, f32::INFINITY).is_err());
    }

    /// Composition is `outer (inner v)`, each entry rounded once: 1 + 2^-24 + 2^-24 is 1 + 2^-23 (an f32), where
    /// summing in f32 would round to 1 at each step.
    #[test]
    fn composition_rounds_each_entry_once() {
        let outer = Transform {
            cols: [[1., 0., 0.], [1., 0., -1.], [1., 1., 0.], [10., 20., 30.]],
        };
        let tiny = 2f32.powi(-24);
        let inner = Transform {
            cols: [[1., tiny, tiny], [0., 1., 0.], [0., 0., 2.], [1., 2., 3.]],
        };
        let c = outer.compose(&inner);
        assert_eq!(c.cols[0], [1. + 2. * tiny, tiny, -tiny]);
        assert_eq!(c.cols[1], [1., 0., -1.]);
        assert_eq!(c.cols[2], [2., 2., 0.]);
        assert_eq!(c.cols[3], [16., 23., 28.]);
        // a point through the composition is the point through inner, then outer
        let p = [0., 1., 1.];
        assert_eq!(c.apply(p), [19., 25., 27.]);
        assert_eq!(c.apply(p), outer.apply(inner.apply(p)));
        assert_eq!(Transform::IDENTITY.compose(&inner), inner);
        assert_eq!(inner.compose(&Transform::IDENTITY), inner);
    }

    /// One singular rule: a map is singular when a transform with its part classifies as degenerate (relative to its
    /// axes' lengths, so a small uniform scale is not). It then turns no normal or tangent and has no inverse, and a
    /// mesh with normals cannot follow it; an invertible map's normals and tangents come out of unit length, the
    /// tangent's w flipped under a mirror.
    #[test]
    fn a_singular_map_turns_no_normal_or_tangent() {
        let near_flat = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 1e-7]];
        for cols in [
            [[0.0; 3]; 3],
            [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            near_flat,
        ] {
            let l = Linear::new(cols);
            let t = Transform {
                cols: [cols[0], cols[1], cols[2], [0.0; 3]],
            };
            assert!(l.singular(), "{cols:?}");
            assert_eq!(t.classify(), TransformKind::Degenerate, "{cols:?}");
            assert_eq!(l.normal([0.0, 0.0, 1.0]), None);
            assert_eq!(l.tangent([1.0, 0.0, 0.0, 1.0]), None);
            assert_eq!(l.inverse_columns(), None);
        }
        let flat = Linear::new([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]]);
        let mut streams = BTreeMap::new();
        streams.insert(Semantic::Position, Stream::F32x3(vec![[0.0; 3]; 3]));
        streams.insert(Semantic::Normal, Stream::F32x3(vec![[0.0, 0.0, 1.0]; 3]));
        let mesh = Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams,
            indices: None,
            material: None,
        }]);
        assert!(matches!(
            mesh.apply_linear(&flat),
            crate::world::Linearity::Unsupported
        ));
        // the rule is relative: a small uniform scale (one source's bone of scale 0.00937, det about 8.2e-7) is not
        // singular, and turns normals and tangents to unit length; one axis squashed to 1e-7 of the others is
        let s = 0.00937f32;
        let small = Linear::new([[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]]);
        assert!(!small.singular());
        assert_eq!(small.normal([0.0, 1.0, 0.0]), Some([0.0, 1.0, 0.0]));
        assert_eq!(small.tangent([1.0, 0.0, 0.0, 1.0]), Some([1.0, 0.0, 0.0, 1.0]));
        assert!(small.inverse_columns().is_some());
        let squashed = Linear::new([[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s * 1e-7]]);
        assert!(squashed.singular());
        // scaled by 2 and mirrored in x: unit normals and tangents, w flipped
        let l = Linear::of_f64([[-2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 2.0]]);
        assert!(!l.singular() && l.mirrors());
        assert_eq!(l.normal([1.0, 0.0, 0.0]), Some([-1.0, 0.0, 0.0]));
        assert_eq!(l.tangent([0.0, 3.0, 0.0, 1.0]), Some([0.0, 1.0, 0.0, -1.0]));
    }

    #[test]
    fn normal_basis_has_explicit_json_and_default() {
        assert_eq!(NormalBasis::default(), NormalConvention::GLTF.basis);
        let convention = NormalConvention {
            basis: NormalBasis::BitangentTangent,
            ..NormalConvention::GLTF
        };
        let value = crate::json::to_value(&convention).expect("normal convention");
        assert_eq!(value["basis"], "BitangentTangent");
    }

    #[test]
    fn collision_soups_and_their_surfaces() {
        let mut w = crate::World::new();
        let mut surfaces = Surfaces::default();
        let (snow, ice) = (surfaces.index("Snow").unwrap(), surfaces.index("Ice").unwrap());
        assert_eq!((snow, ice, surfaces.index("Snow").unwrap()), (0, 1, 0));
        w.insert_resource(surfaces);
        let mut c = Collision::default();
        let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]];
        c.extend(&tri, &[[0, 1, 2]], snow, |p| p);
        c.extend(&tri, &[[0, 1, 2]], ice, |p| [p[0] + 2.0, p[1], p[2]]);
        assert_eq!(c.triangles, [[0, 1, 2], [3, 4, 5]]);
        assert_eq!(c.surfaces, [0, 1]);
        assert_eq!(c.positions[3], [2.0, 0.0, 0.0]);
        let e = w.spawn();
        assert_eq!(c.verify(e, &w), Ok(()));
        let mut bad = c.clone();
        bad.surfaces.pop();
        assert!(bad.verify(e, &w).is_err(), "a surface per triangle");
        let mut bad = c.clone();
        bad.triangles[1][2] = 6;
        assert!(bad.verify(e, &w).is_err(), "indices in range");
        let mut bad = c.clone();
        bad.surfaces[0] = 2;
        assert!(bad.verify(e, &w).is_err(), "surfaces the world knows");
        let mut bad = c.clone();
        bad.positions[0][1] = f32::NAN;
        assert!(bad.verify(e, &w).is_err(), "finite positions");
        assert_eq!(
            c.to_json().unwrap().unwrap(),
            json!({"positions": 6, "triangles": 2, "surfaces": {"0": 1, "1": 1}})
        );
    }

    #[test]
    fn shadows_belong_to_drawn_entities_and_keep_stated_values() {
        let mut w = crate::World::new();
        let bare = w.spawn();
        let proto = w.spawn();
        w.insert(proto, Mesh::default());
        let place = w.spawn();
        w.insert(
            place,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let s = Shadows {
            cast: Some(ShadowCast::Off),
            receive: Some(false),
        };
        assert_eq!(s.verify(place, &w), Ok(()));
        assert_eq!(s.verify(proto, &w), Ok(()), "an entity drawing its own mesh");
        assert!(s.verify(bare, &w).is_err());
        assert_eq!(
            s.to_json().unwrap(),
            Some(serde_json::json!({"cast": "Off", "receive": false})),
            "stated off and false are kept"
        );
        assert_eq!(
            Shadows::default().to_json().unwrap(),
            Some(serde_json::json!({"cast": null, "receive": null}))
        );
    }
    #[test]
    fn lights_keep_to_khr_lights_punctual() {
        let mut w = crate::World::new();
        let e = w.spawn();
        let spot = |inner: f32, outer: f32| Light {
            kind: LightKind::Spot {
                inner_cone: inner,
                outer_cone: outer,
            },
            color: [1.0, 0.5, 0.25],
            intensity: 10.0,
            range: Some(5.0),
            cast_shadows: None,
            radius: None,
            specular: None,
        };
        assert_eq!(spot(0.2, 0.6).verify(e, &w), Ok(()));
        let sized = |r: f32| Light {
            radius: Some(r),
            ..spot(0.2, 0.6)
        };
        assert_eq!(sized(0.0).verify(e, &w), Ok(()), "a radius of 0 is a point");
        assert_eq!(sized(0.25).verify(e, &w), Ok(()));
        assert!(sized(-0.1).verify(e, &w).is_err(), "a negative radius");
        assert!(
            sized(f32::NAN).verify(e, &w).is_err(),
            "a radius that is not finite"
        );
        assert!(spot(0.7, 0.6).verify(e, &w).is_err(), "inner past outer");
        assert!(spot(0.2, 2.0).verify(e, &w).is_err(), "outer past pi / 2");
        let mut l = spot(0.2, 0.6);
        l.range = Some(0.0);
        assert!(l.verify(e, &w).is_err(), "a range that is not positive");
        l.range = None;
        l.intensity = -1.0;
        assert!(l.verify(e, &w).is_err(), "a negative intensity");
        l.intensity = f32::NAN;
        assert!(l.verify(e, &w).is_err(), "non-finite");
        assert_eq!(
            spot(0.25, 0.5).to_json().unwrap().unwrap()["kind"],
            serde_json::json!({"Spot": {"inner_cone": 0.25, "outer_cone": 0.5}})
        );
    }

    #[test]
    fn closures_follow_declared_references_and_subsets_keep_ids() {
        use crate::pbr::{PbrMaterial, TextureRef};
        use crate::{Opaque, OpaqueData};
        let mut w = crate::World::new();
        let texture = w.spawn();
        w.insert(texture, Name("albedo".into()));
        let other_texture = w.spawn();
        let material = w.spawn();
        w.insert(
            material,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(texture)),
                ..PbrMaterial::default()
            },
        );
        let variant = w.spawn();
        w.insert(
            variant,
            PbrMaterial {
                emissive_texture: Some(TextureRef::new(other_texture)),
                ..PbrMaterial::default()
            },
        );
        let prototype = w.spawn();
        w.insert(
            prototype,
            MeshFile {
                path: "a.glb".into(),
                mesh: 0,
                materials: vec![Some(material)],
            },
        );
        w.insert(prototype, MaterialVariants(vec![BTreeMap::from([(1, variant)])]));
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype,
                variant: 1,
            },
        );
        w.insert(placement, Transform::IDENTITY);
        // an opaque component naming an entity is carried, not followed
        let unrelated = w.spawn();
        w.insert(unrelated, Name("elsewhere".into()));
        w.insert_opaque(
            placement,
            "game.link",
            Opaque {
                version: 1,
                data: OpaqueData::Json(json!(unrelated.index())),
            },
        )
        .unwrap();
        w.insert_resource(crate::Surfaces::default());
        let closure = w.closure([placement]);
        assert_eq!(
            closure,
            [texture, other_texture, material, variant, prototype, placement].into()
        );
        let mut sub = w.subset(&closure);
        assert!(!sub.is_alive(unrelated) && sub.get::<Name>(unrelated).is_none());
        assert_eq!(sub.get::<Name>(texture), w.get::<Name>(texture));
        assert_eq!(sub.get::<Instance>(placement), w.get::<Instance>(placement));
        assert!(
            sub.opaque_components()
                .any(|(n, c)| n == "game.link" && c.contains_key(&placement))
        );
        assert!(sub.resource::<crate::Surfaces>().is_some());
        assert_eq!(sub.verify_components(), Vec::new());
        // ids stay unique: a new entity lands above every id of the source
        assert!(sub.spawn().index() > unrelated.index());
        // retain is the same subset in place
        let mut kept = w.clone();
        kept.retain(&closure);
        assert_eq!(format!("{kept:?}"), format!("{:?}", w.subset(&closure)));
        assert!(!kept.is_alive(unrelated));
    }

    #[test]
    fn a_split_off_part_moves_out_and_back() {
        let mut w = crate::World::new();
        let texture = w.spawn();
        w.insert(texture, Name("albedo".into()));
        let prototype = w.spawn();
        w.insert(prototype, Name("rock".into()));
        let placement = w.spawn();
        w.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(placement, Transform::IDENTITY);
        let other = w.spawn();
        w.insert(other, Name("elsewhere".into()));
        let before = format!("{w:?}");
        let next = w.clone().spawn();
        let keep = w.closure([placement]);
        let mut part = w.split_off(&keep);
        assert!(!w.is_alive(placement) && w.get::<Name>(prototype).is_none());
        assert_eq!(w.get::<Name>(other), Some(&Name("elsewhere".into())));
        assert_eq!(
            part.get::<Instance>(placement).map(|i| i.prototype),
            Some(prototype)
        );
        assert!(!part.is_alive(other));
        // the part spawns where a copy would, and what it spawns and despawns leaves no trace
        let spawned = part.spawn();
        assert_eq!(spawned, next);
        part.insert(spawned, Name("chunk".into()));
        part.despawn(spawned);
        w.absorb(part).unwrap();
        assert_eq!(format!("{w:?}"), before);
        assert_eq!(w.get::<Name>(texture), Some(&Name("albedo".into())));
        assert_eq!(w.spawn(), next, "the next id is unchanged");
    }
}
