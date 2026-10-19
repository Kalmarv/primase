//! The slice read and its geometry lowered by the explicit legacy choices, checked against independent data: a
//! SoulsFormats dump of the area (`PRIMASE_FROMSOFT_RIGID_ORACLE`: MSB parts with their matrices, every placed model's
//! FLVER in model space) and material fixtures worked out by hand from that dump's FLVER materials and MTD names.
use primase_core::frame::{Axis, SourceFrame};
use primase_core::{Instance, Mesh, Source, Transform, World};
use primase_fromsoft::materials::Material;
use primase_fromsoft::{dcx::Decoder, sekiro, sekiro::source::PrototypeSource};
use primase_testdata::real_data;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The slice every Sekiro real-data gate reads (east min, north min, east max, north max).
const REGION: [f32; 4] = [-170., 180., -50., 300.];
/// Sekiro's source frame (the reader's documented convention): +X, +Y, -Z.
const FRAME: SourceFrame = SourceFrame::new([Axis::PosX, Axis::PosY, Axis::NegZ], 1.);

fn jsonl(path: &Path) -> Result<Vec<Value>> {
    std::fs::read_to_string(path)?
        .lines()
        .map(|l| Ok(serde_json::from_str(l)?))
        .collect()
}

/// A dumped model's vertex positions in the core frame, bucketed by millimetre for nearest lookups.
fn dumped_points(dump: &Path, file: &str) -> Result<HashMap<[i64; 3], Vec<[f32; 3]>>> {
    let bytes = std::fs::read(dump.join(file))?;
    let mut words = bytes.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w));
    let mut next = || words.next().ok_or("short dump");
    let mut out: HashMap<[i64; 3], Vec<[f32; 3]>> = HashMap::new();
    for _ in 0..next()? {
        for _ in 0..next()? {
            let p = FRAME.point([
                f32::from_bits(next()?),
                f32::from_bits(next()?),
                f32::from_bits(next()?),
            ]);
            for _ in 0..3 {
                next()?;
            }
            out.entry(p.map(|x| (f64::from(x) * 1e3).floor() as i64))
                .or_default()
                .push(p);
        }
    }
    Ok(out)
}

fn near(points: &HashMap<[i64; 3], Vec<[f32; 3]>>, p: [f32; 3], tolerance: f32) -> bool {
    let cell = p.map(|x| (f64::from(x) * 1e3).floor() as i64);
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if let Some(list) = points.get(&[cell[0] + dx, cell[1] + dy, cell[2] + dz])
                    && list
                        .iter()
                        .any(|q| (0..3).all(|i| (q[i] - p[i]).abs() <= tolerance))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// The dump's MSB map piece and object parts, by our placement source label.
fn dump_parts(dump: &Path) -> Result<BTreeMap<String, Value>> {
    jsonl(&dump.join("parts.jsonl"))?
        .into_iter()
        .map(|p| Ok((format!("m11_01_00_00:{}", p["name"].as_str().ok_or("name")?), p)))
        .collect()
}

/// A dumped part's model as our prototype key.
fn model_key(part: &Value) -> Result<String> {
    let model = part["model"].as_str().ok_or("model")?;
    Ok(if part["kind"] == "object" {
        format!("obj/{model}")
    } else {
        format!("m11_01_00_00/{model}")
    })
}

#[test]
fn whole_area_placements_in_both_directions() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(dump) = real_data("PRIMASE_FROMSOFT_RIGID_ORACLE") else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let mut world = World::default();
    let report = sekiro::read_area(root, "m11_01", &decoder, &sekiro::Options::default(), &mut world)?;
    let parts = dump_parts(&dump)?;
    // the models the dump could not read as rigid geometry are the ones the read did not find
    let undumped: std::collections::BTreeSet<String> = jsonl(&dump.join("missing-rigid.jsonl"))?
        .iter()
        .map(|r| Ok(r["key"].as_str().ok_or("key")?.to_owned()))
        .collect::<Result<_>>()?;
    assert_eq!(
        report
            .missing_models
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        undumped
    );
    let ours: BTreeMap<String, String> = world
        .iter::<Instance>()
        .map(|(e, i)| {
            Ok((
                world
                    .get::<Source>(e)
                    .ok_or("placement without source")?
                    .0
                    .clone(),
                world
                    .get::<PrototypeSource>(i.prototype)
                    .ok_or("prototype without source")?
                    .id
                    .clone(),
            ))
        })
        .collect::<Result<_>>()?;
    assert_eq!(ours.len(), report.placements, "placement labels are unique");
    // every placement a dumped part of its model; every dumped part a placement unless its model is missing or empty
    let mut missing = 0;
    let mut absent = Vec::new();
    for (source, part) in &parts {
        let key = model_key(part)?;
        match ours.get(source) {
            Some(id) => assert_eq!(*id, key, "{source}"),
            None if report.missing_models.contains(&key) => missing += 1,
            None => absent.push(source.clone()),
        }
    }
    assert!(
        ours.keys().all(|s| parts.contains_key(s)),
        "a placement that is no dumped part"
    );
    assert_eq!((report.skipped_outside, report.skipped_missing), (0, missing));
    assert_eq!(absent.len(), report.skipped_empty, "parts not placed: {absent:?}");
    println!(
        "whole area: {} parts, {} placements, {missing} of missing models, {} of empty models {absent:?}",
        parts.len(),
        ours.len(),
        report.skipped_empty
    );
    Ok(())
}

