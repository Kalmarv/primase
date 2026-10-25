//! Synthetic DDS fixtures assert independent channel values and baked-binding
//! identity, not merely that lowering ran without an error.
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{ColorSpace, TexFormat, TexRole, Texture, World};
use primase_fromsoft::{
    flver, materials,
    mtd::{MaterialDefinition, Texture as Slot},
    sekiro::pbr,
};
use std::collections::BTreeMap;

/// The water colour the tests state (the earlier skate exporter's).
const WATER: primase_image::Srgba8 = primase_image::Srgba8([48, 98, 122, 190]);

fn source(shader: &str) -> materials::Material {
    let slots = [
        ("Texture2D_0_AlbedoMap", "a"),
        ("Texture2D_1_NormalMap", "n"),
        ("Texture2D_2_MetallicMap", "m"),
    ];
    let definition = MaterialDefinition {
        shader: format!("{shader}.spx"),
        description: String::new(),
        parameters: vec![],
        block_lengths: vec![],
        textures: slots
            .iter()
            .map(|(kind, _)| Slot {
                kind: kind.to_string(),
                uv_number: 1,
                shader_data_index: 0,
                path: String::new(),
                extra: vec![],
            })
            .collect(),
    };
    let src = flver::Material {
        name: "fixture".into(),
        mtd: "fixture.mtd".into(),
        gx: vec![],
        textures: slots
            .iter()
            .map(|(kind, name)| flver::Texture {
                kind: kind.to_string(),
                path: format!("{name}.dds"),
                scale: [1.; 2],
                ..Default::default()
            })
            .collect(),
    };
    materials::resolve(&src, Some(&definition), |_| true)
}
/// `m` with the texture of binding `kind` missing from the catalog, so no preview draws it.
fn unavailable(m: &mut materials::Material, kind: &str) {
    m.bindings.get_mut(kind).expect("a fixture binding").available = false;
}
/// No texture files.
fn no_payloads() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::new()
}
fn payload(rgba: &[u8]) -> Vec<u8> {
    primase_dds::write_dx10(&Texture {
        width: rgba.len() as u32 / 4,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![rgba.to_vec()],
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    })
    .expect("valid synthetic DDS")
}

const UNKNOWN: &str = "metallic without source evidence lowered as dielectric 0";

fn gx(words: &[u32]) -> flver::GxItem {
    flver::GxItem {
        id: *b"GXMD",
        version: 123,
        data: words.iter().flat_map(|v| v.to_le_bytes()).collect(),
    }
}

#[test]
fn evidenced_gx_scalar_is_exact_and_other_families_keep_raw_values() {
    for shader in ["MetallicValue", "MetallicValue_MeshDecal", "Unclassified"] {
        let mut world = World::new();
        let e = world.spawn();
        let mut m = source(shader);
        m.bindings.remove("Texture2D_2_MetallicMap");
        // An unrelated vector precedes the scalar: IDs are not positional.
        m.gx = vec![gx(&[2, 91, 2, 0, 0, 237, 1, 0.37f32.to_bits()])];
        let raw = m.gx.clone();
        world.insert(e, m);
        let bytes = BTreeMap::from([
            ("a".into(), payload(&[80, 90, 100, 255])),
            ("n".into(), payload(&[128, 128, 255, 255])),
        ]);
        let log = pbr::lower(&mut world, &bytes, None, WATER).unwrap();
        let p = world.get::<PbrMaterial>(e).unwrap().clone();
        assert_eq!(world.get::<materials::Material>(e).unwrap().gx, raw);
        assert_eq!(
            log.approximations.get(UNKNOWN).copied(),
            (shader == "Unclassified").then_some(1)
        );
        if shader != "Unclassified" {
            assert_eq!(p.metallic, 0.37);
            let t = world
                .get::<Texture>(p.metallic_roughness_texture.unwrap().texture)
                .unwrap();
            assert_eq!(t.levels[0][2], 255);
            pbr::lower(&mut world, &no_payloads(), None, WATER).unwrap();
            assert_eq!(world.get::<PbrMaterial>(e).unwrap(), &p);
        } else {
            // No evidenced consumer for this family: dielectric, not glTF's 1.
            assert_eq!(p.metallic, 0.);
        }
    }
}

