//! raise-skate on a synthetic core world: raised, its assets written, written as a scene and read back.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::pbr::{AlphaMode as PbrAlpha, PbrMaterial, TextureRef, UvTransform};
use primase_core::{
    Collision, ColorSpace, Instance, Light, LightKind, Mesh, Name, NormalConvention, NormalGreen, NormalX,
    NormalZ, Primitive, Semantic, Stream, Surfaces, TexFormat, TexRole, Texture, Topology, Transform, World,
};
use primase_reskate::components::{
    self, AlphaMode, MaterialClass, SkateLight, SkateMaterial, SkatePrototype, SkateTexture,
};
use primase_reskate::passes::{self, RaiseOptions};
use primase_reskate::{read, write};

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
    w.insert(proto, Mesh::new(vec![prim(rock), prim(plain)]));
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
    RaiseOptions::new("raised", primase_reskate::passes::LightCalibration::Standard)
        .unwrap()
        .with_surfaces(BTreeMap::from([("Snow".to_owned(), "asphalt".to_owned())]))
        .unwrap()
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
    // the spawn, as place-spawn puts it at selection: a ground marker at (east 2, north 1)
    let spawn = w.spawn();
    w.insert(
        spawn,
        primase_core::Marker {
            kind: "spawn".into(),
            ground: Some([2.0, 1.0]),
            yaw: None,
        },
    );
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["materials"], 2);
    assert_eq!(
        log.counts["materials no placed primitive is drawn with, not raised"],
        1
    );
    assert!(!w.has::<SkateMaterial>(unused));
    assert_eq!(log.counts["textures"], 2);
    assert_eq!(log.counts["prototypes"], 1);
    assert_eq!(log.counts["lights"], 1);
    assert_eq!(log.counts["markers"], 1);
    let spawn: Vec<&components::SkateMarker> = w.iter::<components::SkateMarker>().map(|(_, m)| m).collect();
    assert_eq!(spawn.len(), 1);
    assert_eq!(
        (spawn[0].kind, spawn[0].position, spawn[0].snap.clone()),
        (
            components::MarkerKind::Spawn,
            Some(([2.0, -1.0], None)),
            Some(components::Snap::Mode(components::SnapMode::Ground))
        )
    );
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
    // 10 cd through raise-skate's calibration
    assert!((l[0].energy - passes::studio_watts((10.0 * passes::SKATE_LIGHT_SCALE) as f32)).abs() < 1e-6);
    // assets, the scene, and back
    let out = tmp("scene");
    write::interchange::write(&w, &out).unwrap();
    let mut back = World::new();
    read::interchange::read(&out, &mut back).unwrap();
    let protos: Vec<(primase_core::Entity, &SkatePrototype)> = back.iter::<SkatePrototype>().collect();
    assert_eq!(
        (
            protos.len(),
            protos[0].1.id.as_str(),
            back.get::<components::SkateParts>(protos[0].0).unwrap().0.len()
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
fn surface_classes_map_onto_skate_surfaces_unless_the_caller_maps_the_name() {
    use primase_core::SurfaceClass;
    // The hand-worked table of `class_to_skate`: a class names the standard surface a skater feels; water has none.
    let table = [
        (SurfaceClass::Concrete, Some("concrete")),
        (SurfaceClass::Asphalt, Some("asphalt")),
        (SurfaceClass::Stone, Some("marble")),
        (SurfaceClass::Brick, Some("brick")),
        (SurfaceClass::Tile, Some("roofing")),
        (SurfaceClass::Metal, Some("metal")),
        (SurfaceClass::Wood, Some("wood")),
        (SurfaceClass::Glass, Some("glass")),
        (SurfaceClass::Plastic, Some("plastic")),
        (SurfaceClass::Rubber, Some("rubber")),
        (SurfaceClass::Fabric, Some("default")),
        (SurfaceClass::Grass, Some("grass")),
        (SurfaceClass::Dirt, Some("earth")),
        (SurfaceClass::Gravel, Some("gravel")),
        (SurfaceClass::Sand, Some("sand")),
        (SurfaceClass::Mud, Some("earth")),
        (SurfaceClass::Snow, Some("earth")),
        (SurfaceClass::Ice, Some("marble")),
        (SurfaceClass::Water, None),
        (SurfaceClass::Other, Some("default")),
    ];
    for (class, skate) in table {
        assert_eq!(passes::class_to_skate(class), skate, "{class:?}");
    }
    let classes: Vec<_> = table.iter().map(|(class, _)| *class).collect();
    assert_eq!(classes, SurfaceClass::ALL);
    let raised = |class, map: &[(&str, &str)]| {
        let mut w = world();
        w.resource_mut::<Surfaces>().unwrap().0[0].class = class;
        let r = passes::raise_skate(
            &mut w,
            &RaiseOptions::new("x", primase_reskate::passes::LightCalibration::Standard)
                .unwrap()
                .with_surfaces(map.iter().map(|&(a, b)| (a.to_owned(), b.to_owned())).collect())
                .unwrap(),
        );
        r.map(|_| w.resource::<Surfaces>().unwrap().0[0].name.clone())
            .map_err(|e| e.to_string())
    };
    assert_eq!(raised(Some(SurfaceClass::Snow), &[]).unwrap(), "earth");
    // the caller's map of the name wins over the class
    assert_eq!(
        raised(Some(SurfaceClass::Snow), &[("Snow", "asphalt")]).unwrap(),
        "asphalt"
    );
    // a water surface is kept (its triangles water collision), its name a placeholder
    assert_eq!(raised(Some(SurfaceClass::Water), &[]).unwrap(), "default");
}

/// A prototype whose collision is all on a water surface gets water collision; the world's water triangles stay solid,
/// counted; a mixed prototype soup stays solid, counted. Its material's class is not inferred from the collision: only
/// `core.material_usage` makes a material water (`material_usage_makes_water_and_foliage`).
#[test]
fn water_collision() {
    use primase_core::SurfaceClass;
    for mixed in [false, true] {
        let mut w = world();
        w.resource_mut::<Surfaces>().unwrap().0[0].class = Some(SurfaceClass::Water);
        let rock = w.resource_mut::<Surfaces>().unwrap().index("Rock").unwrap();
        w.resource_mut::<Surfaces>().unwrap().0[usize::from(rock)].class = Some(SurfaceClass::Stone);
        let (proto, mesh) = w.iter::<Mesh>().next().map(|(e, m)| (e, m.clone())).unwrap();
        let mut c = Collision::default();
        let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]];
        c.extend(&tri, &[[0, 1, 2]], 0, |p| p);
        if mixed {
            c.extend(&tri, &[[0, 1, 2]], rock, |p| p);
        }
        w.insert(proto, c);
        // no surface map: the water class decides
        let log = passes::raise_skate(
            &mut w,
            &RaiseOptions::new("raised", primase_reskate::passes::LightCalibration::Standard).unwrap(),
        )
        .unwrap();
        let p = w.get::<SkatePrototype>(proto).unwrap();
        let mode = p.collision.as_ref().unwrap().mode;
        let material = mesh.primitives()[0].material.unwrap();
        let class = w.get::<SkateMaterial>(material).unwrap().class;
        assert_ne!(class, MaterialClass::Water);
        assert_eq!(
            log.approximations["water triangles in the world's collision kept solid"],
            1
        );
        if mixed {
            assert_eq!(mode, Some(components::CollisionMode::TriangleMesh));
            assert_eq!(
                log.approximations["water triangles in a solid collision soup kept solid"],
                1
            );
        } else {
            assert_eq!(mode, Some(components::CollisionMode::Water));
            assert_eq!(log.counts["prototypes with water collision"], 1);
        }
    }
}

