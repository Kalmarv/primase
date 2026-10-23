//! Texture preparation for the map writer: tints multiplied into texture copies (`prepare-reskate-textures`), the
//! texture budget (`budget-reskate-textures`), and the writer writing textures as prepared (no cap of its own, no
//! tint). Synthetic worlds, made here; the expected values are worked by hand from the sRGB curve.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pass::{Pass, PassLog};
use primase_core::{
    ColorSpace, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture,
    Topology, Transform, World,
};
use primase_reskate::EncodingOptions;
use primase_reskate::components::{
    FileRef, MaterialClass, Other, SkateBaseFactor, SkateMaterial, SkateTexture,
};
use primase_reskate::passes::{BudgetReskateTextures, PrepareReskateTextures};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir()
        .join(format!("primase-reskate-prepare-textures-{}", std::process::id()))
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

/// A `side` x `side` RGBA8 texture of one texel value, as a raised world holds it.
fn texture(w: &mut World, key: &str, side: u32, texel: [u8; 4]) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(key.into()));
    w.insert(
        e,
        Texture {
            width: side,
            height: side,
            format: TexFormat::Rgba8Unorm,
            levels: vec![texel.repeat((side * side) as usize)],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    w.insert(
        e,
        SkateTexture {
            key: key.into(),
            file: FileRef {
                path: format!("textures/{key}.png"),
                sha256: None,
            },
            stem: None,
            source_format: None,
            source_size: None,
            linear: Some(false),
            green: None,
            z: None,
            other: Other::new(),
        },
    );
    e
}

fn material(w: &mut World, key: &str, base: Option<Entity>, tint: Option<[u8; 3]>) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        SkateMaterial {
            key: key.into(),
            class: MaterialClass::Opaque,
            base,
            normal: None,
            mask: None,
            base_color: None,
            tint,
            alpha: None,
            cutoff: None,
            two_sided: None,
            domain: None,
            metallic: None,
            roughness: None,
            wrap: None,
            ocean: None,
            cast_shadows: None,
            emissive: None,
            vertex_alpha: None,
            other: Other::new(),
        },
    );
    e
}

/// sRGB byte -> linear -> times `t` (linear) -> sRGB byte, by the IEC 61966-2-1 curve.
fn srgb_times(byte: u8, t: f64) -> u8 {
    let lin = |c: f64| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let enc = |l: f64| {
        if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        }
    };
    (enc(lin(f64::from(byte) / 255.0) * t) * 255.0).round() as u8
}

