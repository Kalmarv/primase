//! A region read from the install against an independent exporter's scene of the same region, by meaning.
//!
//! Oracle (`PRIMASE_GTA_SCENE`): a skate-scene folder whose manifest's `region` is `[x0, y0, x1, y1]` in GTA metres,
//! exported from the same install with GTA Online's map; read with `primase-skate-scene`. Placements are matched by
//! archetype (the joaat of the scene prototype's name) and position (1 cm); a match compares orientation and scale
//! (1e-5 per transform column component), tint (the instance variant) and priority (the placement's flag bits 8-11,
//! read as `core.priority`, against `priorityLevel`). Before matching, ours are resolved to GTA Online's map at the default hour (`gta.hour` 12, the
//! hour that exporter fixed; hour 0 is checked against the time archetypes' masks: the placements that come and go must
//! be exactly those the masks say) and go through `gta-select-region` with that exporter's minimum size (0.5 m). Scene
//! placements without a match fail the gate, and so do placements of ours outside interiors the scene leaves out;
//! entities of interiors are counted (which interiors that exporter placed came from its own interior table, not a
//! pass here).
//!
//! Geometry and materials: the matched placements' drawables are attached ([`attach_assets`]) and posed
//! (`gta-pose-fragments`), each prototype drawn at the level that exporter drew (`select-lod{form=1}`: the coarsest LOD at most medium, else the finest; its `--level 1`),
//! the scene's prototypes at that exporter's choice (`select-lod`, its `lod_default`), and go through the GTA passes (`lower-gta-pbr`, `gta-winding`, `gta-tint`, `gta-terrain-layers`, `gta-vertex-colours`), so what is compared
//! is the world once lowered. Per GTA geometry (the scene's parts carry their source model and geometry, ours
//! `gta.sources`; either side may have split it), the scene's triangles are compared with ours by position
//! (0.1 mm, winding-agnostic), then every scene triangle of an identical geometry must be wound as ours. Named class:
//! decal shaders' (and distance_map's, which that exporter classes a decal) geometry the exporter lifted 12 mm along
//! each vertex's drawn face (the normalised sum of its triangles' face normals in the geometry as read; its
//! `--decal-face-lift`), matched within 0.2 mm; not a pass yet.
//! Named class: a geometry whose triangles' facing against its stored normals sums to exactly zero leaves the winding
//! undecided; ours keeps the game's order, that exporter's float sum may flip it.
//! Any other difference fails the gate. On identical geometries every scene vertex must have one of ours at its
//! position with its UV0 (1e-5), its normal (1e-4, against ours normalised: stored normals are quantised; named: a
//! stored zero normal the exporter writes up, and where none is stored the exporter's face normals, checked), and,
//! where the scene carries COLOR_0, its alpha. Textures: each scene texture's largest level must be one of our
//! attached textures' levels and its smaller levels must follow it (that exporter passed DXT blocks through and dropped
//! top levels to a size cap, counted).
//!
//! Materials: the scene's are lowered by `lower-skate-pbr`. Each scene part is compared with our primitive of its
//! geometry holding most of its triangles (by position; for a lifted decal by UV0 with the centroid within 2 cm), by
//! meaning: base texture (its first level), flat base colour, alpha mode, sidedness, vertex alpha (a COLOR_0 alpha
//! below opaque: gta-tint's colour alone is opaque), emission; a tinted part per tint row its placements use, the
//! palette colour as that exporter quantised it (4 bits per channel, near-white none) against ours at the first vertex
//! of each of its triangles (that exporter coloured a triangle by its first vertex's column), the most common; a
//! layered part by slot (null, or texture, alpha and sidedness), weights and every
//! vertex's COLOR_1 (one step of 1/255). Roughness and metallic are the skate writer's class defaults on the scene's
//! side and are not compared. Named classes, choices of that exporter or its skate writer: cutouts and foliage drawn
//! two-sided (GTA's effect records turn culling off for cloth alone), foliage cut off at 0.33, its wood class drawn
//! opaque, surfaces drawn opaque where its probe found the texture without alpha (checked with that probe), one mean
//! colour for a geometry with more than 8 palette colours. Any other difference fails the gate.
//!
//! Needs `PRIMASE_GTA_DIR` and `PRIMASE_GTA_KEYS`.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::Mesh;
use primase_core::pbr::PbrMaterial;
use primase_core::state::{Condition, ResolveState, StateAxis};
use primase_core::{Instance, Name, Transform, World};
use primase_gta::assets::attach_assets;
use primase_gta::components::{GtaArchetype, GtaDrawable, GtaEntity, GtaSources};
use primase_gta::lower::{GtaVertexColours, LowerGtaPbr};
use primase_gta::region::{Region, States, archive_sets, read_region};
use primase_gta::select::SelectRegion;
use primase_gta::terrain::GtaTerrainLayers;
use primase_gta::tint::{GtaTint, GtaTintVariants};
use primase_gta::winding::GtaWinding;
use primase_gta::{Install, Keys};
use primase_skate_scene::{
    LowerSkatePbr, MaterialClass, SkateMaterial, SkateParts, SkatePlacement, SkatePrototype,
};
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
        max_priority: 3,
        min_size: 0.5,
    };
    // the hour axis: placements of time archetypes come and go with gta.hour as their masks say; checked at hour 0
    // against the masks, then the world resolved at the default hour (12, as that exporter fixed it)
    let axis_of = |w: &World, key: &str| {
        w.iter::<StateAxis>()
            .find(|(_, a)| a.key == key)
            .map(|(e, _)| e)
            .expect("axis")
    };
    let (map_axis, hour_axis) = (axis_of(&ours, "gta.map"), axis_of(&ours, "gta.hour"));
    let (mut at0, mut at12) = (0usize, 0usize);
    // (a placement whose condition never holds has no clause)
    for c in ours.iter::<Condition>().filter_map(|(_, c)| c.0.first()) {
        let online =
            c.0.iter()
                .filter(|l| l.axis == map_axis)
                .all(|l| l.values & 2 != 0);
        // and every other axis (a script-managed map's IPL) at its default
        let others =
            c.0.iter()
                .filter(|l| l.axis != map_axis && l.axis != hour_axis)
                .all(|l| {
                    ours.get::<StateAxis>(l.axis)
                        .is_some_and(|a| l.values & (1u64 << a.default) != 0)
                });
        let online = online && others;
        if let Some(h) = c.0.iter().find(|l| l.axis == hour_axis)
            && online
        {
            at0 += usize::from(h.values & 1 != 0);
            at12 += usize::from(h.values & (1 << 12) != 0);
        }
    }
    let resolve = |w: &mut World, assign: &[(&str, &str)]| {
        let rs = ResolveState {
            assign: assign
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let mut log = primase_core::pass::PassLog::default();
        primase_core::pass::Pass::run(&rs, w, &mut log).expect("resolve-state");
        w.iter::<Instance>().count()
    };
    let mut midnight = ours.clone();
    let n0 = resolve(&mut midnight, &[("gta.map", "online"), ("gta.hour", "0")]);
    let n12 = resolve(&mut ours, &[("gta.map", "online")]);
    println!(
        "gta.hour: {} time placements; Online placements at hour 0: {n0}, at 12: {n12}; masks show {at0} and {at12} time placements",
        report.time_placements
    );
    assert_eq!(
        n0 as i64 - n12 as i64,
        at0 as i64 - at12 as i64,
        "hour 0 against the masks"
    );
    let mut log = primase_core::pass::PassLog::default();
    primase_core::pass::Pass::run(&select, &mut ours, &mut log).expect("select");
    println!("select: {:?}", log.counts);

    let mut theirs = World::new();
    primase_skate_scene::read(std::path::Path::new(&scene), &mut theirs).expect("scene");
    // the scene drawn at its own choice of level (its lod_default, which that exporter's --level 1 chose)
    let mut scene_lod = primase_core::pass::PassLog::default();
    primase_core::pass::Pass::run(&primase_core::SelectLod::default(), &mut theirs, &mut scene_lod)
        .expect("scene select-lod");
    println!("scene select-lod: {:?}", scene_lod.counts);

    // ours, resolved to GTA Online's map at hour 12
    let mut index: BTreeMap<(u32, [i64; 3]), Vec<primase_core::Entity>> = BTreeMap::new();
    for (e, _) in ours.iter::<Instance>() {
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
        // the placement's priority bits (flags 8-11), which the scene reader gives as core.priority (none: 0)
        let priority = theirs.get::<primase_core::Priority>(e).map_or(0, |p| p.0);
        if u32::from(priority) != ge.priority_level as u32 {
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
    // PRIMASE_GTA_ONLY (archetype names, comma separated): only those archetypes' placements, on both sides, from here
    // on (a look at one family of a large region; the gate's verdict then covers only them)
    let only_some = primase_testdata::real_data_value("PRIMASE_GTA_ONLY").is_some();
    if let Some(only) = primase_testdata::real_data_value("PRIMASE_GTA_ONLY") {
        let names: std::collections::BTreeSet<u32> = only
            .to_string_lossy()
            .split(',')
            .map(|n| primase_hash::joaat(n.trim().to_lowercase().as_bytes()))
            .collect();
        matched.retain(|e| {
            let p = ours.get::<Instance>(*e).expect("instance").prototype;
            ours.get::<GtaArchetype>(p)
                .is_some_and(|a| names.contains(&a.name))
        });
        let keep: Vec<primase_core::Entity> = theirs
            .iter::<Instance>()
            .filter(|(_, i)| {
                theirs
                    .get::<Name>(i.prototype)
                    .is_some_and(|n| names.contains(&primase_hash::joaat(n.0.to_lowercase().as_bytes())))
            })
            .map(|(e, _)| e)
            .collect();
        theirs = theirs.subset(&theirs.closure(keep));
        println!(
            "only {} archetypes: {} placements of ours, {} of the scene",
            names.len(),
            matched.len(),
            theirs.iter::<Instance>().count()
        );
    }
    // the matched placements' drawables and textures
    let mut drawn = ours.subset(&ours.closure(matched.iter().copied()));
    let assets = attach_assets(&mut drawn, online.as_ref().expect("online"), &keys).expect("assets");
    println!("assets of the matched placements: {assets:?}");
    // fragments' models at their bones' poses, as every other pass expects
    let mut pose_log = primase_core::pass::PassLog::default();
    primase_core::pass::Pass::run(&primase_gta::pose::PoseFragments, &mut drawn, &mut pose_log)
        .expect("poses");
    println!("gta-pose-fragments: {:?}", pose_log.counts);
    // the level that exporter drew (its --level 1): the coarsest LOD at most 1, else the finest
    let mut lod_log = primase_core::pass::PassLog::default();
    primase_core::pass::Pass::run(
        &primase_core::SelectLod {
            choice: primase_core::LodChoice::AtMost(1),
        },
        &mut drawn,
        &mut lod_log,
    )
    .expect("select-lod");
    println!("select-lod{{form=1}}: {:?}", lod_log.counts);

    // the GTA passes: what the world says once lowered (the world as posed kept for the decal lift check: no later
    // pass moves a position)
    let read = drawn.clone();
    let mut log = primase_core::pass::PassLog::default();
    for pass in [
        &LowerGtaPbr::default() as &dyn primase_core::pass::Pass,
        &GtaWinding,
        &GtaTintVariants,
        &GtaTerrainLayers,
        &GtaVertexColours,
    ] {
        if let Err(e) = pass.run(&mut drawn, &mut log) {
            panic!("{}: {e}", pass.name());
        }
    }
    println!(
        "lower-gta-pbr, gta-winding, gta-tint, gta-terrain-layers, gta-vertex-colours: {:?}, approximated {:?}",
        log.counts, log.approximations
    );
    assert!(
        drawn.verify_components().is_empty(),
        "components verify after the passes"
    );

    // geometry: per GTA geometry, the scene's triangles (all parts made from it) against ours (all our primitives made
    // from it), by rounded positions, winding-agnostic; then the winding
    primase_gltf::load_mesh_files(&mut theirs, std::path::Path::new(&scene)).expect("scene meshes");
    // per archetype: our mesh and drawable once lowered, and as read
    type Proto<'a> = (&'a Mesh, &'a GtaSources, &'a Mesh, &'a GtaSources);
    let mut protos: BTreeMap<u32, Proto> = BTreeMap::new();
    for (e, a) in drawn.iter::<GtaArchetype>() {
        if let (Some(m), Some(d), Some(r), Some(rd)) = (
            drawn.get::<Mesh>(e),
            drawn.get::<GtaSources>(e),
            read.get::<Mesh>(e),
            read.get::<GtaSources>(e),
        ) {
            protos.insert(a.name, (m, d, r, rd));
        }
    }
    let tris = |m: &Mesh, prims: &[usize]| -> BTreeMap<[[i64; 3]; 3], usize> {
        let mut out = BTreeMap::new();
        for &i in prims {
            for (k, n) in tris_of(&m.primitives[i]) {
                *out.entry(k).or_default() += n;
            }
        }
        out
    };
    let mut geo: BTreeMap<&str, usize> = BTreeMap::new();
    let mut examples = Vec::new();
    // per shader, the geometries differing and the scene triangles wound the other way, with examples
    let mut differing_by_shader: BTreeMap<String, usize> = BTreeMap::new();
    let mut wound_by_shader: BTreeMap<String, usize> = BTreeMap::new();
    let mut wound_examples: Vec<String> = Vec::new();
    let mut attrs: BTreeMap<&str, usize> = BTreeMap::new();
    for (e, _) in theirs.iter::<SkatePrototype>() {
        let sp_parts: &[primase_skate_scene::Part] = theirs.get::<SkateParts>(e).map_or(&[], |p| &p.0);
        let name = theirs.get::<Name>(e).map_or(String::new(), |n| n.0.clone());
        let Some(&(ours_mesh, d, read_mesh, read_d)) =
            protos.get(&primase_hash::joaat(name.to_lowercase().as_bytes()))
        else {
            *geo.entry("prototypes not compared (no drawable)").or_default() += 1;
            continue;
        };
        let Some(their_mesh) = theirs.get::<Mesh>(e) else {
            *geo.entry("scene prototypes without geometry").or_default() += 1;
            continue;
        };
        let mut groups: BTreeMap<[u32; 2], Vec<usize>> = BTreeMap::new();
        for (i, part) in sp_parts.iter().enumerate() {
            let src = part.source.as_ref().expect("part source");
            groups
                .entry([
                    src["model"].as_u64().expect("model") as u32,
                    src["geometry"].as_u64().expect("geometry") as u32,
                ])
                .or_default()
                .push(i);
        }
        *geo.entry("prototypes compared").or_default() += 1;
        for (g, parts) in groups {
            let ats: Vec<usize> = (0..d.sources.len()).filter(|&k| d.sources[k] == g).collect();
            let ours_t = tris(ours_mesh, &ats);
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
                // winding: each scene triangle as ours wind it (corners in cyclic order)
                let mut ours_o = BTreeMap::new();
                for &k in &ats {
                    ours_o.extend(oriented(&ours_mesh.primitives[k]));
                }
                // a geometry whose triangles' facing against the stored normals sums to zero is undecided: ours keeps
                // the game's order, that exporter's float sum may have flipped it
                let undecided = {
                    let k = read_d
                        .sources
                        .iter()
                        .position(|s| *s == g)
                        .expect("the geometry as read");
                    facing(&read_mesh.primitives[k]).0 == 0.0
                };
                for &pi in &parts {
                    for (k, n) in oriented(&their_mesh.primitives[pi]) {
                        if !ours_o.contains_key(&k) && undecided {
                            *geo.entry("scene triangles wound the other way on geometries their normals leave undecided (facing sum 0)").or_default() += n;
                        } else if !ours_o.contains_key(&k) {
                            *geo.entry("scene triangles wound the other way").or_default() += n;
                            let shader = sp_parts[pi]
                                .source
                                .as_ref()
                                .and_then(|v| v["shader"].as_str())
                                .unwrap_or("?");
                            *wound_by_shader.entry(shader.to_string()).or_default() += n;
                            if wound_examples.len() < 10
                                && !wound_examples.iter().any(|x: &String| x.starts_with(&name))
                            {
                                let k = read_d
                                    .sources
                                    .iter()
                                    .position(|s| *s == g)
                                    .expect("the geometry as read");
                                let (sum, size) = facing(&read_mesh.primitives[k]);
                                wound_examples.push(format!(
                                    "{name} {g:?} ({shader}): facing sum {sum:.3e} of {size:.3e} ({:.2e})",
                                    sum / size.max(f64::MIN_POSITIVE)
                                ));
                            }
                        }
                    }
                }
                // vertex attributes: each scene vertex has one of ours at its position with its UV0, normal and
                // (where the scene carries COLOR_0) colour alpha
                let key = |p: [f32; 3]| p.map(|v| (f64::from(v) * 1e4).round() as i64);
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
                // ours: per position, (UV0, normal, COLOR_0) of every vertex there
                // a primitive without stored normals: the normalised sum of each vertex's triangles' face normals
                // (what that exporter writes there)
                type Attrs = (
                    Option<[f32; 2]>,
                    Option<[f32; 3]>,
                    Option<[u8; 4]>,
                    Option<[f32; 3]>,
                );
                let mut at: BTreeMap<[i64; 3], Vec<Attrs>> = BTreeMap::new();
                for &k in &ats {
                    let op = &ours_mesh.primitives[k];
                    let (ouv, on, oc) = (
                        f2(op, primase_core::Semantic::TexCoord(0)),
                        f3(op, primase_core::Semantic::Normal),
                        c4(op),
                    );
                    let pos = op.positions().expect("positions");
                    let generated: Option<Vec<[f32; 3]>> = on.is_none().then(|| {
                        let mut faces = vec![[0f64; 3]; pos.len()];
                        for t in op.indices.as_ref().expect("indices").as_chunks::<3>().0 {
                            let v = [0, 1, 2].map(|j| pos[t[j] as usize].map(f64::from));
                            let n = cross(sub(v[1], v[0]), sub(v[2], v[0]));
                            for &i in t {
                                faces[i as usize] = add(faces[i as usize], n);
                            }
                        }
                        faces
                            .iter()
                            .map(|n| {
                                let l = dot(*n, *n).sqrt();
                                if l > 0.0 {
                                    n.map(|x| (x / l) as f32)
                                } else {
                                    [0.0, 1.0, 0.0]
                                }
                            })
                            .collect()
                    });
                    for (i, p) in pos.iter().enumerate() {
                        at.entry(key(*p)).or_default().push((
                            ouv.as_ref().map(|v| v[i]),
                            on.as_ref().map(|v| v[i]),
                            oc.as_ref().map(|v| v[i]),
                            generated.as_ref().map(|v| v[i]),
                        ));
                    }
                }
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
                        if let Some(t) = &tuv
                            && !cands.iter().any(|c| c.0.is_some_and(|o| close(&t[vi], &o, 1e-5)))
                        {
                            *attrs.entry("UV0 differing").or_default() += 1;
                        }
                        // the scene's normals are ours normalised (stored normals are quantised, not unit)
                        let unit = |n: [f32; 3]| {
                            let l = n.iter().map(|v| v * v).sum::<f32>().sqrt();
                            if l > 0.0 { n.map(|v| v / l) } else { n }
                        };
                        if let Some(t) = &tn
                            && !cands
                                .iter()
                                .any(|c| c.1.is_some_and(|o| close(&t[vi], &unit(o), 1e-4)))
                        {
                            // a stored normal of zero: that exporter writes straight up
                            let zero_up = t[vi] == [0.0, 1.0, -0.0] || t[vi] == [0.0, 1.0, 0.0];
                            let key = if zero_up && cands.iter().any(|c| c.1 == Some([0.0; 3])) {
                                "zero stored normal (that exporter writes up)"
                            } else if cands.iter().any(|c| c.3.is_some_and(|o| close(&t[vi], &o, 1e-4))) {
                                "no stored normal (that exporter's face normals)"
                            } else {
                                "normal differing (after normalising ours)"
                            };
                            *attrs.entry(key).or_default() += 1;
                        }
                        if let Some(t) = &tc
                            && !cands.iter().any(|c| c.2.is_some_and(|o| o[3] == t[vi][3]))
                        {
                            *attrs.entry("COLOR_0 alpha differing").or_default() += 1;
                        }
                    }
                }
                continue;
            }
            // the exporter's decal lift: every scene triangle of a decal shader's geometry is one of ours, as stored
            // or with each vertex moved 12 mm along its drawn face (the normalised sum of its triangles' face normals),
            // within 0.2 mm
            let shader = sp_parts[parts[0]]
                .source
                .as_ref()
                .and_then(|v| v["shader"].as_str())
                .unwrap_or("?");
            let lifted = (shader.contains("decal") || shader == "distance_map.sps") && {
                let grid = |c: [f64; 3]| c.map(|v| (v / 0.005).floor() as i64);
                let mut cells: BTreeMap<[i64; 3], Vec<[[f64; 3]; 3]>> = BTreeMap::new();
                // per vertex of the geometry as read, the sum of its triangles' (unnormalised) face normals,
                // normalised: the drawn face
                let k = read_d
                    .sources
                    .iter()
                    .position(|s| *s == g)
                    .expect("the geometry as read");
                let op = &read_mesh.primitives[k];
                let pos = op.positions().expect("positions");
                let idx = op.indices.as_ref().expect("indices");
                let mut faces = vec![[0f64; 3]; pos.len()];
                for t in idx.as_chunks::<3>().0 {
                    let v = [0, 1, 2].map(|j| pos[t[j] as usize].map(f64::from));
                    let n = cross(sub(v[1], v[0]), sub(v[2], v[0]));
                    for &i in t {
                        faces[i as usize] = add(faces[i as usize], n);
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
                            let i = t[j] as usize;
                            add(pos[i].map(f64::from), faces[i].map(|x| x * side))
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
                *differing_by_shader.entry(shader.to_string()).or_default() += 1;
                if examples.len() < 10 {
                    examples.push(format!(
                        "{name} model {} geometry {} ({shader}): {missing} of {total} triangles",
                        g[0], g[1]
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
    println!("geometries differing by shader: {differing_by_shader:?}");
    println!("scene triangles wound the other way by shader: {wound_by_shader:?}");
    for x in &wound_examples {
        println!("    wound the other way: {x}");
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
    let mut tex_examples: Vec<String> = Vec::new();
    // our textures by name, for a scene texture with no level of ours: that exporter's copy of the same name
    let mut ours_by_name: BTreeMap<String, Vec<primase_core::Entity>> = BTreeMap::new();
    for (e, _) in drawn.iter::<primase_core::Texture>() {
        if let Some(n) = drawn.get::<Name>(e) {
            ours_by_name.entry(n.0.to_lowercase()).or_default().push(e);
        }
    }
    for (te, tf) in theirs.iter::<primase_core::TextureFile>() {
        let bytes = std::fs::read(std::path::Path::new(&scene).join(&tf.path)).expect("scene texture");
        let t = primase_dds::read(&bytes).expect("dds");
        let named = theirs
            .get::<Name>(te)
            .map(|n| n.0.split('~').next().unwrap_or("").to_lowercase())
            .and_then(|n| ours_by_name.get(&n))
            .cloned()
            .unwrap_or_default();
        let digest = Sha256::digest(&t.levels[0]).to_vec();
        let class = match levels.get(&digest) {
            None if named.iter().any(|&o| white_alpha(&theirs, te, &drawn, o, &scene)) => {
                "scene textures: that exporter's alpha-only (A8) texture as white or grey"
            }
            None if named.iter().any(|&o| same_pixels(&theirs, te, &drawn, o, &scene)) => {
                "scene textures: that exporter's re-encoding of ours (another format, its own levels)"
            }
            None if named
                .iter()
                .any(|&o| replaced_copy(&install, &keys, &drawn, o, &digest).is_some()) =>
            {
                "scene textures: that exporter's copy from a same-named dictionary the game's mounts replace or never mount"
            }
            None => {
                if tex_examples.len() < 12 {
                    tex_examples.push(format!(
                        "{} ({}x{}, {} levels)",
                        tf.path.display(),
                        t.width,
                        t.height,
                        t.levels.len()
                    ));
                }
                "scene textures with no level of ours"
            }
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
    for x in &tex_examples {
        println!("    scene texture with no level of ours: {x}");
    }

    // materials: the scene's lowered by lower-skate-pbr; each scene part's material against our primitive of the
    // same GTA geometry, by meaning
    let mut slog = primase_core::pass::PassLog::default();
    primase_core::pass::Pass::run(&LowerSkatePbr, &mut theirs, &mut slog).expect("lower-skate-pbr");
    // per archetype our prototype (the first: gta-tint's copies for further tint rows come after), and per tint row
    // the prototype drawn with it
    let mut by_name: BTreeMap<u32, primase_core::Entity> = BTreeMap::new();
    let mut by_row: BTreeMap<(u32, u32), primase_core::Entity> = BTreeMap::new();
    for (e, a) in drawn.iter::<GtaArchetype>() {
        if !drawn.has::<GtaDrawable>(e) {
            continue;
        }
        by_name.entry(a.name).or_insert(e);
        if let Some(t) = drawn.get::<GtaTint>(e) {
            by_row.insert((a.name, t.row), e);
        }
    }
    let mut mat: BTreeMap<String, usize> = BTreeMap::new();
    let mut mat_examples: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut mat_effects: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for (e, _) in theirs.iter::<SkatePrototype>() {
        let sp_parts: &[primase_skate_scene::Part] = theirs.get::<SkateParts>(e).map_or(&[], |p| &p.0);
        let name = theirs.get::<Name>(e).map_or(String::new(), |n| n.0.clone());
        let (Some(&oe), Some(their_mesh)) = (
            by_name.get(&primase_hash::joaat(name.to_lowercase().as_bytes())),
            theirs.get::<Mesh>(e),
        ) else {
            continue;
        };
        let (d, om) = (
            drawn.get::<GtaSources>(oe).expect("sources"),
            drawn.get::<Mesh>(oe).expect("mesh"),
        );
        for (i, part) in sp_parts.iter().enumerate() {
            let src = part.source.as_ref().expect("part source");
            let g = [
                src["model"].as_u64().expect("model") as u32,
                src["geometry"].as_u64().expect("geometry") as u32,
            ];
            *mat.entry("scene parts".into()).or_default() += 1;
            let cands: Vec<usize> = (0..d.sources.len()).filter(|&k| d.sources[k] == g).collect();
            if cands.is_empty() {
                *mat.entry("scene parts whose geometry we dropped".into())
                    .or_default() += 1;
                continue;
            }
            // gta-tint splits a geometry by palette colour: our primitive is the one holding most of the scene part's
            // triangles, by position, else (a lifted decal) by UV with the centroid within 2 cm
            let at = {
                let tp = &their_mesh.primitives[i];
                let (want_pos, want_uv) = (tris_of(tp), uv_tris(tp));
                let best = |score: &dyn Fn(usize) -> usize| {
                    cands
                        .iter()
                        .copied()
                        .map(|k| (score(k), k))
                        .max_by_key(|&(n, k)| (n, std::cmp::Reverse(k)))
                        .filter(|&(n, _)| n > 0)
                        .map(|(_, k)| k)
                };
                best(&|k| {
                    let ours = tris_of(&om.primitives[k]);
                    want_pos.keys().filter(|t| ours.contains_key(*t)).count()
                })
                .or_else(|| {
                    best(&|k| {
                        let mut ours: BTreeMap<[[i64; 2]; 3], Vec<[f64; 3]>> = BTreeMap::new();
                        for (uv, c) in uv_tris(&om.primitives[k]) {
                            ours.entry(uv).or_default().push(c);
                        }
                        want_uv
                            .iter()
                            .filter(|(uv, c)| {
                                ours.get(uv).is_some_and(|l| {
                                    l.iter().any(|o| dot(sub(*o, *c), sub(*o, *c)) <= 0.02 * 0.02)
                                })
                            })
                            .count()
                    })
                })
                .unwrap_or(cands[0])
            };
            let op = &om.primitives[at];
            // a tinted part's materials are its variants' (one per tint row)
            let tm = their_mesh.primitives[i]
                .material
                .or_else(|| {
                    theirs
                        .get::<primase_core::MaterialVariants>(e)
                        .and_then(|v| v.0.get(i))
                        .and_then(|m| m.values().next().copied())
                })
                .expect("scene part material");
            let sm = theirs.get::<SkateMaterial>(tm).expect("skate material");
            let class = sm.class.as_str();
            let om_e = op.material.expect("our material");
            if sm.class == MaterialClass::Layered {
                *mat.entry("layered parts".into()).or_default() += 1;
                let diffs = layered_diffs(
                    &theirs,
                    tm,
                    &their_mesh.primitives[i],
                    &drawn,
                    om_e,
                    op,
                    &scene,
                    &install,
                    &keys,
                    only_some,
                );
                // the lookup-blended terrains weigh their layers per pixel from a lookup texture; both sides sample it
                // per vertex (an approximation each), and their samplings differ
                let effect = drawn
                    .get::<primase_gta::components::GtaShader>(om_e)
                    .and_then(|s| primase_gta::shader_names::effect(s.name))
                    .unwrap_or("?");
                let lookup = !matches!(
                    primase_gta::terrain::weights_of(effect),
                    primase_gta::terrain::Weights::VertexColour
                );
                let diffs: Vec<String> = diffs
                    .into_iter()
                    .map(|k| {
                        if lookup && k == "layer weights (COLOR_1) differing at vertices" {
                            "layer weights: lookup-blended terrain, each side's per-vertex sampling of the game's per-pixel lookup"
                                .into()
                        } else {
                            k
                        }
                    })
                    .collect();
                if diffs.is_empty() {
                    *mat.entry("parts equal".into()).or_default() += 1;
                }
                for k in diffs {
                    let ex = mat_examples.entry(k.clone()).or_default();
                    if ex.len() < 6 {
                        ex.push(format!("{name} {g:?} ({effect})"));
                    }
                    *mat.entry(k).or_default() += 1;
                }
                continue;
            }
            let Some(ours_p) = drawn.get::<PbrMaterial>(om_e) else {
                *mat.entry(format!("scene {class} parts whose material of ours has no pbr"))
                    .or_default() += 1;
                continue;
            };
            let tp = theirs.get::<PbrMaterial>(tm).expect("scene pbr material");
            let mut diffs: Vec<String> = Vec::new();
            match (sm.base, ours_p.base_color_texture) {
                (Some(t), Some(o)) => {
                    if !same_texture(&theirs, t, &drawn, o.texture, &scene)
                        && replaced_copy(
                            &install,
                            &keys,
                            &drawn,
                            o.texture,
                            &scene_texture_digest(&theirs, t, &scene),
                        )
                        .is_some()
                    {
                        diffs.push(
                            "base texture: that exporter's copy from a same-named dictionary the game's mounts replace or never mount"
                                .into(),
                        );
                    } else if !same_texture(&theirs, t, &drawn, o.texture, &scene)
                        && white_alpha(&theirs, t, &drawn, o.texture, &scene)
                    {
                        diffs.push(
                            "base texture: that exporter's alpha-only (A8) texture as white or grey".into(),
                        );
                    } else if !same_texture(&theirs, t, &drawn, o.texture, &scene)
                        && same_pixels(&theirs, t, &drawn, o.texture, &scene)
                    {
                        diffs.push("base texture: that exporter's re-encoding of ours (another format, its own levels)".into());
                    } else if !same_texture(&theirs, t, &drawn, o.texture, &scene) {
                        if only_some {
                            let tf = theirs
                                .get::<primase_core::TextureFile>(t)
                                .map(|f| f.path.display().to_string());
                            let ot = drawn.get::<primase_core::Texture>(o.texture);
                            println!(
                                "      base texture {name} {g:?}: scene {tf:?} ({:?}), ours {:?} {:?} {:?}",
                                theirs.get::<Name>(t).map(|n| n.0.clone()),
                                drawn.get::<Name>(o.texture).map(|n| n.0.clone()),
                                drawn.get::<primase_core::Source>(o.texture).map(|n| n.0.clone()),
                                ot.map(|t| (t.width, t.height, t.levels.len()))
                            );
                            if let (Some(tf), Some(ot)) = (theirs.get::<primase_core::TextureFile>(t), ot)
                                && let Ok(st) =
                                    primase_dds::read(&std::fs::read(scene.join(&tf.path)).expect("scene"))
                            {
                                use primase_texcodec::TexDecoder;
                                let d = primase_texcodec::Decoder::default();
                                let a = d.decode_rgba8(st.format, st.width, st.height, &st.levels[0]);
                                let b = d.decode_rgba8(ot.format, ot.width, ot.height, &ot.levels[0]);
                                let stats = match (&a, &b) {
                                    (Ok(a), Ok(b)) if a.len() == b.len() => {
                                        let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max();
                                        let mean = a
                                            .iter()
                                            .zip(b)
                                            .map(|(x, y)| f64::from(x.abs_diff(*y)))
                                            .sum::<f64>()
                                            / a.len() as f64;
                                        let alpha = a
                                            .iter()
                                            .zip(b)
                                            .skip(3)
                                            .step_by(4)
                                            .map(|(x, y)| x.abs_diff(*y))
                                            .max();
                                        let rgb_white =
                                            a.as_chunks::<4>().0.iter().all(|p| p[..3] == [255; 3]);
                                        let grey = a
                                            .as_chunks::<4>()
                                            .0
                                            .iter()
                                            .zip(b.as_chunks::<4>().0)
                                            .map(|(x, y)| x[0].abs_diff(y[3]).max(x[1].abs_diff(y[3])))
                                            .max();
                                        let scene_alpha: std::collections::BTreeSet<u8> =
                                            a.iter().skip(3).step_by(4).copied().collect();
                                        println!(
                                            "        scene rgb against our alpha: max {grey:?}; scene alpha values {}",
                                            scene_alpha.len()
                                        );
                                        format!(
                                            "max {max:?}, mean {mean:.2}, alpha max {alpha:?}, scene rgb all white \
                                             {rgb_white}; scene first texels {:?}, ours {:?}",
                                            &a[..8],
                                            &b[..8]
                                        )
                                    }
                                    _ => format!(
                                        "decode {:?} / {:?}",
                                        a.as_ref().map(|v| v.len()),
                                        b.as_ref().map(|v| v.len())
                                    ),
                                };
                                println!(
                                    "        formats scene {:?} {}x{} {} levels, ours {:?}: {stats}",
                                    st.format,
                                    st.width,
                                    st.height,
                                    st.levels.len(),
                                    ot.format
                                );
                            }
                        }
                        diffs.push("base texture".into());
                    }
                }
                (None, None) => {}
                (t, o) => diffs.push(format!(
                    "base texture: scene {}, ours {}",
                    if t.is_some() { "textured" } else { "flat" },
                    if o.is_some() { "textured" } else { "flat" }
                )),
            }
            // tint: per tint row the scene placed, its colour against ours as that exporter quantised it (4 bits per
            // channel, near-white none): ours is gta-tint's per-vertex colour, the most common at the scene part's
            // vertices on our prototype of that row; a geometry with more than 8 palette colours it gave one mean
            // colour
            let their_rows = theirs
                .get::<primase_core::MaterialVariants>(e)
                .and_then(|v| v.0.get(i))
                .filter(|m| !m.is_empty());
            let tinted = drawn
                .resource::<primase_gta::tint::GtaTinted>()
                .is_some_and(|t| t.0.contains(&om_e));
            let archetype = primase_hash::joaat(name.to_lowercase().as_bytes());
            match (their_rows, tinted) {
                (Some(tv), true) => {
                    *mat.entry("tinted parts".into()).or_default() += 1;
                    for (row, tm) in tv {
                        let want = theirs.get::<SkateMaterial>(*tm).expect("skate material").tint;
                        let ours_e = by_row.get(&(archetype, *row)).copied().unwrap_or(oe);
                        if only_some {
                            let rows: Vec<u32> =
                                by_row.keys().filter(|k| k.0 == archetype).map(|k| k.1).collect();
                            let placed: std::collections::BTreeSet<(u32, u64)> = drawn
                                .iter::<Instance>()
                                .filter(|(_, i)| {
                                    drawn
                                        .get::<GtaArchetype>(i.prototype)
                                        .is_some_and(|a| a.name == archetype)
                                })
                                .map(|(e, i)| {
                                    (
                                        drawn.get::<GtaEntity>(e).map_or(0, |g| g.tint_value),
                                        u64::from(i.variant),
                                    )
                                })
                                .collect();
                            println!(
                                "      tint rows {name}: scene row {row}; our rows {rows:?}; our placements (tint, variant) {placed:?}"
                            );
                        }
                        let tint = drawn
                            .get::<Mesh>(ours_e)
                            .and_then(|m| m.primitives.get(at))
                            .map_or_else(Default::default, |op| tint_at(&their_mesh.primitives[i], op));
                        let got = tint
                            .colours
                            .iter()
                            .max_by_key(|&(_, n)| n)
                            .and_then(|(c, _)| quantised_tint(*c));
                        if want != got {
                            if only_some {
                                tint_debug(&read, oe, g, &their_mesh.primitives[i], om_e, *row);
                                println!("      scene material: {:?}", theirs.get::<SkateMaterial>(*tm));
                            }
                            if tint.palette_colours <= 8 {
                                println!(
                                    "      tint {name} {g:?} row {row}: scene {want:?}, ours {got:?} of {:?} ({} scene triangles, {} palette colours)",
                                    tint.colours, tint.triangles, tint.palette_colours
                                );
                            }
                            diffs.push(if tint.palette_colours > 8 {
                                "tint: that exporter's mean colour (more than 8 palette colours)".into()
                            } else {
                                "tint colour".into()
                            });
                            break;
                        }
                    }
                }
                (None, false) => {
                    if tp
                        .base_color
                        .iter()
                        .zip(&ours_p.base_color)
                        .any(|(a, b)| (a - b).abs() > 1e-6)
                    {
                        diffs.push(format!("base colour factor ({class})"));
                    }
                }
                (t, o) => diffs.push(format!("tinted: scene {}, ours {o}", t.is_some())),
            }
            if tp.alpha_mode != ours_p.alpha_mode {
                // that exporter drew a surface opaque when its probe found the texture without alpha
                let probed_opaque = tp.alpha_mode == primase_core::pbr::AlphaMode::Opaque
                    && sm.class != MaterialClass::Wood
                    && sm.base.is_some_and(|t| !exporter_sees_alpha(&theirs, t, &scene));
                diffs.push(if probed_opaque {
                    format!(
                        "alpha: scene {class} opaque, its texture without alpha by that exporter's probe; ours {:?}",
                        ours_p.alpha_mode
                    )
                } else {
                    format!("alpha: scene {class} {:?}, ours {:?}", tp.alpha_mode, ours_p.alpha_mode)
                });
            }
            if tp.double_sided != ours_p.double_sided {
                diffs.push(format!(
                    "two-sided: scene {class} {}, ours {}",
                    tp.double_sided, ours_p.double_sided
                ));
            }
            // fading by vertex alpha: a COLOR_0 alpha below opaque (gta-tint's colour alone is opaque) at the scene
            // part's vertices (that exporter splits a geometry into parts and gives a part vertex alpha only where
            // one of its own vertices fades)
            let faded = faded_at(&their_mesh.primitives[i], op);
            if sm.vertex_alpha.unwrap_or(false) != faded {
                diffs.push(format!(
                    "vertex alpha: scene {}, ours {faded}",
                    sm.vertex_alpha.unwrap_or(false)
                ));
            }
            let emits = ours_p.emissive_texture.is_some() || ours_p.emissive != [0.0; 3];
            if sm.emissive.unwrap_or(false) != emits {
                diffs.push(format!(
                    "emissive: scene {}, ours {emits}",
                    sm.emissive.unwrap_or(false)
                ));
            }
            if diffs.is_empty() {
                *mat.entry("parts equal".into()).or_default() += 1;
            } else {
                let s = drawn
                    .get::<primase_gta::components::GtaShader>(om_e)
                    .expect("shader");
                let effect = primase_gta::shader_names::effect(s.name).unwrap_or("?");
                for k in &diffs {
                    let ex = mat_examples.entry(k.clone()).or_default();
                    if ex.len() < 6 {
                        ex.push(format!("{name} {g:?} ({effect} b{})", s.render_bucket));
                    }
                    *mat_effects
                        .entry(k.clone())
                        .or_default()
                        .entry(effect.to_string())
                        .or_default() += 1;
                }
                for k in diffs {
                    *mat.entry(k).or_default() += 1;
                }
            }
        }
    }
    println!("materials: {mat:?}");
    for (k, x) in &mat_examples {
        println!("    material: {k}: e.g. {}", x.join("; "));
        if let Some(by) = mat_effects.get(k) {
            println!("        by effect: {by:?}");
        }
    }
    // named classes: choices of that exporter and its skate writer, not the game's
    let named = |k: &str| {
        matches!(
            k,
            "scene parts" | "parts equal" | "tinted parts" | "layered parts"
                // the skate writer draws cutouts and foliage from both sides; GTA's effect records turn culling off for
                // cloth alone
                | "two-sided: scene cutout true, ours false"
                | "two-sided: scene leaves true, ours false"
                // the skate writer's foliage cutoff
                | "alpha: scene leaves Mask { cutoff: 0.33 }, ours Mask { cutoff: 0.5 }"
                // that exporter's wood class: a tree shader's texture with mean alpha 0.9 or more drawn opaque
                | "alpha: scene wood Opaque, ours Mask { cutoff: 0.5 }"
                | "tint: that exporter's mean colour (more than 8 palette colours)"
                // that exporter loads every archive it finds and every pack's own files: a same-named copy the game's
                // mount data never mounts (patchday27ng's fwy_01.rpf) or replaces (a pack file under update.rpf's dlc_patch)
                | "base texture: that exporter's copy from a same-named dictionary the game's mounts replace or never mount"
                // that exporter wrote some formats decoded and uncompressed, with smaller levels of its own
                | "base texture: that exporter's re-encoding of ours (another format, its own levels)"
                // D3D samples an A8 texture as (0, 0, 0, a); that exporter wrote it as (255, 255, 255, a) or (a, a, a, 255)
                | "base texture: that exporter's alpha-only (A8) texture as white or grey"
                | "layer texture: that exporter's copy from a same-named dictionary the game's mounts replace or never mount"
                | "layer texture: that exporter's alpha-only (A8) texture as white or grey"
                | "layer texture: that exporter's re-encoding of ours (another format, its own levels)"
                // both approximate the lookup per vertex (gta-terrain-layers counts it)
                | "layer weights: lookup-blended terrain, each side's per-vertex sampling of the game's per-pixel lookup"
        ) || k.contains("opaque, its texture without alpha by that exporter's probe")
    };
    let unnamed: Vec<&String> = mat.keys().filter(|k| !named(k)).collect();
    assert!(unnamed.is_empty(), "material differences: {unnamed:?}");
    assert!(
        !tex.contains_key("scene textures with no level of ours")
            && !tex.contains_key("scene textures with differing smaller levels"),
        "textures differ"
    );
    assert!(!geo.contains_key("geometries differing"), "geometries differ");
    assert!(
        !geo.contains_key("scene triangles wound the other way"),
        "winding differs"
    );
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

/// Whether scene texture `t` is our texture `o` in another encoding: the same size, and their first levels decoded
/// agree within 4 of 255 per channel (that exporter decoded some formats and wrote them uncompressed, with its own
/// smaller levels).
fn same_pixels(
    theirs: &World,
    t: primase_core::Entity,
    ours: &World,
    o: primase_core::Entity,
    scene: &std::path::Path,
) -> bool {
    use primase_texcodec::TexDecoder;
    let tf = theirs.get::<primase_core::TextureFile>(t).expect("scene texture");
    let Ok(st) = primase_dds::read(&std::fs::read(scene.join(&tf.path)).expect("scene texture")) else {
        return false;
    };
    let Some(ot) = ours.get::<primase_core::Texture>(o) else {
        return false;
    };
    if (st.width, st.height) != (ot.width, ot.height) {
        return false;
    }
    let d = primase_texcodec::Decoder::default();
    let (Ok(a), Ok(b)) = (
        d.decode_rgba8(st.format, st.width, st.height, &st.levels[0]),
        d.decode_rgba8(ot.format, ot.width, ot.height, &ot.levels[0]),
    ) else {
        return false;
    };
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 4)
}

/// Whether scene texture `t` is our alpha-only texture `o` (A8) as white with its alpha or as opaque grey: D3D samples
/// A8 as black (0, 0, 0, a), that exporter wrote it as (255, 255, 255, a) or (a, a, a, 255). Same size, within 4 of
/// 255.
fn white_alpha(
    theirs: &World,
    t: primase_core::Entity,
    ours: &World,
    o: primase_core::Entity,
    scene: &std::path::Path,
) -> bool {
    use primase_texcodec::TexDecoder;
    let Some(ot) = ours.get::<primase_core::Texture>(o) else {
        return false;
    };
    if ot.format != primase_core::TexFormat::A8Unorm {
        return false;
    }
    let tf = theirs.get::<primase_core::TextureFile>(t).expect("scene texture");
    let Ok(st) = primase_dds::read(&std::fs::read(scene.join(&tf.path)).expect("scene texture")) else {
        return false;
    };
    if (st.width, st.height) != (ot.width, ot.height) {
        return false;
    }
    let d = primase_texcodec::Decoder::default();
    let (Ok(a), Ok(b)) = (
        d.decode_rgba8(st.format, st.width, st.height, &st.levels[0]),
        d.decode_rgba8(ot.format, ot.width, ot.height, &ot.levels[0]),
    ) else {
        return false;
    };
    let pairs = || a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0);
    a.len() == b.len()
        && (pairs().all(|(x, y)| x[..3] == [255; 3] && x[3].abs_diff(y[3]) <= 4)
            || pairs().all(|(x, y)| x[3] == 255 && x[..3].iter().all(|c| c.abs_diff(y[3]) <= 4)))
}

/// A copy of our texture `o` (same texture name, in a dictionary of the same file name) other than ours, the one the
/// mount rules make live, whose first level has digest `want`: that copy's path. The game never draws it (an archive no
/// pack mounts, or a pack file its title-update patch replaces).
fn replaced_copy(
    install: &Install,
    keys: &Keys,
    ours: &World,
    o: primase_core::Entity,
    want: &[u8],
) -> Option<String> {
    let source = &ours.get::<primase_core::Source>(o)?.0;
    let (path, name) = source.rsplit_once('#')?;
    let file = path.rsplit('\\').next()?;
    let name = name.to_lowercase();
    let suffix = format!("\\{file}");
    let hit = |t: &primase_gta::texture::Texture| {
        let (format, _) = t.format.texture()?;
        let n = format.level_size(u32::from(t.width), u32::from(t.height));
        (t.name.to_lowercase() == name
            && t.data.len() >= n
            && Sha256::digest(&t.data[..n]).as_slice() == want)
            .then_some(())
    };
    for a in &install.archives {
        for (i, e) in a.entries.iter().enumerate() {
            if e.path == path || !e.path.ends_with(&suffix) {
                continue;
            }
            let Ok(r) = a.resource(&mut std::fs::File::open(&a.file).ok()?, i, keys) else {
                continue;
            };
            let textures: Vec<primase_gta::texture::Texture> = if file.ends_with(".ytd") {
                primase_gta::texture::TextureDictionary::read_resource(&r)
                    .map(|d| d.textures.into_iter().flatten().collect())
                    .unwrap_or_default()
            } else {
                primase_gta::drawable::Drawable::read_resource(&r)
                    .ok()
                    .and_then(|d| d.textures)
                    .map(|d| d.textures.into_iter().flatten().collect())
                    .unwrap_or_default()
            };
            if textures.iter().any(|t| hit(t).is_some()) {
                return Some(e.path.clone());
            }
        }
    }
    None
}

/// The first level of a scene texture file, by digest.
fn scene_texture_digest(theirs: &World, t: primase_core::Entity, scene: &std::path::Path) -> Vec<u8> {
    let tf = theirs.get::<primase_core::TextureFile>(t).expect("scene texture");
    let bytes = std::fs::read(scene.join(&tf.path)).expect("scene texture");
    Sha256::digest(&primase_dds::read(&bytes).expect("dds").levels[0]).to_vec()
}

/// That exporter's texture alpha probe: on the largest level at most 64 texels a side, more than one texel in 200 (at
/// least 2) with alpha below 250.
fn exporter_sees_alpha(theirs: &World, t: primase_core::Entity, scene: &std::path::Path) -> bool {
    use primase_texcodec::TexDecoder;
    let tf = theirs.get::<primase_core::TextureFile>(t).expect("scene texture");
    let tex = primase_dds::read(&std::fs::read(scene.join(&tf.path)).expect("scene texture")).expect("dds");
    let size = |i: usize| ((tex.width >> i).max(1), (tex.height >> i).max(1));
    let level = (0..tex.levels.len())
        .find(|&i| size(i).0 <= 64 && size(i).1 <= 64)
        .unwrap_or(tex.levels.len() - 1);
    let (w, h) = size(level);
    let rgba = primase_texcodec::Decoder::default()
        .decode_rgba8(tex.format, w, h, &tex.levels[level])
        .expect("decode");
    let n = (w * h) as usize;
    let transparent = rgba.as_chunks::<4>().0.iter().filter(|p| p[3] < 250).count();
    transparent > (n / 200).max(1)
}

/// Whether our texture has a level that is the scene texture's first.
fn same_texture(
    theirs: &World,
    t: primase_core::Entity,
    ours: &World,
    o: primase_core::Entity,
    scene: &std::path::Path,
) -> bool {
    let want = scene_texture_digest(theirs, t, scene);
    ours.get::<primase_core::Texture>(o)
        .expect("our texture")
        .levels
        .iter()
        .any(|l| Sha256::digest(l).to_vec() == want)
}

/// A scene layered part against our primitive: the slots (null, or a layer whose base texture, alpha, sidedness and
/// texture coordinates agree), the weights, and every scene vertex's COLOR_1 against ours at its position (within one
/// step of 1/255).
#[allow(clippy::too_many_arguments)]
fn layered_diffs(
    theirs: &World,
    tm: primase_core::Entity,
    tp: &primase_core::Primitive,
    ours: &World,
    om: primase_core::Entity,
    op: &primase_core::Primitive,
    scene: &std::path::Path,
    install: &Install,
    keys: &Keys,
    debug: bool,
) -> Vec<String> {
    use primase_core::{LayeredMaterial, Semantic, Stream};
    let mut diffs = Vec::new();
    let (Some(tl), Some(ol)) = (theirs.get::<LayeredMaterial>(tm), ours.get::<LayeredMaterial>(om)) else {
        return vec!["layered: ours not layered".into()];
    };
    if tl.weights != ol.weights {
        diffs.push(format!(
            "layer weights: scene {:?}, ours {:?}",
            tl.weights, ol.weights
        ));
    }
    for (k, (a, b)) in tl.layers.iter().zip(&ol.layers).enumerate() {
        match (a, b) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                let sm = theirs
                    .get::<SkateMaterial>(a.material)
                    .expect("scene layer material");
                let (tpbr, opbr) = (
                    theirs.get::<PbrMaterial>(a.material).expect("scene layer pbr"),
                    ours.get::<PbrMaterial>(b.material).expect("our layer pbr"),
                );
                match (sm.base, opbr.base_color_texture) {
                    (Some(t), Some(o)) if same_texture(theirs, t, ours, o.texture, scene) => {}
                    (Some(t), Some(o))
                        if replaced_copy(install, keys, ours, o.texture, &scene_texture_digest(theirs, t, scene))
                            .is_some() =>
                    {
                        diffs.push(
                            "layer texture: that exporter's copy from a same-named dictionary the game's mounts replace or never mount"
                                .into(),
                        )
                    }
                    (Some(t), Some(o)) if white_alpha(theirs, t, ours, o.texture, scene) => {
                        diffs.push("layer texture: that exporter's alpha-only (A8) texture as white or grey".into())
                    }
                    (Some(t), Some(o)) if same_pixels(theirs, t, ours, o.texture, scene) => diffs.push(
                        "layer texture: that exporter's re-encoding of ours (another format, its own levels)".into(),
                    ),
                    (t, o) => {
                        if debug {
                            println!(
                                "      layer {k} texture: scene {:?}, ours {:?} {:?}",
                                t.and_then(|t| theirs.get::<Name>(t)).map(|n| n.0.clone()),
                                o.and_then(|o| ours.get::<Name>(o.texture)).map(|n| n.0.clone()),
                                o.and_then(|o| ours.get::<primase_core::Source>(o.texture)).map(|n| n.0.clone())
                            );
                        }
                        diffs.push(format!("layer {k} texture"))
                    }
                }
                if tpbr.alpha_mode != opbr.alpha_mode || tpbr.double_sided != opbr.double_sided {
                    diffs.push(format!("layer {k} alpha or sidedness"));
                }
                if a.texcoord.unwrap_or(0) != b.texcoord.unwrap_or(0) {
                    diffs.push(format!("layer {k} texcoord"));
                }
            }
            _ => diffs.push(format!("layer {k}: scene {}, ours {}", a.is_some(), b.is_some())),
        }
    }
    let channels = |p: &primase_core::Primitive| -> Option<Vec<[f32; 4]>> {
        match p.streams.get(&Semantic::Color(1))? {
            Stream::U8x4(v) => Some(v.iter().map(|c| c.map(|x| f32::from(x) / 255.0)).collect()),
            Stream::F32x4(v) => Some(v.clone()),
            Stream::U16x4(v) => Some(v.iter().map(|c| c.map(|x| f32::from(x) / 65535.0)).collect()),
            _ => None,
        }
    };
    match (channels(tp), channels(op)) {
        (Some(t), Some(o)) => {
            let key = |p: [f32; 3]| p.map(|v| (f64::from(v) * 1e4).round() as i64);
            let mut at: BTreeMap<[i64; 3], Vec<usize>> = BTreeMap::new();
            for (i, p) in op.positions().expect("positions").iter().enumerate() {
                at.entry(key(*p)).or_default().push(i);
            }
            let off = tp
                .positions()
                .expect("positions")
                .iter()
                .enumerate()
                .filter(|(vi, p)| {
                    !at.get(&key(**p)).is_some_and(|c| {
                        c.iter().any(|&c| {
                            t[*vi]
                                .iter()
                                .zip(&o[c])
                                .all(|(a, b)| (a - b).abs() <= 1.0 / 255.0 + 1e-6)
                        })
                    })
                })
                .count();
            if off > 0 {
                if debug {
                    let pos = tp.positions().expect("positions");
                    let first = (0..pos.len()).find(|&vi| {
                        !at.get(&key(pos[vi])).is_some_and(|c| {
                            c.iter().any(|&c| {
                                t[vi]
                                    .iter()
                                    .zip(&o[c])
                                    .all(|(a, b)| (a - b).abs() <= 1.0 / 255.0 + 1e-6)
                            })
                        })
                    });
                    if let Some(vi) = first {
                        println!(
                            "      layer weights: {off} of {} scene vertices off; e.g. scene {:?}, ours {:?}",
                            pos.len(),
                            t[vi],
                            at.get(&key(pos[vi]))
                                .map(|c| c.iter().map(|&c| o[c]).collect::<Vec<_>>())
                        );
                    }
                }
                diffs.push("layer weights (COLOR_1) differing at vertices".into());
            }
        }
        (t, o) => diffs.push(format!("COLOR_1: scene {}, ours {}", t.is_some(), o.is_some())),
    }
    diffs
}

/// Whether `ours`' COLOR_0 alpha is below opaque at a corner of any of `theirs`' triangles ([`corner_match`]).
fn faded_at(theirs: &primase_core::Primitive, ours: &primase_core::Primitive) -> bool {
    let alpha: Vec<bool> = match ours.streams.get(&primase_core::Semantic::Color(0)) {
        Some(primase_core::Stream::U8x4(c)) => c.iter().map(|c| c[3] < 255).collect(),
        Some(primase_core::Stream::U16x4(c)) => c.iter().map(|c| c[3] < 65535).collect(),
        _ => return false,
    };
    corner_match(theirs, ours)
        .iter()
        .flatten()
        .any(|m| m.iter().any(|&v| alpha[v as usize]))
}

/// For each triangle of `theirs` (in order), our vertices at its corners, in its corner order: our triangle with the
/// same corner positions (0.1 mm), else (a decal that exporter lifted) the triangle of ours with the nearest centroid
/// whose corners each lie within 2 cm of one of its corners. `None` where none qualifies.
fn corner_match(theirs: &primase_core::Primitive, ours: &primase_core::Primitive) -> Vec<Option<[u32; 3]>> {
    let key = |v: [f32; 3]| v.map(|x| (f64::from(x) * 1e4).round() as i64);
    let (Some(ours_pos), Some(their_pos)) = (ours.positions(), theirs.positions()) else {
        return Vec::new();
    };
    let f = |v: [f32; 3]| v.map(f64::from);
    let ours_tris = triangles(ours);
    let mut exact: BTreeMap<[[i64; 3]; 3], usize> = BTreeMap::new();
    let mut cells: BTreeMap<[i64; 3], Vec<usize>> = BTreeMap::new();
    let cell = |c: [f64; 3]| c.map(|v| (v / 0.02).floor() as i64);
    for (n, t) in ours_tris.iter().enumerate() {
        let mut k = t.map(|i| key(ours_pos[i as usize]));
        k.sort();
        exact.insert(k, n);
        cells
            .entry(cell(centroid(t.map(|i| f(ours_pos[i as usize])))))
            .or_default()
            .push(n);
    }
    let near = |a: [f64; 3], b: [f64; 3]| dot(sub(a, b), sub(a, b)) <= 0.02 * 0.02;
    triangles(theirs)
        .into_iter()
        .map(|t| {
            let mut k = t.map(|i| key(their_pos[i as usize]));
            k.sort();
            if let Some(&n) = exact.get(&k) {
                let ot = ours_tris[n];
                return Some(t.map(|i| {
                    let at = key(their_pos[i as usize]);
                    ot.iter()
                        .copied()
                        .find(|&v| key(ours_pos[v as usize]) == at)
                        .unwrap_or(ot[0])
                }));
            }
            let corners = t.map(|i| f(their_pos[i as usize]));
            let c = centroid(corners);
            let base = cell(c);
            let mut best: Option<(f64, [u32; 3])> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(list) = cells.get(&[base[0] + dx, base[1] + dy, base[2] + dz]) else {
                            continue;
                        };
                        for &n in list {
                            let ot = ours_tris[n];
                            let oc = centroid(ot.map(|i| f(ours_pos[i as usize])));
                            if !near(oc, c) {
                                continue;
                            }
                            let m = corners.map(|p| {
                                ot.iter()
                                    .copied()
                                    .filter(|&v| near(f(ours_pos[v as usize]), p))
                                    .min_by(|&a, &b| {
                                        let da = sub(f(ours_pos[a as usize]), p);
                                        let db = sub(f(ours_pos[b as usize]), p);
                                        dot(da, da).total_cmp(&dot(db, db))
                                    })
                            });
                            let [Some(a), Some(b), Some(d)] = m else { continue };
                            if a == b || b == d || a == d {
                                continue;
                            }
                            let dist = dot(sub(oc, c), sub(oc, c));
                            if best.is_none_or(|(bd, _)| dist < bd) {
                                best = Some((dist, [a, b, d]));
                            }
                        }
                    }
                }
            }
            best.map(|(_, m)| m)
        })
        .collect()
}

/// For a look at one tint difference: our palette's colours (sRGB bytes) in every row at the columns the scene part's
/// triangles pick (their first vertex's COLOR_0 blue in the geometry as read), with how many triangles pick each.
fn tint_debug(
    read: &World,
    proto: primase_core::Entity,
    g: [u32; 2],
    theirs: &primase_core::Primitive,
    material: primase_core::Entity,
    row: u32,
) {
    use primase_texcodec::TexDecoder;
    let (Some(mesh), Some(sources)) = (read.get::<Mesh>(proto), read.get::<GtaSources>(proto)) else {
        return;
    };
    let Some(k) = sources.sources.iter().position(|s| *s == g) else {
        return;
    };
    let p = &mesh.primitives[k];
    let s = read
        .get::<primase_gta::components::GtaShader>(material)
        .expect("shader");
    let Some(Some(pal)) = primase_gta::lower::texture(s, "TintPaletteTex") else {
        println!("      tint debug: no palette");
        return;
    };
    let t = read.get::<primase_core::Texture>(pal).expect("palette texture");
    let rgba = primase_texcodec::Decoder::default()
        .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
        .expect("palette");
    let (Some(primase_core::Stream::U8x4(c0)), Some(pos)) =
        (p.streams.get(&primase_core::Semantic::Color(0)), p.positions())
    else {
        println!("      tint debug: no COLOR_0");
        return;
    };
    // our triangles by corner positions (1e-4), their first vertex's column
    let key = |v: [f32; 3]| v.map(|x| (f64::from(x) * 1e4).round() as i64);
    let mut cols: BTreeMap<[[i64; 3]; 3], u32> = BTreeMap::new();
    for tri in p.indices.as_ref().expect("indices").as_chunks::<3>().0 {
        let mut k = tri.map(|i| key(pos[i as usize]));
        k.sort();
        let blue = u64::from(c0[tri[0] as usize][2]);
        cols.insert(
            k,
            (blue * u64::from(t.width) / 255).min(u64::from(t.width - 1)) as u32,
        );
    }
    let tp = theirs.positions().expect("positions");
    let mut hist: BTreeMap<u32, usize> = BTreeMap::new();
    for tri in theirs.indices.as_ref().expect("indices").as_chunks::<3>().0 {
        let mut k = tri.map(|i| key(tp[i as usize]));
        k.sort();
        if let Some(&c) = cols.get(&k) {
            *hist.entry(c).or_default() += 1;
        }
    }
    println!(
        "      tint debug: palette {}x{} {:?}, row {row}, columns picked {hist:?}",
        t.width, t.height, t.format
    );
    if hist.is_empty() {
        let uv =
            |p: &primase_core::Primitive, i: usize| match p.streams.get(&primase_core::Semantic::TexCoord(0))
            {
                Some(primase_core::Stream::F32x2(v)) => v.get(i).copied(),
                _ => None,
            };
        for i in 0..3 {
            println!(
                "        ours v{i} {:?} uv {:?} | scene v{i} {:?} uv {:?}",
                pos.get(i),
                uv(p, i),
                tp.get(i),
                uv(theirs, i)
            );
        }
        println!("        ours {} vertices, scene part {}", pos.len(), tp.len());
    }
    for &c in hist.keys() {
        let rows: Vec<[u8; 3]> = (0..t.height)
            .map(|y| {
                let o = ((y * t.width + c) * 4) as usize;
                [rgba[o], rgba[o + 1], rgba[o + 2]]
            })
            .collect();
        println!("        column {c}: rows {rows:?}");
    }
}

/// A scene part's tint as ours draws it ([`tint_at`]).
#[derive(Default)]
struct PartTint {
    /// Per scene triangle matched, gta-tint's colour (sRGB bytes) at the scene triangle's first corner (in its own
    /// order: where winding differs, ours may start elsewhere): that exporter coloured a triangle by its first
    /// vertex's palette column. Counted.
    colours: BTreeMap<[u8; 3], usize>,
    /// The scene part's triangles, and the palette colours of our whole primitive (by its triangles' first vertices).
    triangles: usize,
    palette_colours: usize,
}

/// `theirs`' tint on `ours` (gta-tint's 16-bit linear COLOR_0), its triangles matched to ours ([`corner_match`]).
fn tint_at(theirs: &primase_core::Primitive, ours: &primase_core::Primitive) -> PartTint {
    let mut out = PartTint::default();
    let Some(primase_core::Stream::U16x4(c)) = ours.streams.get(&primase_core::Semantic::Color(0)) else {
        return out;
    };
    let srgb =
        |v: u32| [0, 1, 2].map(|j| primase_image::linear_to_srgb8(f64::from(c[v as usize][j]) / 65535.0));
    out.palette_colours = triangles(ours)
        .iter()
        .map(|t| srgb(t[0]))
        .collect::<BTreeSet<_>>()
        .len();
    for m in corner_match(theirs, ours) {
        out.triangles += 1;
        if let Some(m) = m {
            *out.colours.entry(srgb(m[0])).or_default() += 1;
        }
    }
    out
}

/// The geometry's facing against its stored normals, as gta-winding sums it (each face normal, unnormalised, dotted
/// with the sum of its corners' normals), and the sum of the terms' magnitudes: how decisive the winding choice is.
fn facing(p: &primase_core::Primitive) -> (f64, f64) {
    let (Some(primase_core::Stream::F32x3(n)), Some(pos)) =
        (p.streams.get(&primase_core::Semantic::Normal), p.positions())
    else {
        return (0.0, 0.0);
    };
    let f = |v: [f32; 3]| v.map(f64::from);
    let (mut sum, mut size) = (0.0, 0.0);
    for t in triangles(p) {
        let [a, b, c] = t.map(|i| f(pos[i as usize]));
        let face = cross(sub(b, a), sub(c, a));
        let m = add(add(f(n[t[0] as usize]), f(n[t[1] as usize])), f(n[t[2] as usize]));
        let d = dot(face, m);
        sum += d;
        size += d.abs();
    }
    (sum, size)
}

/// A primitive's triangles' indices, in order.
fn triangles(p: &primase_core::Primitive) -> Vec<[u32; 3]> {
    let idx: Vec<u32> = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..p.vertex_count() as u32).collect());
    idx.as_chunks::<3>().0.to_vec()
}

/// A palette colour as that exporter wrote it as a material tint: 4 bits per channel (low bits 8), near-white none.
fn quantised_tint([r, g, b]: [u8; 3]) -> Option<[u8; 3]> {
    if r >= 0xE0 && g >= 0xE0 && b >= 0xE0 {
        None
    } else {
        Some([r & 0xF0 | 8, g & 0xF0 | 8, b & 0xF0 | 8])
    }
}

/// A primitive's triangles by their corners' positions (0.1 mm), rotated to start at the smallest corner so the
/// winding is kept, counted.
fn oriented(p: &primase_core::Primitive) -> BTreeMap<[[i64; 3]; 3], usize> {
    let mut out = BTreeMap::new();
    let pos = p.positions().expect("positions");
    let idx: Vec<u32> = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..pos.len() as u32).collect());
    for t in idx.as_chunks::<3>().0 {
        let k = [0usize, 1, 2].map(|j| pos[t[j] as usize].map(|v| (f64::from(v) * 1e4).round() as i64));
        let m = (0..3).min_by_key(|&j| k[j]).expect("a corner");
        *out.entry([k[m], k[(m + 1) % 3], k[(m + 2) % 3]]).or_default() += 1;
    }
    out
}

