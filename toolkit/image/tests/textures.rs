#![allow(clippy::unwrap_used)]
use primase_core::*;
use primase_image::*;
fn texture() -> Texture {
    Texture {
        width: 2,
        height: 2,
        format: TexFormat::Rgba8Unorm,
        levels: vec![[128, 64, 0, 93].repeat(4), vec![128, 128, 0, 77]],
        color_space: ColorSpace::Linear,
        role: TexRole::Normal,
        normal: Some(NormalConvention {
            green: NormalGreen::Down,
            z: NormalZ::Rebuild,
            x: NormalX::Red,
            basis: primase_core::NormalBasis::TangentBitangent,
        }),
    }
}
#[test]
fn normalization_all_levels_provenance_and_idempotence() {
    let mut w = World::new();
    let e = w.spawn();
    w.insert(e, texture());
    w.insert(
        e,
        TextureFile {
            path: "original.dds".into(),
        },
    );
    normalize_normals(&mut w).unwrap();
    let t = w.get::<Texture>(e).unwrap();
    assert_eq!(t.levels[0][1], 191);
    assert_eq!(t.levels[0][3], 93);
    assert_eq!(t.levels[1], vec![128, 127, 255, 77]);
    assert_eq!(t.color_space, ColorSpace::Linear);
    assert!(!w.has::<TextureFile>(e));
    let before = t.clone();
    w.insert(
        e,
        TextureFile {
            path: "converted.png".into(),
        },
    );
    assert!(normalize_normals(&mut w).unwrap().counts.is_empty());
    assert_eq!(w.get::<Texture>(e).unwrap().levels, before.levels);
    assert!(w.has::<TextureFile>(e));
}
#[test]
fn normalization_moves_x_from_alpha() {
    let mut w = World::new();
    let e = w.spawn();
    let mut t = texture();
    // DXT5nm: X in alpha, red and blue unused
    t.levels = vec![[255, 64, 0, 200].repeat(4), vec![255, 128, 0, 128]];
    t.normal = Some(NormalConvention {
        green: NormalGreen::Up,
        z: NormalZ::Rebuild,
        x: NormalX::Alpha,
        basis: primase_core::NormalBasis::TangentBitangent,
    });
    w.insert(e, t);
    let log = normalize_normals(&mut w).unwrap();
    assert_eq!(log.counts.get("x moved from alpha to red"), Some(&1));
    let t = w.get::<Texture>(e).unwrap();
    assert_eq!(t.normal, Some(NormalConvention::GLTF));
    // x = 200/127.5 - 1, y = 64/127.5 - 1: z = sqrt(1 - x^2 - y^2) = 0.6547.., encoded 211
    assert_eq!(&t.levels[0][..4], &[200, 64, 211, 200]);
    assert_eq!(t.levels[1], vec![128, 128, 255, 128]);
}