#[test]
fn gx_scalar_rejects_ambiguous_or_invalid_records_and_retains_flagged_values() {
    let half = 0.5f32.to_bits();
    for words in [
        vec![],
        vec![1, 237],
        vec![1, 237, 1],
        vec![2, 237, 1, half, 237, 1, half],
        vec![1, 237, 1, f32::NAN.to_bits()],
        vec![1, 237, 1, 1.01f32.to_bits()],
        vec![1, 237, 1, half, 0],
        vec![1, 237, 0, 1],
        vec![3, 237, 1, half, 237, 0, 1, 237, 0, 0],
    ] {
        let mut world = World::new();
        let e = world.spawn();
        let mut m = source("MetallicValue");
        unavailable(&mut m, "Texture2D_0_AlbedoMap");
        unavailable(&mut m, "Texture2D_1_NormalMap");
        m.gx = vec![gx(&words)];
        world.insert(e, m);
        assert!(
            pbr::lower(&mut world, &no_payloads(), None, WATER).is_err(),
            "{words:?}"
        );
    }
    {
        let mut world = World::new();
        let e = world.spawn();
        let mut m = source("MetallicValue");
        unavailable(&mut m, "Texture2D_0_AlbedoMap");
        unavailable(&mut m, "Texture2D_1_NormalMap");
        m.gx = vec![gx(&[1, 237, 1, half]); 2];
        world.insert(e, m);
        assert!(pbr::lower(&mut world, &no_payloads(), None, WATER).is_err());
    }
    for records in [
        vec![gx(&[2, 237, 1, half, 237, 0, 1])],
        vec![gx(&[1, 237, 9, half])],
    ] {
        let mut world = World::new();
        let e = world.spawn();
        let mut m = source("MetallicValue");
        unavailable(&mut m, "Texture2D_0_AlbedoMap");
        unavailable(&mut m, "Texture2D_1_NormalMap");
        m.gx = records.clone();
        world.insert(e, m);
        let log = pbr::lower(&mut world, &no_payloads(), None, WATER).unwrap();
        assert_eq!(world.get::<PbrMaterial>(e).unwrap().metallic, 0.);
        assert_eq!(log.approximations[UNKNOWN], 1);
        assert_eq!(world.get::<materials::Material>(e).unwrap().gx, records);
    }
}
#[test]
fn source_blue_gloss_and_red_metal_survive_as_distinct_orm_channels() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, source("MetallicTex"));
    let bytes = BTreeMap::from([
        ("a".into(), payload(&[80, 90, 100, 255, 40, 50, 60, 255])),
        ("n".into(), payload(&[128, 128, 0, 255, 128, 128, 255, 255])),
        ("m".into(), payload(&[0, 200, 210, 255, 255, 100, 120, 255])),
    ]);
    assert_eq!(pbr::required_textures(&world).unwrap().len(), 3);
    let log = pbr::lower(&mut world, &bytes, None, WATER).unwrap();
    let mat = world.get::<PbrMaterial>(e).unwrap();
    let orm = world
        .get::<Texture>(mat.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!(orm.levels[0], [255, 255, 0, 255, 255, 13, 255, 255]);
    assert!(mat.occlusion_texture.is_none());
    assert_eq!(log.counts["metallic textures"], 1);
    assert_eq!(
        world
            .get::<Texture>(mat.normal_texture.unwrap().texture)
            .unwrap()
            .levels[0],
        [128, 128, 0, 255, 128, 128, 255, 255]
    );
    assert!(world.get::<materials::Material>(e).unwrap().definition.is_some());
}
#[test]
fn orm_selects_only_base_mip_and_assigns_linear_mask_metadata() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, source("MetallicTex"));
    let normal = Texture {
        width: 2,
        height: 1,
        format: TexFormat::Rgba8Unorm,
        levels: vec![
            vec![128, 128, 0, 255, 128, 128, 255, 255],
            vec![128, 128, 128, 255],
        ],
        color_space: ColorSpace::Linear,
        role: TexRole::Normal,
        normal: None,
    };
    let bytes = BTreeMap::from([
        ("a".into(), payload(&[80, 90, 100, 255])),
        ("n".into(), primase_dds::write_dx10(&normal).unwrap()),
        ("m".into(), payload(&[0, 0, 0, 255])),
    ]);
    pbr::lower(&mut world, &bytes, None, WATER).unwrap();
    let p = world.get::<PbrMaterial>(e).unwrap();
    let t = world
        .get::<Texture>(p.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!((t.width, t.height), (2, 1));
    assert_eq!(t.levels, vec![vec![255, 255, 0, 255, 255, 13, 0, 255]]);
    assert_eq!(t.color_space, ColorSpace::Linear);
    assert_eq!(t.role, TexRole::Mask);
    assert_eq!(t.normal, None);

    let mut missing = bytes;
    missing.get_mut("n").unwrap().truncate(128);
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, source("MetallicTex"));
    assert!(pbr::lower(&mut world, &missing, None, WATER).is_err());
}

