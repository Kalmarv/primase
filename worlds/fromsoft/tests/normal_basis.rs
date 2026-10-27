//! Consumer basis proof. The sample manifest contains independent stored frames,
//! not an exporter-selected green convention. Curl verdicts are measurements only.
mod inputs;

use inputs::{Game, OracleSet, Tool, install, oodle, oracle};
use primase_fromsoft::geometry::tangent_to_scene;
use primase_fromsoft::sekiro::normals::consumer_basis;
mod common;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    v.map(|x| x / l)
}
fn reflect(v: [f64; 3]) -> [f64; 3] {
    [v[0], v[1], -v[2]]
}
fn probe(n: [f64; 3], t: [f64; 3], w: f64, x: f64, y: f64) -> f64 {
    let b = unit(cross(n, t).map(|x| x * w));
    let z = (1. - x * x - y * y).sqrt();
    let wanted = reflect(std::array::from_fn(|i| x * b[i] + y * t[i] + z * n[i]));
    let nt = reflect(n);
    let out = tangent_to_scene([t[0] as f32, t[1] as f32, t[2] as f32, w as f32]);
    let tt = [out[0] as f64, out[1] as f64, out[2] as f64];
    let bt = unit(cross(nt, tt).map(|x| x * out[3] as f64));
    let got: [f64; 3] = std::array::from_fn(|i| y * tt[i] + x * bt[i] + z * nt[i]);
    wanted
        .iter()
        .zip(got)
        .map(|(a, b)| (a - b).abs())
        .fold(0., f64::max)
}
#[test]
fn handedness_and_axis_swap_follow_shader_basis() {
    for w in [-1., 1.] {
        for (x, y) in [(0.6, 0.2), (-0.3, 0.7)] {
            assert!(probe([0., 1., 0.], [1., 0., 0.], w, x, y) < 1e-6);
        }
    }
    assert!(consumer_basis("MetallicValue").unwrap());
    assert!(!consumer_basis("GXFlver_ColDifSpcBumpIbl").unwrap());
    assert!(consumer_basis("unknown").is_err());
}
#[test]
fn curl_verdicts_are_named_measurements() -> Result<()> {
    let Some(path) = oracle(OracleSet::Scripts, Game::Sekiro, "normal-curl.json") else {
        return Ok(());
    };
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(path)?)?;
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut gl = Vec::new();
    for row in &rows {
        let verdict = row["verdict"].as_str().ok_or("verdict")?;
        *counts.entry(verdict.to_owned()).or_default() += 1;
        if verdict == "opengl" {
            gl.push(row["name"].as_str().ok_or("name")?.to_owned());
        }
    }
    assert!(!rows.is_empty());
    println!(
        "curl measurement {} textures: {counts:?}; OpenGL verdicts {gl:?}; no per-texture conversion exceptions",
        rows.len()
    );
    Ok(())
}

#[test]
fn source_bytes_wait_for_the_programs_choice() {
    use primase_core::{ColorSpace, Mesh, NormalConvention, Semantic, Stream, TexFormat, TexRole, Texture};
    use primase_fromsoft::sekiro::normals;
    // In-disk XY isolates axis/sign selection from radial projection.
    let source = vec![vec![100, 120, 30, 40, 150, 160, 70, 80], vec![90, 100, 110, 120]];
    let t = Texture {
        width: 2,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: source.clone(),
        color_space: ColorSpace::Linear,
        role: TexRole::Normal,
        normal: Some(normals::SOURCE),
    };
    // a writer reading supplied tangents (glTF): the source axes swapped against the stored frame
    let (mut world, texture, _) = common::quad(t.clone());
    let log = &common::image_passes(&mut world, "normalize-normals").steps[0].log;
    assert_eq!(log.counts["normal tangent-plane axes swapped"], 1);
    let got = world.get::<Texture>(texture).unwrap();
    assert_eq!(got.normal, Some(NormalConvention::GLTF));
    for (got, raw) in got.levels.iter().zip(&source) {
        for (g, r) in got.as_chunks::<4>().0.iter().zip(raw.as_chunks::<4>().0) {
            assert_eq!([g[0], g[1], g[3]], [r[1], r[0], r[3]]);
        }
    }
    // a target deriving its frame from the coordinates with the bitangent towards -v (ReSkate's Studio): the earlier
    // skate rule, red kept and green flipped, comes out of the frames, and the mesh gets the derived frame
    let (mut world, texture, mesh) = common::quad(t);
    let report = common::image_passes(&mut world, "normalize-normals, uv-frame-normals{bitangent=-v}");
    let got = world.get::<Texture>(texture).unwrap();
    for (got, raw) in got.levels.iter().zip(&source) {
        for (g, r) in got.as_chunks::<4>().0.iter().zip(raw.as_chunks::<4>().0) {
            assert_eq!([g[0], g[1], g[3]], [r[0], 255 - r[1], r[3]]);
        }
    }
    let log = &report.steps[1].log;
    assert_eq!(log.counts["primitives given the derived tangent frame"], 1);
    assert!(log.approximations.is_empty(), "{log:?}");
    let Some(Stream::F32x4(tangents)) = world.get::<Mesh>(mesh).unwrap().primitives()[0]
        .streams
        .get(&Semantic::Tangent(0))
    else {
        panic!("tangents")
    };
    // T along +u (+x); cross(N, T) * w = cross(+y, +x) = -z: along -v
    assert_eq!(tangents[0], [1., 0., 0., 1.]);
    // the bitangent towards +v instead: red and green swapped back to the source's, nothing negated
    let (mut world, texture, _) = common::quad(world.get::<Texture>(texture).unwrap().clone());
    let mut source_again = world.get::<Texture>(texture).unwrap().clone();
    source_again.normal = Some(normals::SOURCE);
    for level in &mut source_again.levels {
        for p in level.as_chunks_mut::<4>().0 {
            p[1] = 255 - p[1];
        }
    }
    world.insert(texture, source_again);
    common::image_passes(&mut world, "normalize-normals, uv-frame-normals{bitangent=+v}");
    for (got, raw) in world.get::<Texture>(texture).unwrap().levels.iter().zip(&source) {
        for (g, r) in got.as_chunks::<4>().0.iter().zip(raw.as_chunks::<4>().0) {
            assert_eq!([g[0], g[1]], [r[0], r[1]]);
        }
    }
}