fn linear(byte: u8) -> f64 {
    let c = f64::from(byte) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A tint is multiplied into a copy of the base texture, in linear light, alpha untouched: one copy per (texture,
/// tint), shared by materials alike; the base stays for the untinted material; white multiplies nothing; a tint
/// with no texture is left out and reported; a second run changes nothing.
#[test]
fn tints_are_multiplied_into_texture_copies() {
    let mut w = World::new();
    let base = texture(&mut w, "brick", 4, [200, 100, 50, 128]);
    let red = [255, 128, 128];
    let (a, b) = (
        material(&mut w, "a", Some(base), Some(red)),
        material(&mut w, "b", Some(base), Some(red)),
    );
    let plain = material(&mut w, "plain", Some(base), None);
    let white = material(&mut w, "white", Some(base), Some([255; 3]));
    let flat = material(&mut w, "flat", None, Some(red));
    // a texture only ever drawn tinted
    let stone = texture(&mut w, "stone", 4, [50, 50, 50, 255]);
    let c = material(&mut w, "c", Some(stone), Some(red));
    // what the placed primitives draw: brick plain and tinted, stone only tinted
    for (i, m) in [a, b, plain, c].into_iter().enumerate() {
        quad(&mut w, m, 1.0, 5.0 * i as f32);
    }
    let mut log = PassLog::default();
    PrepareReskateTextures.run(&mut w, &mut log).unwrap();
    let m = |e| w.get::<SkateMaterial>(e).unwrap().clone();
    let copy = m(a).base.unwrap();
    assert_eq!(m(b).base, Some(copy));
    assert_ne!(copy, base);
    assert_eq!(
        (m(a).tint, m(plain).base, m(white).base, m(white).tint),
        (None, Some(base), Some(base), None)
    );
    assert_eq!((m(flat).base, m(flat).tint), (None, None));
    assert_eq!(log.counts["tinted texture copies"], 2);
    // stone is drawn by no placed primitive now: its pixels stay, it is no longer the map's
    assert_ne!(m(c).base, Some(stone));
    assert!(!w.has::<SkateTexture>(stone) && w.has::<Texture>(stone));
    assert_eq!(
        log.omitted["material tints without a base texture (nothing to tint)"],
        1
    );
    // the copy's texel: each channel times the tint's in linear light; alpha as it was
    let t = w.get::<Texture>(copy).unwrap();
    let want = [
        srgb_times(200, linear(255)),
        srgb_times(100, linear(128)),
        srgb_times(50, linear(128)),
        128,
    ];
    assert_eq!(&t.levels[0][..4], &want);
    assert_eq!((t.width, t.height, t.format), (4, 4, TexFormat::Rgba8Unorm));
    // named as its base, recorded as made from it
    assert_eq!(w.get::<Name>(copy).unwrap().0, "brick");
    let st = w.get::<SkateTexture>(copy).unwrap();
    assert_eq!(
        (st.source_format.as_deref(), st.source_size),
        (Some("R8G8B8A8"), Some([4, 4]))
    );
    // brick is still drawn (by `plain`'s quad): still the map's
    assert!(w.has::<SkateTexture>(base));
    // again: nothing to do
    let before = w.iter::<Texture>().count();
    PrepareReskateTextures
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    assert_eq!(w.iter::<Texture>().count(), before);
}

/// A quad of `side` metres drawn with `m`, placed once at `x`.
fn quad(w: &mut World, m: Entity, side: f32, x: f32) {
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![
                    [0.0, 0.0, 0.0],
                    [side, 0.0, 0.0],
                    [0.0, 0.0, side],
                    [side, 0.0, side],
                ]),
            )]),
            indices: Some(vec![0, 1, 2, 2, 1, 3]),
            material: Some(m),
        }]),
    );
    let at = w.spawn();
    w.insert(
        at,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [x, 0.0, 0.0];
    w.insert(at, t);
}

