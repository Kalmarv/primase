//! lower-skate-pbr turns a loaded Studio MSK into glTF's packed occlusion-roughness-metallic texture, pixel exact.

#![allow(clippy::unwrap_used)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{ColorSpace, Name, TexFormat, TexRole, Texture, World};
use primase_reskate::components::{MaterialClass, Other, SkateMaterial};
use primase_reskate::passes::lower::{LowerSkatePbr, msk_to_orm};

fn rgba8(width: u32, height: u32, levels: Vec<Vec<u8>>) -> Texture {
    Texture {
        width,
        height,
        format: TexFormat::Rgba8Unorm,
        levels,
        color_space: ColorSpace::Linear,
        role: TexRole::Other,
        normal: None,
    }
}

#[test]
fn every_byte_of_every_level_maps_as_specified() {
    // 2x1 base level and its 1x1 mip: R metallic, G smoothness, B occlusion, A opacity.
    let msk = rgba8(
        2,
        1,
        vec![vec![0, 255, 255, 255, 200, 30, 90, 17], vec![100, 142, 172, 136]],
    );
    let orm = msk_to_orm(&msk).unwrap();
    assert_eq!(
        orm.levels,
        [vec![255, 0, 0, 255, 90, 225, 200, 255], vec![172, 113, 100, 255]]
    );
    assert_eq!((orm.width, orm.height, orm.format), (2, 1, TexFormat::Rgba8Unorm));
    assert_eq!(
        (orm.color_space, orm.role, orm.normal),
        (ColorSpace::Linear, TexRole::Mask, None)
    );
    // Exhaustive over byte values per channel.
    let all: Vec<u8> = (0..=255u8).flat_map(|v| [v, v, v, v]).collect();
    let mapped = msk_to_orm(&rgba8(256, 1, vec![all])).unwrap();
    for (v, p) in mapped.levels[0].as_chunks::<4>().0.iter().enumerate() {
        let v = v as u8;
        assert_eq!(*p, [v, 255 - v, v, 255]);
    }
    assert!(
        msk_to_orm(&Texture {
            format: TexFormat::Bc1,
            ..rgba8(4, 4, vec![vec![0; 8]])
        })
        .is_err()
    );
}

#[test]
fn materials_bind_one_shared_orm_with_unit_factors() {
    let mut w = World::new();
    let msk = w.spawn();
    w.insert(msk, rgba8(1, 1, vec![vec![0, 64, 200, 255]]));
    w.insert(msk, Name("wall_msk".into()));
    let unloaded = w.spawn();
    let material = |w: &mut World, key: &str, mask, roughness| {
        let e = w.spawn();
        w.insert(
            e,
            SkateMaterial {
                key: key.into(),
                class: MaterialClass::Opaque,
                base: None,
                normal: None,
                mask,
                base_color: None,
                tint: None,
                alpha: None,
                cutoff: None,
                two_sided: None,
                domain: None,
                metallic: None,
                roughness,
                wrap: None,
                ocean: None,
                cast_shadows: None,
                emissive: None,
                vertex_alpha: None,
                other: Other::new(),
            },
        );
        e
    };
    let a = material(&mut w, "a", Some(msk), Some(0.3));
    let b = material(&mut w, "b", Some(msk), None);
    let plain = material(&mut w, "plain", None, Some(0.3));
    let missing = material(&mut w, "missing", Some(unloaded), None);
    let malformed = w.spawn();
    w.insert(malformed, rgba8(2, 1, vec![vec![0; 8], vec![0; 3]]));
    let bad_material = material(&mut w, "short mip", Some(malformed), None);
    let mut log = PassLog::default();
    LowerSkatePbr.run(&mut w, &mut log).unwrap();
    assert_eq!(log.counts.get("MSK masks lowered to ORM textures"), Some(&1));
    let (pa, pb) = (w.get::<PbrMaterial>(a).unwrap(), w.get::<PbrMaterial>(b).unwrap());
    let orm = pa.metallic_roughness_texture.unwrap().texture;
    assert_eq!(pa.occlusion_texture.unwrap().texture, orm);
    assert_eq!(
        pb.metallic_roughness_texture.unwrap().texture,
        orm,
        "one derived texture per mask"
    );
    assert_eq!(
        (pa.metallic, pa.roughness, pa.occlusion_strength),
        (1.0, 1.0, 1.0)
    );
    assert_eq!(w.get::<Name>(orm).unwrap().0, "wall_msk@orm");
    assert_eq!(w.get::<Texture>(orm).unwrap().levels, [vec![200, 191, 0, 255]]);
    assert_eq!(
        w.get::<SkateMaterial>(a).unwrap().mask,
        Some(msk),
        "the skate material keeps its MSK"
    );
    let pp = w.get::<PbrMaterial>(plain).unwrap();
    assert_eq!((pp.metallic_roughness_texture, pp.roughness), (None, 0.3));
    assert!(
        w.get::<PbrMaterial>(missing)
            .unwrap()
            .metallic_roughness_texture
            .is_none()
    );
    assert_eq!(
        log.approximations
            .get("Studio MSK mask not loaded as RGBA8 left out"),
        Some(&1)
    );
    assert!(
        w.get::<PbrMaterial>(bad_material)
            .unwrap()
            .metallic_roughness_texture
            .is_none()
    );
    assert_eq!(
        log.approximations
            .get("Studio MSK mask with inconsistent mip sizes left out"),
        Some(&1)
    );
    // Only material a sets a scalar alongside its MSK.
    assert_eq!(
        log.approximations
            .get("scalar metallic/roughness ignored where an MSK is bound (factors 1)"),
        Some(&1)
    );
}
