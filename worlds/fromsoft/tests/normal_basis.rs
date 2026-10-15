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
fn axis_conversion_preserves_encoded_values_and_mips() {
    use primase_core::{ColorSpace, NormalConvention, NormalGreen, NormalZ, TexFormat, TexRole, Texture};
    let mut t = Texture {
        width: 2,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![vec![10, 20, 30, 40, 50, 60, 70, 80], vec![90, 100, 110, 120]],
        color_space: ColorSpace::Linear,
        role: TexRole::Normal,
        normal: None,
    };
    primase_fromsoft::sekiro::normals::swap_normal_axes(&mut t).unwrap();
    assert_eq!(
        t.levels,
        [vec![20, 10, 30, 40, 60, 50, 70, 80], vec![100, 90, 110, 120]]
    );
    assert_eq!(
        t.normal,
        Some(NormalConvention {
            green: NormalGreen::Up,
            z: NormalZ::Rebuild
        })
    );
}
