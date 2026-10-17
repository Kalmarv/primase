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
    let snow = surfaces.index("Snow").unwrap();
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
            cast_shadows: None,
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
            cast_shadows: None,
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
    // a material nothing placed is drawn with, its transform unbaked: left out, not refused
    let (base, _) = w.iter::<Texture>().next().unwrap();
    let unused = w.spawn();
    w.insert(
        unused,
        PbrMaterial {
            base_color_texture: Some(TextureRef {
                transform: Some(UvTransform::from_bottom_left([2.0, 2.0], [0.0, 0.0])),
                ..TextureRef::new(base)
            }),
            ..PbrMaterial::default()
        },
    );
    let log = scene::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["materials"], 2);
    assert_eq!(
        log.counts["materials no placed primitive is drawn with, not raised"],
        1
    );
    assert!(!w.has::<SkateMaterial>(unused));
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
    let protos: Vec<(primase_core::Entity, &SkatePrototype)> = back.iter::<SkatePrototype>().collect();
    assert_eq!(
        (
            protos.len(),
            protos[0].1.id.as_str(),
            back.get::<scene::SkateParts>(protos[0].0).unwrap().0.len()
        ),
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

fn masked_world() -> (World, primase_core::Entity, primase_core::Entity) {
    let mut w = world();
    let material = w.iter::<PbrMaterial>().next().unwrap().0;
    let image = w.spawn();
    let mut pixels = texture(TexRole::Mask);
    pixels.levels = vec![[64, 128, 255, 17].repeat(4), vec![200, 31, 80, 99]];
    w.insert(image, pixels);
    let m = w.get_mut::<PbrMaterial>(material).unwrap();
    m.metallic = 1.0;
    m.roughness = 1.0;
    m.metallic_roughness_texture = Some(TextureRef::new(image));
    m.occlusion_texture = Some(TextureRef::new(image));
    (w, material, image)
}

#[test]
fn packed_masks_preserve_channels_mips_and_material_bindings() {
    let (mut w, m, source) = masked_world();
    let original = w.get::<PbrMaterial>(m).unwrap().clone();
    let source_pixels = w.get::<Texture>(source).unwrap().levels.clone();
    let log = scene::raise_skate(&mut w, &options()).unwrap();
    let raised = w.get::<SkateMaterial>(m).unwrap();
    assert_eq!((raised.metallic, raised.roughness), (None, None));
    let mask = raised.mask.unwrap();
    let pixels = w.get::<Texture>(mask).unwrap();
    // Literal oracle, independent of the mapper and inverse converter.
    assert_eq!(
        pixels.levels,
        [[255, 127, 64, 255].repeat(4), vec![80, 224, 200, 255]]
    );
    assert_eq!(pixels.color_space, ColorSpace::Linear);
    assert_eq!(pixels.role, TexRole::Mask);
    assert_eq!(
        scene::msk_to_orm(pixels).unwrap().levels,
        [[64, 128, 255, 255].repeat(4), vec![200, 31, 80, 255]]
    );
    assert_eq!(w.get::<PbrMaterial>(m), Some(&original));
    assert_eq!(w.get::<Texture>(source).unwrap().levels, source_pixels);
    assert_eq!(log.counts["derived MSK textures"], 1);
    assert_eq!(log.counts["PBR materials raised with an MSK binding"], 1);
    assert!(
        !log.approximations
            .contains_key("PBR mask factors quantized to UNORM8")
    );
    let assets = tmp("mask-assets");
    scene::prepare_scene_assets(&mut w, &assets).unwrap();
    let out = tmp("mask-scene");
    scene::write(&w, &out).unwrap();
    let mut read = World::new();
    scene::read(&out, &mut read).unwrap();
    let material = read
        .iter::<SkateMaterial>()
        .find(|(_, x)| x.key == "rock")
        .unwrap()
        .1;
    let target = read.get::<SkateTexture>(material.mask.unwrap()).unwrap();
    assert!(out.join(&target.file.path).is_file());
    assert_eq!((material.metallic, material.roughness), (None, None));
}

#[test]
fn mask_factors_and_explicit_occlusion_have_literal_results() {
    for (mr, ao, factors, expected) in [
        (true, true, [0.5, 0.5, 0.5], [128, 191, 160, 255]),
        (true, false, [0.0, 1.0, 1.0], [0, 127, 255, 255]),
        (false, true, [0.0, 0.0, 0.0], [0, 255, 255, 255]),
        (false, true, [1.0, 1.0, 1.0], [255, 0, 64, 255]),
    ] {
        let (mut w, material, _) = masked_world();
        let m = w.get_mut::<PbrMaterial>(material).unwrap();
        if !mr {
            m.metallic_roughness_texture = None;
        }
        if !ao {
            m.occlusion_texture = None;
        }
        [m.metallic, m.roughness, m.occlusion_strength] = factors;
        let log = scene::raise_skate(&mut w, &options()).unwrap();
        let mask = w.get::<SkateMaterial>(material).unwrap().mask.unwrap();
        assert_eq!(w.get::<Texture>(mask).unwrap().levels[0], expected.repeat(4));
        assert_eq!(
            log.approximations
                .get("PBR mask factors quantized to UNORM8")
                .copied()
                .unwrap_or(0),
            u64::from(factors == [0.5; 3])
        );
    }
}

#[test]
fn mask_cache_separates_factors_and_counts_materials_in_variants() {
    let (mut w, first, _) = masked_world();
    let shared = w.spawn();
    let material = w.get::<PbrMaterial>(first).unwrap().clone();
    w.insert(shared, material.clone());
    let zero = w.spawn();
    w.insert(
        zero,
        PbrMaterial {
            metallic: 0.0,
            ..material
        },
    );
    let prototype = w.iter::<Mesh>().next().unwrap().0;
    w.insert(
        prototype,
        primase_core::MaterialVariants(vec![BTreeMap::from([(0, shared), (1, zero)])]),
    );
    let log = scene::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["PBR materials raised with an MSK binding"], 3);
    assert_eq!(log.counts["derived MSK textures"], 2);
    let mask = |e| w.get::<SkateMaterial>(e).unwrap().mask.unwrap();
    assert_eq!(mask(first), mask(shared));
    assert_ne!(mask(first), mask(zero));
    assert_eq!(w.get::<Texture>(mask(zero)).unwrap().levels[0][0], 0);
}

