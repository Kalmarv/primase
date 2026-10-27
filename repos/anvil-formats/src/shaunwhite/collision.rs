//! PC collision objects in their authored local frame (Z up). MeshShape stores explicit float4 vertices, u16
//! indices and one material-table index per triangle; its Havok MOPP is acceleration data, retained undecoded.
//! No Havok packfile/tagfile is embedded in this layout. The stored MOPP code version is 5; the SDK version is
//! not established. Field order is AnvilToolkit 1.3.7's AC2 schema, with this game's vertices widened to float4.
//!
//! Surfaces are object ids, not destination surface enums. The referenced SWCollisionMaterial objects have their
//! authored names in the entry directory (ConcreteVert, ConcreteNonRidable, WoodDeck, ...). Their game-specific
//! physics parameters are not decoded here. Preserve the ids/names when building the consumer's surface table.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use primase_foundations::bytes::{Cursor, Endian, Field, FieldError};

const LE: Endian = Endian::Little;
pub const MESH_SHAPE: u32 = 0xB22B_3E61;
pub const CAPSULE_SHAPE: u32 = 0xB859_9052;
pub const LIST_SHAPE: u32 = 0x86EB_FD8D;
pub const BARREL_SHAPE: u32 = 0x97CD_8890;
pub const BOX_SHAPE: u32 = 0x4EC6_8E98;
pub const SPHERE_SHAPE: u32 = 0xFA3F_7A18;
pub const MULTI_MESH_SHAPE: u32 = 0x9A50_3477;
pub const MULTI_INERT_COMPONENT: u32 = 0x8BBE_EE57;
pub const SURFACE_MATERIAL: u32 = 0xEF0C_7E58;
pub const INERT_COMPONENT: u32 = 0x0E5A_450A;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("collision {field}: {why}")]
    Invalid { field: &'static str, why: String },
}

fn invalid(field: &'static str, why: impl Into<String>) -> Error {
    Error::Invalid {
        field,
        why: why.into(),
    }
}
fn word(c: &mut Cursor<'_>, field: &'static str) -> Result<u32, Error> {
    Ok(c.u32(LE).field(field)?)
}
fn number(c: &mut Cursor<'_>, field: &'static str) -> Result<f32, Error> {
    let value = c.f32(LE).field(field)?;
    if !value.is_finite() {
        return Err(invalid(field, "non-finite float"));
    }
    Ok(value)
}
fn floats<const N: usize>(c: &mut Cursor<'_>, field: &'static str) -> Result<[f32; N], Error> {
    let mut values = [0.; N];
    for value in &mut values {
        *value = number(c, field)?;
    }
    Ok(values)
}
fn expected(field: &'static str, value: u32, want: u32) -> Result<(), Error> {
    if value != want {
        return Err(invalid(field, format!("{value:#x}, expected {want:#x}")));
    }
    Ok(())
}
fn finish(c: &Cursor<'_>) -> Result<(), Error> {
    if c.remaining() != 0 {
        return Err(invalid("object end", format!("{} trailing bytes", c.remaining())));
    }
    Ok(())
}
fn blob<'a>(c: &mut Cursor<'a>, field: &'static str) -> Result<&'a [u8], Error> {
    let n = c.count(LE, 1).field(field)?;
    Ok(c.take(n).field(field)?)
}

/// The stored collision filter; layer and part are kept as source enum values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub id: u32,
    pub layer: u32,
    pub part: u32,
}
impl Filter {
    fn read(c: &mut Cursor<'_>) -> Result<Self, Error> {
        let id = word(c, "filter id")?;
        expected("filter class", word(c, "filter class")?, 0x43EF_99C2)?;
        Ok(Self {
            id,
            layer: word(c, "filter layer")?,
            part: word(c, "filter part")?,
        })
    }
}

/// One explicit collision triangle with its source material object's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Triangle {
    pub indices: [u32; 3],
    pub surface_id: u32,
}

/// A mesh collision shape, in local source coordinates. Vertices remain float4 (w=0); AABBs retain their stored w.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshShape<'a> {
    pub id: u32,
    pub vertices: Vec<[f32; 4]>,
    pub triangles: Vec<Triangle>,
    pub materials: Vec<u32>,
    pub material_indices: &'a [u8],
    pub mopp: &'a [u8],
    pub mopp_offset_and_scale: [f32; 4],
    pub filter: Filter,
    pub mopp_version: u32,
    pub bounds: [[f32; 4]; 2],
}

