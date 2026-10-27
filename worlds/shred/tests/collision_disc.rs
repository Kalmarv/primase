//! The disc's level collision through `read_collision`: counts and surface histograms equal the offline recon's
//! (`HK.md`, a separate census: 646,081 triangles, 36 surface ids), the flag words equal its census, the
//! winding is counter-clockwise-front (the topmost near-horizontal triangles of the fixed meshes face up), and the
//! collision sits on the render geometry of the same level (the frame check).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use havok_formats::packfile::{
    Packfile,
    physics::{self, RigidBody as StoredBody, Shape as StoredShape},
};
use inputs::{Game, Query, expect_found, install, install_file};
use primase_core::{
    Collider, Collision, Instance, Mesh, Shape, Source, Transform, World,
    frame::{Axis, SourceFrame},
};
use primase_shred::{
    components::{CollisionSurfaces, DynamicShape, RigidBody},
    read::{
        collision::{self, CollisionReport},
        level,
    },
};
use rme_formats::{pak::Pak, surfaces::SurfaceTable};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn paks(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = e.expect("directory entry").path();
        if path.is_dir() {
            paks(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
            out.push(path);
        }
    }
}

fn all_paks(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    paks(&root.join("PS3_GAME/USRDIR"), &mut out);
    out.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", out.len());
    out
}

fn surfaces() -> Option<SurfaceTable> {
    let db = install_file(Game::Shred, "PS3_GAME/USRDIR/database/main.db")?;
    Some(SurfaceTable::from_main_db(&std::fs::read(db).unwrap()).unwrap())
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().to_uppercase()
}

/// A PAK's collision in a world of its own.
fn read(path: &Path, table: &SurfaceTable, frame: SourceFrame) -> (World, CollisionReport) {
    let pak = Pak::open(path).unwrap();
    let mut world = World::new();
    let report = collision::read_collision(&mut world, &[(&stem(path), &pak)], table, frame)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    (world, report)
}

fn world_point(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|k| t.cols[0][k] * p[0] + t.cols[1][k] * p[1] + t.cols[2][k] * p[2] + t.cols[3][k])
}

/// Check the association against the original Havok material-index sequences, before surface expansion. This
/// exercises the mapping; it is not an independent oracle for the Havok decoder (the hand-worked fixture is).
fn assert_material_sequences(path: &Path, world: &World) {
    let pak = Pak::open(path).unwrap();
    let mut expected = BTreeMap::new();
    for (i, entry) in pak.entries().iter().enumerate() {
        if !entry.name.ends_with(b".hkw") {
            continue;
        }
        let payload = pak.payload(i).unwrap();
        let pack = Packfile::read(&payload).unwrap();
        for (body_index, at) in physics::rigid_bodies(&pack).into_iter().enumerate() {
            let body = StoredBody::read(&pack, at).unwrap();
            let mut stack = vec![body.shape];
            let mut leaf_index = 0;
            while let Some(at) = stack.pop() {
                match StoredShape::read(&pack, at).unwrap() {
                    StoredShape::Mopp { child } | StoredShape::ConvexTranslate { child, .. } => {
                        stack.push(child)
                    }
                    StoredShape::List { children } => stack.extend(children.into_iter().rev()),
                    leaf => {
                        if let StoredShape::ExtendedMesh(mesh) = leaf {
                            let pairs: Vec<_> = mesh
                                .subparts
                                .iter()
                                .flat_map(|sub| {
                                    sub.material_indices.iter().map(|&m| {
                                        let a = sub.materials[usize::from(m)].a;
                                        (((a >> 3) & 0x3f) as u16, a & !0x1f8)
                                    })
                                })
                                .collect();
                            let key = format!(
                                "{}:{:?}:body{body_index}:{leaf_index}",
                                stem(path),
                                String::from_utf8_lossy(&entry.name)
                            );
                            assert!(expected.insert(key, pairs).is_none());
                        }
                        leaf_index += 1;
                    }
                }
            }
        }
    }
    let origins = world.resource::<CollisionSurfaces>().unwrap();
    let mut soups: BTreeMap<_, _> = world.iter::<Collision>().collect();
    soups.extend(world.iter::<DynamicShape>().filter_map(|(e, shape)| match shape {
        DynamicShape::Soup(c) => Some((e, c)),
        DynamicShape::Primitive(_) => None,
    }));
    for (e, soup) in soups {
        let key = &world.get::<Source>(e).unwrap().0;
        let original = expected.remove(key).unwrap();
        let recovered: Vec<_> = soup
            .surfaces
            .iter()
            .map(|i| {
                let s = &origins.0[i];
                (s.database_id, s.flags)
            })
            .collect();
        assert_eq!(recovered, original, "{}: {key}", path.display());
    }
    assert!(expected.is_empty(), "{}: omitted source mesh", path.display());
}

