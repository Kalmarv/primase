//! Level collision: the base and layer `.hkw` Havok 7.1.0-r1 packfiles of the PAKs the program supplies, as core
//! collision on entities (`primase_havok::packfile` reads the files; layouts and censuses are its tables).
//!
//! One entity per leaf shape of each rigid body, with the body's transform (frame-converted) and the shape in the
//! body's space (frame-converted points): a body that moves keeps its own transform. A fixed body (motion type 5) is
//! static collision: a mesh leaf is one `core.collision` soup, any other leaf a `core.collider`. A body of any other
//! motion type is kept as data (`shred.rigid_body` with its motion type, its shape in `shred.dynamic_shape`) and is
//! never merged into the static collision.
//!
//! Shapes: the extended mesh's subparts are one soup in stored vertex and index order. The data's winding is
//! counter-clockwise-front (the world's convention) and is kept: it is the render geometry's opposite, whose triangles
//! are clockwise against their stored normals. A triangle's surface is the `surfaces` row `(a >> 3) & 0x3F` of its
//! material word and the other bits of `a` travel as `shred.collision_flags`. A convex-vertices shape is
//! `Shape::Convex` of its vertices (the stored planes are not exact faces); box, sphere, capsule and cylinder are
//! core's primitives, so nothing is tessellated. Havok's convex radius (a skin around every convex shape) is not
//! added to any of them. A `ConvexTranslate` offsets its child; a list and a MOPP tree hold their children. Only
//! meshes carry a surface: every other leaf is surface 0 (`default`).
use super::pak::Pak;
use crate::{
    Error, Result,
    components::{CollisionFlags, DynamicShape, RigidBody as BodyData},
    invalid,
};
use primase_core::{
    Collider, Collision, Shape as CoreShape, Source, Surfaces, Transform, World, frame::SourceFrame,
};
use primase_havok::packfile::{
    Packfile,
    physics::{self, ExtendedMesh, RigidBody, Shape},
};
use primase_sqlite::{Database, Value};
use std::collections::BTreeMap;

/// The surface field of a triangle's material word: bits 3..=8.
pub const SURFACE_SHIFT: u32 = 3;
pub const SURFACE_MASK: u32 = 0x3F;
/// How deep a body's shape tree may go (the data's is 3).
const MAX_DEPTH: usize = 16;

/// MAIN.DB's `surfaces` table: the name of each id (`surfaces.id`, 0..n in order), which is the index into the
/// world's `core.surfaces` that [`read_collision`] sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceTable(pub Vec<String>);

impl SurfaceTable {
    pub fn from_main_db(db: &[u8]) -> Result<Self> {
        let table = Database::open(db)
            .and_then(|d| d.table("surfaces"))
            .map_err(|e| invalid(0, format!("MAIN.DB: {e}")))?;
        let (Some(id), Some(name)) = (table.column("id"), table.column("NAME")) else {
            return Err(invalid(0, "MAIN.DB surfaces has no id and NAME columns"));
        };
        let mut names = Vec::new();
        for row in &table.rows {
            let (Some(Value::Text(i)), Some(Value::Text(n))) = (row.values.get(id), row.values.get(name))
            else {
                return Err(invalid(
                    0,
                    format!("MAIN.DB surfaces row {}: id and NAME are not text", row.rowid),
                ));
            };
            if i.parse::<usize>().ok() != Some(names.len()) {
                return Err(invalid(
                    0,
                    format!(
                        "MAIN.DB surfaces row {}: id {i:?} is not {}",
                        row.rowid,
                        names.len()
                    ),
                ));
            }
            names.push(n.clone());
        }
        Ok(Self(names))
    }

    fn to_world(&self) -> Result<Surfaces> {
        let mut s = Surfaces::default();
        for (i, name) in self.0.iter().enumerate() {
            if usize::from(s.index(name).map_err(|e| invalid(0, e))?) != i {
                return Err(invalid(
                    0,
                    format!("MAIN.DB surfaces: name {name:?} repeats (row {i})"),
                ));
            }
        }
        Ok(s)
    }
}

/// What one `.hkw` entry contributed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReport {
    pub pak: String,
    pub entry: Vec<u8>,
    pub bodies: usize,
    pub fixed_triangles: usize,
    pub dynamic_triangles: usize,
}

