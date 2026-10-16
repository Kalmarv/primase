//! A region read from the install against an independent exporter's scene of the same region, by meaning.
//!
//! Oracle (`PRIMASE_GTA_SCENE`): a skate-scene folder whose manifest's `region` is `[x0, y0, x1, y1]` in GTA metres,
//! exported from the same install with GTA Online's map; read with `primase-skate-scene`. Placements are matched by
//! archetype (the joaat of the scene prototype's name) and position (1 cm); a match compares orientation and scale (1e-5 per transform column component),
//! tint (the instance variant) and priority (flag bits 8-11 against `priorityLevel`). Placements of ours the scene
//! leaves out are counted by LOD level and by whether the pivot is inside the region (the exporter's selection is
//! not a pass yet); scene placements without a match fail the gate.
//!
//! Geometry: the matched placements' drawables are attached ([`attach_assets`]) and, per GTA geometry (the scene's
//! parts carry their source model and geometry; a geometry the exporter split into several parts is gathered), the
//! scene's triangles are compared with ours by position (0.1 mm, winding-agnostic). Named class: decal shaders'
//! geometry the exporter lifted 12 mm along each vertex's drawn face (the normalised sum of its triangles' face
//! normals; its `--decal-face-lift`), matched within 0.2 mm; not a pass yet. Any other difference fails the gate.
//! Fragments (yft) are not read yet and are counted.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use std::collections::BTreeMap;

use primase_core::Mesh;
use primase_core::state::Condition;
use primase_core::{Instance, Name, Transform, World};
use primase_gta::assets::attach_assets;
use primase_gta::components::{GtaArchetype, GtaDrawable, GtaEntity};
use primase_gta::region::{Region, States, archive_sets, read_region};
use primase_gta::{Install, Keys};
use primase_skate_scene::{SkatePlacement, SkatePrototype};

/// Position key: centimetres.
fn key(t: &Transform) -> [i64; 3] {
    t.cols[3].map(|v| (f64::from(v) * 100.0).round() as i64)
}