impl<'a> MeshShape<'a> {
    /// A complete managed MeshShape body (without its directory envelope), checked to its exact end.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        match Shape::parse(bytes)? {
            Shape::Mesh(mesh) => Ok(mesh),
            _ => Err(invalid("shape class", "not a MeshShape")),
        }
    }
    fn read(c: &mut Cursor<'a>, id: u32) -> Result<Self, Error> {
        let count = c.count(LE, 16).field("vertex count")?;
        let mut vertices = Vec::with_capacity(count);
        for _ in 0..count {
            let v: [f32; 4] = floats(c, "vertex")?;
            if v.get(3) != Some(&0.) {
                return Err(invalid("vertex w", "expected zero"));
            }
            vertices.push(v);
        }
        let mopp = blob(c, "MOPP bytes")?;
        let mopp_offset_and_scale = floats(c, "MOPP offset and scale")?;
        let count = c.count(LE, 2).field("triangle index count")?;
        if !count.is_multiple_of(3) {
            return Err(invalid("triangle index count", "not a multiple of three"));
        }
        let mut indices = Vec::with_capacity(count);
        for _ in 0..count {
            let i = c.u16(LE).field("triangle index")?;
            if usize::from(i) >= vertices.len() {
                return Err(invalid(
                    "triangle index",
                    format!("{i} past {} vertices", vertices.len()),
                ));
            }
            indices.push(u32::from(i));
        }
        let material_indices = blob(c, "triangle material indices")?;
        if material_indices.len() != count.checked_div(3).unwrap_or(0) {
            return Err(invalid("triangle material indices", "need one per triangle"));
        }
        let count = c.count(LE, 4).field("material count")?;
        let mut materials = Vec::with_capacity(count);
        for _ in 0..count {
            materials.push(word(c, "material object id")?);
        }
        let triangles = indices
            .as_chunks::<3>()
            .0
            .iter()
            .zip(material_indices)
            .map(|(indices, &m)| {
                let surface_id = materials.get(usize::from(m)).copied().ok_or_else(|| {
                    invalid(
                        "triangle material index",
                        format!("{m} past {} materials", materials.len()),
                    )
                })?;
                Ok(Triangle {
                    indices: *indices,
                    surface_id,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let filter = Filter::read(c)?;
        let mopp_version = word(c, "MOPP version")?;
        expected("MOPP version", mopp_version, 5)?;
        let bounds = [floats(c, "local AABB min")?, floats(c, "local AABB max")?];
        for (&lo, &hi) in bounds
            .first()
            .into_iter()
            .flatten()
            .take(3)
            .zip(bounds.get(1).into_iter().flatten().take(3))
        {
            if lo > hi {
                return Err(invalid("local AABB", "min exceeds max"));
            }
        }
        Ok(Self {
            id,
            vertices,
            triangles,
            materials,
            material_indices,
            mopp,
            mopp_offset_and_scale,
            filter,
            mopp_version,
            bounds,
        })
    }
}

/// A list's child pointer, either inline, unresolved by id, or explicitly null. External/link/local-link tags are
/// retained (1, 2, 5); a consumer resolves the referenced object in its chosen archive order.
#[derive(Clone, Debug, PartialEq)]
pub enum Child<'a> {
    Inline(Box<Shape<'a>>),
    Reference { tag: u8, id: u32 },
    Null,
}

/// Authored shape data; no tessellation, hull construction or choice of destination surfaces happens here.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape<'a> {
    Mesh(MeshShape<'a>),
    /// The contained entity/shape arrays and MOPP as authored. Pairing is retained by position; the consumer
    /// resolves the contained entities' own transforms, not the containing MultiInert entity's transform.
    MultiMesh {
        id: u32,
        entity_ids: Vec<u32>,
        shape_ids: Vec<u32>,
        mopp: &'a [u8],
        offset_and_scale: [f32; 4],
    },
    Capsule {
        id: u32,
        bottom: [f32; 4],
        top: [f32; 4],
        radius: f32,
        surface_id: u32,
    },
    List {
        id: u32,
        children: Vec<Child<'a>>,
        /// One trailing byte in this game's list objects, absent from the AC2 schema's field list.
        flag: u8,
    },
    Box {
        id: u32,
        half_extents: [f32; 4],
        matrix: [f32; 16],
        surface_id: u32,
    },
    Sphere {
        id: u32,
        center: [f32; 4],
        radius: f32,
        surface_id: u32,
    },
    /// BarrelShape is authored as a transformed convex hull, not an inferred mathematical cylinder.
    Convex {
        id: u32,
        matrix: [f32; 16],
        planes: Vec<[f32; 4]>,
        vertices: Vec<[f32; 4]>,
        surface_id: u32,
    },
}

impl<'a> Shape<'a> {
    /// Read one complete managed collision object. Unsupported classes, pointer tags, non-finite coordinates,
    /// invalid indices/counts, nested depth over 64 and bytes left over are named errors.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes);
        let shape = Self::read(&mut c, 0)?;
        finish(&c)?;
        Ok(shape)
    }
    fn read(c: &mut Cursor<'a>, depth: usize) -> Result<Self, Error> {
        if depth > 64 {
            return Err(invalid("shape depth", "exceeds 64"));
        }
        let managed = c.u8().field("managed flag")?;
        if managed > 1 {
            return Err(invalid("managed flag", managed.to_string()));
        }
        let id = word(c, "shape id")?;
        let class = word(c, "shape class")?;
        match class {
            MESH_SHAPE => Ok(Self::Mesh(MeshShape::read(c, id)?)),
            MULTI_MESH_SHAPE => {
                let count = c.count(LE, 5).field("contained entity count")?;
                let mut entity_ids = Vec::with_capacity(count);
                for _ in 0..count {
                    expected(
                        "entity handle tag",
                        u32::from(c.u8().field("entity handle tag")?),
                        0,
                    )?;
                    entity_ids.push(word(c, "contained entity")?);
                }
                let count = c.count(LE, 4).field("contained shape count")?;
                if count != entity_ids.len() {
                    return Err(invalid("contained shape count", "differs from entity count"));
                }
                let mut shape_ids = Vec::with_capacity(count);
                for _ in 0..count {
                    shape_ids.push(word(c, "contained shape")?);
                }
                Ok(Self::MultiMesh {
                    id,
                    entity_ids,
                    shape_ids,
                    mopp: blob(c, "multi MOPP bytes")?,
                    offset_and_scale: floats(c, "multi MOPP offset and scale")?,
                })
            }
            CAPSULE_SHAPE => {
                let bottom = floats(c, "capsule bottom")?;
                let top = floats(c, "capsule top")?;
                let radius = number(c, "capsule radius")?;
                if radius <= 0. {
                    return Err(invalid("capsule radius", "not positive"));
                }
                Ok(Self::Capsule {
                    id,
                    bottom,
                    top,
                    radius,
                    surface_id: word(c, "capsule material")?,
                })
            }
            LIST_SHAPE => {
                let count = c.count(LE, 1).field("list child count")?;
                let mut children = Vec::with_capacity(count);
                for _ in 0..count {
                    let tag = c.u8().field("child pointer")?;
                    children.push(match tag {
                        0 | 4 => Child::Inline(Box::new(Self::read(c, depth.saturating_add(1))?)),
                        1 | 2 | 5 => Child::Reference {
                            tag,
                            id: word(c, "child reference")?,
                        },
                        3 => Child::Null,
                        _ => return Err(invalid("child pointer", format!("unsupported tag {tag}"))),
                    });
                }
                Ok(Self::List {
                    id,
                    children,
                    flag: c.u8().field("list flag")?,
                })
            }
            BOX_SHAPE => Ok(Self::Box {
                id,
                half_extents: floats(c, "box half extents")?,
                matrix: floats(c, "box matrix")?,
                surface_id: word(c, "box material")?,
            }),
            SPHERE_SHAPE => {
                let center = floats(c, "sphere center")?;
                let radius = number(c, "sphere radius")?;
                if radius <= 0. {
                    return Err(invalid("sphere radius", "not positive"));
                }
                Ok(Self::Sphere {
                    id,
                    center,
                    radius,
                    surface_id: word(c, "sphere material")?,
                })
            }
            BARREL_SHAPE => {
                let matrix = floats(c, "convex matrix")?;
                let count = c.count(LE, 16).field("plane count")?;
                let mut planes = Vec::with_capacity(count);
                for _ in 0..count {
                    planes.push(floats(c, "convex plane")?);
                }
                let count = c.count(LE, 16).field("hull vertex count")?;
                let mut vertices = Vec::with_capacity(count);
                for _ in 0..count {
                    vertices.push(floats(c, "convex vertex")?);
                }
                Ok(Self::Convex {
                    id,
                    matrix,
                    planes,
                    vertices,
                    surface_id: word(c, "convex material")?,
                })
            }
            _ => Err(invalid("shape class", format!("unsupported {class:#010x}"))),
        }
    }
}

/// A MultiInertComponent's own shape container. Its entity handles and bare shape ids stay unresolved.
#[derive(Clone, Debug, PartialEq)]
pub struct MultiInertComponent<'a> {
    pub managed: u8,
    pub id: u32,
    pub resource_id: u32,
    pub resource_kind: u8,
    pub resource_target: u32,
    pub shape: Shape<'a>,
}
impl<'a> MultiInertComponent<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let (value, used) = Self::parse_prefix(bytes)?;
        if used != bytes.len() {
            return Err(invalid("object end", "trailing bytes after MultiInertComponent"));
        }
        Ok(value)
    }
    /// One inline component at the start of an Entity's remaining bytes, and its consumed length. The component's
    /// counted arrays delimit it; subsequent Entity components are not part of this object.
    pub fn parse_prefix(bytes: &'a [u8]) -> Result<(Self, usize), Error> {
        let mut c = Cursor::new(bytes);
        let managed = c.u8().field("managed flag")?;
        if managed > 1 {
            return Err(invalid("managed flag", managed.to_string()));
        }
        let id = word(&mut c, "multi inert id")?;
        expected(
            "component class",
            word(&mut c, "component class")?,
            MULTI_INERT_COMPONENT,
        )?;
        let resource_id = word(&mut c, "resource reference id")?;
        expected(
            "resource reference class",
            word(&mut c, "resource reference class")?,
            0x3C6C_77C8,
        )?;
        let resource_kind = c.u8().field("resource kind")?;
        let resource_target = word(&mut c, "resource target")?;
        let shape = Shape::read(&mut c, 0)?;
        if !matches!(shape, Shape::MultiMesh { .. }) {
            return Err(invalid("multi inert shape", "not a MultiMeshShape"));
        }
        let used = bytes.len().saturating_sub(c.remaining());
        Ok((
            Self {
                managed,
                id,
                resource_id,
                resource_kind,
                resource_target,
                shape,
            },
            used,
        ))
    }
}