#[test]
fn installed_area_against_soulsformats() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(dump) = real_data("PRIMASE_FROMSOFT_RIGID_ORACLE") else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 512 * 1024 * 1024,
    };
    let mut world = World::default();
    let report = sekiro::read_area(
        root,
        "m11_01",
        &decoder,
        &sekiro::Options {
            region: Some(REGION),
            ..Default::default()
        },
        &mut world,
    )?;
    println!(
        "read: {} prototypes ({} far), {} placements ({} far), {} materials, {} texture files, {} shader programs",
        report.prototypes,
        report.far_prototypes,
        report.placements,
        report.far_placements,
        report.materials,
        report.textures,
        report.shader_programs
    );
    // Placements, as read (before any pass drops some): every one an MSB part, with its prototype and its matrix,
    // and every part accounted for: kept, outside the region, or of a model missing or empty.
    let parts = dump_parts(&dump)?;
    assert_eq!(
        report.placements + report.skipped_outside + report.skipped_missing + report.skipped_empty,
        parts.len(),
        "parts accounted for"
    );
    let mut largest = 0f32;
    let mut placements = 0;
    for (e, instance) in world.iter::<Instance>() {
        let source = &world.get::<Source>(e).ok_or("placement without source")?.0;
        let part = parts
            .get(source)
            .ok_or_else(|| format!("{source}: not an MSB part"))?;
        let proto = world
            .get::<PrototypeSource>(instance.prototype)
            .ok_or("prototype without source")?;
        assert_eq!(proto.id, model_key(part)?, "{source} prototype");
        let rows: [[f32; 3]; 4] = serde_json::from_value(part["matrix"].clone())?;
        let want = FRAME.transform(&Transform { cols: rows });
        let got = world.get::<Transform>(e).ok_or("placement without transform")?;
        assert_eq!(got.cols[3], want.cols[3], "{source} translation");
        for (got, want) in got.cols[..3]
            .iter()
            .flatten()
            .zip(want.cols[..3].iter().flatten())
        {
            let d = (got - want).abs() / want.abs().max(1.);
            largest = largest.max(d);
            assert!(d <= 1e-5, "{source} basis {got} vs {want}");
        }
        placements += 1;
    }
    assert!(placements > 0);
    println!(
        "placements {placements}: every one an MSB part with its model, translation exact, basis within {largest:e}"
    );

    let lowered = common::run(&mut world, common::LEGACY_GEOMETRY)?;
    for step in &lowered.steps {
        println!(
            "{}: {:?} {:?}",
            step.pass, step.log.counts, step.log.approximations
        );
    }

    // Geometry: every lowered vertex is a vertex of the dumped model, in model space, within 0.2 mm.
    let models: BTreeMap<String, String> = jsonl(&dump.join("models.jsonl"))?
        .into_iter()
        .map(|m| {
            Ok((
                m["key"].as_str().ok_or("key")?.to_owned(),
                m["file"].as_str().ok_or("file")?.to_owned(),
            ))
        })
        .collect::<Result<_>>()?;
    let mut vertices = 0;
    let mut prototypes = 0;
    let mut by_source = BTreeMap::new();
    for (e, mesh) in world.iter::<Mesh>() {
        let source = &world.get::<PrototypeSource>(e).ok_or("mesh without source")?.id;
        by_source.insert(source.clone(), e);
        let points = dumped_points(
            &dump,
            models
                .get(source)
                .ok_or_else(|| format!("{source}: not dumped"))?,
        )?;
        for (i, primitive) in mesh.primitives().iter().enumerate() {
            for &q in primitive.positions().ok_or("positions")? {
                assert!(
                    near(&points, q, 2e-4),
                    "{source} part {i}: vertex {q:?} is not in the FLVER"
                );
                vertices += 1;
            }
        }
        prototypes += 1;
    }
    println!("geometry: {prototypes} prototypes, {vertices} vertices, all FLVER vertices");

    // Materials, worked out by hand from the dump's FLVER materials (explicit texture bindings) and MTD names.
    for (source, part, class, base, normal) in [
        // M[AMSN_light]: albedo and normal bound in the FLVER; no alpha in the MTD name.
        (
            "m11_01_00_00/m000900",
            0,
            "opaque",
            Some("m11_metalparts_00_a"),
            Some("m11_metalparts_00_n"),
        ),
        // M[m11_02]_Cmn_Wood1_Multi: main layer albedo slot 8, normal slot 3, as bound in the FLVER.
        (
            "m11_01_00_00/m000900",
            1,
            "opaque",
            Some("m11_wood_13_a"),
            Some("m11_wood_07_n"),
        ),
        (
            "m11_01_00_00/m000900",
            2,
            "opaque",
            Some("m11_wood_13_a"),
            Some("m11_wood_08_n"),
        ),
        // M[m11_01]_Tessellation_Water: no albedo bound; water.
        ("m11_01_00_00/m210000", 0, "water", None, None),
    ] {
        let e = by_source
            .get(source)
            .ok_or_else(|| format!("{source}: not lowered"))?;
        let primitive = &world.get::<Mesh>(*e).ok_or("mesh")?.primitives()[part];
        let m = primitive
            .material
            .and_then(|m| world.get::<Material>(m))
            .ok_or("part without its source material")?;
        assert_eq!(m.class, class, "{source}/{part} class");
        assert_eq!(m.albedo.as_deref(), base, "{source}/{part} base");
        assert_eq!(m.normal.as_deref(), normal, "{source}/{part} normal");
    }
    Ok(())
}