fn rejected_unchanged(mut w: World) {
    let mut before = w.clone();
    assert!(scene::raise_skate(&mut w, &options()).is_err());
    assert_eq!(
        w.entities().collect::<Vec<_>>(),
        before.entities().collect::<Vec<_>>()
    );
    assert_eq!(w.resources_json(), before.resources_json());
    for e in w.entities() {
        assert_eq!(w.components_json(e), before.components_json(e));
        if let Some(t) = w.get::<Texture>(e) {
            assert_eq!(t.levels, before.get::<Texture>(e).unwrap().levels);
        }
    }
    assert_eq!(w.spawn(), before.spawn());
}

#[test]
fn unsupported_masks_refuse_without_partial_surface_or_entity_changes() {
    for case in 0..11 {
        let (mut w, material, source) = masked_world();
        match case {
            0 => w.get_mut::<Texture>(source).unwrap().levels[1]
                .pop()
                .map(|_| ())
                .unwrap(),
            1 => w.get_mut::<Texture>(source).unwrap().levels.clear(),
            2 => w.get_mut::<Texture>(source).unwrap().color_space = ColorSpace::Srgb,
            3 => w.get_mut::<PbrMaterial>(material).unwrap().metallic = f32::NAN,
            4 => w.get_mut::<PbrMaterial>(material).unwrap().roughness = 2.0,
            5 => w.get_mut::<PbrMaterial>(material).unwrap().normal_scale = 0.5,
            6 => {
                w.get_mut::<PbrMaterial>(material)
                    .unwrap()
                    .occlusion_texture
                    .as_mut()
                    .unwrap()
                    .texcoord = 1
            }
            7 => {
                w.get_mut::<PbrMaterial>(material)
                    .unwrap()
                    .metallic_roughness_texture
                    .as_mut()
                    .unwrap()
                    .wrap = [primase_core::pbr::Wrap::Clamp; 2]
            }
            8 => {
                let other = w.spawn();
                w.insert(other, texture(TexRole::Mask));
                w.get_mut::<PbrMaterial>(material).unwrap().occlusion_texture = Some(TextureRef::new(other));
            }
            9 => {
                w.despawn(source);
            }
            10 => w.get_mut::<PbrMaterial>(material).unwrap().emissive_strength = f32::INFINITY,
            _ => unreachable!(),
        }
        rejected_unchanged(w);
    }
}

