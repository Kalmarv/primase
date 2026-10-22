//! The glTF reader's images and the material lowering, end to end.
//!
//! - A hand-worked document: one PNG image sampled as base colour and as a normal map (two texture entities, one per
//!   use), a JPEG, a KTX2 and an unknown image refused by name, a sampler, a texture transform; read, lowered by the
//!   pass manager with its checks on, and every value compared with what the document states.
//! - T1 (`PRIMASE_KHRONOS_GLTF`, third-party files): every image of every fixture is identified as the Khronos
//!   validator and Pillow identify it (`validator.json`'s `info.resources`, and `images.json`, which `dump_images.py`
//!   writes with Pillow): a PNG is read at the size the validator reports with exactly the RGBA8 pixels Pillow decodes
//!   (sha256 of the rows), a JPEG is refused by name, and each lowered material states the factors its JSON states
//!   (defaults per the specification) with a texture exactly where its image was read.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;

use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::pbr::{AlphaMode, Filter, PbrMaterial, Wrap};
use primase_core::{ColorSpace, TexFormat, TexRole, Texture, World};
use primase_gltf::{GltfMaterialTextures, GltfSlot, read_with_images};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Run `lower-gltf-materials` with the pass manager's checks on.
fn lower(w: &mut World) -> RunReport {
    let mut reg = PassRegistry::new();
    primase_gltf::register(&mut reg);
    let pipeline = reg.parse("lower-gltf-materials").unwrap();
    let mut report = RunReport::default();
    PassManager::new(reg)
        .with_pass_checks(true)
        .run(w, &pipeline, &mut report)
        .unwrap();
    report
}

fn pad(bin: &mut Vec<u8>) {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
}

#[test]
fn a_document_with_images_reads_and_lowers_to_what_it_states() {
    let png = common::png(2, 1, |x, _| [10 + x as u8 * 100, 20, 30, 255]);
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];
    let ktx2 = b"\xABKTX 20\xBB\r\n\x1A\n\0\0\0\0";
    let mut bin = Vec::new();
    for v in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
        for c in v {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let mut views = vec![json!({"buffer": 0, "byteOffset": 0, "byteLength": 36})];
    for image in [&png[..], &jpeg[..], &ktx2[..]] {
        pad(&mut bin);
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": image.len()}));
        bin.extend_from_slice(image);
    }
    pad(&mut bin);
    let doc = json!({
        "asset": {"version": "2.0"},
        "extensionsUsed": ["KHR_texture_transform"],
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "material": 0}]}],
        "materials": [{
            "name": "stated",
            "pbrMetallicRoughness": {
                "baseColorFactor": [0.5, 0.5, 0.5, 1.0],
                "baseColorTexture": {"index": 0, "extensions": {"KHR_texture_transform": {"scale": [2.0, 3.0]}}},
                "metallicRoughnessTexture": {"index": 1}
            },
            "normalTexture": {"index": 0, "scale": 0.5},
            "emissiveTexture": {"index": 2},
            "alphaMode": "BLEND",
            "doubleSided": true
        }],
        "samplers": [{"magFilter": 9728, "minFilter": 9728, "wrapS": 33071}],
        "textures": [{"source": 0, "sampler": 0}, {"source": 1}, {"source": 2}],
        "images": [
            {"name": "ramp", "bufferView": 1, "mimeType": "image/png"},
            {"bufferView": 2, "mimeType": "image/jpeg"},
            {"bufferView": 3, "mimeType": "image/ktx2"}
        ],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                       "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]}],
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}]
    });
    let glb = primase_glb::assemble(doc.to_string().as_bytes(), &bin).unwrap();
    let gltf = primase_glb::Gltf::from_glb(&glb).unwrap();
    let mut w = World::new();
    let report = primase_gltf::read::read(&gltf, "doc.glb", &mut w).unwrap();
    assert_eq!(w.verify_components(), vec![]);
    assert_eq!((report.textures, report.images, report.images_read), (3, 3, 1));
    assert_eq!(
        report.images_refused,
        BTreeMap::from([
            ("JPEG (no JPEG decoder in the toolkit)".to_owned(), 1),
            ("KTX2 (KTX2/Basis not read)".to_owned(), 1),
        ])
    );
    let notices = report.notices();
    assert!(
        notices
            .iter()
            .any(|n| n.reason == "images refused: KTX2 (KTX2/Basis not read)" && n.count == 1)
    );

    // the PNG is read once per use: sRGB colour for the base colour, linear normal for the normal map
    let (material, slots) = w
        .iter::<GltfMaterialTextures>()
        .next()
        .map(|(e, s)| (e, s.clone()))
        .unwrap();
    assert_eq!(
        slots.0.keys().copied().collect::<Vec<_>>(),
        [GltfSlot::BaseColor, GltfSlot::Normal]
    );
    let (base, normal) = (slots.0[&GltfSlot::BaseColor], slots.0[&GltfSlot::Normal]);
    assert_ne!(base.texture, normal.texture);
    let pixels = [10, 20, 30, 255, 110, 20, 30, 255];
    let t = |e| w.get::<Texture>(e).unwrap().clone();
    let (b, n) = (t(base.texture), t(normal.texture));
    for x in [&b, &n] {
        assert_eq!((x.width, x.height, x.format), (2, 1, TexFormat::Rgba8Unorm));
        assert_eq!(x.levels, [pixels.to_vec()]);
    }
    assert_eq!(
        (b.color_space, b.role, b.normal),
        (ColorSpace::Srgb, TexRole::Color, None)
    );
    assert_eq!((n.color_space, n.role), (ColorSpace::Linear, TexRole::Normal));
    assert_eq!(n.normal, Some(primase_core::NormalConvention::GLTF));
    assert_eq!(base.filters.mag, Some(Filter::Nearest));
    assert_eq!(base.wrap, [Wrap::Clamp, Wrap::Repeat]);

    let run = lower(&mut w);
    let m = w.get::<PbrMaterial>(material).unwrap();
    assert_eq!(m.base_color, [0.5, 0.5, 0.5, 1.0]);
    assert_eq!((m.metallic, m.roughness), (1.0, 1.0));
    assert_eq!(
        (m.normal_scale, m.alpha_mode, m.double_sided),
        (0.5, AlphaMode::Blend, true)
    );
    let base_ref = m.base_color_texture.unwrap();
    assert_eq!(base_ref.texture, base.texture);
    assert_eq!(base_ref.transform.unwrap().scale, [2.0, 3.0]);
    assert_eq!(m.normal_texture.unwrap().texture, normal.texture);
    assert!(m.metallic_roughness_texture.is_none() && m.emissive_texture.is_none());
    let log = &run.steps[0].log;
    assert_eq!(
        log.approximations
            .get("texture slots left untextured: their image was not read"),
        Some(&2)
    );
}

