//! Full-area object SOUP against reference collision revision bd77fcb.
//! Stored classifications are exact. Independent convex tessellation is compared
//! by bidirectional, material-preserving surface probes within 0.2 mm after placement.
use primase_core::{Collision, Surfaces, World, gate::real_data};
use primase_fromsoft::{dcx::Decoder, sekiro};
use primase_skate_scene::{SkateCollision, SkateScene, SkateSoup};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[test]
#[allow(unsafe_code)]
fn placed_object_soup() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(reference) = real_data("PRIMASE_FROMSOFT_OBJECT_SCENE") else {
        return Ok(());
    };
    let Some(availability) = real_data("PRIMASE_FROMSOFT_MATERIAL_ORACLE") else {
        return Ok(());
    };
    let Some(output) = std::env::var_os("PRIMASE_FROMSOFT_OUTPUT") else {
        return Ok(());
    };
    // SAFETY: an explicitly supplied trusted native decoder.
    let dll = unsafe { primase_compress::oodle::Oodle::load(dll, primase_compress::oodle::Version::V6) }?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let archive = primase_fromsoft::Archive::open(root)?;
    let mut world = object_world(&archive, &decoder, &reference, &availability)?;
    let report = sekiro::objects::assemble(&archive, &decoder, "m11_01_00_00", None, &mut world)?;
    println!(
        "objects: models {} with collision {}; placements {} with collision {}; shapes {:?}; motion {:?}; keyframed omitted {}",
        report.models,
        report.models_with_collision,
        report.placements,
        report.placements_with_collision,
        report.triangles_by_shape,
        report.bodies_by_motion,
        report.keyframed_left_out.len()
    );
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reference.join("export_report.json"))?)?;
    let expected = &expected["object_collision"];
    assert_eq!(report.models, expected["models"].as_u64().unwrap() as usize);
    assert_eq!(
        report.placements_with_collision,
        expected["placements_with_collision"].as_u64().unwrap() as usize
    );
    let shapes: BTreeMap<String, usize> = serde_json::from_value(expected["triangles_by_shape"].clone())?;
    assert_eq!(
        report.triangles_by_shape.keys().collect::<Vec<_>>(),
        shapes.keys().collect::<Vec<_>>()
    );
    for (kind, &count) in &shapes {
        if kind != "hknpConvexPolytopeShape" {
            assert_eq!(report.triangles_by_shape[kind], count);
        }
    }
    assert_eq!(
        report.models_with_collision,
        expected["models_with_collision"].as_u64().unwrap() as usize
    );
    assert_eq!(
        report.placements,
        expected["placements"].as_u64().unwrap() as usize
    );
    assert_eq!(
        report.bodies_by_motion,
        serde_json::from_value(expected["bodies_by_motion_type"].clone())?
    );
    assert_eq!(
        report.keyframed_left_out,
        serde_json::from_value::<Vec<String>>(expected["keyframed_left_out"].clone())?
    );
    let (entity, collision) = world
        .iter::<Collision>()
        .find(|(e, _)| {
            world
                .get::<SkateCollision>(*e)
                .is_some_and(|c| c.0.table.files.iter().any(|f| f.path == "collision/objects.bin"))
        })
        .ok_or("object soup")?;
    let soup = primase_skate_scene::collision::soup(&world, entity, None)?.ok_or("converted soup")?;
    let want =
        primase_skate_scene::tables::read_soup(&std::fs::read(reference.join("collision/objects.bin"))?, 17)?;
    let want = SkateSoup {
        positions: want.positions,
        triangles: want.triangles,
        surfaces: want.surfaces,
    };
    let (forward, max_forward) = surface_probes(&soup, &want);
    let (reverse, max_reverse) = surface_probes(&want, &soup);
    println!(
        "object SOUP reference bd77fcb: triangles ours {} reference {}; material-preserving probes {} + {}, max {} m; bound 0.0002 m",
        soup.triangles.len(),
        want.triangles.len(),
        forward,
        reverse,
        max_forward.max(max_reverse)
    );
    assert!(max_forward <= 2e-4 && max_reverse <= 2e-4);
    let mut collision_world = World::default();
    let e = collision_world.spawn();
    collision_world.insert(e, collision.clone());
    collision_world.insert_resource(world.resource::<Surfaces>().unwrap().clone());
    collision_world.insert(e, world.get::<SkateCollision>(entity).unwrap().clone());
    collision_world.insert_resource(SkateScene {
        name: "object_collision_gate".into(),
        ..Default::default()
    });
    let out = std::path::PathBuf::from(output).join("object-scene");
    primase_skate_scene::write(&collision_world, &out)?;
    let mut reread = World::default();
    primase_skate_scene::read(&out, &mut reread)?;
    if let Some(validator) = real_data("PRIMASE_SCENE_SKATE") {
        let result = std::process::Command::new("dotnet")
            .arg(validator)
            .arg("validate")
            .arg(out)
            .output()?;
        println!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(result.status.success());
    }
    Ok(())
}

