//! Havok 7.1.0-r1 physics class views over a [`Packfile`] (4-byte pointers, big endian). The level packfiles carry no
//! reflection (`__types__` is empty), so every member offset below is stated from the 7.1 member order (SDK source
//! order, `hkVector4` members 16-aligned) and **confirmed on the data**: the last column names the check that held on
//! all 490 SHRED files (12,789 bodies, 1,758 extended meshes, 6,445 subparts, 646,081 triangles, 8,780 convex shapes).
//! A layout that does not hold is an error naming the offset, never a guess.
//!
//! | class | member | offset | confirmed by |
//! |---|---|---|---|
//! | hkpRigidBody | shape pointer | `+0x10` | never null; its class is a known shape |
//! | | shape key | `+0x14` | `0xFFFFFFFF` in every body |
//! | | name pointer | `+0x74` | C string in every body |
//! | | material: response type (u8), friction, restitution | `+0x8C`, `+0x90`, `+0x94` | type 1 in every body; finite |
//! | | collision filter info | `+0xA8` | low 16 bits `0xFFFF` in every body |
//! | | motion type (u8) | `+0xE8` | 2, 3 or 5 (sphere inertia, box inertia, fixed); fixed = all 1,758 meshes |
//! | | rotation, three columns of 16 bytes (xyz used) | `+0xF0` | orthonormal, det +1 in every body |
//! | | translation | `+0x120` | equals the swept centre `+0x130` unless the body has a centre-of-mass offset |
//! | hkpMoppBvTreeShape | child shape pointer | `+0x34` | a mesh (1,704) or a list (1) |
//! | hkpStorageExtendedMeshShape | aabb half extents, centre | `+0x90`, `+0xA0` | every vertex inside (tolerance 0.06) |
//! | | numBitsForSubpartIndex | `+0xB0` | 12 |
//! | | trianglesSubparts (hkArray, 96-byte elements) | `+0xB4` | |
//! | | weldingInfo (hkArray of u16) | `+0xCC` | length = total triangles |
//! | | meshStorage (hkArray of pointers) | `+0xF0` | length = subparts; each a MeshSubpartStorage |
//! | TrianglesSubpart (96 B) | material index striding (u16), material count (u16) | `+0x08` | 2; = storage material count |
//! | | triangle count, vertex striding, vertex count | `+0x14`, `+0x1C`, `+0x20` | = storage; 16 |
//! | | index striding, striding type | `+0x44`, `+0x48` | 16 bytes (4 u32 per triangle); `0x03000000` |
//! | | triangle offset | `+0x4C` | cumulative triangles of the earlier subparts |
//! | MeshSubpartStorage | vertices (hkVector4) | `+0x08` | count = subpart; w ignored |
//! | | indices8, indices16 | `+0x14`, `+0x20` | empty in every subpart |
//! | | indices32 | `+0x2C` | 4 per triangle; every index < vertex count; the 4th is 0 |
//! | | materialIndices8 | `+0x38` | empty |
//! | | materials (12-byte records `a, b, c`) | `+0x44` | `b == 0x3F80` in every record; `c` is padding |
//! | | namedMaterials | `+0x50` | empty |
//! | | materialIndices16 | `+0x5C` | one per triangle, each < the material count |
//! | hkpConvexVerticesShape | radius, aabb half extents, centre | `+0x10`, `+0x20`, `+0x30` | every vertex inside the aabb |
//! | | rotatedVertices (FourVectors, 48 B: `x[4]`, `y[4]`, `z[4]`) | `+0x40` | |
//! | | vertex count | `+0x4C` | fits the FourVectors (`4 n-3 ..= 4 n`) |
//! | | planeEquations (16 B) | `+0x54` | stored, **not exact hull faces** (normals not unit, some violated) |
//! | hkpBoxShape | radius, half extents | `+0x10`, `+0x20` | |
//! | hkpSphereShape | radius | `+0x10` | |
//! | hkpCapsuleShape | radius, vertex A, vertex B | `+0x10`, `+0x20`, `+0x30` | |
//! | hkpCylinderShape | radius, cylinder radius, base-radius factor, A, B | `+0x10`, `+0x14`, `+0x18`, `+0x20`, `+0x30` | |
//! | hkpConvexTranslateShape | child, translation | `+0x18`, `+0x20` | a box (1,384) or sphere (16) |
//! | hkpListShape | childInfo (hkArray of 16-byte records, shape pointer first) | `+0x18` | |
//!
//! `hkpMoppCode`, `hkpConvexVerticesConnectivity`, the constraint classes and the container/system classes are
//! recognised by name and counted by [`census`], not decoded.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
use std::collections::BTreeMap;