#[test]
fn projection_outside_the_disk_is_the_same_in_either_frame() {
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    use primase_fromsoft::sekiro::normals;
    let source = Texture {
        width: 1,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![vec![10, 20, 30, 40]],
        color_space: ColorSpace::Linear,
        role: TexRole::Normal,
        normal: Some(normals::SOURCE),
    };
    // Source XY = (-235, -215)/255, outside the disk. Normalize by sqrt(235^2 + 215^2), set Z=0 (encoded 128),
    // then encode XY: against the stored frame the axes swap (41, 33); against the -v derived frame red stays and
    // green flips (33, 214), the bytes the earlier skate rule gave.
    for (pipeline, expected) in [
        ("normalize-normals", vec![41, 33, 128, 40]),
        (
            "normalize-normals, uv-frame-normals{bitangent=-v}",
            vec![33, 214, 128, 40],
        ),
    ] {
        let (mut world, texture, _) = common::quad(source.clone());
        let report = common::image_passes(&mut world, pipeline);
        assert_eq!(
            report.steps[0].log.counts["normal pixels projected onto unit disk"],
            1
        );
        assert_eq!(
            world.get::<Texture>(texture).unwrap().levels,
            vec![expected],
            "{pipeline}"
        );
    }
}

#[test]
fn gx_emissive_basis_rebuilds_z_independent_of_blue() {
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    use primase_fromsoft::sekiro::normals;
    assert!(consumer_basis("GXFlver_ColDifSpcBumpEmiIbl").unwrap());
    for pipeline in [
        "normalize-normals",
        "normalize-normals, uv-frame-normals{bitangent=-v}",
    ] {
        let (mut world, texture, _) = common::quad(Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![100, 120, 0, 255, 100, 120, 255, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Normal,
            normal: Some(normals::SOURCE),
        });
        common::image_passes(&mut world, pipeline);
        let pixels = &world.get::<Texture>(texture).unwrap().levels[0];
        assert_eq!(&pixels[..4], &pixels[4..]);
        assert!(pixels[2] > 200);
    }
}

#[test]
fn castle_gx_emissive_geometry() -> Result<()> {
    use fromsoft_formats::dcx::Decoder;
    use primase_core::{Instance, Mesh, Name, World};
    use primase_fromsoft::{materials::Material, sekiro};
    let Some(root) = install(Game::Sekiro) else {
        return Ok(());
    };
    let Some(root) = common::sekiro(&root)? else {
        return Ok(());
    };
    let Some(dll) = oodle(Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1024 * 1024 * 1024,
    };
    let mut world = World::default();
    let report = sekiro::read_game_area(&root, "m11_01", &decoder, &sekiro::Options::default(), &mut world)?;
    common::run(&mut world, common::LEGACY_GEOMETRY)?;
    let mut parts = 0;
    let mut triangles = 0;
    let mut placed = 0;
    for (entity, mesh) in world.iter::<Mesh>() {
        if world.get::<Name>(entity).is_none_or(|n| n.0 != "o006000") {
            continue;
        }
        placed += world
            .iter::<Instance>()
            .filter(|(_, i)| i.prototype == entity)
            .count();
        for p in mesh.primitives() {
            if p.material
                .and_then(|e| world.get::<Material>(e))
                .is_some_and(|m| m.shader == "GXFlver_ColDifSpcBumpEmiIbl")
            {
                parts += 1;
                triangles += p.indices.as_ref().unwrap().len() / 3;
            }
        }
    }
    assert_eq!((parts, triangles, placed), (1, 2638, 1));
    println!(
        "castle assembly {} placements; evidenced GX emissive: {parts} part, {triangles} triangles, {placed} placement",
        report.placements
    );
    Ok(())
}
