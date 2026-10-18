//! Emission brighter than 1 is written as `KHR_materials_emissive_strength`; a material at the default strength is
//! written as before.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::PbrMaterial;
use primase_core::{Mesh, Transform, World};

mod common;

fn world(strength: f32) -> World {
    let mut w = World::new();
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            emissive: [1.0, 0.5, 0.25],
            emissive_strength: strength,
            ..PbrMaterial::default()
        },
    );
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![common::rich_primitive(Some(m), 0.0)]));
    w.insert(e, Transform::IDENTITY);
    w
}

fn document(w: &World) -> serde_json::Value {
    let (bytes, _) = primase_gltf::write(w, &primase_gltf::WriteOptions::default()).unwrap();
    let chunks = primase_gltf::glb::parse(&bytes).unwrap();
    serde_json::from_slice(chunks.json).unwrap()
}

#[test]
fn emissive_strength_is_written_as_khr_materials_emissive_strength() {
    let doc = document(&world(5.0));
    let m = &doc["materials"][0];
    assert_eq!(m["emissiveFactor"], serde_json::json!([1.0, 0.5, 0.25]));
    assert_eq!(
        m["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
        5.0
    );
    assert!(
        doc["extensionsUsed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "KHR_materials_emissive_strength")
    );
    // not required: a consumer without it still draws the (dimmer) emission
    assert!(doc.get("extensionsRequired").is_none_or(|r| {
        !r.as_array()
            .unwrap()
            .iter()
            .any(|e| e == "KHR_materials_emissive_strength")
    }));
    // the default strength writes no extension
    let doc = document(&world(1.0));
    assert!(doc["materials"][0].get("extensions").is_none());
    assert!(doc.get("extensionsUsed").is_none_or(|u| {
        !u.as_array()
            .unwrap()
            .iter()
            .any(|e| e == "KHR_materials_emissive_strength")
    }));
}

#[test]
fn a_data_texture_is_sampled_nearest_and_its_image_marked_data() {
    use primase_core::pbr::TextureRef;
    use primase_core::{ColorSpace, TexFormat, TexRole, Texture};
    let mut w = world(1.0);
    let palette = w.spawn();
    w.insert(
        palette,
        Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![10, 20, 30, 255, 40, 50, 60, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Data,
            normal: None,
        },
    );
    let (m, _) = w
        .iter::<PbrMaterial>()
        .next()
        .map(|(e, m)| (e, m.clone()))
        .unwrap();
    w.get_mut::<PbrMaterial>(m).unwrap().base_color_texture = Some(TextureRef::new(palette));
    let doc = document(&w);
    let t = &doc["textures"][0];
    let sampler = &doc["samplers"][t["sampler"].as_u64().unwrap() as usize];
    assert_eq!(
        (sampler["magFilter"].as_u64(), sampler["minFilter"].as_u64()),
        (Some(9728), Some(9728))
    );
    assert_eq!(
        doc["images"][t["source"].as_u64().unwrap() as usize]["extras"]["primase.role"],
        "data"
    );
}
