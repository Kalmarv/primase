//! Level collision: the base and layer `.hkw` Havok 7.1.0-r1 packfiles of the PAKs the program supplies, as core
//! collision on entities (`havok_formats::packfile` reads the files; layouts and censuses are its tables).
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
//! material word; that id and the remaining opaque bits identify a `shred.collision_surfaces` row. A convex-vertices shape is
//! `Shape::Convex` of its vertices (the stored planes are not exact faces); box, sphere, capsule and cylinder are
//! core's primitives, so nothing is tessellated. Havok's convex radius (a skin around every convex shape) is not
//! added to any of them. A `ConvexTranslate` offsets its child; a list and a MOPP tree hold their children. Only
//! meshes carry a surface: every other leaf is surface 0 (`default`).
use crate::{
    Error, Result,
    components::{CollisionSurface, CollisionSurfaces, DynamicShape, RigidBody as BodyData},
    invalid,
};
use primase_core::{
    Collider, Collision, Shape as CoreShape, Source, Surfaces, Transform, World, frame::SourceFrame,
};
use havok_formats::packfile::{
    Packfile,
    physics::{self, ExtendedMesh, RigidBody, Shape},
};
use rme_formats::{pak::Pak, surfaces::SurfaceTable};
use std::collections::{BTreeMap, BTreeSet};

/// The surface field of a triangle's material word: bits 3..=8.
pub const SURFACE_SHIFT: u32 = 3;
pub const SURFACE_MASK: u32 = 0x3F;
/// How deep a body's shape tree may go (the data's is 3).
const MAX_DEPTH: usize = 16;

/// The world's zero-flag `core.surfaces` rows named by the table, in id order.
fn to_world(table: &SurfaceTable) -> Result<Surfaces> {
    let mut s = Surfaces::default();
    for (i, name) in table.0.iter().enumerate() {
        if usize::from(s.index(name).map_err(|e| invalid(0, e))?) != i {
            return Err(invalid(
                0,
                format!("MAIN.DB surfaces: name {name:?} repeats (row {i})"),
            ));
        }
    }
    Ok(s)
}

/// The surfaces and their source identities the world holds after reading: the table's rows, then one row for each
/// (database id, flag word) pair in `pairs` the table's rows do not already carry.
fn catalog(
    table: &SurfaceTable,
    world: &World,
    pairs: BTreeSet<(u16, u32)>,
) -> Result<(Surfaces, CollisionSurfaces)> {
    let base = to_world(table)?;
    if table.0.len() > 64 {
        return Err(invalid(0, "MAIN.DB has more than 64 surface ids"));
    }
    let mut sources = CollisionSurfaces(
        table
            .0
            .iter()
            .enumerate()
            .map(|(id, name)| {
                (
                    id as u16,
                    CollisionSurface {
                        database_id: id as u16,
                        database_name: name.clone(),
                        flags: 0,
                    },
                )
            })
            .collect(),
    );
    let mut surfaces = match (
        world.resource::<Surfaces>(),
        world.resource::<CollisionSurfaces>(),
    ) {
        (None, None) => base,
        (Some(held), None) if *held == base => held.clone(),
        (Some(held), Some(origins)) => {
            // Reading into a lowered/renamed table would mix source and destination state.
            if held.0.len() != origins.0.len() || !held.0.starts_with(&base.0) {
                return Err(invalid(0, "existing collision surfaces are not the source table"));
            }
            for (i, surface) in held.0.iter().enumerate() {
                let source = origins
                    .0
                    .get(&(i as u16))
                    .ok_or_else(|| invalid(0, "existing collision surface has no source identity"))?;
                if table.0.get(usize::from(source.database_id)) != Some(&source.database_name)
                    || *surface != primase_core::Surface::new(surface_name(source))
                {
                    return Err(invalid(
                        0,
                        "existing collision surface identity differs from MAIN.DB",
                    ));
                }
            }
            primase_core::Resource::verify(origins, world).map_err(|e| invalid(0, e))?;
            sources = origins.clone();
            held.clone()
        }
        _ => {
            return Err(invalid(
                0,
                "the world's core.surfaces is not MAIN.DB's source table",
            ));
        }
    };
    let known: BTreeSet<_> = sources.0.values().map(|s| (s.database_id, s.flags)).collect();
    for (id, flags) in pairs.difference(&known).copied() {
        let name = table
            .0
            .get(usize::from(id))
            .ok_or_else(|| invalid(0, "material surface past MAIN.DB"))?;
        let source = CollisionSurface {
            database_id: id,
            database_name: name.clone(),
            flags,
        };
        let name = surface_name(&source);
        if surfaces.0.iter().any(|s| s.name == name) {
            return Err(invalid(0, format!("collision surface name {name:?} repeats")));
        }
        let index = surfaces.index(&name).map_err(|e| invalid(0, e))?;
        sources.0.insert(index, source);
    }
    Ok((surfaces, sources))
}