/// `core.material_usage` says what a material's surfaces are: water is skate's water class with Studio's ocean; foliage
/// is skate's leaves in the foliage domain (alpha and sidedness from the source); foliage the source draws over the
/// surface beneath (a decal draw order) stays a decal, counted.
#[test]
fn material_usage_makes_water_and_foliage() {
    use primase_core::{DrawLayer, DrawOrder, MaterialUsage};
    let mut w = world();
    let named = |w: &World, name: &str| {
        w.iter::<Name>()
            .find(|(_, n)| n.0 == name)
            .map(|(e, _)| e)
            .unwrap()
    };
    let (rock, plain) = (named(&w, "rock"), named(&w, "plain"));
    w.insert(rock, MaterialUsage::Foliage);
    w.insert(plain, MaterialUsage::Water);
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    let m = |e| w.get::<SkateMaterial>(e).unwrap();
    assert_eq!(
        (m(rock).class, m(rock).domain, m(rock).alpha, m(rock).two_sided),
        (
            MaterialClass::Leaves,
            Some(components::Domain::Foliage),
            Some(AlphaMode::Mask),
            Some(true)
        )
    );
    assert_eq!(
        (m(plain).class, m(plain).domain, m(plain).ocean),
        (MaterialClass::Water, None, Some(true))
    );
    assert_eq!(log.counts["materials raised as foliage (core.material_usage)"], 1);
    assert_eq!(log.counts["materials raised as water (core.material_usage)"], 1);
    // foliage drawn as a decal over the ground keeps the decal
    let mut w = world();
    let rock = named(&w, "rock");
    w.insert(rock, MaterialUsage::Foliage);
    w.insert(rock, DrawOrder::conventional(DrawLayer::Decal, 2500));
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    let m = w.get::<SkateMaterial>(rock).unwrap();
    assert_eq!(
        (m.class, m.domain),
        (MaterialClass::Decal, Some(components::Domain::Decal))
    );
    assert_eq!(
        log.counts["foliage materials kept as decals (drawn over the surface beneath)"],
        1
    );
}