#[test]
fn region_placements_against_the_scene() {
    let (Some(dir), Some(key_dir), Some(scene)) = (
        primase_testdata::real_data("PRIMASE_GTA_DIR"),
        primase_testdata::real_data("PRIMASE_GTA_KEYS"),
        primase_testdata::real_data("PRIMASE_GTA_SCENE"),
    ) else {
        return;
    };
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(std::path::Path::new(&scene).join("scene.json")).expect("manifest"),
    )
    .expect("manifest json");
    let r: Vec<f32> = manifest["region"]
        .as_array()
        .expect("region")
        .iter()
        .map(|v| v.as_f64().expect("number") as f32)
        .collect();
    let region = Region {
        min: [r[0], r[1]],
        max: [r[2], r[3]],
        reach: 600.0,
    };
    let keys = Keys::from_dir(&key_dir).expect("key material");
    let install = Install::open(&dir, &keys).expect("install");
    let (story, online) = archive_sets(&install, &keys, States::All).expect("archive sets");
    let (ours, report) = read_region(&story, online.as_ref(), &keys, &region).expect("region");
    println!("{report:?}");

    let mut theirs = World::new();
    primase_skate_scene::read(std::path::Path::new(&scene), &mut theirs).expect("scene");

    // ours, in GTA Online's map: placements without a condition or with one allowing value 1
    let mut index: BTreeMap<(u32, [i64; 3]), Vec<primase_core::Entity>> = BTreeMap::new();
    for (e, _) in ours.iter::<Instance>() {
        if ours
            .get::<Condition>(e)
            .is_some_and(|c| c.0.iter().all(|cl| cl.0.iter().any(|l| l.values & 2 == 0)))
        {
            continue;
        }
        let archetype = ours.get::<GtaEntity>(e).expect("entity").archetype;
        let t = ours.get::<Transform>(e).expect("transform");
        index.entry((archetype, key(t))).or_default().push(e);
    }
    let total_ours: usize = index.values().map(Vec::len).sum();
    let mut matched = std::collections::BTreeSet::new();
    let mut unmatched = Vec::new();
    let mut diffs: BTreeMap<&str, usize> = BTreeMap::new();
    let mut max_col_err = 0f64;
    let mut scene_count = 0;
    for (e, inst) in theirs.iter::<Instance>() {
        if theirs.get::<SkatePlacement>(e).is_none() {
            continue;
        }
        scene_count += 1;
        let name = theirs
            .get::<Name>(inst.prototype)
            .map_or(String::new(), |n| n.0.clone());
        let t = theirs.get::<Transform>(e).expect("transform");
        let Some(&ours_e) = index
            .get(&(primase_hash::joaat(name.to_lowercase().as_bytes()), key(t)))
            .and_then(|v| v.iter().find(|x| !matched.contains(*x)))
        else {
            unmatched.push(format!("{name} at {:?}", t.cols[3]));
            continue;
        };
        matched.insert(ours_e);
        let ot = ours.get::<Transform>(ours_e).expect("transform");
        let err = (0..3)
            .flat_map(|c| (0..3).map(move |i| (c, i)))
            .map(|(c, i)| (f64::from(ot.cols[c][i]) - f64::from(t.cols[c][i])).abs())
            .fold(0f64, f64::max);
        max_col_err = max_col_err.max(err);
        if err > 1e-5 {
            *diffs.entry("orientation or scale").or_default() += 1;
        }
        let ge = ours.get::<GtaEntity>(ours_e).expect("entity");
        if ours.get::<Instance>(ours_e).expect("instance").variant != inst.variant
            && u64::from(ge.tint_value) != u64::from(inst.variant)
        {
            *diffs.entry("tint").or_default() += 1;
        }
        let flags = theirs.get::<SkatePlacement>(e).expect("placement").unknown_flags;
        if (flags >> 8) & 0xF != ge.priority_level as u32 {
            *diffs.entry("priority").or_default() += 1;
        }
    }
    // ours the scene left out
    let mut left: BTreeMap<String, usize> = BTreeMap::new();
    for e in index.values().flatten().filter(|e| !matched.contains(*e)) {
        let ge = ours.get::<GtaEntity>(*e).expect("entity");
        let p = ours.get::<Transform>(*e).expect("transform").cols[3];
        // world frame: x east, -z north
        let inside =
            p[0] >= region.min[0] && p[0] < region.max[0] && -p[2] >= region.min[1] && -p[2] < region.max[1];
        *left
            .entry(format!(
                "lod {} {}",
                ge.lod_level,
                if inside { "inside" } else { "outside" }
            ))
            .or_default() += 1;
    }
    println!(
        "scene {scene_count} placements, ours {total_ours} (Online); matched {}, scene-only {}; on matches: {diffs:?}, \
         largest column difference {max_col_err:e}",
        matched.len(),
        unmatched.len()
    );
    println!("ours only: {left:?}");
    for u in unmatched.iter().take(15) {
        println!("    scene only: {u}");
    }
    // the matched placements' drawables and textures
    let mut drawn = ours.subset(&ours.closure(matched.iter().copied()));
    let assets = attach_assets(&mut drawn, online.as_ref().expect("online"), &keys).expect("assets");
    println!("assets of the matched placements: {assets:?}");

    // geometry: per GTA geometry, the scene's triangles (all parts made from it) against ours, by rounded positions
    // and winding-agnostic
    primase_gltf::load_mesh_files(&mut theirs, std::path::Path::new(&scene)).expect("scene meshes");
    let mut protos: BTreeMap<u32, (&Mesh, &GtaDrawable)> = BTreeMap::new();
    for (e, a) in drawn.iter::<GtaArchetype>() {
        if let (Some(m), Some(d)) = (drawn.get::<Mesh>(e), drawn.get::<GtaDrawable>(e)) {
            protos.insert(a.name, (m, d));
        }
    }
    let tris = |m: &Mesh, prims: &[usize]| -> BTreeMap<[[i64; 3]; 3], usize> {
        let mut out = BTreeMap::new();
        for &i in prims {
            let p = &m.primitives[i];
            let pos = p.positions().expect("positions");
            let idx: Vec<u32> = p
                .indices
                .clone()
                .unwrap_or_else(|| (0..pos.len() as u32).collect());
            for t in idx.as_chunks::<3>().0 {
                let mut k =
                    [0usize, 1, 2].map(|j| pos[t[j] as usize].map(|v| (f64::from(v) * 1e4).round() as i64));
                k.sort();
                *out.entry(k).or_default() += 1;
            }
        }
        out
    };
    let mut geo: BTreeMap<&str, usize> = BTreeMap::new();
    let mut examples = Vec::new();
    for (e, sp) in theirs.iter::<SkatePrototype>() {
        let name = theirs.get::<Name>(e).map_or(String::new(), |n| n.0.clone());
        let Some(&(ours_mesh, d)) = protos.get(&primase_hash::joaat(name.to_lowercase().as_bytes())) else {
            *geo.entry("prototypes not compared (no drawable)").or_default() += 1;
            continue;
        };
        let Some(their_mesh) = theirs.get::<Mesh>(e) else {
            *geo.entry("scene prototypes without geometry").or_default() += 1;
            continue;
        };
        let mut groups: BTreeMap<(u64, u64), Vec<usize>> = BTreeMap::new();
        for (i, part) in sp.parts.iter().enumerate() {
            let src = part.source.as_ref().expect("part source");
            groups
                .entry((
                    src["model"].as_u64().expect("model"),
                    src["geometry"].as_u64().expect("geometry"),
                ))
                .or_default()
                .push(i);
        }
        *geo.entry("prototypes compared").or_default() += 1;
        for ((model, g), parts) in groups {
            let first: u64 = d.models[0][..model as usize].iter().map(|&n| u64::from(n)).sum();
            let ours_t = tris(ours_mesh, &[(first + g) as usize]);
            let theirs_t = tris(their_mesh, &parts);
            *geo.entry("geometries compared").or_default() += 1;
            let missing: usize = theirs_t
                .iter()
                .map(|(k, n)| n.saturating_sub(*ours_t.get(k).unwrap_or(&0)))
                .sum();
            let dropped: usize = ours_t
                .iter()
                .map(|(k, n)| n.saturating_sub(*theirs_t.get(k).unwrap_or(&0)))
                .sum();
            let total = theirs_t.values().sum::<usize>();
            *geo.entry("scene triangles").or_default() += total;
            if missing == 0 {
                *geo.entry("geometries identical").or_default() += 1;
                *geo.entry("our triangles the scene leaves out").or_default() += dropped;
                continue;
            }
            // the exporter's decal lift: every scene triangle of a decal shader's geometry is one of ours, as stored
            // or with each vertex moved 12 mm along its drawn face (the normalised sum of its triangles' face normals),
            // within 0.2 mm
            let shader = sp.parts[parts[0]]
                .source
                .as_ref()
                .and_then(|v| v["shader"].as_str())
                .unwrap_or("?");
            let lifted = shader.contains("decal") && {
                let grid = |c: [f64; 3]| c.map(|v| (v / 0.005).floor() as i64);
                let mut cells: BTreeMap<[i64; 3], Vec<[[f64; 3]; 3]>> = BTreeMap::new();
                let op = &ours_mesh.primitives[(first + g) as usize];
                let pos = op.positions().expect("positions");
                // per vertex, the sum of its triangles' (unnormalised) face normals, normalised: the drawn face
                let idx = op.indices.as_ref().expect("indices");
                let mut faces = vec![[0f64; 3]; pos.len()];
                for t in idx.as_chunks::<3>().0 {
                    let v = [0, 1, 2].map(|j| pos[t[j] as usize].map(f64::from));
                    let n = cross(sub(v[1], v[0]), sub(v[2], v[0]));
                    for &k in t {
                        faces[k as usize] = add(faces[k as usize], n);
                    }
                }
                let faces: Vec<[f64; 3]> = faces
                    .iter()
                    .map(|n| {
                        let l = dot(*n, *n).sqrt();
                        if l > 0.0 { n.map(|x| x / l) } else { [0.0; 3] }
                    })
                    .collect();
                for t in idx.as_chunks::<3>().0 {
                    for side in [0.0, 0.012, -0.012] {
                        let w = [0, 1, 2].map(|j| {
                            let (p, n) = (pos[t[j] as usize].map(f64::from), faces[t[j] as usize]);
                            add(p, n.map(|x| x * side))
                        });
                        cells.entry(grid(centroid(w))).or_default().push(w);
                    }
                }
                let near = |a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]| {
                    a.iter()
                        .all(|p| b.iter().any(|q| dot(sub(*p, *q), sub(*p, *q)).sqrt() <= 2e-4))
                };
                parts.iter().all(|&i| {
                    let tp = &their_mesh.primitives[i];
                    let pos = tp.positions().expect("positions");
                    tp.indices
                        .as_ref()
                        .expect("indices")
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .all(|t| {
                            let v = [0, 1, 2].map(|j| pos[t[j] as usize].map(f64::from));
                            let c = grid(centroid(v));
                            (-1..=1).any(|dx| {
                                (-1..=1).any(|dy| {
                                    (-1..=1).any(|dz| {
                                        cells
                                            .get(&[c[0] + dx, c[1] + dy, c[2] + dz])
                                            .is_some_and(|l| l.iter().any(|w| near(&v, w)))
                                    })
                                })
                            })
                        })
                })
            };
            if lifted {
                *geo.entry("decal geometries lifted 12 mm along the drawn face (the exporter's decal lift, not a pass yet)").or_default() += 1;
            } else {
                *geo.entry("geometries differing").or_default() += 1;
                if examples.len() < 10 {
                    examples.push(format!(
                        "{name} model {model} geometry {g} ({shader}): {missing} of {total} triangles"
                    ));
                }
            }
        }
    }
    println!("geometry: {geo:?}");
    for x in &examples {
        println!("    differing: {x}");
    }
    assert!(!geo.contains_key("geometries differing"), "geometries differ");
    assert!(unmatched.is_empty() && diffs.is_empty());
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
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
fn centroid(v: [[f64; 3]; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| (v[0][i] + v[1][i] + v[2][i]) / 3.0)
}