#[test]
fn counts_surfaces_and_flags_equal_the_recon() {
    let (Some(root), Some(table)) = (install(Game::Shred), surfaces()) else {
        return;
    };
    assert_eq!(table.0.len(), 54);
    assert_eq!(
        (table.0[0].as_str(), table.0[53].as_str()),
        ("default", "snow_Whistler")
    );
    let mut total = CollisionReport::default();
    let mut files = BTreeMap::new();
    for path in all_paks(&root) {
        let (world, r) = read(&path, &table, SourceFrame::WORLD);
        assert_material_sequences(&path, &world);
        let origins = world.resource::<CollisionSurfaces>().unwrap();
        let mut recovered = BTreeMap::new();
        for soup in world
            .iter::<Collision>()
            .map(|(_, c)| c)
            .chain(world.iter::<DynamicShape>().filter_map(|(_, s)| match s {
                DynamicShape::Soup(c) => Some(c),
                DynamicShape::Primitive(_) => None,
            }))
        {
            for index in &soup.surfaces {
                let source = &origins.0[index];
                *recovered.entry((source.database_id, source.flags)).or_default() += 1;
            }
        }
        assert_eq!(
            recovered,
            r.material_triangles,
            "{}: source material identity",
            path.display()
        );
        for (pair, count) in &recovered {
            *total.material_triangles.entry(*pair).or_default() += count;
        }
        assert!(
            world.verify_components().is_empty(),
            "{}: {:?}",
            path.display(),
            world.verify_components()
        );
        total.bodies += r.bodies;
        total.fixed_bodies += r.fixed_bodies;
        total.leaves += r.leaves;
        total.convex += r.convex;
        total.primitives += r.primitives;
        total.fixed_triangles += r.fixed_triangles;
        total.dynamic_triangles += r.dynamic_triangles;
        total.max_radius = total.max_radius.max(r.max_radius);
        for (k, v) in r.surface_triangles {
            *total.surface_triangles.entry(k).or_default() += v;
        }
        for (k, v) in r.flag_triangles {
            *total.flag_triangles.entry(k).or_default() += v;
        }
        // the world holds exactly what the report counts
        let soups = world.column::<Collision>().map_or(0, |c| c.len());
        let colliders = world.column::<Collider>().map_or(0, |c| c.len());
        let dynamic = world.column::<DynamicShape>().map_or(0, |c| c.len());
        assert_eq!(soups + colliders + dynamic, r.leaves);
        for f in r.files {
            let name = String::from_utf8_lossy(&f.entry).replace(char::from(92), "/");
            files.insert(
                format!("{}__{name}", f.pak),
                (f.bodies, f.fixed_triangles, f.dynamic_triangles),
            );
        }
    }
    eprintln!(
        "SHRED collision: {} bodies ({} fixed), {} leaves ({} convex, {} primitives), {} fixed + {} dynamic triangles, max convex radius {}",
        total.bodies,
        total.fixed_bodies,
        total.leaves,
        total.convex,
        total.primitives,
        total.fixed_triangles,
        total.dynamic_triangles,
        total.max_radius
    );
    // 490 files in the recon; the 8 ragdoll `.HKX` outside the PAKs are not level collision
    assert_eq!(files.len(), 482);
    assert_eq!((total.bodies, total.fixed_bodies), (12_641, 2_021));
    assert_eq!(
        (total.leaves, total.convex, total.primitives),
        (12_672, 8_756, 10_914)
    );
    assert_eq!(total.fixed_triangles + total.dynamic_triangles, 646_081);
    assert_eq!(total.dynamic_triangles, 0);
    assert_eq!(total.material_triangles.len(), 92);
    assert_eq!(total.material_triangles.values().sum::<usize>(), 646_081);
    eprintln!(
        "SHRED source material identities: {} pairs, {} flag words, 646081 triangle references recovered",
        total.material_triangles.len(),
        total.flag_triangles.len()
    );
    assert!(total.max_radius <= 0.06, "convex radius {}", total.max_radius);
    let histogram: Vec<(u16, usize)> = total.surface_triangles.into_iter().collect();
    assert_eq!(
        histogram,
        [
            (0, 456_551),
            (1, 23_297),
            (2, 2_274),
            (3, 71_881),
            (5, 149),
            (6, 601),
            (8, 1_712),
            (11, 830),
            (14, 4),
            (17, 74),
            (18, 4_005),
            (24, 2),
            (25, 401),
            (26, 54),
            (27, 2_504),
            (28, 242),
            (29, 2_318),
            (30, 3_268),
            (31, 2_053),
            (32, 7_785),
            (33, 436),
            (34, 257),
            (35, 97),
            (36, 2_791),
            (37, 2_372),
            (39, 94),
            (41, 2),
            (45, 402),
            (46, 1_771),
            (47, 7_314),
            (48, 15_652),
            (49, 2_760),
            (50, 59),
            (51, 430),
            (52, 11_241),
            (53, 20_398)
        ]
    );
    // the recon's census of the bits above the surface field, per triangle
    assert_eq!(
        total.flag_triangles,
        BTreeMap::from([
            (0x0, 564_228),
            (0x1, 19_408),
            (0x4, 8_827),
            (0x801, 4),
            (0x800, 10_201),
            (0x1000, 22_317),
            (0x2000, 1_529),
            (0x4000, 2_927),
            (0x4800, 48),
            (0x8000_0000, 15_856),
            (0x8000_0800, 10),
            (0x8000_4000, 17),
            (0x8000_6000, 709)
        ])
    );
    // the recon's per-file counts (bodies, fixed triangles) of the level bases and CHARLIE's training layer
    for (file, bodies, triangles) in [
        ("CHARLIE__levels/charlie/charlie.hkw", 1, 3_660),
        ("DELTA__levels/delta/delta.hkw", 75, 21_361),
        ("ECHO__levels/echo/echo.hkw", 961, 93_803),
        ("FOXTROT__levels/foxtrot/foxtrot.hkw", 489, 42_645),
        ("INDIA__levels/india/india.hkw", 898, 37_983),
        ("JULIET__levels/juliet/juliet.hkw", 1_101, 39_831),
        ("MIKE__levels/mike/mike.hkw", 621, 53_583),
        ("NOVEMBER__levels/november/november.hkw", 853, 39_452),
        ("OSCAR__levels/oscar/oscar.hkw", 475, 25_685),
        ("PAPA__levels/papa/papa.hkw", 764, 17_130),
        (
            "00000004__levels/charlie/trainingsession1/trainingsession1.hkw",
            165,
            6_206,
        ),
    ] {
        assert_eq!(files.get(file), Some(&(bodies, triangles, 0)), "{file}");
    }
}