#[test]
fn a_material_the_source_draws_as_a_decal_is_raised_as_skates_decal() {
    use primase_core::{DrawLayer, DrawOrder};
    let mut w = world();
    let plain = w
        .iter::<Name>()
        .find(|(_, n)| n.0 == "plain")
        .map(|(e, _)| e)
        .unwrap();
    w.insert(plain, DrawOrder::conventional(DrawLayer::Decal, 2500));
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["materials raised as decals (core.draw_order)"], 1);
    let mats: BTreeMap<&str, &SkateMaterial> = w
        .iter::<SkateMaterial>()
        .map(|(_, m)| (m.key.as_str(), m))
        .collect();
    assert_eq!(mats["plain"].class, MaterialClass::Decal);
    assert_eq!(mats["plain"].domain, Some(components::Domain::Decal));
    // without a draw order the class comes from the alpha mode, as before
    assert_eq!(mats["rock"].class, MaterialClass::Cutout);
    assert_eq!(mats["rock"].domain, None);
}

#[test]
fn what_skate_cannot_take_is_refused() {
    // a scene without a name: refused before anything runs
    let err = RaiseOptions::new("", primase_reskate::passes::LightCalibration::Standard).unwrap_err();
    assert!(err.contains("scene name"), "{err}");
    // a surface map naming a surface skate does not have
    let mut w = world();
    let err = passes::raise_skate(
        &mut w,
        &RaiseOptions::new("x", primase_reskate::passes::LightCalibration::Standard)
            .unwrap()
            .with_surfaces(BTreeMap::from([("Snow".to_owned(), "lava".to_owned())]))
            .unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("Snow -> lava"), "{err}");
    assert!(
        w.resource::<components::SkateScene>().is_none(),
        "refused input leaves the world unchanged"
    );
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
        passes::raise_skate(&mut w, &options())
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
        passes::raise_skate(&mut w, &options())
            .unwrap_err()
            .to_string()
            .contains("normalize-normals")
    );
    // raised twice
    let mut w = world();
    passes::raise_skate(&mut w, &options()).unwrap();
    assert!(passes::raise_skate(&mut w, &options()).is_err());
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
    let log = passes::raise_skate(&mut w, &options()).unwrap();
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
        passes::lower::msk_to_orm(pixels).unwrap().levels,
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
    let out = tmp("mask-scene");
    write::interchange::write(&w, &out).unwrap();
    let mut read = World::new();
    read::interchange::read(&out, &mut read).unwrap();
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
        let log = passes::raise_skate(&mut w, &options()).unwrap();
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
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["PBR materials raised with an MSK binding"], 3);
    assert_eq!(log.counts["derived MSK textures"], 2);
    let mask = |e| w.get::<SkateMaterial>(e).unwrap().mask.unwrap();
    assert_eq!(mask(first), mask(shared));
    assert_ne!(mask(first), mask(zero));
    assert_eq!(w.get::<Texture>(mask(zero)).unwrap().levels[0][0], 0);
}

