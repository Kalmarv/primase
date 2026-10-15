//! Bounded layered-material pilot. Same decoded inputs isolate shader arithmetic;
//! D3D-decoded inputs separately expose codec convention differences.
use primase_core::gate::real_data;
use primase_fromsoft::{
    layer_bake::{BaseMipBilinearWrap, Inputs, Prepared},
    materials::{Binding, LayeredMaterial, stem},
    mtd::MaterialDefinition,
};
use primase_texcodec::{Decoder, TexDecoder};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc, time::Instant};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn decode_error(e: impl std::fmt::Display) -> primase_fromsoft::Error {
    primase_fromsoft::Error::Invalid {
        format: "pilot texture",
        offset: 0,
        detail: e.to_string(),
    }
}

#[test]
fn five_permutation_pilot() -> Result<()> {
    let Some(root) = real_data("PRIMASE_FROMSOFT_LAYER_PILOT") else {
        return Ok(());
    };
    let cases: Value = serde_json::from_slice(&std::fs::read(root.join("cases.json"))?)?;
    let references: Value = serde_json::from_slice(&std::fs::read(root.join("reference.json"))?)?;
    let mut failures = Vec::new();
    for case in cases.as_array().ok_or("cases")? {
        let id = case["id"].as_u64().ok_or("id")?;
        let metadata: Value = serde_json::from_slice(&std::fs::read(root.join(format!("{id}.json")))?)?;
        let definition = MaterialDefinition::read(&std::fs::read(root.join(format!("{id}.mtd")))?)?;
        let mut bindings = BTreeMap::new();
        for t in &definition.textures {
            if !t.path.is_empty() {
                bindings.insert(
                    t.kind.clone(),
                    Binding {
                        texture: stem(&t.path),
                        scale: [1.; 2],
                        uv_number: t.uv_number,
                    },
                );
            }
        }
        for t in metadata["bindings"].as_array().ok_or("bindings")? {
            let path = t["path"].as_str().ok_or("path")?;
            if path.is_empty() {
                continue;
            }
            bindings.insert(
                t["kind"].as_str().ok_or("kind")?.into(),
                Binding {
                    texture: stem(path),
                    scale: [
                        t["scale"][0].as_f64().ok_or("scale")? as f32,
                        t["scale"][1].as_f64().ok_or("scale")? as f32,
                    ],
                    uv_number: 1,
                },
            );
        }
        let material = LayeredMaterial {
            mtd: case["mtd"].as_str().ok_or("mtd")?.into(),
            definition,
            bindings,
        };
        for mode in ["same-input", "D3D"] {
            let prepared = Prepared::new(
                &std::fs::read(root.join(format!("{id}.ppo")))?,
                &material,
                |name, srgb| {
                    let Some(m) = metadata["images"].get(name) else {
                        return Ok(None);
                    };
                    let (w, h, rgba) = if mode == "D3D" {
                        let t = primase_dds::read(&std::fs::read(root.join(m["dds"].as_str().unwrap()))?)
                            .map_err(decode_error)?;
                        let rgba = Decoder::default()
                            .decode_rgba8(t.format, t.width, t.height, &t.levels[0])
                            .map_err(decode_error)?;
                        let oracle = std::fs::read(root.join(m["file"].as_str().unwrap()))?;
                        assert_eq!(rgba.len(), oracle.len());
                        let mut max = [0u8; 4];
                        let mut changed = [0u64; 4];
                        for (i, (&a, &b)) in rgba.iter().zip(&oracle).enumerate() {
                            max[i % 4] = max[i % 4].max(a.abs_diff(b));
                            changed[i % 4] += u64::from(a != b);
                        }
                        println!(
                            "input {name} {:?}: max {max:?}, changed components {changed:?}",
                            t.format
                        );
                        (t.width, t.height, rgba)
                    } else {
                        (
                            m["width"].as_u64().unwrap() as u32,
                            m["height"].as_u64().unwrap() as u32,
                            std::fs::read(root.join(m["file"].as_str().unwrap()))?,
                        )
                    };
                    Ok(Some(Arc::new(BaseMipBilinearWrap::new(w, h, &rgba, srgb)?)))
                },
            )?;
            println!(
                "case {id}/{mode}: decoded bytes {}, policies {:?}",
                prepared.decoded_bytes, prepared.policies
            );
            if case["shader"] == "M[Multiple]" && case["switch"] == 27 {
                assert_eq!(
                    prepared
                        .policies
                        .get("preview default for missing NormalMap binding"),
                    Some(&1)
                );
                assert!(
                    !prepared
                        .policies
                        .contains_key("preview default for missing ? binding")
                );
            }
            for r in references
                .as_array()
                .ok_or("reference")?
                .iter()
                .filter(|r| r["id"] == id)
            {
                let variant = r["variant"].as_u64().ok_or("variant")?;
                let params = match variant {
                    0 => Inputs::default(),
                    1 => Inputs {
                        weights_snow: [0.25, 0.5, 0.75, 0.25],
                        tint_color: [0.8, 0.7, 0.6, 1.],
                        detail: 1.,
                    },
                    2 => Inputs {
                        weights_snow: [1.; 4],
                        tint_color: [1.; 4],
                        detail: 0.,
                    },
                    _ => return Err("unknown variant".into()),
                };
                let size = r["size"].as_u64().ok_or("size")? as u32;
                let period = [
                    r["period"][0].as_f64().ok_or("period")? as f32,
                    r["period"][1].as_f64().ok_or("period")? as f32,
                ];
                let timer = Instant::now();
                let bake = prepared.bake(size, period, params)?;
                let seconds = timer.elapsed().as_secs_f64();
                println!(
                    "case {id}/{mode}/{} variant {variant}: {seconds:.6}s, {} instructions/pixel, {} samples/pixel",
                    r["cap"],
                    bake.instructions / (size as u64 * size as u64),
                    bake.samples / (size as u64 * size as u64)
                );
                for (kind, pixels) in [("albedo", &bake.albedo), ("normal", &bake.normal)] {
                    let reference =
                        std::fs::read(root.join(format!("{id}-{}-{variant}-{kind}.rgba", r["cap"])))?;
                    assert_eq!(pixels.len(), reference.len());
                    let mut hist = [[0u64; 256]; 4];
                    for (i, (a, b)) in pixels.iter().zip(&reference).enumerate() {
                        hist[i % 4][a.abs_diff(*b) as usize] += 1;
                    }
                    let max: [usize; 4] =
                        std::array::from_fn(|c| hist[c].iter().rposition(|&n| n != 0).unwrap());
                    let p99: [usize; 4] = std::array::from_fn(|c| {
                        let target = (size as u64 * size as u64 * 99).div_ceil(100);
                        let mut sum = 0;
                        hist[c]
                            .iter()
                            .position(|n| {
                                sum += n;
                                sum >= target
                            })
                            .unwrap()
                    });
                    println!("{kind}: max {max:?}, p99 {p99:?}");
                    // One encoded level is the pilot hypothesis, not permission
                    // to widen the bound if a nonlinear shader amplifies error.
                    if max.iter().any(|&v| v > 1) {
                        failures.push(format!("{id}/{mode}/{}/{variant}/{kind}: {max:?}", r["cap"]));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}
