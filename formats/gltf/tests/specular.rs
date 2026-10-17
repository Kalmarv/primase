//! A stated specular reflectance is written as `KHR_materials_specular` (also at the extension's defaults); a
//! material that states none is written as before.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::{PbrMaterial, Specular, TextureRef};
use primase_core::{ColorSpace, Mesh, TexFormat, TexRole, Texture, Transform, World};

mod common;

fn texture(w: &mut World, rgba: [u8; 4], color_space: ColorSpace) -> primase_core::Entity {
    let t = w.spawn();
    w.insert(
        t,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![rgba.to_vec()],
            color_space,
            role: TexRole::Other,
            normal: None,
        },
    );
    t
}

fn world(specular: impl FnOnce(&mut World) -> Option<Specular>) -> World {
    let mut w = World::new();
    let s = specular(&mut w);
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            roughness: 0.4,
            metallic: 0.0,
            specular: s,
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

fn used(doc: &serde_json::Value, list: &str) -> bool {
    doc.get(list).is_some_and(|u| {
        u.as_array()
            .unwrap()
            .iter()
            .any(|e| e == "KHR_materials_specular")
    })
}

#[test]
fn specular_is_written_as_khr_materials_specular() {
    let doc = document(&world(|w| {
        let strength = texture(w, [0, 0, 0, 17], ColorSpace::Linear);
        let colour = texture(w, [200, 180, 160, 255], ColorSpace::Srgb);
        Some(Specular {
            factor: 0.5,
            texture: Some(TextureRef::new(strength)),
            color: [1.0, 0.75, 0.5],
            color_texture: Some(TextureRef::new(colour)),
        })
    }));
    let ext = &doc["materials"][0]["extensions"]["KHR_materials_specular"];
    assert_eq!(ext["specularFactor"], 0.5);
    assert_eq!(ext["specularColorFactor"], serde_json::json!([1.0, 0.75, 0.5]));
    let (a, b) = (
        ext["specularTexture"]["index"].as_u64().unwrap() as usize,
        ext["specularColorTexture"]["index"].as_u64().unwrap() as usize,
    );
    assert_ne!(a, b);
    assert_eq!(doc["textures"].as_array().unwrap().len(), 2);
    assert!(used(&doc, "extensionsUsed") && !used(&doc, "extensionsRequired"));
    // stated at the defaults: still written (a fact), with no factors
    let doc = document(&world(|_| Some(Specular::default())));
    assert_eq!(
        doc["materials"][0]["extensions"]["KHR_materials_specular"],
        serde_json::json!({})
    );
    assert!(used(&doc, "extensionsUsed"));
    // not stated: no extension at all
    let doc = document(&world(|_| None));
    assert!(doc["materials"][0].get("extensions").is_none());
    assert!(!used(&doc, "extensionsUsed"));
}

#[test]
fn specular_out_of_range_is_invalid_and_its_textures_are_references() {
    for bad in [
        Specular {
            factor: 1.5,
            ..Specular::default()
        },
        Specular {
            color: [-0.1, 1.0, 1.0],
            ..Specular::default()
        },
        Specular {
            factor: f32::NAN,
            ..Specular::default()
        },
    ] {
        let w = world(|_| Some(bad));
        assert!(!w.verify_components().is_empty(), "{bad:?}");
    }
    let mut strength = None;
    let w = world(|w| {
        let t = texture(w, [0, 0, 0, 255], ColorSpace::Linear);
        strength = Some(t);
        Some(Specular {
            texture: Some(TextureRef::new(t)),
            ..Specular::default()
        })
    });
    let material = w.iter::<PbrMaterial>().next().unwrap().0;
    assert!(w.closure([material]).contains(&strength.unwrap()));
}