/// The fixed 150-byte InertComponent payload, including its rigid body's shape reference and source physics fields.
/// The entity's transform places the shape; the stored inertia tensor is not a placement matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct InertComponent {
    /// The managed byte is 0 for the measured inline inert; standalone objects commonly store 1.
    pub managed: u8,
    pub id: u32,
    pub resource_id: u32,
    pub resource_kind: u8,
    pub resource_target: u32,
    pub damage: u32,
    pub rigid_body_id: u32,
    pub filter: Filter,
    pub shape_id: u32,
    pub mass: f32,
    pub damping: [f32; 2],
    pub flags: [u8; 4],
    pub inertia: [f32; 16],
    pub walkability: u32,
    /// The remaining 12 authored bytes, whose meaning is not established.
    pub tail: [u8; 12],
}
impl InertComponent {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let mut c = Cursor::new(bytes);
        let managed = c.u8().field("managed flag")?;
        if managed > 1 {
            return Err(invalid("managed flag", managed.to_string()));
        }
        let id = word(&mut c, "inert id")?;
        expected(
            "component class",
            word(&mut c, "component class")?,
            INERT_COMPONENT,
        )?;
        let resource_id = word(&mut c, "resource reference id")?;
        expected(
            "resource reference class",
            word(&mut c, "resource reference class")?,
            0x3C6C_77C8,
        )?;
        let resource_kind = c.u8().field("resource kind")?;
        let resource_target = word(&mut c, "resource target")?;
        let damage = word(&mut c, "damage")?;
        let rigid_body_id = word(&mut c, "rigid body id")?;
        expected("rigid body class", word(&mut c, "rigid body class")?, 0x228F_402A)?;
        let filter = Filter::read(&mut c)?;
        let shape_id = word(&mut c, "rigid body shape")?;
        let mass = number(&mut c, "mass")?;
        let damping = [
            number(&mut c, "linear damping")?,
            number(&mut c, "angular damping")?,
        ];
        let flags = c.array().field("rigid body flags")?;
        let inertia = floats(&mut c, "inertia tensor")?;
        let walkability = word(&mut c, "walkability")?;
        let tail = c.array().field("inert tail")?;
        finish(&c)?;
        Ok(Self {
            managed,
            id,
            resource_id,
            resource_kind,
            resource_target,
            damage,
            rigid_body_id,
            filter,
            shape_id,
            mass,
            damping,
            flags,
            inertia,
            walkability,
            tail,
        })
    }
}