#[test]
fn baked_bindings_and_factors_are_authoritative() {
    let mut world = World::new();
    let base = world.spawn();
    let normal = world.spawn();
    let packed = world.spawn();
    let e = world.spawn();
    let mut m = source("M[Multiple]");
    m.layered = Some(materials::LayeredMaterial {
        mtd: m.mtd.clone(),
        definition: m.definition.as_ref().and_then(|d| d.mtd()).cloned().unwrap(),
        bindings: m.bindings.clone(),
    });
    world.insert(e, m);
    let expected = PbrMaterial {
        base_color_texture: Some(TextureRef::new(base)),
        normal_texture: Some(TextureRef::new(normal)),
        metallic_roughness_texture: Some(TextureRef::new(packed)),
        metallic: 0.2,
        roughness: 1.,
        ..Default::default()
    };
    world.insert(e, expected.clone());
    assert!(pbr::required_textures(&world).unwrap().is_empty());
    pbr::lower(&mut world, &no_payloads(), None, WATER).unwrap();
    assert_eq!(world.get::<PbrMaterial>(e).unwrap(), &expected);
}
#[test]
fn missing_source_data_is_not_a_silent_default() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, source("MetallicTex"));
    // a texture the material names and the install does not hold is no error (the material is drawn without it), but it
    // is never silent: the log names it and the entity
    let log = pbr::lower(&mut world, &no_payloads(), None, WATER).unwrap();
    let normal = "named normal texture not in the install: flat normal";
    assert_eq!(log.approximations.get(normal).copied(), Some(1));
    assert_eq!(log.approximated_on[normal], std::collections::BTreeSet::from([e]));
}

#[test]
fn baked_emission_presence_controls_missing_radiance_diagnostic() {
    for radiance in [None, Some(0u8), Some(255)] {
        let mut world = World::new();
        let e = world.spawn();
        let mut m = source("M[Multiple]_Emissive");
        m.layered = Some(materials::LayeredMaterial {
            mtd: m.mtd.clone(),
            definition: m.definition.as_ref().and_then(|d| d.mtd()).cloned().unwrap(),
            bindings: m.bindings.clone(),
        });
        world.insert(e, m);
        let mut expected = PbrMaterial::default();
        if let Some(value) = radiance {
            let texture = world.spawn();
            world.insert(
                texture,
                Texture {
                    width: 1,
                    height: 1,
                    format: TexFormat::Rgba8Unorm,
                    levels: vec![vec![value, value, value, 255]],
                    color_space: ColorSpace::Srgb,
                    role: TexRole::Color,
                    normal: None,
                },
            );
            expected.emissive_texture = Some(TextureRef::new(texture));
            expected.emissive = [1.; 3];
            expected.emissive_strength = 4.;
        }
        world.insert(e, expected.clone());
        let log = pbr::lower(&mut world, &no_payloads(), None, WATER).unwrap();
        assert_eq!(world.get::<PbrMaterial>(e).unwrap(), &expected);
        assert_eq!(
            log.approximations
                .contains_key("source emission without proven radiance; retained dialect"),
            radiance.is_none()
        );
        assert_eq!(
            log.counts
                .get("existing emission textures preserved")
                .copied()
                .unwrap_or(0),
            u64::from(radiance.is_some())
        );
    }
}