/// What `info.resources` of a model's `validator.json` says of image `i`: its MIME type, width and height.
fn validator_image(validator: &Value, i: usize) -> (String, u64, u64) {
    let pointer = format!("/images/{i}");
    let r = validator["info"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pointer"] == pointer)
        .unwrap_or_else(|| panic!("the validator lists no {pointer}"));
    (
        r["mimeType"].as_str().unwrap().to_owned(),
        r["image"]["width"].as_u64().unwrap(),
        r["image"]["height"].as_u64().unwrap(),
    )
}

#[test]
fn t1_khronos_images_and_materials_read_as_the_validator_and_pillow_see_them() {
    let Some(root) = primase_testdata::real_data("PRIMASE_KHRONOS_GLTF") else {
        return;
    };
    let mut models: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    models.sort();
    let (mut png, mut jpeg, mut slots_read, mut slots_unread) = (0, 0, 0, 0);
    for dir in models {
        let model = dir.file_name().unwrap().to_string_lossy().into_owned();
        let glb = dir.join(format!("{model}.glb"));
        let gltf = if glb.exists() {
            primase_glb::Gltf::from_glb(&std::fs::read(&glb).unwrap()).unwrap()
        } else {
            let json = std::fs::read(dir.join(format!("{model}.gltf"))).unwrap();
            primase_glb::Gltf::from_gltf(&json, |_, uri| {
                std::fs::read(dir.join(uri)).map_err(|e| e.to_string())
            })
            .unwrap()
        };
        let mut w = World::new();
        let report = read_with_images(&gltf, &model, &mut w, &mut |uri| {
            std::fs::read(dir.join(uri)).map_err(|e| e.to_string())
        })
        .unwrap_or_else(|e| panic!("{model}: {e}"));
        assert_eq!(w.verify_components(), vec![], "{model}");
        lower(&mut w);

        // the images: Pillow's reading and the validator's, per image
        let validator: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("validator.json")).unwrap()).unwrap();
        let pillow: Value = std::fs::read_to_string(dir.join("images.json"))
            .map(|s| serde_json::from_str(&s).unwrap())
            .unwrap_or_else(|_| json!({"images": []}));
        let images = pillow["images"].as_array().unwrap();
        assert_eq!(images.len(), report.images, "{model}: images");
        let (mut want_png, mut want_jpeg) = (0, 0);
        for (i, image) in images.iter().enumerate() {
            let (mime, width, height) = validator_image(&validator, i);
            assert_eq!(
                (
                    image["width"].as_u64().unwrap(),
                    image["height"].as_u64().unwrap()
                ),
                (width, height),
                "{model} image {i}: Pillow and the validator agree on the size"
            );
            match (image["format"].as_str().unwrap(), mime.as_str()) {
                ("PNG", "image/png") => want_png += 1,
                ("JPEG", "image/jpeg") => want_jpeg += 1,
                other => panic!("{model} image {i}: {other:?}"),
            }
            // every image of the fixtures is sampled by a core slot, so each is read or refused
            if image["format"] == "PNG" {
                let found = w
                    .iter::<Texture>()
                    .filter(|(_, t)| (u64::from(t.width), u64::from(t.height)) == (width, height))
                    .any(|(_, t)| {
                        let digest: String = Sha256::digest(&t.levels[0])
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect();
                        t.format == TexFormat::Rgba8Unorm && digest == image["rgba8_sha256"].as_str().unwrap()
                    });
                assert!(found, "{model} image {i}: no texture holds Pillow's RGBA8 pixels");
            }
        }
        assert_eq!(report.images_read, want_png, "{model}: PNGs read");
        assert_eq!(
            report
                .images_refused
                .iter()
                .map(|(k, v)| (k.starts_with("JPEG"), *v))
                .collect::<Vec<_>>(),
            if want_jpeg > 0 {
                vec![(true, want_jpeg)]
            } else {
                vec![]
            },
            "{model}: JPEGs refused by name"
        );
        png += want_png;
        jpeg += want_jpeg;

        // the materials: what the JSON states, defaults per the specification; a texture where its image was read
        for (e, o) in w
            .opaque_components()
            .filter(|(n, _)| *n == "gltf.material")
            .flat_map(|(_, c)| c.iter())
        {
            let primase_core::OpaqueData::Json(j) = &o.data else {
                panic!("{model}: material data")
            };
            let m = w
                .get::<PbrMaterial>(*e)
                .unwrap_or_else(|| panic!("{model}: a material was not lowered"));
            let pbr = &j["pbrMetallicRoughness"];
            let factor = |v: &Value, d: f64| v.as_f64().unwrap_or(d) as f32;
            assert_eq!(m.metallic, factor(&pbr["metallicFactor"], 1.0), "{model}");
            assert_eq!(m.roughness, factor(&pbr["roughnessFactor"], 1.0), "{model}");
            let color = pbr["baseColorFactor"].as_array().map_or([1.0; 4], |a| {
                std::array::from_fn(|k| a[k].as_f64().unwrap() as f32)
            });
            assert_eq!(m.base_color, color, "{model}");
            assert_eq!(
                m.double_sided,
                j["doubleSided"].as_bool().unwrap_or(false),
                "{model}"
            );
            for slot in GltfSlot::ALL {
                let stated = slot.info(j).is_some();
                let read = w
                    .get::<GltfMaterialTextures>(*e)
                    .is_some_and(|t| t.0.contains_key(&slot));
                let lowered = match slot {
                    GltfSlot::BaseColor => m.base_color_texture.is_some(),
                    GltfSlot::MetallicRoughness => m.metallic_roughness_texture.is_some(),
                    GltfSlot::Normal => m.normal_texture.is_some(),
                    GltfSlot::Occlusion => m.occlusion_texture.is_some(),
                    GltfSlot::Emissive => m.emissive_texture.is_some(),
                };
                assert_eq!(lowered, read, "{model}: a lowered slot is a read one");
                if stated {
                    if read {
                        slots_read += 1;
                    } else {
                        slots_unread += 1;
                    }
                }
            }
        }
    }
    println!(
        "T1: {png} PNGs read as Pillow reads them, {jpeg} JPEGs refused; {slots_read} slots textured, {slots_unread} left untextured"
    );
}
