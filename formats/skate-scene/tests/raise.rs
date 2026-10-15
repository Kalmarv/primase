//! raise-skate on a synthetic core world: raised, its assets written, written as a scene and read back.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    Collision, ColorSpace, Instance, Light, LightKind, Mesh, Name, NormalConvention, NormalGreen, NormalX,
    NormalZ, Primitive, Semantic, Stream, Surfaces, TexFormat, TexRole, Texture, Topology, Transform, World,
};
use primase_skate_scene::{
    self as scene, AlphaMode, MaterialClass, RaiseOptions, SkateLight, SkateMaterial, SkatePrototype,
    SkateTexture,
};

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("skate-raise")
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    d
}

fn texture(role: TexRole) -> Texture {
    Texture {
        width: 2,
        height: 2,
        format: TexFormat::Rgba8Unorm,
        levels: vec![[200, 100, 50, 255].repeat(4)],
        color_space: if role == TexRole::Color {
            ColorSpace::Srgb
        } else {
            ColorSpace::Linear
        },
        role,
        normal: (role == TexRole::Normal).then_some(NormalConvention::GLTF),
    }
}

fn world() -> World {
    let mut w = World::new();
    let base = w.spawn();
    w.insert(base, Name("rock albedo".into()));
    w.insert(base, texture(TexRole::Color));
    let normal = w.spawn();
    w.insert(normal, Name("rock normal".into()));
    w.insert(normal, texture(TexRole::Normal));
    let rock = w.spawn();
    w.insert(rock, Name("rock".into()));
    w.insert(
        rock,
        PbrMaterial {
            base_color: [0.5, 1.0, 1.0, 1.0],
            base_color_texture: Some(TextureRef::new(base)),
            normal_texture: Some(TextureRef::new(normal)),
            metallic: 0.0,
            roughness: 0.75,
            alpha_mode: PbrAlpha::Mask { cutoff: 0.4 },
            double_sided: true,
            ..PbrMaterial::default()
        },
    );
    let plain = w.spawn();
    w.insert(plain, Name("plain".into()));
    w.insert(
        plain,
        PbrMaterial {
            base_color: [0.2, 0.2, 0.2, 1.0],
            metallic: 0.0,
            ..PbrMaterial::default()
        },
    );
    let proto = w.spawn();
    w.insert(proto, Name("boulder".into()));
    let prim = |m| Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([
            (
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]]),
            ),
            (
                Semantic::TexCoord(0),
                Stream::F32x2(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
            ),
        ]),
        indices: None,
        material: Some(m),
    };
    w.insert(
        proto,
        Mesh {
            primitives: vec![prim(rock), prim(plain)],
        },
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
    t.cols[3] = [3.0, 0.0, 0.0];
    w.insert(at, t);
    // collision made of a game's surface
    let mut surfaces = Surfaces::default();
    let snow = surfaces.index("Snow");
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        &[[0, 1, 2]],
        snow,
        |p| p,
    );
    w.insert(ground, c);
    // a spot light pointing down, and a sun
    let lamp = w.spawn();
    w.insert(
        lamp,
        Light {
            kind: LightKind::Spot {
                inner_cone: 0.25,
                outer_cone: 0.5,
            },
            color: [1.0, 0.5, 0.25],
            intensity: 10.0,
            range: Some(8.0),
        },
    );
    let mut t = Transform::IDENTITY;
    // local -Z onto world -Y
    t.cols[1] = [0.0, 0.0, -1.0];
    t.cols[2] = [0.0, 1.0, 0.0];
    t.cols[3] = [0.0, 5.0, 0.0];
    w.insert(lamp, t);
    let sun = w.spawn();
    w.insert(
        sun,
        Light {
            kind: LightKind::Directional,
            color: [1.0; 3],
            intensity: 3.0,
            range: None,
        },
    );
    w
}

fn options() -> RaiseOptions {
    RaiseOptions {
        name: "raised".into(),
        surfaces: BTreeMap::from([("Snow".to_owned(), "asphalt".to_owned())]),
    }
}

