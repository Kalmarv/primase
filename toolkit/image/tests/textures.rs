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