#[test]
fn collision_winding_is_counter_clockwise_front() {
    let (Some(root), Some(table)) = (install(Game::Shred), surfaces()) else {
        return;
    };
    // The topmost near-horizontal triangle of each 2 m cell of a level's fixed meshes is an exterior surface: it
    // faces up in a counter-clockwise-front world. The cell's winner is chosen by height alone, never by its facing.
    let (mut up, mut down) = (0usize, 0usize);
    let mut large = 0;
    for path in all_paks(&root) {
        let (world, _) = read(&path, &table, SourceFrame::WORLD);
        let mut cells: BTreeMap<(String, i32, i32), (f32, bool)> = BTreeMap::new();
        for (e, soup) in world.iter::<Collision>() {
            let t = world.get::<Transform>(e).unwrap();
            let source = &world.get::<Source>(e).unwrap().0;
            let file = source.split(":body").next().unwrap().to_string();
            for tri in &soup.triangles {
                let [a, b, c] = tri.map(|i| world_point(t, soup.positions[i as usize]));
                let (u, v) = (
                    [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
                    [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
                );
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if len <= 0.05 || (n[1] / len).abs() <= 0.98 {
                    continue;
                }
                let y = (a[1] + b[1] + c[1]) / 3.0;
                let cell = (
                    file.clone(),
                    ((a[0] + b[0] + c[0]) / 6.0).floor() as i32,
                    ((a[2] + b[2] + c[2]) / 6.0).floor() as i32,
                );
                let slot = cells.entry(cell).or_insert((f32::NEG_INFINITY, false));
                if y > slot.0 {
                    *slot = (y, n[1] > 0.0);
                }
            }
        }
        let mut per_file: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for ((file, ..), (_, facing_up)) in &cells {
            let n = per_file.entry(file).or_default();
            if *facing_up {
                n.0 += 1;
            } else {
                n.1 += 1;
            }
        }
        for (file, (u, d)) in per_file {
            up += u;
            down += d;
            if u + d >= 200 {
                large += 1;
                assert!(u > d, "{file}: {u} cells face up, {d} face down");
            }
        }
    }
    eprintln!(
        "SHRED collision winding: topmost horizontal cells face up {up}, down {down}; {large} files with >= 200 cells"
    );
    // the recon's Python reader: 40,368 up and 4,103 down over 83 files, every file of >= 200 cells mostly up
    assert_eq!((up, down, large), (40_368, 4_103, 29));
}

#[test]
fn mike_collision_lies_on_mike_render_geometry() {
    let (Some(root), Some(table)) = (install(Game::Shred), surfaces()) else {
        return;
    };
    let path = root.join("PS3_GAME/USRDIR/levels/mike.pak");
    let pak = Pak::open(&path).unwrap();
    let (render, _) = level::read_level(&[("MIKE", &pak)], SourceFrame::WORLD).unwrap();
    // render surface points: every placed vertex, edge midpoint and centroid, in 0.25 m cells
    let mut grid: BTreeMap<(i32, i32, i32), Vec<[f32; 3]>> = BTreeMap::new();
    let cell = |p: [f32; 3]| {
        (
            (p[0] * 4.0).floor() as i32,
            (p[1] * 4.0).floor() as i32,
            (p[2] * 4.0).floor() as i32,
        )
    };
    for (e, inst) in render.iter::<Instance>() {
        let t = *render.get::<Transform>(e).unwrap();
        let Some(mesh) = render.get::<Mesh>(inst.prototype) else {
            continue;
        };
        for prim in mesh.primitives() {
            let (Some(pos), Some(idx)) = (prim.positions(), prim.indices.as_ref()) else {
                continue;
            };
            for tri in idx.as_chunks::<3>().0 {
                let [a, b, c] = [0, 1, 2].map(|k| world_point(&t, pos[tri[k] as usize]));
                let mid =
                    |p: [f32; 3], q: [f32; 3]| std::array::from_fn::<f32, 3, _>(|k| (p[k] + q[k]) / 2.0);
                let centroid = std::array::from_fn(|k| (a[k] + b[k] + c[k]) / 3.0);
                for p in [a, b, c, mid(a, b), mid(b, c), mid(c, a), centroid] {
                    grid.entry(cell(p)).or_default().push(p);
                }
            }
        }
    }
    let (world, report) = read(&path, &table, SourceFrame::WORLD);
    assert!(report.fixed_triangles > 0);
    let mut distances = Vec::new();
    for (e, soup) in world.iter::<Collision>() {
        let t = world.get::<Transform>(e).unwrap();
        for &v in &soup.positions {
            let p = world_point(t, v);
            let c = cell(p);
            let mut best = f32::INFINITY;
            // search out to 1 m: farther than that is "not on the render geometry"
            for dx in -4..=4 {
                for dy in -4..=4 {
                    for dz in -4..=4 {
                        for q in grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)).into_iter().flatten() {
                            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2))
                                .sqrt();
                            best = best.min(d);
                        }
                    }
                }
            }
            distances.push(best);
        }
    }
    distances.sort_by(f32::total_cmp);
    let q = |f: f64| distances[((distances.len() - 1) as f64 * f) as usize];
    eprintln!(
        "SHRED MIKE collision-to-render distance over {} vertices: median {:.4} m, 75% {:.4}, 86% {:.4}, 96% {:.4}",
        distances.len(),
        q(0.5),
        q(0.75),
        q(0.86),
        q(0.96)
    );
    assert!(q(0.5) < 0.01, "median {}", q(0.5));
}

