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
        textures: slots
            .iter()
            .map(|(kind, name)| flver::Texture {
                kind: kind.to_string(),
                path: format!("{name}.dds"),
                scale: [1.; 2],
            })
            .collect(),
    };
    materials::resolve(&src, Some(&definition), |_| true)
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
    let log = pbr::lower(&mut world, &bytes).unwrap();
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
fn baked_bindings_and_factors_are_authoritative() {
    let mut world = World::new();
    let base = world.spawn();
    let normal = world.spawn();
    let packed = world.spawn();
    let e = world.spawn();
    let mut m = source("M[Multiple]");
    m.layered = Some(materials::LayeredMaterial {
        mtd: m.mtd.clone(),
        definition: m.definition.clone().unwrap(),
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
    pbr::lower(&mut world, &BTreeMap::new()).unwrap();
    assert_eq!(world.get::<PbrMaterial>(e).unwrap(), &expected);
}
#[test]
fn missing_source_data_is_not_a_silent_default() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, source("MetallicTex"));
    assert!(pbr::lower(&mut world, &BTreeMap::new()).is_err());
}

#[test]
fn selected_normal_identity_controls_both_transforms() {
    let mut world = World::new();
    let e = world.spawn();
    let mut m = source("MetallicTex");
    let kind = "Texture2D_3_NormalMap";
    let mut slot = m.definition.as_ref().unwrap().textures[1].clone();
    slot.kind = kind.into();
    m.definition.as_mut().unwrap().textures.push(slot);
    m.bindings.insert(
        kind.into(),
        materials::Binding {
            texture: "selected".into(),
            scale: [2., 1.],
            uv_number: 1,
        },
    );
    m.normal = Some("selected".into());
    world.insert(e, m);
    let bytes = BTreeMap::from([
        ("a".into(), payload(&[80, 90, 100, 255])),
        (
            "selected".into(),
            payload(&[128, 128, 0, 255, 128, 128, 255, 255]),
        ),
        ("m".into(), payload(&[0, 0, 0, 255, 255, 0, 0, 255])),
    ]);
    pbr::lower(&mut world, &bytes).unwrap();
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
    pbr::lower(&mut world, &bytes).unwrap();
    let expected = world.get::<PbrMaterial>(e).unwrap().clone();
    let texture = expected.metallic_roughness_texture.unwrap().texture;
    assert_eq!(
        world.get::<Texture>(texture).unwrap().levels[0],
        [255, 13, 51, 255]
    );
    let entities = world.entities().count();
    pbr::lower(&mut world, &BTreeMap::new()).unwrap();
    assert_eq!(world.get::<PbrMaterial>(e).unwrap(), &expected);
    assert_eq!(world.entities().count(), entities);
    assert_eq!(
        world.get::<Texture>(texture).unwrap().levels[0],
        [255, 13, 51, 255]
    );
}