/// Counts of everything read; triangles are collision triangles (one per mesh triangle).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionReport {
    pub files: Vec<FileReport>,
    pub bodies: usize,
    pub fixed_bodies: usize,
    /// Leaf shapes (entities), fixed and not: meshes plus `primitives`.
    pub leaves: usize,
    /// Convex-vertices leaves.
    pub convex: usize,
    /// Leaves that are not meshes (convex shapes included).
    pub primitives: usize,
    pub fixed_triangles: usize,
    pub dynamic_triangles: usize,
    /// Triangles per surface id, fixed and not.
    pub surface_triangles: BTreeMap<u16, usize>,
    /// Triangles per flag word (`a` without the surface bits), fixed and not.
    pub flag_triangles: BTreeMap<u32, usize>,
    /// The largest convex radius over the box, convex-vertices, capsule and cylinder shapes read (never added).
    pub max_radius: f32,
}

/// One leaf of a body's shape tree: its shape, offset by the `ConvexTranslate`s above it.
struct Leaf {
    shape: Shape,
    offset: [f32; 3],
}

fn leaves(pack: &Packfile<'_>, root: usize) -> Result<Vec<Leaf>> {
    let mut out = Vec::new();
    let mut stack = vec![(root, [0.0f32; 3], 0usize)];
    while let Some((at, offset, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(invalid(at, "shape tree deeper than 16"));
        }
        match Shape::read(pack, at).map_err(|e| invalid(at, e))? {
            Shape::Mopp { child } => stack.push((child, offset, depth + 1)),
            Shape::ConvexTranslate { child, translation } => {
                stack.push((child, moved(offset, translation), depth + 1));
            }
            // pushed in reverse so the children come out in stored order
            Shape::List { children } => {
                stack.extend(children.into_iter().rev().map(|c| (c, offset, depth + 1)))
            }
            shape => out.push(Leaf { shape, offset }),
        }
    }
    Ok(out)
}

fn moved(p: [f32; 3], by: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| p[k] + by[k])
}

/// The extended mesh as one soup in the target frame, with each triangle's flag word.
fn soup(
    mesh: &ExtendedMesh,
    offset: [f32; 3],
    frame: SourceFrame,
    surfaces: usize,
    at: usize,
) -> Result<(Collision, Vec<u32>)> {
    let mut soup = Collision::default();
    let mut flags = Vec::new();
    for sub in &mesh.subparts {
        let base = u32::try_from(soup.positions.len()).map_err(|_| invalid(at, "mesh past 2^32 vertices"))?;
        soup.positions
            .extend(sub.vertices.iter().map(|&v| frame.point(moved(v, offset))));
        for (tri, &m) in sub.triangles.iter().zip(&sub.material_indices) {
            let a = sub
                .materials
                .get(usize::from(m))
                .ok_or_else(|| invalid(at, format!("material index {m} past {}", sub.materials.len())))?
                .a;
            let surface = (a >> SURFACE_SHIFT) & SURFACE_MASK;
            if surface as usize >= surfaces {
                return Err(invalid(
                    at,
                    format!("surface id {surface} past the {surfaces} of MAIN.DB"),
                ));
            }
            let mut t = tri.map(|i| i + base);
            if frame.flips_winding() {
                t.swap(1, 2);
            }
            soup.triangles.push(t);
            soup.surfaces.push(surface as u16);
            flags.push(a & !(SURFACE_MASK << SURFACE_SHIFT));
        }
    }
    Ok((soup, flags))
}

/// A length in the frame's units.
fn scaled(frame: SourceFrame, length: f32) -> f32 {
    (f64::from(length) * frame.metres_per_unit()) as f32
}

/// Read every `.hkw` entry of `paks` (in the order given) into `world` and set the world's `core.surfaces` from
/// `surfaces`. `frame` is the program's assertion about the source frame, as for [`super::level::read_level`].
pub fn read_collision(
    world: &mut World,
    paks: &[(&str, &Pak)],
    surfaces: &SurfaceTable,
    frame: SourceFrame,
) -> Result<CollisionReport> {
    let table = surfaces.to_world()?;
    match world.resource::<Surfaces>() {
        None => {
            world.insert_resource(table);
        }
        Some(existing) if *existing == table => {}
        Some(_) => return Err(invalid(0, "the world's core.surfaces is not MAIN.DB's table")),
    }
    let mut report = CollisionReport::default();
    for &(label, pak) in paks {
        for (i, entry) in pak.entries().iter().enumerate() {
            if !entry.name.ends_with(b".hkw") {
                continue;
            }
            let payload = pak.payload(i)?;
            let file = read_file(
                world,
                &payload,
                (label, &entry.name),
                surfaces.0.len(),
                frame,
                &mut report,
            )
            .map_err(|e| match e {
                Error::Invalid { offset, detail } => Error::Invalid {
                    offset,
                    detail: format!("{label}:{}: {detail}", String::from_utf8_lossy(&entry.name)),
                },
                other => other,
            })?;
            report.files.push(file);
        }
    }
    Ok(report)
}