#[test]
fn emission_losses_are_per_material_and_do_not_claim_radiance_transport() {
    let (mut w, first, source) = masked_world();
    let m = w.get_mut::<PbrMaterial>(first).unwrap();
    m.emissive = [0.25, 0.5, 1.0];
    m.emissive_strength = 10.0;
    m.emissive_texture = Some(TextureRef::new(source));
    let other = w.iter::<PbrMaterial>().find(|(e, _)| *e != first).unwrap().0;
    w.get_mut::<PbrMaterial>(other).unwrap().emissive_texture = Some(TextureRef::new(source));
    let log = scene::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(
        log.approximations["PBR emission texture binding not representable in skate-scene"],
        2
    );
    assert_eq!(
        log.approximations["PBR emissive radiance not representable in skate-scene"],
        1
    );
    assert_eq!(w.get::<SkateMaterial>(first).unwrap().emissive, None);
    assert_eq!(w.get::<PbrMaterial>(first).unwrap().emissive_strength, 10.0);
}

#[test]
fn unbound_normal_scale_is_inert_and_scalar_materials_stay_scalar() {
    let mut w = world();
    let m = w
        .iter::<PbrMaterial>()
        .find(|(_, m)| m.normal_texture.is_none())
        .unwrap()
        .0;
    w.get_mut::<PbrMaterial>(m).unwrap().normal_scale = 0.5;
    scene::raise_skate(&mut w, &options()).unwrap();
    let raised = w.get::<SkateMaterial>(m).unwrap();
    assert_eq!(
        (raised.mask, raised.metallic, raised.roughness),
        (None, Some(0.0), Some(1.0))
    );
}

#[test]
fn explicit_raise_uses_pbr_and_counts_shared_quantization_per_material() {
    let (mut w, first, source) = masked_world();
    w.get_mut::<PbrMaterial>(first).unwrap().metallic = 0.5;
    let second = w.iter::<PbrMaterial>().find(|(e, _)| *e != first).unwrap().0;
    let pbr = w.get::<PbrMaterial>(first).unwrap().clone();
    w.insert(second, pbr.clone());
    // A retained dialect value is provenance, not the authority at this explicit conversion boundary.
    w.insert(
        first,
        SkateMaterial {
            key: "conflicting".into(),
            class: MaterialClass::Glass,
            base: None,
            normal: None,
            mask: Some(source),
            base_color: Some([0; 4]),
            tint: None,
            alpha: None,
            cutoff: None,
            two_sided: None,
            domain: None,
            metallic: Some(1.0),
            roughness: Some(0.0),
            wrap: None,
            ocean: None,
            cast_shadows: None,
            emissive: Some(true),
            vertex_alpha: None,
            other: Default::default(),
        },
    );
    let log = scene::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.approximations["PBR mask factors quantized to UNORM8"], 2);
    assert_eq!(log.counts["derived MSK textures"], 1);
    let a = w.get::<SkateMaterial>(first).unwrap();
    let b = w.get::<SkateMaterial>(second).unwrap();
    assert_eq!(a.mask, b.mask);
    assert_ne!(a.mask, Some(source));
    assert_eq!(a.class, MaterialClass::Cutout);
    assert_eq!(
        (a.base, a.normal),
        (
            pbr.base_color_texture.map(|r| r.texture),
            pbr.normal_texture.map(|r| r.texture)
        )
    );
    assert_eq!((a.metallic, a.roughness, a.emissive), (None, None, None));
    assert_eq!(w.get::<PbrMaterial>(first), Some(&pbr));
}