#[test]
fn frames_move_points_and_flip_winding() {
    let (Some(root), Some(table)) = (install(Game::Shred), surfaces()) else {
        return;
    };
    let path = root.join("PS3_GAME/USRDIR/levels/mike.pak");
    let (identity, _) = read(&path, &table, SourceFrame::WORLD);
    let mirror = SourceFrame::new([Axis::NegX, Axis::PosY, Axis::PosZ], 2.0);
    assert!(mirror.flips_winding());
    let (mirrored, _) = read(&path, &table, mirror);
    let mut checked = (0, 0, 0);
    for ((e, a), (f, b)) in identity.iter::<Collision>().zip(mirrored.iter::<Collision>()) {
        assert_eq!((e, a.triangles.len()), (f, b.triangles.len()));
        assert_eq!(a.surfaces, b.surfaces);
        for (p, q) in a.positions.iter().zip(&b.positions) {
            assert_eq!(*q, mirror.point(*p));
        }
        for (s, t) in a.triangles.iter().zip(&b.triangles) {
            assert_eq!(*t, [s[0], s[2], s[1]]);
        }
        checked.0 += 1;
    }
    for ((_, a), (_, b)) in identity.iter::<Collider>().zip(mirrored.iter::<Collider>()) {
        match (&a.shape, &b.shape) {
            (Shape::Box { half_extents: h, .. }, Shape::Box { half_extents: g, .. }) => {
                assert_eq!(*g, [h[0] * 2.0, h[1] * 2.0, h[2] * 2.0]);
                checked.1 += 1;
            }
            (Shape::Sphere { radius: r, .. }, Shape::Sphere { radius: s, .. }) => {
                assert_eq!(*s, r * 2.0);
                checked.2 += 1;
            }
            _ => {}
        }
    }
    eprintln!(
        "SHRED collision frames: {} soups, {} boxes, {} spheres checked",
        checked.0, checked.1, checked.2
    );
    assert!(checked.0 > 0 && checked.1 > 0);
    // the same bodies carry their data under any frame
    let bodies = |w: &World| w.iter::<RigidBody>().map(|(_, b)| b.clone()).collect::<Vec<_>>();
    assert_eq!(bodies(&identity), bodies(&mirrored));
}
