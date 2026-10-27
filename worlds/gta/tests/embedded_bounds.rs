//! Props' embedded collision against CodeWalker's reading of the same bounds.
//!
//! `oracles.codewalker.gta/bounds` is a folder of thin dumps of CodeWalker's reading of props' bounds (a ydr's own, a
//! yft's first physics LOD's): per leaf bound in order its type and the composite flags it was placed with, and per
//! polygon its type, its material's type and (triangles) its vertices placed by CodeWalker (the geometry's centre and
//! the child's transform). Each prop is read the way a conversion reads it (one prototype, `attach_assets`) and lowered
//! by `lower-gta-embedded-collision`.
//!
//! Checked: the prototype's collision is CodeWalker's triangles in order, each placed within 0.1 mm (the core frame
//! `C (x, y, z) = (x, z, -y)` of CodeWalker's position) and made of its material's name in `materials.dat`; the
//! triangles between them are the tessellations of the other polygons and primitive bounds, as many as each makes, a
//! box polygon's through CodeWalker's four placed corners and a box, disc, capsule or cylinder bound's the eight
//! corners of its box placed by CodeWalker; the foliage and river leaves are left out.
#![allow(clippy::unwrap_used)]

mod inputs;

use inputs::{Game, OracleSet, install, keys, oracle};
use primase_core::pass::{Pass, PassLog};
use primase_core::{Collision, Instance, Surfaces, World};
use primase_gta::components::GtaArchetype;
use primase_gta::components::collision::GtaMaterials;
use primase_gta::passes::collision::{FlaggedVolumes, LowerGtaEmbeddedCollision};
use primase_gta::read::assets::attach_assets;
use primase_gta::read::collision::material_names;
use rage_engine_formats::archive::archive_set::{ArchiveSet, Install, MapState};
use rage_engine_formats::archive::keys::Keys;
use rage_engine_formats::map::{ArchetypeDef, ArchetypeKind};
use serde_json::Value as Json;

/// How many triangles our lowering makes of a polygon or primitive bound that is no triangle.
fn tessellated(kind: &str) -> usize {
    match kind {
        "Box" | "Disc" => 12,
        "Sphere" => 32,
        // an 8-sided tube and two spheres
        "Capsule" => 16 + 2 * 32,
        // an 8-sided tube and its caps
        "Cylinder" => 16 + 16,
        "Cloth" => 0,
        k => panic!("no tessellation of a {k}"),
    }
}

/// Every one of CodeWalker's `points` (GTA's frame) is a corner of the `n` triangles from `at` (core frame), within
/// 0.1 mm, and those triangles have `corners` distinct corners.
fn corners_in(c: &Collision, at: usize, n: usize, points: &Json, corners: usize, what: &str) {
    let mut ours: Vec<[f32; 3]> = Vec::new();
    for t in &c.triangles[at..at + n] {
        for &k in t {
            let p = c.positions[k as usize];
            if !ours.iter().any(|q| (0..3).all(|i| (p[i] - q[i]).abs() < 1e-4)) {
                ours.push(p);
            }
        }
    }
    assert_eq!(ours.len(), corners, "{what}: corners");
    for v in points.as_array().unwrap() {
        let g: [f32; 3] = std::array::from_fn(|i| v[i].as_f64().unwrap() as f32);
        let want = [g[0], g[2], -g[1]];
        assert!(
            ours.iter().any(|q| (0..3).all(|i| (q[i] - want[i]).abs() < 1e-4)),
            "{what}: CodeWalker's corner {want:?} not among ours {ours:?}"
        );
    }
}

