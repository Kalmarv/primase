//! Independent RGBA-byte hashes for caller-provided PNGs and explicit material factors.
//! This covers converter inputs, not a full scene bridge or native rendering.
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{ColorSpace, Instance, Mesh, Primitive, TexFormat, TexRole, Texture, World};
use primase_reskate::{
    components::SkateMaterial,
    passes::{RaiseOptions, raise_skate},
};
use primase_testdata::{Game, OracleSet, oracle};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Deserialize)]
struct Census {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    source: PathBuf,
    source_sha256: String,
    metallic: f32,
    roughness: f32,
    occlusion: bool,
    strength: f32,
    width: u32,
    height: u32,
    mask_sha256: String,
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn source_mask_channels_match_independent_pixels() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = oracle(OracleSet::Pillow, Game::Sekiro, "skate-mask") else {
        return Ok(());
    };
    let census: Census = serde_json::from_slice(&std::fs::read(root.join("cases.json"))?)?;
    assert!(!census.cases.is_empty(), "configured corpus is empty");
    let mut texels = 0u64;
    for case in &census.cases {
        let data = std::fs::read(root.join(&case.source))?;
        assert_eq!(hash(&data), case.source_sha256);
        let image = primase_image::read_png(&data)?.image;
        let primase_image::Samples::U8(pixels) = image.samples else {
            return Err("RGBA8 corpus required".into());
        };
        assert_eq!((image.width, image.height), (case.width, case.height));
        let mut world = World::new();
        let source = world.spawn();
        world.insert(
            source,
            Texture {
                width: image.width,
                height: image.height,
                format: TexFormat::Rgba8Unorm,
                levels: vec![pixels],
                color_space: ColorSpace::Linear,
                role: TexRole::Mask,
                normal: None,
            },
        );
        let material = world.spawn();
        world.insert(
            material,
            PbrMaterial {
                metallic: case.metallic,
                roughness: case.roughness,
                metallic_roughness_texture: Some(TextureRef::new(source)),
                occlusion_texture: case.occlusion.then(|| TextureRef::new(source)),
                occlusion_strength: case.strength,
                ..Default::default()
            },
        );
        let prototype = world.spawn();
        world.insert(
            prototype,
            Mesh::new(vec![Primitive {
                material: Some(material),
                topology: primase_core::Topology::Triangles,
                indices: None,
                streams: std::collections::BTreeMap::from([
                    (
                        primase_core::Semantic::Position,
                        primase_core::Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
                    ),
                    (
                        primase_core::Semantic::TexCoord(0),
                        primase_core::Stream::F32x2(vec![[0.0; 2], [1.0, 0.0], [0.0, 1.0]]),
                    ),
                ]),
            }]),
        );
        let placement = world.spawn();
        world.insert(
            placement,
            Instance {
                prototype,
                variant: 0,
            },
        );
        let log = raise_skate(
            &mut world,
            &RaiseOptions::new(
                "mask-census",
                primase_reskate::passes::LightCalibration::Standard,
                primase_reskate::passes::Unclassified::LeaveOut,
            )
            .unwrap(),
        )?;
        let mask = world
            .get::<SkateMaterial>(material)
            .ok_or("target material missing")?
            .mask
            .ok_or("mask binding missing")?;
        let pixels = &world.get::<Texture>(mask).ok_or("mask pixels missing")?.levels[0];
        assert_eq!(
            hash(pixels),
            case.mask_sha256,
            "{} metallic {}",
            case.source.display(),
            case.metallic
        );
        assert_eq!(log.counts["PBR materials raised with an MSK binding"], 1);
        texels += u64::from(image.width) * u64::from(image.height);
    }
    eprintln!(
        "{} mask cases, {texels} base-mip texels exact against independent pixel hashes",
        census.cases.len()
    );
    Ok(())
}
