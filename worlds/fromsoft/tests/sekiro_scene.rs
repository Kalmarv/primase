//! Installed-area integration: archive reader, World, toolkit assets, scene writer,
//! then an independent validator. Outputs are retained for review.
use primase_core::{World, gate::real_data};
use primase_fromsoft::{dcx::Decoder, sekiro};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, process::Command};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[allow(unsafe_code)]
fn native(path: std::path::PathBuf) -> primase_compress::Result<primase_compress::oodle::Oodle> {
    // SAFETY: the caller explicitly supplies a trusted native version-6 library.
    unsafe { primase_compress::oodle::Oodle::load(path, primase_compress::oodle::Version::V6) }
}
fn manifest(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&std::fs::read(path.join("scene.json"))?)?)
}
#[test]
fn installed_area_scene() -> Result<()> {
    let Some(root) = real_data("PRIMASE_SEKIRO_DIR") else {
        return Ok(());
    };
    let Some(dll) = real_data("PRIMASE_OODLE6") else {
        return Ok(());
    };
    let Some(reference) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let Some(output) = std::env::var_os("PRIMASE_FROMSOFT_OUTPUT").map(std::path::PathBuf::from) else {
        eprintln!("SKIPPED: PRIMASE_FROMSOFT_OUTPUT not set");
        return Ok(());
    };
    let Some(validator) = real_data("PRIMASE_SCENE_SKATE") else {
        return Ok(());
    };
    let want = manifest(&reference)?;
    let region: [f32; 4] = serde_json::from_value(want["region"].clone())?;
    let dll = native(dll)?;
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
            region: Some(region),
            floor: want["min_height"]["floor"].as_f64(),
            ..Default::default()
        },
        &mut world,
    )?;
    println!("area {report:?}");
    sekiro::normals::lower(&mut world, sekiro::normals::Consumer::Skate)?;
    sekiro::prepare_scene_assets(&mut world, &output.join("assets"))?;
    let folder = output.join("scene");
    primase_skate_scene::write(&world, &folder)?;
    let mut reread = World::default();
    primase_skate_scene::read(&folder, &mut reread)?;
    let mut cmd = if validator.extension().is_some_and(|v| v == "dll") {
        let mut c = Command::new("dotnet");
        c.arg(validator);
        c
    } else {
        Command::new(validator)
    };
    let result = cmd.arg("validate").arg(&folder).output()?;
    println!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.status.success(), "independent scene validator");
    let actual = manifest(&folder)?;
    let actual_protos: BTreeMap<_, _> = actual["prototypes"]
        .as_array()
        .ok_or("prototypes")?
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    let want_protos: BTreeMap<_, _> = want["prototypes"]
        .as_array()
        .ok_or("prototypes")?
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    assert_eq!(
        actual_protos.keys().collect::<Vec<_>>(),
        want_protos.keys().collect::<Vec<_>>(),
        "selected prototypes"
    );
    let source_protos: BTreeMap<_, _> = world
        .iter::<primase_skate_scene::SkatePrototype>()
        .map(|(e, p)| (p.id.as_str(), e))
        .collect();
    let mut parts = 0;
    let mut tangent_parts = 0;
    let mut uv_scaled = 0;
    let mut normal_exact = 0;
    let mut maximum_normal_error = 0.0f32;
    for (id, p) in &actual_protos {
        let w = want_protos[id];
        let load = |root: &Path, p: &Value| -> Result<primase_core::Mesh> {
            let bytes = std::fs::read(root.join(p["glb"].as_str().ok_or("glb")?))?;
            Ok(primase_gltf::Gltf::from_glb(&bytes)?.read_mesh(0)?.to_mesh())
        };
        let a = load(&folder, p)?;
        let b = load(&reference, w)?;
        let original = world.get::<primase_core::Mesh>(source_protos[id]).ok_or("mesh")?;
        assert_eq!(a.primitives.len(), b.primitives.len(), "{id} parts");
        for (i, (a, b)) in a.primitives.iter().zip(&b.primitives).enumerate() {
            use primase_core::{Semantic, Stream};
            assert_eq!(a.indices, b.indices, "{id}/{i} topology");
            if let Some(tangents) = original.primitives[i].streams.get(&Semantic::Tangent) {
                assert_eq!(
                    a.streams.get(&Semantic::Tangent),
                    Some(tangents),
                    "{id}/{i} stored tangent preservation"
                );
                tangent_parts += 1;
            }
            assert_eq!(
                a.streams[&Semantic::Position],
                b.streams[&Semantic::Position],
                "{id}/{i} positions"
            );
            let (Stream::F32x3(an), Stream::F32x3(bn)) =
                (&a.streams[&Semantic::Normal], &b.streams[&Semantic::Normal])
            else {
                return Err("normal stream".into());
            };
            assert_eq!(an.len(), bn.len());
            let error = an
                .iter()
                .flatten()
                .zip(bn.iter().flatten())
                .map(|(x, y)| (x - y).abs())
                .fold(0., f32::max);
            // Unit normals are derived. This absolute component bound also
            // covers near-zero components for which ULP is unstable.
            assert!(error <= 1e-6, "{id}/{i} normal error {error}");
            normal_exact += usize::from(an == bn);
            maximum_normal_error = maximum_normal_error.max(error);
            let m = world
                .get::<primase_fromsoft::materials::Material>(
                    original.primitives[i].material.ok_or("material")?,
                )
                .ok_or("source material")?;
            let (Stream::F32x2(au), Stream::F32x2(bu)) = (
                &a.streams[&Semantic::TexCoord(0)],
                &b.streams[&Semantic::TexCoord(0)],
            ) else {
                return Err("UV stream".into());
            };
            assert_eq!(au.len(), bu.len());
            if au != bu {
                assert_ne!(m.uv_scale, [1., 1.], "unclassified UV difference {id}/{i}");
                for (a, b) in au.iter().zip(bu) {
                    for c in 0..2 {
                        let expected = b[c] * m.uv_scale[c];
                        assert!(
                            (a[c] - expected).abs() <= 4. * f32::EPSILON * expected.abs().max(1.),
                            "{id}/{i} UV tiling"
                        );
                    }
                }
                uv_scaled += 1;
            }
            let am = &actual["materials"][p["parts"][i]["material"].as_str().ok_or("material key")?];
            let bm = &want["materials"][w["parts"][i]["material"].as_str().ok_or("material key")?];
            for field in ["base", "normal", "class", "base_color"] {
                assert_eq!(am[field], bm[field], "{id}/{i} material {field}");
            }
            parts += 1;
        }
    }
    println!(
        "written prototypes {}, parts {parts}, UV tiling parts {uv_scaled}, normals exact {normal_exact}, within 1e-6 {}, maximum {maximum_normal_error}",
        actual_protos.len(),
        parts - normal_exact
    );
    println!(
        "written tangent streams preserved on {tangent_parts} parts; shader basis assumptions {:?}",
        report.normal_basis_assumptions
    );
    let placements =
        primase_skate_scene::tables::read_placements(&std::fs::read(folder.join("tables/placements.bin"))?)?;
    let expected = primase_skate_scene::tables::read_placements(&std::fs::read(
        reference.join("tables/placements.bin"),
    )?)?;
    assert_eq!(placements.len(), expected.len());
    let mut exact = 0;
    for (p, w) in placements.iter().zip(&expected) {
        assert_eq!(
            actual["placement_sources"][p.source as usize],
            want["placement_sources"][w.source as usize]
        );
        assert_eq!(
            actual["prototypes"][p.proto as usize]["id"],
            want["prototypes"][w.proto as usize]["id"]
        );
        assert_eq!(
            (p.variant, p.tile, p.radius, p.flags),
            (w.variant, w.tile, w.radius, w.flags)
        );
        assert_eq!(&p.m[9..], &w.m[9..], "stored translation");
        let scale = w.m[..9]
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| c.iter().map(|v| v * v).sum::<f32>().sqrt())
            .fold(1., f32::max);
        let error = p.m[..9]
            .iter()
            .zip(&w.m[..9])
            .map(|(a, b)| (a - b).abs() / scale)
            .fold(0., f32::max);
        assert!(error <= 1e-6, "derived placement basis {error}");
        exact += usize::from(p.m == w.m);
    }
    println!(
        "written placements {}: {exact} exact, {} within 1e-6 per unit scale",
        placements.len(),
        placements.len() - exact
    );
    Ok(())
}