fn surface_name(source: &CollisionSurface) -> String {
    if source.flags == 0 {
        source.database_name.clone()
    } else {
        format!("{} [shred-flags=0x{:08x}]", source.database_name, source.flags)
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
    /// Triangles per source material identity (database surface id, opaque flag word).
    pub material_triangles: BTreeMap<(u16, u32), usize>,
    /// The largest convex radius over the box, convex-vertices, capsule and cylinder shapes read (never added).
    pub max_radius: f32,
}

impl CollisionReport {
    /// The log lines of a collision read.
    pub fn lines(&self) -> primase_core::convert::Report {
        vec![(
            "shred collision".into(),
            format!(
                "{} .hkw; {} bodies ({} fixed); {} leaf shapes ({} convex, {} other primitives); {} fixed + {} dynamic triangles",
                self.files.len(),
                self.bodies,
                self.fixed_bodies,
                self.leaves,
                self.convex,
                self.primitives - self.convex,
                self.fixed_triangles,
                self.dynamic_triangles
            ),
        )]
    }
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

/// The extended mesh as one soup in the target frame. Surface indices carry the complete source material identity.
fn soup(
    mesh: &ExtendedMesh,
    offset: [f32; 3],
    frame: SourceFrame,
    catalog: &BTreeMap<(u16, u32), u16>,
    at: usize,
) -> Result<Collision> {
    let mut soup = Collision::default();
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
            let flags = a & !(SURFACE_MASK << SURFACE_SHIFT);
            let index = catalog
                .get(&(surface as u16, flags))
                .ok_or_else(|| invalid(at, "material identity absent from source surface table"))?;
            let mut t = tri.map(|i| i + base);
            if frame.flips_winding() {
                t.swap(1, 2);
            }
            soup.triangles.push(t);
            soup.surfaces.push(*index);
        }
    }
    Ok(soup)
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
    // Census used pairs first so new indices are sorted, independent of archive enumeration order. No policy
    // enters here: only identities actually selected by source triangle material indices get additional rows.
    let mut pairs = BTreeSet::new();
    for &(_, pak) in paks {
        for (i, entry) in pak.entries().iter().enumerate() {
            if !entry.name.ends_with(b".hkw") {
                continue;
            }
            let payload = pak.payload(i)?;
            let pack = Packfile::read(&payload).map_err(|e| invalid(0, e))?;
            for at in physics::rigid_bodies(&pack) {
                let body = RigidBody::read(&pack, at).map_err(|e| invalid(at, e))?;
                for leaf in leaves(&pack, body.shape)? {
                    if let Shape::ExtendedMesh(mesh) = leaf.shape {
                        for sub in mesh.subparts {
                            for m in sub.material_indices {
                                let a = sub.materials[usize::from(m)].a;
                                pairs.insert((
                                    ((a >> SURFACE_SHIFT) & SURFACE_MASK) as u16,
                                    a & !(SURFACE_MASK << SURFACE_SHIFT),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    let (table, origins) = catalog(surfaces, world, pairs)?;
    let catalog = origins
        .0
        .iter()
        .map(|(&i, s)| ((s.database_id, s.flags), i))
        .collect();
    world.insert_resource(table);
    world.insert_resource(origins);
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
                &catalog,
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
    catalog: &BTreeMap<(u16, u32), u16>,
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
                    // Counts remain database identities, not the expanded core surface indices.
                    for sub in &mesh.subparts {
                        for &m in &sub.material_indices {
                            let a = sub.materials[usize::from(m)].a;
                            let id = ((a >> SURFACE_SHIFT) & SURFACE_MASK) as u16;
                            let flags = a & !(SURFACE_MASK << SURFACE_SHIFT);
                            *report.surface_triangles.entry(id).or_default() += 1;
                            *report.flag_triangles.entry(flags).or_default() += 1;
                            *report.material_triangles.entry((id, flags)).or_default() += 1;
                        }
                    }
                    let soup = soup(&mesh, leaf.offset, frame, catalog, at)?;
                    let triangles = soup.triangles.len();
                    if fixed {
                        report.fixed_triangles += triangles;
                        file.fixed_triangles += triangles;
                        world.insert(e, soup);
                    } else {
                        report.dynamic_triangles += triangles;
                        file.dynamic_triangles += triangles;
                        world.insert(e, DynamicShape::Soup(soup));
                    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use havok_formats::packfile::physics::{MeshMaterial, Subpart};

    #[test]
    fn material_indices_select_distinct_pairs_and_repeated_pairs_share_a_surface() {
        let table = SurfaceTable(vec!["default".into(), "hazard".into()]);
        let pairs = BTreeSet::from([(0, 0x1000), (0, 1)]);
        let (surfaces, origins) = catalog(&table, &World::new(), pairs).unwrap();
        assert_eq!(
            surfaces.0.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            [
                "default",
                "hazard",
                "default [shred-flags=0x00000001]",
                "default [shred-flags=0x00001000]",
            ]
        );
        let catalog = origins
            .0
            .iter()
            .map(|(&i, s)| ((s.database_id, s.flags), i))
            .collect();
        let mesh = ExtendedMesh {
            aabb_half_extents: [1.0; 3],
            aabb_center: [0.0; 3],
            welding: vec![0; 3],
            subparts: vec![Subpart {
                vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                triangles: vec![[0, 1, 2]; 3],
                material_indices: vec![1, 0, 1],
                triangle_offset: 0,
                materials: vec![
                    MeshMaterial {
                        a: 1,
                        b: 0x3f80,
                        c: 0,
                    },
                    MeshMaterial {
                        a: 0x1000,
                        b: 0x3f80,
                        c: 0,
                    },
                ],
            }],
        };
        let c = soup(&mesh, [0.0; 3], SourceFrame::WORLD, &catalog, 0).unwrap();
        assert_eq!(c.surfaces, [3, 2, 3]);
        assert_eq!(
            c.surfaces.iter().map(|i| origins.0[i].flags).collect::<Vec<_>>(),
            [0x1000, 1, 0x1000]
        );
        let mut world = World::new();
        world.insert_resource(surfaces);
        world.insert_resource(origins.clone());
        let e = world.spawn();
        world.insert(e, DynamicShape::Soup(c));
        assert!(world.verify_components().is_empty());
        assert_eq!(
            world.subset(&BTreeSet::from([e])).resource::<CollisionSurfaces>(),
            Some(&origins)
        );
    }

    #[test]
    fn additional_reads_append_sorted_new_pairs_without_changing_existing_indices() {
        let table = SurfaceTable(vec!["default".into()]);
        let (surfaces, origins) = catalog(&table, &World::new(), BTreeSet::from([(0, 0x1000)])).unwrap();
        let mut world = World::new();
        world.insert_resource(surfaces);
        world.insert_resource(origins);
        let (surfaces, origins) = catalog(&table, &world, BTreeSet::from([(0, 1), (0, 0x1000)])).unwrap();
        assert_eq!(surfaces.0.len(), 3);
        assert_eq!(origins.0[&1].flags, 0x1000);
        assert_eq!(origins.0[&2].flags, 1);
        world.resource_mut::<Surfaces>().unwrap().0[0].name = "renamed".into();
        assert!(catalog(&table, &world, BTreeSet::new()).is_err());
    }

    #[test]
    fn authored_names_cannot_alias_generated_surface_names() {
        let table = SurfaceTable(vec!["default".into(), "default [shred-flags=0x00000001]".into()]);
        assert!(catalog(&table, &World::new(), BTreeSet::from([(0, 1)])).is_err());
    }
}