use super::Packfile;
use crate::Result;

/// Classes decoded by this module.
pub const DECODED: [&str; 11] = [
    "hkpRigidBody",
    "hkpMoppBvTreeShape",
    "hkpStorageExtendedMeshShape",
    "hkpStorageExtendedMeshShapeMeshSubpartStorage",
    "hkpConvexVerticesShape",
    "hkpBoxShape",
    "hkpSphereShape",
    "hkpCapsuleShape",
    "hkpCylinderShape",
    "hkpListShape",
    "hkpConvexTranslateShape",
];
/// Classes recognised by name and reported, not decoded.
pub const RECOGNISED: [&str; 9] = [
    "hkpMoppCode",
    "hkpConvexVerticesConnectivity",
    "hkpConstraintInstance",
    "hkpRagdollConstraintData",
    "hkpLimitedHingeConstraintData",
    "hkpBreakableConstraintData",
    "hkRootLevelContainer",
    "hkpPhysicsData",
    "hkpPhysicsSystem",
];

/// What the objects of a packfile are: counts per class, split by what this module does with them.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Census {
    /// Classes with a view here.
    pub decoded: BTreeMap<String, usize>,
    /// Classes known by name, with no view.
    pub recognised: BTreeMap<String, usize>,
    /// Classes this module has never seen: nothing is read from them.
    pub unknown: BTreeMap<String, usize>,
}

/// Count every object of `pack` by class (the report of what was and was not read).
pub fn census(pack: &Packfile<'_>) -> Census {
    let mut out = Census::default();
    for (name, n) in pack.census() {
        let bucket = if DECODED.contains(&name) {
            &mut out.decoded
        } else if RECOGNISED.contains(&name) {
            &mut out.recognised
        } else {
            &mut out.unknown
        };
        bucket.insert(name.to_string(), n);
    }
    out
}

/// Offsets of every `hkpRigidBody`, in data order.
pub fn rigid_bodies(pack: &Packfile<'_>) -> Vec<usize> {
    pack.objects_of("hkpRigidBody").collect()
}

/// Motion types seen in the data (Havok's `hkpMotion::MotionType`: 2 sphere inertia, 3 box inertia, 5 fixed).
pub const MOTION_SPHERE_INERTIA: u8 = 2;
pub const MOTION_BOX_INERTIA: u8 = 3;
pub const MOTION_FIXED: u8 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct RigidBody {
    pub offset: usize,
    /// The collision shape's object offset.
    pub shape: usize,
    pub name: String,
    /// `hkpMaterial` response type (1 = simple contact).
    pub response_type: u8,
    pub friction: f32,
    pub restitution: f32,
    /// High 16 bits: group; low 16 bits are `0xFFFF` in the data.
    pub collision_filter_info: u32,
    pub motion_type: u8,
    /// Rotation as three columns: `world = x*c0 + y*c1 + z*c2 + translation`.
    pub rotation: [[f32; 3]; 3],
    pub translation: [f32; 3],
}

impl RigidBody {
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpRigidBody" {
            return Err(pack.err(offset, "not a hkpRigidBody"));
        }
        let shape = pack.target(at(0x10)?, "rigid body shape")?;
        pack.class_at(shape)?;
        if pack.u32(at(0x14)?)? != 0xFFFF_FFFF {
            return Err(pack.err(at(0x14)?, "shape key is not the invalid key"));
        }
        let name = pack.c_string(pack.target(at(0x74)?, "rigid body name")?)?;
        let response_type = pack.u8(at(0x8C)?)?;
        let filter = pack.u32(at(0xA8)?)?;
        if filter & 0xFFFF != 0xFFFF {
            return Err(pack.err(at(0xA8)?, "collision filter info low half is not 0xFFFF"));
        }
        let motion_type = pack.u8(at(0xE8)?)?;
        if ![MOTION_SPHERE_INERTIA, MOTION_BOX_INERTIA, MOTION_FIXED].contains(&motion_type) {
            return Err(pack.err(at(0xE8)?, format!("motion type {motion_type} is not 2, 3 or 5")));
        }
        let mut rotation = [[0.0; 3]; 3];
        for (i, col) in rotation.iter_mut().enumerate() {
            *col = pack.floats::<3>(at(0xF0usize.saturating_add(i.saturating_mul(16)))?)?;
        }
        let dot = |a: &[f32; 3], b: &[f32; 3]| {
            a.iter()
                .zip(b)
                .map(|(x, y)| f64::from(*x) * f64::from(*y))
                .sum::<f64>()
        };
        for (i, a) in rotation.iter().enumerate() {
            for (j, b) in rotation.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                if (dot(a, b) - want).abs() > 2e-3 {
                    return Err(pack.err(at(0xF0)?, "rotation is not orthonormal"));
                }
            }
        }
        let [c0, c1, c2] = &rotation;
        let det = c0[0] * (c1[1] * c2[2] - c1[2] * c2[1]) - c0[1] * (c1[0] * c2[2] - c1[2] * c2[0])
            + c0[2] * (c1[0] * c2[1] - c1[1] * c2[0]);
        if (det - 1.0).abs() > 2e-3 {
            return Err(pack.err(at(0xF0)?, "rotation determinant is not +1"));
        }
        Ok(Self {
            offset,
            shape,
            name,
            response_type,
            friction: pack.f32(at(0x90)?)?,
            restitution: pack.f32(at(0x94)?)?,
            collision_filter_info: filter,
            motion_type,
            rotation,
            translation: pack.floats::<3>(at(0x120)?)?,
        })
    }

    /// A shape-local point in world space.
    pub fn to_world(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.translation;
        for (axis, col) in p.iter().zip(&self.rotation) {
            for (o, c) in out.iter_mut().zip(col) {
                *o += axis * c;
            }
        }
        out
    }
}