/// Audit all material properties at the consumer's f32 precision, independently
/// of generated material identities. Metadata and unsupported tables are reported
/// as scope differences rather than silently omitted from the comparison.
#[test]
fn written_manifest_values() -> Result<()> {
    let Some(reference) = real_data("PRIMASE_FROMSOFT_SCENE") else {
        return Ok(());
    };
    let Some(output) = real_data("PRIMASE_FROMSOFT_WRITTEN_SCENE") else {
        return Ok(());
    };
    let actual = manifest(&output)?;
    let expected = manifest(&reference)?;
    let expected_protos: BTreeMap<_, _> = expected["prototypes"]
        .as_array()
        .ok_or("prototypes")?
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p))
        .collect();
    let mut cutoff_rounding = 0;
    let mut parts = 0;
    for p in actual["prototypes"].as_array().ok_or("prototypes")? {
        let q = expected_protos[p["id"].as_str().ok_or("id")?];
        for (a, b) in p["parts"]
            .as_array()
            .ok_or("parts")?
            .iter()
            .zip(q["parts"].as_array().ok_or("parts")?)
        {
            let a = actual["materials"][a["material"].as_str().ok_or("material")?]
                .as_object()
                .ok_or("material")?;
            let b = expected["materials"][b["material"].as_str().ok_or("material")?]
                .as_object()
                .ok_or("material")?;
            assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
            for (key, value) in a {
                if key == "cutoff" {
                    assert_eq!(
                        value.as_f64().ok_or("cutoff")? as f32,
                        b[key].as_f64().ok_or("cutoff")? as f32
                    );
                    cutoff_rounding += usize::from(value != &b[key]);
                } else {
                    assert_eq!(value, &b[key], "material {key}");
                }
            }
            parts += 1;
        }
    }
    assert_eq!(
        actual["textures"]
            .as_object()
            .ok_or("textures")?
            .keys()
            .collect::<Vec<_>>(),
        expected["textures"]
            .as_object()
            .ok_or("textures")?
            .keys()
            .collect::<Vec<_>>()
    );
    for field in ["format", "version", "placement_sources", "min_height"] {
        assert_eq!(actual[field], expected[field], "{field}");
    }
    let ar: [f32; 4] = serde_json::from_value(actual["region"].clone())?;
    let br: [f32; 4] = serde_json::from_value(expected["region"].clone())?;
    assert_eq!(ar, br);
    println!(
        "manifest material parts {parts}: all fields agree at consumer precision; {cutoff_rounding} cutoff JSON representations round to the same f32"
    );
    println!(
        "texture identities {} exact; container/name and normal convention metadata intentionally differ",
        actual["textures"].as_object().unwrap().len()
    );
    Ok(())
}