/// A primitive's triangles by their corners' positions (0.1 mm, corners sorted, so winding-agnostic), counted.
fn tris_of(p: &primase_core::Primitive) -> BTreeMap<[[i64; 3]; 3], usize> {
    let mut out = BTreeMap::new();
    let pos = p.positions().expect("positions");
    let idx: Vec<u32> = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..pos.len() as u32).collect());
    for t in idx.as_chunks::<3>().0 {
        let mut k = [0usize, 1, 2].map(|j| pos[t[j] as usize].map(|v| (f64::from(v) * 1e4).round() as i64));
        k.sort();
        *out.entry(k).or_default() += 1;
    }
    out
}

/// A primitive's triangles by their corners' UV0 (1e-4, corners sorted), each with its centroid, in order.
fn uv_tris(p: &primase_core::Primitive) -> Vec<([[i64; 2]; 3], [f64; 3])> {
    let (Some(primase_core::Stream::F32x2(uv)), Some(pos)) =
        (p.streams.get(&primase_core::Semantic::TexCoord(0)), p.positions())
    else {
        return Vec::new();
    };
    let idx: Vec<u32> = p
        .indices
        .clone()
        .unwrap_or_else(|| (0..uv.len() as u32).collect());
    idx.as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let mut k = t.map(|i| uv[i as usize].map(|v| (f64::from(v) * 1e4).round() as i64));
            k.sort();
            (k, centroid(t.map(|i| pos[i as usize].map(f64::from))))
        })
        .collect()
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
