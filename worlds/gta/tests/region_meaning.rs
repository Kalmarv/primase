//! A region read from the install against an independent exporter's scene of the same region, by meaning.
//!
//! Oracle (`PRIMASE_GTA_SCENE`): a skate-scene folder whose manifest's `region` is `[x0, y0, x1, y1]` in GTA metres,
//! exported from the same install with GTA Online's map; read with `primase-skate-scene`. Placements are matched by
//! archetype (the joaat of the scene prototype's name) and position (1 cm); a match compares orientation and scale (1e-5 per transform column component),
//! tint (the instance variant) and priority (flag bits 8-11 against `priorityLevel`). Before matching, ours go
//! through `gta-select-region` with that exporter's settings (hour 12, minimum size 0.5 m). Scene placements without
//! a match fail the gate, and so do placements of ours outside interiors the scene leaves out; entities of interiors
//! are counted (which interiors that exporter placed came from its own interior table, not a pass here).
//!
//! Geometry: the matched placements' drawables are attached ([`attach_assets`]) and, per GTA geometry (the scene's
//! parts carry their source model and geometry; a geometry the exporter split into several parts is gathered), the
//! scene's triangles are compared with ours by position (0.1 mm, winding-agnostic). Named class: decal shaders'
//! geometry the exporter lifted 12 mm along each vertex's drawn face (the normalised sum of its triangles' face
//! normals; its `--decal-face-lift`), matched within 0.2 mm; not a pass yet. Any other difference fails the gate.
//! On identical geometries every scene vertex must have one of ours at its position with its UV0 (1e-5), its normal
//! (1e-4, against ours normalised: stored normals are quantised; a stored zero normal the exporter writes up, named),
//! and, where the scene carries COLOR_0, its alpha. Textures: each scene texture's largest level must be one of our
//! attached textures' levels and its smaller levels must follow it (that exporter passed DXT blocks through and dropped
//! top levels to a size cap, counted).
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
use primase_gta::select::SelectRegion;
use primase_gta::{Install, Keys};
use primase_skate_scene::{SkatePlacement, SkatePrototype};
use sha2::{Digest, Sha256};

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
    let (mut ours, report) = read_region(&story, online.as_ref(), &keys, &region).expect("region");
    println!("{report:?}");
    // the earlier exporter's region rules, as a pass
    let select = SelectRegion {
        min: region.min,
        max: region.max,
        hour: 12,
        max_priority: 3,
        min_size: 0.5,
    };
    let mut log = primase_core::pass::PassLog::default();
    primase_core::pass::Pass::run(&select, &mut ours, &mut log).expect("select");
    println!("select: {:?}", log.counts);

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
                "{}lod {} {}",
                if ge.interior.is_some() {
                    "interior entity, "
                } else {
                    ""
                },
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
    let mut by_map: BTreeMap<&str, usize> = BTreeMap::new();
    for e in index.values().flatten().filter(|e| !matched.contains(*e)) {
        let ge = ours.get::<GtaEntity>(*e).expect("entity");
        if ge.interior.is_none() {
            *by_map.entry(ge.ymap.as_str()).or_default() += 1;
        }
    }
    println!("ours only outside interiors, by map: {by_map:?}");
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
    let mut attrs: BTreeMap<&str, usize> = BTreeMap::new();
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
                // vertex attributes: each scene vertex has one of ours at its position with its UV0, normal and
                // (where the scene carries COLOR_0) colour alpha
                let op = &ours_mesh.primitives[(first + g) as usize];
                let key = |p: [f32; 3]| p.map(|v| (f64::from(v) * 1e4).round() as i64);
                let mut at: BTreeMap<[i64; 3], Vec<usize>> = BTreeMap::new();
                for (i, p) in op.positions().expect("positions").iter().enumerate() {
                    at.entry(key(*p)).or_default().push(i);
                }
                let f2 = |pr: &primase_core::Primitive, sem| match pr.streams.get(&sem) {
                    Some(primase_core::Stream::F32x2(v)) => Some(v.clone()),
                    _ => None,
                };
                let f3 = |pr: &primase_core::Primitive, sem| match pr.streams.get(&sem) {
                    Some(primase_core::Stream::F32x3(v)) => Some(v.clone()),
                    _ => None,
                };
                let c4 =
                    |pr: &primase_core::Primitive| match pr.streams.get(&primase_core::Semantic::Color(0)) {
                        Some(primase_core::Stream::U8x4(v)) => Some(v.clone()),
                        _ => None,
                    };
                let (ouv, on, oc) = (
                    f2(op, primase_core::Semantic::TexCoord(0)),
                    f3(op, primase_core::Semantic::Normal),
                    c4(op),
                );
                for &pi in &parts {
                    let tp = &their_mesh.primitives[pi];
                    let (tuv, tn, tc) = (
                        f2(tp, primase_core::Semantic::TexCoord(0)),
                        f3(tp, primase_core::Semantic::Normal),
                        c4(tp),
                    );
                    for (vi, p) in tp.positions().expect("positions").iter().enumerate() {
                        let cands = at.get(&key(*p)).map(Vec::as_slice).unwrap_or(&[]);
                        *attrs.entry("scene vertices").or_default() += 1;
                        let close = |a: &[f32], b: &[f32], tol: f32| {
                            a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
                        };
                        if let (Some(t), Some(o)) = (&tuv, &ouv)
                            && !cands.iter().any(|&c| close(&t[vi], &o[c], 1e-5))
                        {
                            *attrs.entry("UV0 differing").or_default() += 1;
                        }
                        // the scene's normals are ours normalised (stored normals are quantised, not unit)
                        let unit = |n: [f32; 3]| {
                            let l = n.iter().map(|v| v * v).sum::<f32>().sqrt();
                            if l > 0.0 { n.map(|v| v / l) } else { n }
                        };
                        if let (Some(t), Some(o)) = (&tn, &on)
                            && !cands.iter().any(|&c| close(&t[vi], &unit(o[c]), 1e-4))
                        {
                            // a stored normal of zero: that exporter writes straight up
                            let zero_up = t[vi] == [0.0, 1.0, -0.0] || t[vi] == [0.0, 1.0, 0.0];
                            let key = if zero_up && cands.iter().any(|&c| o[c].iter().all(|v| *v == 0.0)) {
                                "zero stored normal (that exporter writes up)"
                            } else {
                                "normal differing (after normalising ours)"
                            };
                            *attrs.entry(key).or_default() += 1;
                        }
                        if let Some(t) = &tc {
                            match &oc {
                                Some(o) if cands.iter().any(|&c| o[c][3] == t[vi][3]) => {}
                                _ => *attrs.entry("COLOR_0 alpha differing").or_default() += 1,
                            }
                        }
                    }
                }
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
    println!("vertex attributes of identical geometries: {attrs:?}");
    for x in &examples {
        println!("    differing: {x}");
    }
    // textures: each scene texture's largest level is one of our textures' levels, and its smaller levels follow it
    // (that exporter passed DXT blocks through and may drop top levels to a size cap)
    let mut levels: BTreeMap<Vec<u8>, (primase_core::Entity, usize)> = BTreeMap::new();
    for (e, t) in drawn.iter::<primase_core::Texture>() {
        for (i, l) in t.levels.iter().enumerate() {
            levels.entry(Sha256::digest(l).to_vec()).or_insert((e, i));
        }
    }
    let mut tex: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, tf) in theirs.iter::<primase_core::TextureFile>() {
        let bytes = std::fs::read(std::path::Path::new(&scene).join(&tf.path)).expect("scene texture");
        let t = primase_dds::read(&bytes).expect("dds");
        let class = match levels.get(&Sha256::digest(&t.levels[0]).to_vec()) {
            None => "scene textures with no level of ours",
            Some(&(e, first)) => {
                let ours_t = drawn.get::<primase_core::Texture>(e).expect("texture");
                let rest = t
                    .levels
                    .iter()
                    .enumerate()
                    .all(|(i, l)| ours_t.levels.get(first + i) == Some(l));
                match (rest, first) {
                    (true, 0) if t.levels.len() == ours_t.levels.len() => "scene textures identical",
                    (true, 0) => "scene textures identical, fewer smallest levels",
                    (true, _) => "scene textures identical from a smaller level (size cap)",
                    (false, _) => "scene textures with differing smaller levels",
                }
            }
        };
        *tex.entry(class).or_default() += 1;
    }
    println!("textures: {tex:?}");
    assert!(
        !tex.contains_key("scene textures with no level of ours")
            && !tex.contains_key("scene textures with differing smaller levels"),
        "textures differ"
    );
    assert!(!geo.contains_key("geometries differing"), "geometries differ");
    assert!(
        by_map.is_empty(),
        "placements of ours outside interiors the scene leaves out"
    );
    assert!(
        !attrs.keys().any(|k| k.contains("differing")),
        "vertex attributes differ: {attrs:?}"
    );
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