#[test]
fn props_collide_by_codewalkers_reading_of_their_bounds() {
    let (Some(dir), Some(keys), Some(dumps)) = (
        install(Game::Gta),
        keys(Game::Gta),
        oracle(OracleSet::CodeWalker, Game::Gta, "bounds"),
    ) else {
        return;
    };
    let keys = Keys::from_dir(&keys, Some(&primase_gta::read::key_digests::DIGESTS)).unwrap();
    let set = ArchiveSet::build(Install::open(&dir, &keys).unwrap(), &keys, MapState::Story).unwrap();
    let names = material_names(&String::from_utf8_lossy(
        &set.install
            .binary(set.live("dat", "materials").unwrap(), &keys)
            .unwrap(),
    ))
    .unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&dumps)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no dumps in {}", dumps.display());
    for file in files {
        let oracle: Json = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        let path = oracle["path"].as_str().unwrap();
        let file_name = path.rsplit('\\').next().unwrap();
        let (stem, ext) = file_name.rsplit_once('.').unwrap();
        let live = set.path(set.live(ext, stem).unwrap()).unwrap();
        assert!(live.eq_ignore_ascii_case(path), "{stem}: live copy {live}");
        let mut w = World::new();
        w.insert_resource(GtaMaterials(names.clone()));
        let proto = w.spawn();
        let def = ArchetypeDef {
            kind: ArchetypeKind::Base,
            name: primase_foundations::hash::joaat(stem.as_bytes()),
            lod_dist: 100.0,
            flags: 0,
            special_attribute: 0,
            bb_min: [0.0; 3],
            bb_max: [0.0; 3],
            bs_centre: [0.0; 3],
            bs_radius: 0.0,
            hd_texture_dist: 0.0,
            texture_dictionary: 0,
            clip_dictionary: 0,
            drawable_dictionary: 0,
            physics_dictionary: 0,
            asset_type: if ext == "yft" { 1 } else { 2 },
            asset_name: primase_foundations::hash::joaat(stem.as_bytes()),
            time_flags: None,
            extensions: 0,
            interior: None,
        };
        w.insert(proto, GtaArchetype::new("test", &def));
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let report = attach_assets(&mut w, &set, &keys).unwrap();
        assert_eq!(report.embedded_bounds, 1, "{stem}: its bound");
        // flagged foliage and river leaves left out, as the leaf loop below skips them
        LowerGtaEmbeddedCollision {
            volumes: FlaggedVolumes::LEAVE_OUT,
        }
        .run(&mut w, &mut PassLog::default())
        .unwrap();
        let c = w.get::<Collision>(proto).expect("collision");
        let surfaces = &w.resource::<Surfaces>().unwrap().0;

        let (mut at, mut triangles, mut tessellations) = (0usize, 0usize, 0usize);
        for (l, leaf) in oracle["leaves"].as_array().unwrap().iter().enumerate() {
            let flags = leaf["flags"].as_u64().unwrap();
            if flags & (1 << 19 | 1 << 27) != 0 {
                continue;
            }
            let Some(polygons) = leaf["polygons"].as_array() else {
                let kind = leaf["type"].as_str().unwrap();
                let n = tessellated(kind);
                // a box, disc, capsule or cylinder bound is its box, placed: CodeWalker's eight corners
                if kind != "Sphere" && kind != "Cloth" {
                    corners_in(c, at, n, &leaf["corners"], 8, &format!("{stem} leaf {l}"));
                }
                at += n;
                tessellations += n;
                continue;
            };
            for (k, poly) in polygons.iter().enumerate() {
                let kind = poly["type"].as_str().unwrap();
                if kind != "Triangle" {
                    let n = tessellated(kind);
                    if kind == "Box" {
                        corners_in(
                            c,
                            at,
                            n,
                            &poly["vertices"],
                            8,
                            &format!("{stem} leaf {l} polygon {k}"),
                        );
                    }
                    at += n;
                    tessellations += n;
                    continue;
                }
                let t = c
                    .triangles
                    .get(at)
                    .unwrap_or_else(|| panic!("{stem} leaf {l} polygon {k}: no triangle"));
                for (corner, v) in t.iter().zip(poly["vertices"].as_array().unwrap()) {
                    let g: [f32; 3] = std::array::from_fn(|i| v[i].as_f64().unwrap() as f32);
                    let want = [g[0], g[2], -g[1]];
                    let ours = c.positions[*corner as usize];
                    assert!(
                        (0..3).all(|i| (ours[i] - want[i]).abs() < 1e-4),
                        "{stem} leaf {l} polygon {k}: {ours:?}, CodeWalker {want:?}"
                    );
                }
                let material = usize::try_from(poly["material"].as_i64().unwrap()).unwrap();
                assert_eq!(
                    surfaces[usize::from(c.surfaces[at])].name,
                    names[material],
                    "{stem} leaf {l} polygon {k}: surface"
                );
                at += 1;
                triangles += 1;
            }
        }
        assert_eq!(at, c.triangles.len(), "{stem}: triangles");
        println!(
            "{stem}: {} triangles, {triangles} of them CodeWalker's, {tessellations} tessellated",
            c.triangles.len()
        );
    }
}