/// Over the budget, the heaviest textures by placed area stay and one place is white: a material whose base texture
/// goes draws white with that texture's mean colour as its factor; ties go by texture key, whatever order the world
/// was made in; within the budget nothing changes.
#[test]
fn the_budget_keeps_the_heaviest_textures() {
    let make = |reverse: bool| {
        let mut w = World::new();
        // areas 9, 4, 4 (a tie) and 1 square metres
        let mut specs = vec![
            ("big", 3.0, [10, 20, 30, 255]),
            ("tie_b", 2.0, [0; 4]),
            ("tie_a", 2.0, [0; 4]),
            ("small", 1.0, [255, 0, 0, 255]),
        ];
        if reverse {
            specs.reverse();
        }
        let mut mats = BTreeMap::new();
        for (i, (key, side, texel)) in specs.into_iter().enumerate() {
            let t = texture(&mut w, key, 4, texel);
            let m = material(&mut w, key, Some(t), None);
            quad(&mut w, m, side, 10.0 * i as f32);
            mats.insert(key, (m, t));
        }
        (w, mats)
    };
    for reverse in [false, true] {
        let (mut w, mats) = make(reverse);
        let mut log = PassLog::default();
        BudgetReskateTextures::new(3)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        let base = |k: &str| w.get::<SkateMaterial>(mats[k].0).unwrap().base;
        assert_eq!(base("big"), Some(mats["big"].1), "reverse {reverse}");
        assert_eq!(base("tie_a"), Some(mats["tie_a"].1), "reverse {reverse}");
        assert_eq!(base("tie_b"), None, "reverse {reverse}");
        assert_eq!(base("small"), None, "reverse {reverse}");
        let small = w.get::<SkateMaterial>(mats["small"].0).unwrap();
        assert_eq!(small.base_color, Some([255, 255, 255, 255]));
        let f = w.get::<SkateBaseFactor>(mats["small"].0).unwrap().0;
        assert_eq!(f, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(log.counts["textures dropped by the budget"], 2);
    }
    let (mut w, mats) = make(false);
    BudgetReskateTextures::new(4)
        .unwrap()
        .run(&mut w, &mut PassLog::default())
        .unwrap();
    assert_eq!(
        w.get::<SkateMaterial>(mats["small"].0).unwrap().base,
        Some(mats["small"].1)
    );
    assert!(w.iter::<SkateBaseFactor>().next().is_none());
}

/// The writer writes a texture at the size it is given (no cap of its own) and does not tint: a material still
/// tinted is reported, and its texture written as it is.
#[test]
fn the_writer_writes_textures_as_prepared() {
    let mut w = World::new();
    let t = texture(&mut w, "big", 512, [90, 90, 90, 255]);
    let m = material(&mut w, "m", Some(t), Some([255, 0, 0]));
    quad(&mut w, m, 2.0, 0.0);
    w.insert_resource(primase_reskate::components::SkateScene {
        name: "prepared".into(),
        ..Default::default()
    });
    // the prototypes as the dialect needs them, from raise-skate's rules for a mesh with one part
    let protos: Vec<Entity> = w.iter::<Mesh>().map(|(e, _)| e).collect();
    for (i, p) in protos.into_iter().enumerate() {
        w.insert(
            p,
            primase_reskate::components::SkatePrototype {
                id: format!("p{i}"),
                glb: Some(FileRef {
                    path: format!("prototypes/p{i}.glb"),
                    sha256: None,
                }),
                key: None,
                foliage: None,
                ground: None,
                collision: None,
                other: Other::new(),
            },
        );
        w.insert(
            p,
            primase_reskate::components::SkateParts(vec![primase_reskate::components::Part::default()]),
        );
    }
    // a spawn with its height (the map has no ground to snap to)
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_reskate::components::SkateMarker {
            kind: primase_reskate::components::MarkerKind::Spawn,
            position: Some(([1.0, 1.0], Some(0.0))),
            yaw: Some(0.0),
            m: None,
            blueprint: None,
            snap: None,
            lift: None,
            shelter: None,
            trainer: None,
            other: Other::new(),
        },
    );
    let placements: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
    for e in placements {
        w.insert(e, primase_reskate::components::SkatePlacement::default());
    }
    let out_root = tmp("writer");
    let r = primase_reskate::write_map(
        &w,
        &PathBuf::new(),
        &EncodingOptions {
            out_root: out_root.clone(),
            ..EncodingOptions::default()
        },
    )
    .unwrap();
    assert!(
        r.warnings.iter().any(|x| x.contains("tints not written")),
        "{:?}",
        r.warnings
    );
    assert!(
        r.omitted
            .iter()
            .any(|(what, n)| what.starts_with("skate.material tint") && *n == 1),
        "{:?}",
        r.omitted
    );
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_root.join("prepared/textures_index.json")).unwrap())
            .unwrap();
    let big = index["textures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["sources"][0] == "big")
        .unwrap();
    assert_eq!((big["w"].as_u64(), big["h"].as_u64()), (Some(512), Some(512)));
}

/// A base texture whose level cannot be decoded keeps its material's tint, counted (as the pass's documentation says),
/// and the rest of the pass runs: it is not an error of the whole map.
#[test]
fn a_texture_that_cannot_be_decoded_keeps_its_tint() {
    let mut w = World::new();
    let broken = texture(&mut w, "broken", 4, [200, 100, 50, 255]);
    w.get_mut::<Texture>(broken).unwrap().levels = vec![vec![1, 2, 3]];
    let fine = texture(&mut w, "fine", 4, [50, 50, 50, 255]);
    let (a, b) = (
        material(&mut w, "a", Some(broken), Some([255, 128, 128])),
        material(&mut w, "b", Some(fine), Some([255, 128, 128])),
    );
    let mut log = PassLog::default();
    PrepareReskateTextures.run(&mut w, &mut log).unwrap();
    assert_eq!(w.get::<SkateMaterial>(a).unwrap().tint, Some([255, 128, 128]));
    assert_eq!(w.get::<SkateMaterial>(a).unwrap().base, Some(broken));
    assert_eq!(w.get::<SkateMaterial>(b).unwrap().tint, None);
    assert_eq!(log.counts["material tints kept: base texture not decodable"], 1);
}