fn rejected_unchanged(mut w: World) {
    let mut before = w.clone();
    assert!(passes::raise_skate(&mut w, &options()).is_err());
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
    let log = passes::raise_skate(&mut w, &options()).unwrap();
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
    passes::raise_skate(&mut w, &options()).unwrap();
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
    let log = passes::raise_skate(&mut w, &options()).unwrap();
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

/// A blend (`core.layered`) a placed primitive is drawn with: a layered skate material, the blend kept, its layers'
/// materials raised although no primitive draws them directly, and a layer with no `pbr.material` refused.
#[test]
fn a_drawn_blend_becomes_a_layered_material() {
    use primase_core::{Layer, LayerWeights, LayeredMaterial, WeightMapping};
    let mut w = world();
    let (proto, _) = w.iter::<Mesh>().next().map(|(e, m)| (e, m.clone())).unwrap();
    let mats: Vec<primase_core::Entity> = w.iter::<PbrMaterial>().map(|(e, _)| e).collect();
    // two layer-only materials, a blend of them (and a null slot) drawn by the prototype's second primitive
    let (base, _) = w.iter::<Texture>().next().unwrap();
    let mut layers = Vec::new();
    for n in ["dirt", "grass"] {
        let e = w.spawn();
        w.insert(e, Name(n.into()));
        w.insert(
            e,
            PbrMaterial {
                base_color_texture: Some(TextureRef::new(base)),
                ..PbrMaterial::default()
            },
        );
        layers.push(e);
    }
    let blend = w.spawn();
    w.insert(blend, Name("terrain".into()));
    w.insert(
        blend,
        LayeredMaterial {
            layers: vec![
                Some(Layer {
                    material: layers[0],
                    texcoord: None,
                    transform: None,
                }),
                None,
                Some(Layer {
                    material: layers[1],
                    texcoord: None,
                    transform: None,
                }),
            ],
            weights: LayerWeights::new(Semantic::Color(1), WeightMapping::RgbProduct4),
        },
    );
    let mut mesh = w.get::<Mesh>(proto).unwrap().clone();
    *mesh.primitive_mut(1).expect("a listed primitive").material_mut() = Some(blend);
    w.insert(proto, mesh);
    // a layer with no pbr.material is refused, the world unchanged
    let orphan = w.spawn();
    let mut l = w.get::<LayeredMaterial>(blend).unwrap().clone();
    l.layers[1] = Some(Layer {
        material: orphan,
        texcoord: None,
        transform: None,
    });
    w.insert(blend, l.clone());
    let e = passes::raise_skate(&mut w, &options()).unwrap_err().to_string();
    assert!(e.contains("has no pbr.material"), "{e}");
    assert!(w.iter::<SkateMaterial>().next().is_none());
    l.layers[1] = None;
    w.insert(blend, l);
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["layered materials"], 1);
    // rock (drawn), dirt and grass (layers); plain is no longer drawn
    assert_eq!(log.counts["materials"], 3);
    let m = w.get::<SkateMaterial>(blend).unwrap();
    assert_eq!((m.key.as_str(), m.class), ("terrain", MaterialClass::Layered));
    assert!(w.has::<LayeredMaterial>(blend));
    for e in layers {
        assert_eq!(w.get::<SkateMaterial>(e).unwrap().class, MaterialClass::Opaque);
    }
    assert!(!w.has::<SkateMaterial>(mats[1]), "plain is drawn by nothing now");
}