fn object_world(
    archive: &primase_fromsoft::Archive,
    decoder: &Decoder<'_>,
    reference: &std::path::Path,
    availability: &std::path::Path,
) -> Result<World> {
    use primase_core::{Instance, Mesh, Primitive, Semantic, Source, Stream, Topology};
    use primase_fromsoft::{
        Binder,
        flver::{MaterialGeometry, Model},
        materials,
        msb::{MapLayout, PartKind},
        mtd::MaterialDefinition,
    };
    use primase_skate_scene::SkatePrototype;
    let scene: serde_json::Value = serde_json::from_slice(&std::fs::read(reference.join("scene.json"))?)?;
    let wanted: std::collections::BTreeSet<_> = scene["prototypes"]
        .as_array()
        .ok_or("prototypes")?
        .iter()
        .filter_map(|p| p["id"].as_str())
        .filter(|s| s.starts_with("obj/"))
        .collect();
    let available: BTreeMap<String, serde_json::Value> =
        std::fs::read_to_string(availability.join("models.jsonl"))?
            .lines()
            .map(|l| -> Result<_> {
                let v: serde_json::Value = serde_json::from_str(l)?;
                Ok((v["key"].as_str().ok_or("key")?.into(), v))
            })
            .collect::<Result<_>>()?;
    let mut definitions = BTreeMap::new();
    for f in Binder::read(&archive.read("/mtd/allmaterialbnd.mtdbnd.dcx", decoder)?, decoder)?.files {
        let name = f.name.as_deref().unwrap_or("");
        if name.to_lowercase().ends_with(".mtd") {
            definitions.insert(
                materials::stem(name),
                MaterialDefinition::read(&decoder.decode_if_dcx(&f.bytes)?)?,
            );
        }
    }
    let mut world = World::default();
    let mut entities = BTreeMap::new();
    for key in wanted {
        let model = &key[4..];
        let binder = Binder::read(
            &archive.read(&format!("/obj/{model}.objbnd.dcx"), decoder)?,
            decoder,
        )?;
        let f = binder
            .files
            .iter()
            .find(|f| {
                f.name.as_deref().unwrap_or("").rsplit(['/', '\\']).next() == Some(&format!("{model}.flver"))
            })
            .ok_or("FLVER")?;
        let model = Model::read(&f.bytes)?;
        let model = primase_fromsoft::rigid::lower_model(&model, &mut primase_core::pass::PassLog::default())
            .map_err(|e| format!("{key}: {e}"))?;
        let m: Vec<_> = model
            .materials
            .iter()
            .map(|m| {
                materials::resolve(m, definitions.get(&materials::stem(&m.mtd)), |n| {
                    available[key]["available"][n].as_bool() == Some(true)
                })
            })
            .collect();
        let choices: Vec<_> = m
            .iter()
            .map(|m| MaterialGeometry {
                skip: m.skip.is_some(),
                uv_scale: m.uv_scale,
            })
            .collect();
        let mut mesh = Mesh::default();
        for part in model.geometry(&choices)? {
            let material = world.spawn();
            world.insert(material, m[part.material].clone());
            mesh.primitives.push(Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(part.mesh.positions))]),
                indices: Some(part.mesh.indices),
                material: Some(material),
            });
        }
        let e = world.spawn();
        world.insert(e, mesh);
        world.insert(
            e,
            SkatePrototype {
                id: key.into(),
                glb: None,
                key: None,
                foliage: None,
                ground: None,
                parts: Vec::new(),
                collision: None,
                other: Default::default(),
            },
        );
        entities.insert(key.to_owned(), e);
    }
    let layout = MapLayout::read(&archive.read("/map/mapstudio/m11_01_00_00.msb.dcx", decoder)?)?;
    for p in layout.parts.iter().filter(|p| p.kind == PartKind::Object) {
        let Some(i) = p.model else {
            continue;
        };
        let key = format!("obj/{}", layout.models[i].name);
        let Some(&prototype) = entities.get(&key) else {
            continue;
        };
        let e = world.spawn();
        world.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        world.insert(e, p.transform()?);
        world.insert(e, Source(format!("obj:{}", p.name)));
    }
    Ok(world)
}

