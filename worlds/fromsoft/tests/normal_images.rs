//! Offline normal-image differential against the committed consumer rule.
//! Only unambiguous cached source DDS names are compared; uncovered names counted.
use primase_core::{TexRole, Texture};
mod common;
use primase_fromsoft::sekiro::normals;
use primase_testdata::real_data;
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[test]
fn source_normals_against_consumer_export() -> Result<()> {
    let Some(root) = real_data("PRIMASE_FROMSOFT_BHD_ORACLE") else {
        return Ok(());
    };
    let Some(scene) = real_data("PRIMASE_FROMSOFT_NORMAL_SCENE") else {
        return Ok(());
    };
    let mut names = BTreeMap::<String, BTreeSet<String>>::new();
    for l in std::fs::read_to_string(root.join("tpf/packages.jsonl"))?.lines() {
        let p: serde_json::Value = serde_json::from_str(l)?;
        if p["area"] != true {
            continue;
        }
        for t in p["textures"].as_array().ok_or("textures")? {
            names
                .entry(t["name"].as_str().ok_or("name")?.into())
                .or_default()
                .insert(t["sha256"].as_str().ok_or("hash")?.into());
        }
    }
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(scene.join("scene.json"))?)?;
    let mut hist = [[0u64; 256]; 4];
    let mut normalized_hist = [[0u64; 256]; 4];
    let mut compared = 0;
    let mut uncovered = 0;
    let mut resized = 0;
    let mut outside = 0u64;
    let mut alpha_data = 0u64;
    let mut max_angle = 0f64;
    for (key, value) in manifest["textures"].as_object().ok_or("textures")? {
        let Some(name) = key.strip_suffix("@normal") else {
            continue;
        };
        let Some(hashes) = names.get(name).filter(|h| h.len() == 1) else {
            uncovered += 1;
            continue;
        };
        let mut texture = primase_dds::read(&std::fs::read(
            root.join("tpf/area-dds")
                .join(format!("{}.dds", hashes.first().unwrap())),
        )?)?;
        texture.role = TexRole::Normal;
        texture.normal = Some(normals::SOURCE);
        texture.levels.truncate(1);
        let png =
            primase_image::read_png(&std::fs::read(scene.join(value["file"].as_str().ok_or("file")?))?)?
                .image;
        if texture.width != png.width || texture.height != png.height {
            resized += 1;
            continue;
        }
        let primase_image::Samples::U8(want) = png.samples else {
            return Err("non-u8 normal PNG".into());
        };
        use primase_texcodec::TexDecoder;
        let decoded = primase_texcodec::Decoder::default().decode_rgba8(
            texture.format,
            texture.width,
            texture.height,
            &texture.levels[0],
        )?;
        for (p, q) in decoded.as_chunks::<4>().0.iter().zip(want.as_chunks::<4>().0) {
            let x = f64::from(p[0]) / 127.5 - 1.;
            let y = 1. - f64::from(p[1]) / 127.5;
            let z = (1. - x * x - y * y).max(0.).sqrt();
            let len = (x * x + y * y + z * z).sqrt();
            let expected = [x / len, y / len, z / len].map(|v| ((v * 0.5 + 0.5) * 255.).round() as u8);
            for i in 0..3 {
                normalized_hist[i][expected[i].abs_diff(q[i]) as usize] += 1;
            }
            normalized_hist[3][255u8.abs_diff(q[3]) as usize] += 1;
        }
        // the earlier skate rule, as the frames give it for Sekiro's most common stored frame
        let (mut w, e, _) = common::quad(texture);
        common::image_passes(&mut w, "normalize-normals, uv-frame-normals{bitangent=-v}");
        let got = &w.get::<Texture>(e).unwrap().levels[0];
        assert_eq!(got.len(), want.len());
        for (p, q) in decoded.as_chunks::<4>().0.iter().zip(got.as_chunks::<4>().0) {
            alpha_data += u64::from(p[3] != 255);
            assert_eq!(p[3], q[3], "source alpha retained");
            let x = f64::from(p[0]) / 127.5 - 1.;
            let y = 1. - f64::from(p[1]) / 127.5;
            if x * x + y * y > 1. {
                outside += 1;
                // Compare the directions of the old and corrected quantized
                // outputs. Before byte encoding, radial projection changes no angle.
                let a = [x, y, 1. / 255.];
                let b = [q[0], q[1], q[2]].map(|v| f64::from(v) / 127.5 - 1.);
                let cross = [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ];
                let cross_length = cross.iter().map(|x| x * x).sum::<f64>().sqrt();
                let dot = a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
                max_angle = max_angle.max(cross_length.atan2(dot).to_degrees());
            }
        }
        for (i, (&a, &b)) in got.iter().zip(&want).enumerate() {
            hist[i % 4][a.abs_diff(b) as usize] += 1;
        }
        compared += 1;
    }
    println!(
        "normal reference 49ceabd: {compared} matched source images, {uncovered} missing/ambiguous cached names (including bakes), {resized} size mismatches"
    );
    for (label, table) in [("current", hist), ("normalized diagnostic", normalized_hist)] {
        for (i, h) in table.iter().enumerate() {
            let total: u64 = h.iter().sum();
            let max = h.iter().rposition(|&x| x != 0).unwrap_or(0);
            let mut sum = 0;
            let p99 = h
                .iter()
                .position(|&x| {
                    sum += x;
                    sum * 100 >= total * 99
                })
                .unwrap_or(0);
            println!("{label} channel {i}: samples {total}, max {max}, p99 {p99}");
            if i < 3 {
                assert!(max <= 1, "RGB exceeds independent decoder/rounding bound");
            }
        }
    }
    println!(
        "outside-disk pixels {outside}; prequantization angle change 0; maximum encoded-direction angle change {max_angle} degrees; nonopaque source alpha pixels {alpha_data}, all retained"
    );
    assert!(compared > 0);
    Ok(())
}