/// The world's markers become skate markers on the same entities: a ground marker (the spawn place-spawn puts at
/// selection) snaps to the ground and keeps its yaw; one with a transform keeps its position and height and faces its
/// local +Z; a kind skate has no marker for is left out, counted.
#[test]
fn markers_are_raised_from_core_markers() {
    use primase_core::Marker;
    let mut w = world();
    w.remove_resource::<Surfaces>();
    let marker = |w: &mut World, kind: &str, ground: Option<[f32; 2]>, yaw: Option<f32>| {
        let e = w.spawn();
        w.insert(
            e,
            Marker {
                kind: kind.into(),
                ground,
                yaw,
            },
        );
        e
    };
    let spawn = marker(&mut w, "spawn", Some([6.0, 2.0]), Some(90.0));
    let travel = marker(&mut w, "travel", None, None);
    // local +Z onto world +X: facing 90 degrees
    let mut t = Transform::IDENTITY;
    t.cols[0] = [0.0, 0.0, -1.0];
    t.cols[2] = [1.0, 0.0, 0.0];
    t.cols[3] = [4.0, 1.5, -3.0];
    w.insert(travel, t);
    let waypoint = marker(&mut w, "waypoint", Some([0.0, 0.0]), None);
    let log = passes::raise_skate(
        &mut w,
        &RaiseOptions::new("m", primase_reskate::passes::LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    let s = w.get::<components::SkateMarker>(spawn).unwrap();
    assert_eq!(
        (s.kind, s.position, s.yaw, s.snap.clone()),
        (
            components::MarkerKind::Spawn,
            Some(([6.0, -2.0], None)),
            Some(90.0),
            Some(components::Snap::Mode(components::SnapMode::Ground))
        )
    );
    let t = w.get::<components::SkateMarker>(travel).unwrap();
    assert_eq!(
        (t.kind, t.position, t.snap.clone()),
        (
            components::MarkerKind::Travel,
            Some(([4.0, -3.0], Some(1.5))),
            None
        )
    );
    assert!((t.yaw.unwrap() - 90.0).abs() < 1e-9, "{:?}", t.yaw);
    assert!(!w.has::<components::SkateMarker>(waypoint));
    assert_eq!(log.counts["markers"], 2);
    assert_eq!(
        log.counts["markers of kind 'waypoint' not raised (no skate marker kind)"],
        1
    );
}

/// A surface with no class and no map entry takes `unclassified` (counted by name); a prototype's own
/// `core.collision` becomes its triangle-mesh soup (counted).
#[test]
fn unclassified_surfaces_and_prototype_collision() {
    let mut w = world();
    let (proto, _) = w.iter::<Mesh>().next().map(|(e, m)| (e, m.clone())).unwrap();
    w.insert(
        proto,
        primase_core::Collision {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            surfaces: vec![0],
        },
    );
    let log = passes::raise_skate(
        &mut w,
        &RaiseOptions::new("x", primase_reskate::passes::LightCalibration::Standard)
            .unwrap()
            .with_unclassified("concrete")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(log.counts["unclassified surfaces made concrete: Snow"], 1);
    assert_eq!(w.resource::<Surfaces>().unwrap().0[0].name, "concrete");
    assert_eq!(log.counts["prototypes with their own collision"], 1);
    let p = w.get::<components::SkatePrototype>(proto).unwrap();
    let soup = p.collision.as_ref().unwrap();
    assert_eq!(soup.mode, Some(components::CollisionMode::TriangleMesh));
    // an unclassified surface that is no skate surface is refused
    let e = passes::raise_skate(
        &mut world(),
        &RaiseOptions::new("x", primase_reskate::passes::LightCalibration::Standard)
            .unwrap()
            .with_unclassified("lava")
            .unwrap(),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("unclassified=lava"), "{e}");
}

/// A surface with no class, no map entry and no `unclassified` surface is never guessed: its triangles are left out
/// and reported by name (an omission, not a count), the rest of a collision kept with its rock flags, and a collision
/// left with no triangle removed; the surface map or `unclassified` restores the triangles.
#[test]
fn unmapped_surfaces_are_left_out_and_reported() {
    let mut w = world();
    let (proto, _) = w.iter::<Mesh>().next().map(|(e, m)| (e, m.clone())).unwrap();
    let concrete = w.resource_mut::<Surfaces>().unwrap().index("concrete").unwrap();
    // the prototype's own collision: a Snow triangle (natural rock) and a concrete one (not)
    w.insert(
        proto,
        Collision {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0]],
            triangles: vec![[0, 1, 2], [1, 3, 2]],
            surfaces: vec![0, concrete],
        },
    );
    w.insert(proto, components::SkateRock(vec![true, false]));
    let ground = w
        .iter::<Collision>()
        .find(|&(e, _)| e != proto)
        .map(|(e, _)| e)
        .unwrap();
    let unmapped = options().with_surfaces(BTreeMap::new()).unwrap();
    let log = passes::raise_skate(&mut w, &unmapped).unwrap();
    let reason = "collision triangles on a surface with no skate surface, left out: Snow";
    assert_eq!(log.omitted.get(reason), Some(&2));
    assert!(!log.counts.contains_key(reason));
    assert_eq!(log.counts["collisions with no triangle left, removed"], 1);
    // the world's soup was all Snow: gone; the prototype keeps its concrete triangle and that triangle's rock flag
    assert!(!w.has::<Collision>(ground));
    let c = w.get::<Collision>(proto).unwrap();
    assert_eq!(
        (c.triangles.clone(), c.surfaces.clone()),
        (vec![[1, 3, 2]], vec![concrete])
    );
    assert_eq!(w.get::<components::SkateRock>(proto).unwrap().0, vec![false]);
    assert!(w.get::<SkatePrototype>(proto).unwrap().collision.is_some());
    // mapped, nothing is left out
    let mut w = world();
    let log = passes::raise_skate(
        &mut w,
        &options()
            .with_surfaces(BTreeMap::from([("Snow".to_owned(), "earth".to_owned())]))
            .unwrap(),
    )
    .unwrap();
    assert!(log.omitted.is_empty());
    assert!(w.has::<Collision>(ground));
}

/// A placed prototype with collision and no mesh collides without drawing: a prototype with no GLB and no parts, its
/// soup kept through the scene file (counted); one with neither is left out.
#[test]
fn a_prototype_that_collides_without_drawing() {
    let mut w = world();
    let place = |w: &mut World, proto| {
        let at = w.spawn();
        w.insert(
            at,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(at, Transform::IDENTITY);
        at
    };
    let fence = w.spawn();
    w.insert(fence, Name("fence".into()));
    let mut c = Collision::default();
    c.extend(
        &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
        &[[0, 1, 2]],
        0,
        |p| p,
    );
    w.insert(fence, c);
    place(&mut w, fence);
    let nothing = w.spawn();
    w.insert(nothing, Name("nothing".into()));
    let nothing_at = place(&mut w, nothing);
    let log = passes::raise_skate(&mut w, &options()).unwrap();
    assert_eq!(log.counts["prototypes"], 2);
    assert_eq!(log.counts["prototypes that collide without drawing"], 1);
    assert_eq!(log.counts["prototypes with their own collision"], 1);
    assert!(!w.has::<SkatePrototype>(nothing));
    let p = w.get::<SkatePrototype>(fence).unwrap();
    assert_eq!(
        (p.glb.is_none(), p.collision.as_ref().unwrap().mode),
        (true, Some(components::CollisionMode::TriangleMesh))
    );
    assert!(w.get::<components::SkateParts>(fence).unwrap().0.is_empty());
    // through the scene file (which takes placements of skate prototypes only): no GLB, the soup kept on its
    // asphalt surface
    w.despawn(nothing_at);
    let out = tmp("collision_only_scene");
    write::interchange::write(&w, &out).unwrap();
    let mut back = World::new();
    read::interchange::read(&out, &mut back).unwrap();
    let (e, p) = back
        .iter::<SkatePrototype>()
        .find(|(_, p)| p.id == "fence")
        .unwrap();
    assert!(p.glb.is_none() && p.collision.is_some());
    let c = back.get::<Collision>(e).unwrap();
    assert_eq!(c.triangles.len(), 1);
    assert_eq!(
        back.resource::<Surfaces>().unwrap().0[c.surfaces[0] as usize].name,
        "asphalt"
    );
}

/// The pass's options: a surface map and the unclassified surface round-trip; what does not make sense is refused,
/// and so is a spawn (placed at selection by place-spawn, not per part by raise-skate).
#[test]
fn raise_skate_pass_options() {
    use primase_core::pass::Options;
    let o = Options::from([
        ("name".to_owned(), "m".to_owned()),
        ("light_scale".to_owned(), "standard".to_owned()),
        ("map".to_owned(), "PUDDLE:default;SNOW:asphalt".to_owned()),
        ("unclassified".to_owned(), "default".to_owned()),
    ]);
    let pass = passes::raise_skate_pass(&o).unwrap();
    assert_eq!(
        passes::raise_skate_pass(&pass.options()).unwrap().options(),
        pass.options()
    );
    // the name has no default
    let Err(e) = passes::raise_skate_pass(&Options::new()) else {
        panic!("raise-skate without a name");
    };
    assert!(e.contains("name=N is required"), "{e}");
    let spawn: Options = [
        ("name".to_owned(), "m".to_owned()),
        ("light_scale".to_owned(), "standard".to_owned()),
        ("spawn".to_owned(), "centre".to_owned()),
    ]
    .into();
    let Err(e) = passes::raise_skate_pass(&spawn) else {
        panic!("spawn is not a raise-skate option");
    };
    assert!(e.contains("place-spawn"), "{e}");
    for bad in [
        vec![("spawn_x", "1"), ("spawn_z", "1")],
        vec![("spawn_yaw", "180")],
        vec![("name", "a b")],
        vec![("map", "PUDDLE")],
        vec![("colour", "red")],
    ] {
        // each beside a name and a calibration, so it is what is refused
        let o: Options = [("name", "m"), ("light_scale", "standard")]
            .iter()
            .chain(&bad)
            .map(|&(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        assert!(passes::raise_skate_pass(&o).is_err(), "{bad:?}");
    }
}

/// Skate reflects every dielectric at F0 0.04: a source reflecting less is folded into roughness so its highlight
/// peaks no higher (alpha' = alpha sqrt(0.04 / F0)); 0.04 or more keeps its roughness; nothing reflected is rough.
#[test]
fn reflectance_folds_into_roughness_by_the_highlight_peak() {
    assert_eq!(passes::raise::fold_reflectance(0.5, 0.04), 0.5);
    assert_eq!(passes::raise::fold_reflectance(0.5, 0.2), 0.5);
    assert_eq!(passes::raise::fold_reflectance(0.5, 0.0), 1.0);
    // F0 0.01: alpha 0.25 x 2 = 0.5, roughness sqrt(0.5)
    assert!((passes::raise::fold_reflectance(0.5, 0.01) - 0.5f64.sqrt()).abs() < 1e-12);
    assert_eq!(passes::raise::fold_reflectance(0.1, 1e-9), 1.0);
}

/// A scalar material: specular factor 0.25 (F0 0.01) at roughness 0.5 is raised at roughness sqrt(0.5); the same with
/// metal in it keeps its roughness (its reflectance is its base colour's).
#[test]
fn scalar_materials_fold_their_specular_reflectance() {
    for (metallic, want) in [(0.0, std::f64::consts::FRAC_1_SQRT_2), (1.0, 0.5)] {
        let mut w = world();
        let plain = w
            .iter::<PbrMaterial>()
            .find(|(_, m)| m.base_color_texture.is_none())
            .unwrap()
            .0;
        let m = w.get_mut::<PbrMaterial>(plain).unwrap();
        m.metallic = metallic;
        m.roughness = 0.5;
        m.specular = Some(primase_core::pbr::Specular {
            factor: 0.25,
            ..primase_core::pbr::Specular::default()
        });
        let log = passes::raise_skate(&mut w, &options()).unwrap();
        let r = w.get::<SkateMaterial>(plain).unwrap().roughness.unwrap();
        assert!((r - want).abs() < 1e-4, "roughness {r}");
        assert_eq!(
            log.approximations.contains_key(
                "specular reflectance below skate's 0.04 folded into roughness (GGX highlight peak kept)"
            ),
            metallic == 0.0
        );
    }
}

/// The mask folds per texel when the strength texture is its source (lower-gta-pbr's derived texture: A strength,
/// G roughness): colour 0.5 (F0 0.02 x A). Literal: texel G 128, A 17 -> roughness 0.2510, F0 0.001333 -> 0.5874,
/// green 105; texel G 31, A 99 -> 0.0608, F0 0.007765 -> 0.0916, green 232. With another strength texture the factor
/// alone folds: F0 0.02 -> green 179.
#[test]
fn masks_fold_reflectance_per_texel() {
    for (same, level0) in [(true, [0, 105, 64, 255]), (false, [0, 179, 64, 255])] {
        let (mut w, material, image) = masked_world();
        let other = w.spawn();
        w.insert(other, texture(TexRole::Mask));
        let m = w.get_mut::<PbrMaterial>(material).unwrap();
        m.metallic = 0.0;
        m.roughness = 0.5;
        m.specular = Some(primase_core::pbr::Specular {
            factor: 1.0,
            texture: Some(TextureRef::new(if same { image } else { other })),
            color: [0.5; 3],
            color_texture: None,
        });
        let log = passes::raise_skate(&mut w, &options()).unwrap();
        let mask = w.get::<SkateMaterial>(material).unwrap().mask.unwrap();
        let levels = &w.get::<Texture>(mask).unwrap().levels;
        assert_eq!(levels[0], level0.repeat(4));
        if same {
            assert_eq!(levels[1], vec![0, 232, 200, 255]);
        }
        assert_eq!(
            log.approximations.contains_key(
                "specular strength textures other than the mask source not folded (their factor only)"
            ),
            !same
        );
    }
}

/// A material without specular keeps the unfolded mask bytes: roughness 0.3 gives greens 255 - 0.3 x 128 = 216.6 -> 217
/// and 255 - 0.3 x 31 = 245.7 -> 246, as before the fold existed.
#[test]
fn masks_without_specular_keep_their_bytes() {
    let (mut w, material, _) = masked_world();
    let m = w.get_mut::<PbrMaterial>(material).unwrap();
    m.metallic = 0.0;
    m.roughness = 0.3;
    m.specular = None;
    passes::raise_skate(&mut w, &options()).unwrap();
    let mask = w.get::<SkateMaterial>(material).unwrap().mask.unwrap();
    let levels = &w.get::<Texture>(mask).unwrap().levels;
    assert_eq!(levels[0], [0, 217, 64, 255].repeat(4));
    assert_eq!(levels[1], vec![0, 246, 200, 255]);
}