#[test]
fn a_core_world_becomes_a_scene() {
    let mut w = world();
    let log = scene::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["materials"], 2);
    assert_eq!(log.counts["textures"], 2);
    assert_eq!(log.counts["prototypes"], 1);
    assert_eq!(log.counts["lights"], 1);
    assert_eq!(
        log.counts["directional lights not raised (skate lights itself by its time of day)"],
        1
    );
    let mats: BTreeMap<&str, &SkateMaterial> = w
        .iter::<SkateMaterial>()
        .map(|(_, m)| (m.key.as_str(), m))
        .collect();
    let rock = mats["rock"];
    assert_eq!(
        (rock.class, rock.alpha, rock.cutoff),
        (MaterialClass::Cutout, Some(AlphaMode::Mask), Some(0.4))
    );
    assert_eq!(rock.tint, Some([188, 255, 255]), "the factor as an sRGB tint");
    assert_eq!(rock.two_sided, Some(true));
    let plain = mats["plain"];
    assert_eq!(plain.class, MaterialClass::Opaque);
    assert_eq!(plain.base_color, Some([124, 124, 124, 255]));
    let keys: Vec<String> = w.iter::<SkateTexture>().map(|(_, t)| t.key.clone()).collect();
    assert_eq!(keys, ["rock_albedo", "rock_normal"]);
    let l: Vec<&SkateLight> = w.iter::<SkateLight>().map(|(_, l)| l).collect();
    assert_eq!(l.len(), 1);
    assert!(l[0].spot && (l[0].spot_size - 1.0).abs() < 1e-6 && (l[0].spot_blend - 0.5).abs() < 1e-6);
    assert_eq!(l[0].direction, [0.0, -1.0, 0.0]);
    assert!((l[0].energy - scene::studio_watts(10.0)).abs() < 1e-3);
    // assets, the scene, and back
    let assets = tmp("assets");
    scene::prepare_scene_assets(&mut w, &assets).unwrap();
    let out = tmp("scene");
    scene::write(&w, &out).unwrap();
    let mut back = World::new();
    scene::read(&out, &mut back).unwrap();
    let protos: Vec<&SkatePrototype> = back.iter::<SkatePrototype>().map(|(_, p)| p).collect();
    assert_eq!(
        (protos.len(), protos[0].id.as_str(), protos[0].parts.len()),
        (1, "boulder", 2)
    );
    assert_eq!(back.iter::<Instance>().count(), 1);
    assert_eq!(back.iter::<SkateLight>().count(), 1);
    // the soup's surface renamed onto skate's asphalt
    let names: Vec<String> = back
        .resource::<Surfaces>()
        .unwrap()
        .0
        .iter()
        .map(|s| s.name.clone())
        .collect();
    let c: Vec<&Collision> = back.iter::<Collision>().map(|(_, c)| c).collect();
    assert_eq!(c.len(), 1);
    assert_eq!(names[c[0].surfaces[0] as usize], "asphalt");
}

#[test]
fn what_skate_cannot_take_is_refused() {
    // a surface with no skate surface
    let mut w = world();
    let err = scene::raise_skate(
        &mut w,
        &RaiseOptions {
            name: "x".into(),
            surfaces: BTreeMap::new(),
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("Snow"), "{err}");
    // a texture transform still to bake
    let mut w = world();
    let (m, _) = w.iter::<PbrMaterial>().next().unwrap();
    w.get_mut::<PbrMaterial>(m)
        .unwrap()
        .base_color_texture
        .as_mut()
        .unwrap()
        .transform = Some(UvTransform::IDENTITY);
    assert!(
        scene::raise_skate(&mut w, &options())
            .unwrap_err()
            .to_string()
            .contains("bake")
    );
    // a normal map not in glTF's convention
    let mut w = world();
    let (n, _) = w
        .iter::<Texture>()
        .find(|(_, t)| t.role == TexRole::Normal)
        .unwrap();
    w.get_mut::<Texture>(n).unwrap().normal = Some(NormalConvention {
        green: NormalGreen::Down,
        z: NormalZ::Stored,
        x: NormalX::Red,
        basis: primase_core::NormalBasis::TangentBitangent,
    });
    assert!(
        scene::raise_skate(&mut w, &options())
            .unwrap_err()
            .to_string()
            .contains("normalize-normals")
    );
    // raised twice
    let mut w = world();
    scene::raise_skate(&mut w, &options()).unwrap();
    assert!(scene::raise_skate(&mut w, &options()).is_err());
}
