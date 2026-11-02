//! A material with `core.water_body` is written as `KHR_materials_transmission`, `KHR_materials_volume` (its
//! extinction as the light left after one metre) and `KHR_materials_ior`, opaque and white, its first wave as the
//! normal texture where it has none; what glTF has no form for is counted by name.

#![allow(clippy::unwrap_used)]

use primase_core::pbr::{AlphaMode, PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    ColorSpace, Mesh, NormalConvention, TexFormat, TexRole, Texture, Transform, WaterBody, WaveLayer, World,
};

mod common;

const EXTINCTION: [f32; 3] = [0.215, 0.115, 0.09];

fn world(waves: usize) -> World {
    let mut w = World::new();
    let normal = w.spawn();
    w.insert(
        normal,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![128, 128, 255, 255]],
            color_space: ColorSpace::Linear,
            role: TexRole::Normal,
            normal: Some(NormalConvention::GLTF),
        },
    );
    let m = w.spawn();
    w.insert(
        m,
        PbrMaterial {
            base_color: [0.03, 0.12, 0.19, 0.75],
            alpha_mode: AlphaMode::Blend,
            ..PbrMaterial::default()
        },
    );
    let wave = WaveLayer {
        normal: TextureRef {
            transform: Some(UvTransform {
                offset: [0.; 2],
                scale: [4.; 2],
                rotation: 0.,
            }),
            ..TextureRef::new(normal)
        },
        velocity: [-0.0003, 0.0015],
        strength: 0.8,
    };
    w.insert(
        m,
        WaterBody {
            extinction: EXTINCTION,
            scatter: [0.; 3],
            ior: 1.5,
            waves: vec![wave; waves],
            shore_fade: 1. / 3.,
        },
    );
    let e = w.spawn();
    w.insert(e, Mesh::new(vec![common::rich_primitive(Some(m), 0.0)]));
    w.insert(e, Transform::IDENTITY);
    w
}

fn written(w: &World) -> (serde_json::Value, primase_gltf::WriteReport) {
    let (bytes, report) = primase_gltf::write(w, &primase_gltf::WriteOptions::default()).unwrap();
    let chunks = primase_glb::parse(&bytes).unwrap();
    (serde_json::from_slice(chunks.json).unwrap(), report)
}

#[test]
fn a_water_body_is_written_as_transmission_volume_and_ior() {
    let (doc, report) = written(&world(2));
    let m = &doc["materials"][0];
    let x = &m["extensions"];
    assert_eq!(x["KHR_materials_transmission"]["transmissionFactor"], 1.0);
    // the light left after the attenuation distance: exp(-extinction) over one metre
    assert_eq!(x["KHR_materials_volume"]["attenuationDistance"], 1.0);
    let colour: Vec<f64> = x["KHR_materials_volume"]["attenuationColor"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    for (written, sigma) in colour.iter().zip(EXTINCTION) {
        assert!((-written.ln() - f64::from(sigma)).abs() < 1e-6, "{colour:?}");
    }
    assert!(x["KHR_materials_volume"].get("thicknessFactor").is_none());
    assert_eq!(x["KHR_materials_ior"]["ior"], 1.5);
    // opaque and white: glTF draws what is behind by its transmission and tints it by the base colour
    assert!(m.get("alphaMode").is_none());
    assert!(m["pbrMetallicRoughness"].get("baseColorFactor").is_none());
    // the first wave is the normal texture, at its strength and tiling
    assert!((m["normalTexture"]["scale"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert_eq!(
        m["normalTexture"]["extensions"]["KHR_texture_transform"]["scale"],
        serde_json::json!([4.0, 4.0])
    );
    let used: Vec<&str> = doc["extensionsUsed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    for e in [
        "KHR_materials_transmission",
        "KHR_materials_volume",
        "KHR_materials_ior",
    ] {
        assert!(used.contains(&e), "{used:?}");
    }
    assert_eq!(report.water_bodies, 1);
    for what in [
        primase_gltf::write::WATER_THICKNESS,
        primase_gltf::write::WATER_MOTION,
        primase_gltf::write::WATER_WAVES,
        primase_gltf::write::WATER_SHORE,
        primase_gltf::write::WATER_BASE_COLOUR,
        primase_gltf::write::WATER_ALPHA,
    ] {
        assert_eq!(
            report.water_unwritten.get(what),
            Some(&1),
            "{what}: {:?}",
            report.water_unwritten
        );
    }
    // no scatter, so none counted; the water body is expressed, not left without a form
    assert!(
        !report
            .water_unwritten
            .contains_key(primase_gltf::write::WATER_SCATTER)
    );
    assert!(!report.components_without_form.contains_key("core.water_body"));
}

#[test]
fn one_wave_is_the_normal_texture_and_none_is_counted_past_it() {
    let (_, report) = written(&world(1));
    assert!(
        !report
            .water_unwritten
            .contains_key(primase_gltf::write::WATER_WAVES)
    );
}