#[test]
fn selected_normal_identity_controls_both_transforms() {
    let mut world = World::new();
    let e = world.spawn();
    let mut m = source("MetallicTex");
    let kind = "Texture2D_3_NormalMap";
    let mut slot = m.definition.as_ref().and_then(|d| d.mtd()).unwrap().textures[1].clone();
    slot.kind = kind.into();
    m.definition
        .as_mut()
        .and_then(|d| d.mtd_mut())
        .unwrap()
        .textures
        .push(slot);
    m.bindings.insert(
        kind.into(),
        materials::Binding {
            texture: "selected".into(),
            scale: [2., 1.],
            uv_number: 1,
            address: [None; 2],
            available: true,
        },
    );
    // the preview picks it: no normal of the base map's family, and slot 3 before the others
    world.insert(e, m);
    let bytes = BTreeMap::from([
        ("a".into(), payload(&[80, 90, 100, 255])),
        (
            "selected".into(),
            payload(&[128, 128, 0, 255, 128, 128, 255, 255]),
        ),
        ("m".into(), payload(&[0, 0, 0, 255, 255, 0, 0, 255])),
    ]);
    pbr::lower(&mut world, &bytes, None, WATER).unwrap();
    let p = world.get::<PbrMaterial>(e).unwrap();
    let normal = p.normal_texture.unwrap();
    let orm = p.metallic_roughness_texture.unwrap();
    assert_eq!(normal.transform.unwrap().scale, [2., 1.]);
    assert_eq!(orm.transform, normal.transform);
    // Metal sampled at U=.125/.375: wrapped bilinear values are both .25.
    assert_eq!(
        world.get::<Texture>(orm.texture).unwrap().levels[0],
        [255, 255, 64, 255, 255, 13, 64, 255]
    );
}

#[test]
fn repeated_lowering_preserves_channels_factors_and_entity_count() {
    let mut world = World::new();
    let e = world.spawn();
    let mut m = source("MetallicValue");
    m.bindings.remove("Texture2D_2_MetallicMap");
    world.insert(e, m);
    world.insert(
        e,
        PbrMaterial {
            metallic: 0.2,
            ..Default::default()
        },
    );
    let bytes = BTreeMap::from([
        ("a".into(), payload(&[80, 90, 100, 255])),
        ("n".into(), payload(&[128, 128, 255, 255])),
    ]);
    pbr::lower(&mut world, &bytes, None, WATER).unwrap();
    let expected = world.get::<PbrMaterial>(e).unwrap().clone();
    let texture = expected.metallic_roughness_texture.unwrap().texture;
    assert_eq!(
        world.get::<Texture>(texture).unwrap().levels[0],
        [255, 13, 51, 255]
    );
    let entities = world.entities().count();
    pbr::lower(&mut world, &no_payloads(), None, WATER).unwrap();
    assert_eq!(world.get::<PbrMaterial>(e).unwrap(), &expected);
    assert_eq!(world.entities().count(), entities);
    assert_eq!(
        world.get::<Texture>(texture).unwrap().levels[0],
        [255, 13, 51, 255]
    );
}

#[test]
fn metallic_without_source_evidence_is_counted_dielectric() {
    let bytes = BTreeMap::from([
        ("a".into(), payload(&[80, 90, 100, 255])),
        ("n".into(), payload(&[128, 128, 255, 255])),
    ]);
    // A metal-map family without its map, a scalar family without GXMD, and an
    // unclassified family: none has a traced metallic source.
    for (shader, normal) in [
        ("MetallicTex", true),
        ("MetallicValue", true),
        ("MetallicValue", false),
        ("Unclassified", true),
    ] {
        let mut world = World::new();
        let e = world.spawn();
        let mut m = source(shader);
        m.bindings.remove("Texture2D_2_MetallicMap");
        if !normal {
            unavailable(&mut m, "Texture2D_1_NormalMap");
        }
        world.insert(e, m);
        let log = pbr::lower(&mut world, &bytes, None, WATER).unwrap();
        let p = world.get::<PbrMaterial>(e).unwrap();
        let effective = match p.metallic_roughness_texture {
            Some(r) => p.metallic * f32::from(world.get::<Texture>(r.texture).unwrap().levels[0][2]) / 255.,
            None => p.metallic,
        };
        assert_eq!(effective, 0., "{shader} normal={normal}");
        assert_eq!(log.approximations[UNKNOWN], 1, "{shader} normal={normal}");
    }
    // A traced map is evidence even when it samples zero, and is not counted.
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, source("MetallicTex"));
    let mut traced = bytes.clone();
    traced.insert("m".into(), payload(&[0, 0, 0, 255]));
    let log = pbr::lower(&mut world, &traced, None, WATER).unwrap();
    assert!(!log.approximations.contains_key(UNKNOWN));
    assert_eq!(log.counts["metallic textures"], 1);
}