fn surface_probes(source: &SkateSoup, target: &SkateSoup) -> (usize, f64) {
    const BOUND: f64 = 2e-4;
    let cell = |x: f64| (x / 2.).floor() as i64;
    let triangles: Vec<_> = target
        .triangles
        .iter()
        .map(|t| t.map(|i| target.positions[i as usize].map(f64::from)))
        .collect();
    let mut grid = BTreeMap::<(u8, i64, i64, i64), Vec<usize>>::new();
    for (i, t) in triangles.iter().enumerate() {
        let bounds = [0, 1, 2].map(|a| {
            (
                cell(t.iter().map(|v| v[a]).fold(f64::INFINITY, f64::min) - BOUND),
                cell(t.iter().map(|v| v[a]).fold(f64::NEG_INFINITY, f64::max) + BOUND),
            )
        });
        for x in bounds[0].0..=bounds[0].1 {
            for y in bounds[1].0..=bounds[1].1 {
                for z in bounds[2].0..=bounds[2].1 {
                    grid.entry((target.surfaces[i], x, y, z)).or_default().push(i);
                }
            }
        }
    }
    let mut maximum = 0f64;
    let mut probes = 0;
    let mut failures = 0;
    for (i, t) in source.triangles.iter().enumerate() {
        let vertices = t.map(|i| source.positions[i as usize].map(f64::from));
        let center = std::array::from_fn(|a| vertices.iter().map(|v| v[a]).sum::<f64>() / 3.);
        for p in vertices.into_iter().chain([center]) {
            let candidates = grid.get(&(source.surfaces[i], cell(p[0]), cell(p[1]), cell(p[2])));
            let distance = candidates
                .into_iter()
                .flatten()
                .map(|&j| primase_meshops::point_triangle_distance_squared(p, triangles[j]))
                .fold(f64::INFINITY, f64::min)
                .sqrt();
            if distance > BOUND {
                if failures < 8 {
                    eprintln!(
                        "probe triangle {i} surface {} point {p:?} distance {distance}",
                        source.surfaces[i]
                    );
                }
                failures += 1;
            }
            maximum = maximum.max(distance);
            probes += 1;
        }
    }
    eprintln!("surface probe failures {failures}/{probes}");
    (probes, maximum)
}

#[test]
fn placed_surface_gate_detects_missing_faces_displacement_and_materials() {
    let a = SkateSoup {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        surfaces: vec![3, 3],
    };
    let mut b = a.clone();
    b.triangles = vec![[0, 1, 3], [1, 2, 3]];
    assert_eq!(surface_probes(&a, &b), (8, 0.));
    assert_eq!(surface_probes(&b, &a), (8, 0.));
    b.triangles.pop();
    b.surfaces.pop();
    assert!(surface_probes(&a, &b).1 > 2e-4);
    let mut b = a.clone();
    b.positions.iter_mut().for_each(|p| p[2] += 0.001);
    assert!(surface_probes(&a, &b).1 > 2e-4);
    let mut b = a.clone();
    b.surfaces.fill(4);
    assert!(surface_probes(&a, &b).1.is_infinite());
}