fn read_file(
    world: &mut World,
    bytes: &[u8],
    (label, name): (&str, &[u8]),
    surfaces: usize,
    frame: SourceFrame,
    report: &mut CollisionReport,
) -> Result<FileReport> {
    let pack = Packfile::read(bytes).map_err(|e| invalid(0, e))?;
    let mut file = FileReport {
        pak: label.into(),
        entry: name.to_vec(),
        bodies: 0,
        fixed_triangles: 0,
        dynamic_triangles: 0,
    };
    for (n, at) in physics::rigid_bodies(&pack).into_iter().enumerate() {
        let body = RigidBody::read(&pack, at).map_err(|e| invalid(at, e))?;
        let r = body.rotation;
        let transform = frame.transform(&Transform {
            cols: [r[0], r[1], r[2], body.translation],
        });
        if transform.cols.iter().flatten().any(|x| !x.is_finite()) {
            return Err(invalid(at, "body transform overflow in requested frame"));
        }
        let data = BodyData {
            name: body.name,
            motion_type: body.motion_type,
            response_type: body.response_type,
            friction: body.friction,
            restitution: body.restitution,
            collision_filter_info: body.collision_filter_info,
        };
        let fixed = data.is_fixed();
        report.bodies += 1;
        report.fixed_bodies += usize::from(fixed);
        file.bodies += 1;
        for (k, leaf) in leaves(&pack, body.shape)?.into_iter().enumerate() {
            let e = world.spawn();
            world.insert(e, transform);
            world.insert(e, data.clone());
            world.insert(
                e,
                Source(format!("{label}:{:?}:body{n}:{k}", String::from_utf8_lossy(name))),
            );
            report.leaves += 1;
            let p = |v| frame.point(moved(v, leaf.offset));
            let shape = match leaf.shape {
                Shape::ExtendedMesh(mesh) => {
                    let (soup, flags) = soup(&mesh, leaf.offset, frame, surfaces, at)?;
                    let triangles = soup.triangles.len();
                    for (&s, &f) in soup.surfaces.iter().zip(&flags) {
                        *report.surface_triangles.entry(s).or_default() += 1;
                        *report.flag_triangles.entry(f).or_default() += 1;
                    }
                    if fixed {
                        report.fixed_triangles += triangles;
                        file.fixed_triangles += triangles;
                        world.insert(e, soup);
                    } else {
                        report.dynamic_triangles += triangles;
                        file.dynamic_triangles += triangles;
                        world.insert(e, DynamicShape::Soup(soup));
                    }
                    world.insert(e, CollisionFlags(flags));
                    continue;
                }
                Shape::ConvexVertices(c) => {
                    report.convex += 1;
                    report.max_radius = report.max_radius.max(c.radius);
                    CoreShape::Convex {
                        points: c.vertices.iter().map(|&v| p(v)).collect(),
                    }
                }
                Shape::Box { radius, half_extents } => {
                    report.max_radius = report.max_radius.max(radius);
                    // a frame permutes (and may negate) the axes the half extents run along
                    CoreShape::Box {
                        center: p([0.0; 3]),
                        half_extents: frame.direction(half_extents).map(|h| scaled(frame, h.abs())),
                    }
                }
                Shape::Sphere { radius } => CoreShape::Sphere {
                    center: p([0.0; 3]),
                    radius: scaled(frame, radius),
                },
                Shape::Capsule { radius, a, b } => {
                    report.max_radius = report.max_radius.max(radius);
                    CoreShape::Capsule {
                        a: p(a),
                        b: p(b),
                        radius: scaled(frame, radius),
                    }
                }
                Shape::Cylinder {
                    radius,
                    cylinder_radius,
                    a,
                    b,
                    ..
                } => {
                    report.max_radius = report.max_radius.max(radius);
                    CoreShape::Cylinder {
                        a: p(a),
                        b: p(b),
                        radius: scaled(frame, cylinder_radius),
                    }
                }
                Shape::Mopp { .. } | Shape::ConvexTranslate { .. } | Shape::List { .. } => {
                    return Err(invalid(at, "a container shape reached as a leaf"));
                }
            };
            report.primitives += 1;
            let collider = Collider { shape, surface: 0 };
            if fixed {
                world.insert(e, collider);
            } else {
                world.insert(e, DynamicShape::Primitive(collider));
            }
        }
    }
    Ok(file)
}