/// One triangle material record: `a` is the whole per-triangle word (kept intact; bits 3..=8 are a surface id, the
/// other bits are opaque flags), `b` is `0x3F80` in all data, `c` is padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshMaterial {
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subpart {
    /// Shape-local vertices (the stored w lane is dropped).
    pub vertices: Vec<[f32; 3]>,
    /// Triangles as stored (`i0, i1, i2`), indexing `vertices`.
    pub triangles: Vec<[u32; 3]>,
    /// Per triangle, an index into `materials`.
    pub material_indices: Vec<u16>,
    pub materials: Vec<MeshMaterial>,
    /// Cumulative triangle count of the earlier subparts.
    pub triangle_offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtendedMesh {
    pub aabb_half_extents: [f32; 3],
    pub aabb_center: [f32; 3],
    pub subparts: Vec<Subpart>,
    /// Per triangle (all subparts in order), the 16-bit welding info.
    pub welding: Vec<u16>,
}

impl ExtendedMesh {
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpStorageExtendedMeshShape" {
            return Err(pack.err(offset, "not a hkpStorageExtendedMeshShape"));
        }
        if pack.u32(at(0xB0)?)? != 12 {
            return Err(pack.err(at(0xB0)?, "numBitsForSubpartIndex is not 12"));
        }
        let subs = pack.array_header(at(0xB4)?)?;
        let storage = pack.array_header(at(0xF0)?)?;
        if storage.len != subs.len {
            return Err(pack.err(at(0xF0)?, "mesh storage count differs from the subpart count"));
        }
        if pack.array_header(at(0xC0)?)?.len != 0 {
            return Err(pack.err(at(0xC0)?, "shape subparts are not supported"));
        }
        let mut subparts = Vec::new();
        let mut total = 0usize;
        for i in 0..subs.len {
            let sub = pack.at(subs.data.unwrap_or(0), i.saturating_mul(96))?;
            let store = pack.target(
                pack.at(storage.data.unwrap_or(0), i.saturating_mul(4))?,
                "mesh storage",
            )?;
            if pack.class_at(store)? != "hkpStorageExtendedMeshShapeMeshSubpartStorage" {
                return Err(pack.err(store, "mesh storage is not a MeshSubpartStorage"));
            }
            let s = read_subpart(pack, sub, store, total)?;
            total = pack.at(total, s.triangles.len())?;
            subparts.push(s);
        }
        let weld = pack.array_header(at(0xCC)?)?;
        if weld.len != total {
            return Err(pack.err(at(0xCC)?, "welding info count differs from the triangle count"));
        }
        let mut welding = Vec::new();
        for i in 0..weld.len {
            welding.push(pack.u16(pack.at(weld.data.unwrap_or(0), i.saturating_mul(2))?)?);
        }
        Ok(Self {
            aabb_half_extents: pack.floats::<3>(at(0x90)?)?,
            aabb_center: pack.floats::<3>(at(0xA0)?)?,
            subparts,
            welding,
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.subparts.iter().map(|s| s.triangles.len()).sum()
    }
}

fn read_subpart(pack: &Packfile<'_>, sub: usize, store: usize, expected_offset: usize) -> Result<Subpart> {
    let sa = |add| pack.at(sub, add);
    let ta = |add| pack.at(store, add);
    if pack.u16(sa(0x08)?)? != 2 {
        return Err(pack.err(sa(0x08)?, "material index striding is not 2"));
    }
    let n_materials = usize::from(pack.u16(sa(0x0A)?)?);
    let n_tris = pack.count(sa(0x14)?)?;
    let n_verts = pack.count(sa(0x20)?)?;
    if pack.u32(sa(0x1C)?)? != 16 || pack.u32(sa(0x44)?)? != 16 {
        return Err(pack.err(sa(0x1C)?, "vertex or index striding is not 16"));
    }
    if pack.u32(sa(0x48)?)? != 0x0300_0000 {
        return Err(pack.err(sa(0x48)?, "index striding type is not 32-bit"));
    }
    if pack.count(sa(0x4C)?)? != expected_offset {
        return Err(pack.err(
            sa(0x4C)?,
            "triangle offset is not the earlier subparts' triangle count",
        ));
    }
    let verts = pack.array_header(ta(0x08)?)?;
    if verts.len != n_verts {
        return Err(pack.err(ta(0x08)?, "storage vertex count differs from the subpart"));
    }
    for (off, what) in [
        (0x14, "indices8"),
        (0x20, "indices16"),
        (0x38, "materialIndices8"),
        (0x50, "namedMaterials"),
    ] {
        if pack.array_header(ta(off)?)?.len != 0 {
            return Err(pack.err(ta(off)?, format!("{what} is not empty")));
        }
    }
    let mut vertices = Vec::new();
    for i in 0..verts.len {
        let v = pack.at(verts.data.unwrap_or(0), i.saturating_mul(16))?;
        vertices.push(pack.floats::<3>(v)?);
        pack.f32(pack.at(v, 12)?)?;
    }
    let idx = pack.array_header(ta(0x2C)?)?;
    if Some(idx.len) != n_tris.checked_mul(4) {
        return Err(pack.err(ta(0x2C)?, "index count is not four per triangle"));
    }
    let mut triangles = Vec::new();
    for t in 0..n_tris {
        let at = pack.at(idx.data.unwrap_or(0), t.saturating_mul(16))?;
        let word = |k: usize| -> Result<u32> { pack.u32(pack.at(at, k.saturating_mul(4))?) };
        let tri = [word(0)?, word(1)?, word(2)?];
        if tri
            .iter()
            .any(|&i| usize::try_from(i).map_or(true, |i| i >= n_verts))
        {
            return Err(pack.err(at, "triangle index is outside the vertices"));
        }
        if word(3)? != 0 {
            return Err(pack.err(pack.at(at, 12)?, "fourth index word is not zero"));
        }
        triangles.push(tri);
    }
    let mats = pack.array_header(ta(0x44)?)?;
    if mats.len != n_materials {
        return Err(pack.err(ta(0x44)?, "storage material count differs from the subpart"));
    }
    let mut materials = Vec::new();
    for m in 0..mats.len {
        let r = pack.at(mats.data.unwrap_or(0), m.saturating_mul(12))?;
        let rec = MeshMaterial {
            a: pack.u32(r)?,
            b: pack.u32(pack.at(r, 4)?)?,
            c: pack.u32(pack.at(r, 8)?)?,
        };
        if rec.b != 0x3F80 {
            return Err(pack.err(pack.at(r, 4)?, "material record word b is not 0x3F80"));
        }
        materials.push(rec);
    }
    let mi = pack.array_header(ta(0x5C)?)?;
    if mi.len != n_tris {
        return Err(pack.err(ta(0x5C)?, "material index count differs from the triangle count"));
    }
    let mut material_indices = Vec::new();
    for t in 0..mi.len {
        let i = pack.u16(pack.at(mi.data.unwrap_or(0), t.saturating_mul(2))?)?;
        if usize::from(i) >= n_materials {
            return Err(pack.err(ta(0x5C)?, "material index is outside the materials"));
        }
        material_indices.push(i);
    }
    Ok(Subpart {
        vertices,
        triangles,
        material_indices,
        materials,
        triangle_offset: expected_offset,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvexVertices {
    pub radius: f32,
    pub aabb_half_extents: [f32; 3],
    pub aabb_center: [f32; 3],
    pub vertices: Vec<[f32; 3]>,
    /// Stored plane equations `(nx, ny, nz, d)`: approximate, not exact hull faces.
    pub planes: Vec<[f32; 4]>,
}

impl ConvexVertices {
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        let at = |add| pack.at(offset, add);
        if pack.class_at(offset)? != "hkpConvexVerticesShape" {
            return Err(pack.err(offset, "not a hkpConvexVerticesShape"));
        }
        let groups = pack.array_header(at(0x40)?)?;
        let n = pack.count(at(0x4C)?)?;
        let capacity = groups.len.saturating_mul(4);
        if n > capacity || n <= capacity.saturating_sub(4) {
            return Err(pack.err(at(0x4C)?, "vertex count does not fit the FourVectors"));
        }
        let mut vertices = Vec::new();
        for j in 0..n {
            let (g, lane) = (j / 4, j % 4);
            let base = pack.at(groups.data.unwrap_or(0), g.saturating_mul(48))?;
            let mut v = [0.0; 3];
            for (axis, slot) in v.iter_mut().enumerate() {
                *slot = pack.f32(pack.at(
                    base,
                    axis.saturating_mul(16).saturating_add(lane.saturating_mul(4)),
                )?)?;
            }
            vertices.push(v);
        }
        let pl = pack.array_header(at(0x54)?)?;
        let mut planes = Vec::new();
        for i in 0..pl.len {
            planes.push(pack.floats::<4>(pack.at(pl.data.unwrap_or(0), i.saturating_mul(16))?)?);
        }
        Ok(Self {
            radius: pack.f32(at(0x10)?)?,
            aabb_half_extents: pack.floats::<3>(at(0x20)?)?,
            aabb_center: pack.floats::<3>(at(0x30)?)?,
            vertices,
            planes,
        })
    }
}

/// A collision shape, one view per class.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A MOPP tree; its code is not decoded. The child holds the geometry.
    Mopp {
        child: usize,
    },
    ExtendedMesh(ExtendedMesh),
    ConvexVertices(ConvexVertices),
    Box {
        radius: f32,
        half_extents: [f32; 3],
    },
    Sphere {
        radius: f32,
    },
    Capsule {
        radius: f32,
        a: [f32; 3],
        b: [f32; 3],
    },
    Cylinder {
        radius: f32,
        cylinder_radius: f32,
        base_radius_factor: f32,
        a: [f32; 3],
        b: [f32; 3],
    },
    ConvexTranslate {
        child: usize,
        translation: [f32; 3],
    },
    List {
        children: Vec<usize>,
    },
}

impl Shape {
    /// The shape object at `offset`; a class this module has no view for is an error naming it.
    pub fn read(pack: &Packfile<'_>, offset: usize) -> Result<Self> {
        let at = |add| pack.at(offset, add);
        let class = pack.class_at(offset)?;
        Ok(match class {
            "hkpMoppBvTreeShape" => Self::Mopp {
                child: pack.target(at(0x34)?, "MOPP child shape")?,
            },
            "hkpStorageExtendedMeshShape" => Self::ExtendedMesh(ExtendedMesh::read(pack, offset)?),
            "hkpConvexVerticesShape" => Self::ConvexVertices(ConvexVertices::read(pack, offset)?),
            "hkpBoxShape" => Self::Box {
                radius: pack.f32(at(0x10)?)?,
                half_extents: pack.floats::<3>(at(0x20)?)?,
            },
            "hkpSphereShape" => Self::Sphere {
                radius: pack.f32(at(0x10)?)?,
            },
            "hkpCapsuleShape" => Self::Capsule {
                radius: pack.f32(at(0x10)?)?,
                a: pack.floats::<3>(at(0x20)?)?,
                b: pack.floats::<3>(at(0x30)?)?,
            },
            "hkpCylinderShape" => Self::Cylinder {
                radius: pack.f32(at(0x10)?)?,
                cylinder_radius: pack.f32(at(0x14)?)?,
                base_radius_factor: pack.f32(at(0x18)?)?,
                a: pack.floats::<3>(at(0x20)?)?,
                b: pack.floats::<3>(at(0x30)?)?,
            },
            "hkpConvexTranslateShape" => Self::ConvexTranslate {
                child: pack.target(at(0x18)?, "convex translate child")?,
                translation: pack.floats::<3>(at(0x20)?)?,
            },
            "hkpListShape" => {
                let info = pack.array_header(at(0x18)?)?;
                let mut children = Vec::new();
                for i in 0..info.len {
                    let rec = pack.at(info.data.unwrap_or(0), i.saturating_mul(16))?;
                    children.push(pack.target(rec, "list child shape")?);
                }
                Self::List { children }
            }
            other => return Err(pack.err(offset, format!("no view for shape class {other}"))),
        })
    }
}
