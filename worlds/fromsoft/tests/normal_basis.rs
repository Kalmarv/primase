//! Consumer basis proof. The sample manifest contains independent stored frames,
//! not an exporter-selected green convention. Curl verdicts are measurements only.
use primase_core::gate::real_data;
use primase_fromsoft::{flver::tangent_to_scene, sekiro::normals::consumer_basis};
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
fn sampled_consumer_frames() -> Result<()> {
    let Some(root) = real_data("PRIMASE_FROMSOFT_NORMAL_FRAMES") else {
        return Ok(());
    };
    let mut count = 0;
    let mut maximum = 0f64;
    for line in std::fs::read_to_string(root.join("frames.jsonl"))?.lines() {
        let v: serde_json::Value = serde_json::from_str(line)?;
        let n: [f64; 3] = serde_json::from_value(v["n"].clone())?;
        let t: [f64; 3] = serde_json::from_value(v["t"].clone())?;
        let w = v["w"].as_f64().ok_or("w")?;
        for (x, y) in [(0.6, 0.2), (-0.3, 0.7)] {
            let e = probe(n, t, w, x, y);
            assert!(e <= 1e-6, "{} error {e}", v["key"]);
            maximum = maximum.max(e);
            count += 1;
        }
    }
    assert!(count > 0);
    println!("consumer stored-frame probes {count}, maximum component error {maximum}, bound 1e-6");
    Ok(())
}
#[test]
fn curl_verdicts_are_named_measurements() -> Result<()> {
    let Some(path) = real_data("PRIMASE_FROMSOFT_NORMAL_CURL") else {
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
fn source_bytes_wait_for_consumer_selection() {
    use primase_core::{ColorSpace, NormalConvention, TexFormat, TexRole, Texture, World};
    use primase_fromsoft::sekiro::normals::{self, Consumer};
    // In-disk XY isolates consumer axis/sign selection from radial projection.
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
    let mut world = World::default();
    let e = world.spawn();
    world.insert(e, t);
    let mut skate = World::default();
    let se = skate.spawn();
    skate.insert(se, world.get::<Texture>(e).unwrap().clone());
    normals::lower(&mut skate, Consumer::Skate).unwrap();
    let st = skate.get::<Texture>(se).unwrap();
    for (got, raw) in st.levels.iter().zip(&source) {
        for (g, r) in got.as_chunks::<4>().0.iter().zip(raw.as_chunks::<4>().0) {
            assert_eq!([g[0], g[1], g[3]], [r[0], 255 - r[1], r[3]]);
        }
    }
    assert_eq!(st.normal, Some(NormalConvention::GLTF));
    assert!(
        normals::lower(&mut skate, Consumer::Skate)
            .unwrap()
            .counts
            .is_empty()
    );
    assert_eq!(world.get::<Texture>(e).unwrap().levels, source);
    assert_eq!(world.get::<Texture>(e).unwrap().normal, Some(normals::SOURCE));
    let log = normals::lower(&mut world, Consumer::Gltf).unwrap();
    assert_eq!(log.counts["normal tangent-plane axes swapped"], 1);
    let t = world.get::<Texture>(e).unwrap();
    assert_eq!(t.normal, Some(NormalConvention::GLTF));
    for (got, raw) in t.levels.iter().zip(&source) {
        for (g, r) in got.as_chunks::<4>().0.iter().zip(raw.as_chunks::<4>().0) {
            assert_eq!([g[0], g[1], g[3]], [r[1], r[0], r[3]]);
        }
    }
    assert!(
        normals::lower(&mut world, Consumer::Gltf)
            .unwrap()
            .counts
            .is_empty()
    );
}
#[test]
fn consumer_selection_projects_outside_disk_without_mutating_source() {
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture, World};
    use primase_fromsoft::sekiro::normals::{self, Consumer};
    let source = Texture {
        width: 1,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![vec![10, 20, 30, 40]],
        color_space: ColorSpace::Linear,
        role: TexRole::Normal,
        normal: Some(normals::SOURCE),
    };
    // Source XY = (-235, -215)/255, outside the disk. Normalize by
    // sqrt(235^2 + 215^2), set Z=0 (encoded 128), then encode XY.
    // Skate flips green: (33,214); glTF swaps the source axes: (41,33).
    for (consumer, expected) in [
        (Consumer::Skate, vec![33, 214, 128, 40]),
        (Consumer::Gltf, vec![41, 33, 128, 40]),
    ] {
        let mut world = World::default();
        let e = world.spawn();
        world.insert(e, source.clone());
        assert_eq!(world.get::<Texture>(e).unwrap().levels, source.levels);
        let log = normals::lower(&mut world, consumer).unwrap();
        assert_eq!(log.counts["normal pixels projected onto unit disk"], 1);
        assert_eq!(world.get::<Texture>(e).unwrap().levels, vec![expected]);
        assert_eq!(source.levels, vec![vec![10, 20, 30, 40]]);
    }
}
#[test]
fn uv_frame_classes_remain_measurements() -> Result<()> {
    let Some(path) = real_data("PRIMASE_FROMSOFT_UV_FRAME_CENSUS") else {
        return Ok(());
    };
    let input = std::fs::read_to_string(path)?;
    let mut textures = 0;
    let mut total = 0f64;
    let mut area = std::collections::BTreeMap::<String, f64>::new();
    for line in input.lines().skip(1).filter(|s| !s.is_empty()) {
        let fields = line.split('\t').collect::<Vec<_>>();
        assert_eq!(fields.len(), 5);
        let weight: f64 = fields[1].parse()?;
        assert!(weight.is_finite() && weight >= 0.);
        total += weight;
        textures += 1;
        for class in fields[4].split(", ") {
            let (name, fraction) = class.rsplit_once(' ').ok_or("class fraction")?;
            let fraction: f64 = fraction.parse()?;
            assert!((0. ..=1.).contains(&fraction));
            *area.entry(name.to_owned()).or_default() += weight * fraction;
        }
    }
    assert!(textures > 0 && total > 0.);
    println!(
        "UV-frame census {textures} textures, area {total} m2; rounded source fractions, not per-texture conversion rules"
    );
    for (class, weight) in area {
        println!("UV-frame {class}: {} percent area", weight / total * 100.);
    }
    Ok(())
}