#[test]
fn normalization_errors_before_any_mutation() {
    let mut w = World::new();
    let a = w.spawn();
    w.insert(a, texture());
    let b = w.spawn();
    let mut invalid = texture();
    invalid.normal = None;
    w.insert(b, invalid);
    assert!(normalize_normals(&mut w).is_err());
    assert_eq!(w.get::<Texture>(a).unwrap().levels, texture().levels);
}
#[test]
fn load_png_dds_preserves_levels_and_files() {
    let dir = std::env::temp_dir().join(format!("primase-image-load-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut t = texture();
    t.role = TexRole::Other;
    t.normal = None;
    std::fs::write(dir.join("t.dds"), primase_dds::write_dx10(&t).unwrap()).unwrap();
    std::fs::write(
        dir.join("t.png"),
        write_png(&Image {
            width: 1,
            height: 1,
            samples: Samples::U16(vec![0, 65535, 32768, 1234]),
        })
        .unwrap(),
    )
    .unwrap();
    let mut w = World::new();
    let a = w.spawn();
    w.insert(
        a,
        TextureFile {
            path: dir.join("t.dds"),
        },
    );
    let b = w.spawn();
    w.insert(b, TextureFile { path: "t.png".into() });
    let c = w.spawn();
    w.insert(
        c,
        TextureFile {
            path: "missing".into(),
        },
    );
    assert!(load_textures(&mut w, &dir).is_err());
    assert!(!w.has::<Texture>(a));
    w.remove::<TextureFile>(c);
    let r = load_textures(&mut w, &dir).unwrap();
    assert_eq!(r.textures, 2);
    assert_eq!(w.get::<Texture>(a).unwrap().levels, t.levels);
    assert!(w.has::<TextureFile>(a));
    assert_eq!(w.get::<Texture>(b).unwrap().format, TexFormat::Rgba16Unorm);
    assert_eq!(load_textures(&mut w, &dir).unwrap().textures, 0);
    let png = read_png(&texture_level_png(w.get::<Texture>(a).unwrap(), 1).unwrap()).unwrap();
    assert_eq!(png.image.samples, Samples::U8(t.levels[1].clone()));
    std::fs::remove_file(dir.join("t.dds")).unwrap();
    std::fs::remove_file(dir.join("t.png")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn rgb_png_has_no_alpha_channel() {
    let rgb = vec![1, 2, 3, 254, 0, 127];
    let bytes = primase_image::write_png_rgb(2, 1, &rgb).unwrap();
    assert_eq!(bytes[25], 2);
    assert_eq!(bytes[24], 8);
    assert_eq!(
        primase_image::read_png(&bytes).unwrap().image.samples,
        primase_image::Samples::U8(vec![1, 2, 3, 255, 254, 0, 127, 255])
    );
    assert_eq!(bytes, primase_image::write_png_rgb(2, 1, &rgb).unwrap());
    assert!(primase_image::write_png_rgb(2, 1, &rgb[..5]).is_err());
}
#[test]
fn normalization_swaps_source_basis_after_channel_and_sign_decode() {
    let mut world = World::new();
    let e = world.spawn();
    let mut t = texture();
    t.levels = vec![[255, 64, 0, 200].repeat(4), vec![255, 128, 0, 128]];
    t.normal = Some(NormalConvention {
        green: NormalGreen::Down,
        z: NormalZ::Rebuild,
        x: NormalX::Alpha,
        basis: NormalBasis::BitangentTangent,
    });
    world.insert(e, t);
    world.insert(
        e,
        TextureFile {
            path: "source.dds".into(),
        },
    );
    let log = normalize_normals(&mut world).unwrap();
    assert_eq!(log.counts.get("normal tangent-plane axes swapped"), Some(&1));
    let t = world.get::<Texture>(e).unwrap();
    assert_eq!(&t.levels[0][..4], &[191, 200, 211, 200]);
    assert_eq!(&t.levels[1], &[127, 128, 255, 128]);
    assert_eq!(t.normal, Some(NormalConvention::GLTF));
    assert!(!world.has::<TextureFile>(e));
    let levels = t.levels.clone();
    assert!(normalize_normals(&mut world).unwrap().counts.is_empty());
    assert_eq!(world.get::<Texture>(e).unwrap().levels, levels);
}

#[test]
fn rebuild_projects_outside_disk_on_every_mip_without_changing_alpha() {
    let mut t = texture();
    t.normal.as_mut().unwrap().green = NormalGreen::Up;
    t.levels = vec![
        vec![255, 255, 7, 11, 0, 0, 9, 22, 128, 128, 3, 33, 255, 128, 4, 44],
        vec![0, 255, 2, 55],
    ];
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, t);
    let log = normalize_normals(&mut world).unwrap();
    let t = world.get::<Texture>(e).unwrap();
    assert_eq!(
        t.levels[0],
        [
            218, 218, 128, 11, 37, 37, 128, 22, 128, 128, 255, 33, 255, 128, 128, 44
        ]
    );
    assert_eq!(t.levels[1], [37, 218, 128, 55]);
    assert_eq!(log.counts["normal pixels projected onto unit disk"], 4);
    assert!(normalize_normals(&mut world).unwrap().counts.is_empty());
}

#[test]
fn rebuild_blue_rounding_matches_reference_for_every_xy_pair() {
    let mut projected = 0;
    let mut maximum_angle = 0f64;
    for r in 0..=255u8 {
        for g in 0..=255u8 {
            let x = f32::from(r) / 255. * 2. - 1.;
            let y = f32::from(g) / 255. * 2. - 1.;
            let reference = ((1f32 - x * x - y * y).max(0.).sqrt() * 0.5 + 0.5) * 255.;
            let x = f64::from(r) / 127.5 - 1.;
            let y = f64::from(g) / 127.5 - 1.;
            let blue = ((1f64 - x * x - y * y).max(0.).sqrt() * 0.5 + 0.5) * 255.;
            assert_eq!(blue.round() as u8, blue.round_ties_even() as u8);
            assert_eq!(blue.round_ties_even() as u8, reference.round_ties_even() as u8);
            let mut pixel = [r, g, 0, 71];
            let changed = primase_image::rebuild_normal_z(&mut pixel);
            assert_eq!(changed, x * x + y * y > 1.);
            assert_eq!(pixel[2], reference.round_ties_even() as u8);
            assert_eq!(pixel[3], 71);
            if changed {
                projected += 1;
                let old = [x, y, 1. / 255.];
                let new = [pixel[0], pixel[1], pixel[2]].map(|v| f64::from(v) / 127.5 - 1.);
                let norm = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
                let dot = old.iter().zip(new).map(|(a, b)| a * b).sum::<f64>();
                let angle = (dot / (norm(old) * norm(new))).clamp(-1., 1.).acos().to_degrees();
                maximum_angle = maximum_angle.max(angle);
                assert!(angle <= 0.31, "{r},{g}: {angle}");
            } else {
                assert_eq!(&pixel[..2], &[r, g]);
            }
        }
    }
    assert!(projected > 0);
    println!(
        "65536 XY pairs, {projected} projected, maximum encoded direction change {maximum_angle} degrees"
    );
}
