//! `bake-forzatech-emission-product` on hand-built materials: the emission map times the base colour map once both are
//! drawn through one set, the emission folded into a factor, a material with no base colour map, and one whose maps are
//! still on different sets.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::{
    ColorSpace, Entity, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole, Texture, Topology, World,
};
use primase_forzatech::components::EmissionByBaseColour;
use primase_forzatech::passes::BakeForzaEmissionProduct;
use primase_foundations::color::{linear_to_srgb8, srgb8_to_linear};
use std::collections::BTreeMap;

fn texture(world: &mut World, name: &str, size: u32, texel: [u8; 4]) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(name.to_owned()));
    world.insert(
        e,
        Texture {
            width: size,
            height: size,
            format: TexFormat::Rgba8Unorm,
            levels: vec![texel.repeat((size * size) as usize)],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    e
}

/// A marked material with the given slots, drawn by a one-triangle mesh.
fn marked(world: &mut World, base: Option<TextureRef>, emission: Option<TextureRef>) -> Entity {
    let m = world.spawn();
    world.insert(
        m,
        PbrMaterial {
            base_color_texture: base,
            emissive_texture: emission,
            emissive: [1.0, 0.5, 0.25],
            emissive_strength: 3.0,
            ..PbrMaterial::default()
        },
    );
    world.insert(m, EmissionByBaseColour);
    let mesh = world.spawn();
    world.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]),
            indices: None,
            material: Some(m),
        }]),
    );
    m
}

fn run(world: &mut World) -> PassLog {
    let mut log = PassLog::default();
    BakeForzaEmissionProduct.run(world, &mut log).unwrap();
    log
}

#[test]
fn the_emission_map_times_the_base_colour_map_is_one_emission_map() {
    let mut world = World::new();
    let base = texture(&mut world, "base", 4, [200, 100, 50, 255]);
    let glow = texture(&mut world, "glow", 2, [255, 128, 0, 255]);
    let m = marked(
        &mut world,
        Some(TextureRef::new(base)),
        Some(TextureRef::new(glow)),
    );
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(m).unwrap();
    let slot = pbr.emissive_texture.unwrap();
    assert_ne!(slot.texture, glow);
    assert_eq!((slot.texcoord, slot.transform), (0, None));
    let made = world.get::<Texture>(slot.texture).unwrap();
    assert_eq!(
        (made.width, made.height, made.color_space),
        (4, 4, ColorSpace::Srgb)
    );
    // channel by channel in linear light
    let want: Vec<u8> = [(200u8, 255u8), (100, 128), (50, 0)]
        .iter()
        .map(|&(b, e)| linear_to_srgb8(srgb8_to_linear(b) * srgb8_to_linear(e)))
        .collect();
    for texel in made.levels[0].chunks(4) {
        for (got, want) in texel[..3].iter().zip(&want) {
            assert!(got.abs_diff(*want) <= 1, "{texel:?} against {want:?}");
        }
        assert_eq!(texel[3], 255);
    }
    // the factor is the lowering's
    assert_eq!((pbr.emissive, pbr.emissive_strength), ([1.0, 0.5, 0.25], 3.0));
    assert!(!world.has::<EmissionByBaseColour>(m));
    assert_eq!(
        log.counts["EmissiveAltUV emission maps multiplied by the base colour map"],
        1
    );
    assert_eq!(
        log.counts["emission maps made as the emission map times the base colour map"],
        1
    );
}

#[test]
fn a_floating_point_emission_map_is_divided_by_its_largest_value_and_the_strength_takes_it_back() {
    let mut world = World::new();
    let base = texture(&mut world, "base", 2, [255, 255, 255, 255]);
    // 2 x 2 texels of radiance: 4, 2, 1 and 0 in red
    let glow = world.spawn();
    let texels: [[f32; 4]; 4] = [
        [4.0, 1.0, 0.0, 1.0],
        [2.0, 1.0, 0.0, 1.0],
        [1.0, 1.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
    ];
    world.insert(
        glow,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba32Float,
            levels: vec![texels.iter().flatten().flat_map(|v| v.to_le_bytes()).collect()],
            color_space: ColorSpace::Linear,
            role: TexRole::Color,
            normal: None,
        },
    );
    let m = marked(
        &mut world,
        Some(TextureRef::new(base)),
        Some(TextureRef::new(glow)),
    );
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(m).unwrap();
    // the strength was 3: it is 3 x 4 now, the map holds the ratios
    assert_eq!(pbr.emissive_strength, 12.0);
    let made = world
        .get::<Texture>(pbr.emissive_texture.unwrap().texture)
        .unwrap();
    let red: Vec<f64> = made.levels[0].chunks(4).map(|t| srgb8_to_linear(t[0])).collect();
    for (got, want) in red.iter().zip([1.0, 0.5, 0.25, 0.0]) {
        assert!((got - want).abs() < 0.01, "{red:?}");
    }
    assert_eq!(
        log.counts["floating-point emission maps divided by their largest value, which multiplies the emissive strength"],
        1
    );
}

#[test]
fn an_emission_folded_into_the_factor_takes_the_base_colour_map_as_its_pattern() {
    let mut world = World::new();
    let base = texture(&mut world, "base", 4, [200, 100, 50, 255]);
    let base_ref = TextureRef::new(base);
    let m = marked(&mut world, Some(base_ref), None);
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(m).unwrap();
    assert_eq!(pbr.emissive_texture, Some(base_ref));
    assert_eq!(
        log.counts["EmissiveAltUV emission folded into a factor: the base colour map is the emission map"],
        1
    );
    assert!(!world.has::<EmissionByBaseColour>(m));
}

#[test]
fn a_material_with_no_base_colour_map_keeps_its_emission_map() {
    let mut world = World::new();
    let glow = texture(&mut world, "glow", 2, [255, 128, 0, 255]);
    let glow_ref = TextureRef {
        texcoord: 4,
        ..TextureRef::new(glow)
    };
    let m = marked(&mut world, None, Some(glow_ref));
    let log = run(&mut world);
    assert_eq!(
        world.get::<PbrMaterial>(m).unwrap().emissive_texture,
        Some(glow_ref)
    );
    assert_eq!(
        log.counts["EmissiveAltUV emission with no base colour map: the emission map alone"],
        1
    );
}

#[test]
fn maps_still_on_different_sets_lose_their_emission_counted() {
    let mut world = World::new();
    let base = texture(&mut world, "base", 4, [200, 100, 50, 255]);
    let glow = texture(&mut world, "glow", 2, [255, 128, 0, 255]);
    let m = marked(
        &mut world,
        Some(TextureRef::new(base)),
        Some(TextureRef {
            texcoord: 4,
            ..TextureRef::new(glow)
        }),
    );
    let log = run(&mut world);
    let pbr = world.get::<PbrMaterial>(m).unwrap();
    assert_eq!(pbr.emissive_texture, None);
    assert_eq!((pbr.emissive, pbr.emissive_strength), ([0.0; 3], 1.0));
    let why = "emission texture on TEXCOORD4 (EmissiveAltUV) left out: the material's maps could not be drawn through one coordinate set";
    assert_eq!(log.approximations[why], 1);
    assert_eq!(log.approximations[&format!("{why}: triangles")], 1);
    assert!(log.approximated_on[why].contains(&m));
}

#[test]
fn an_unmarked_world_is_left_alone() {
    let mut world = World::new();
    let base = texture(&mut world, "base", 4, [200, 100, 50, 255]);
    let m = world.spawn();
    world.insert(
        m,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(base)),
            ..PbrMaterial::default()
        },
    );
    let before = world.get::<PbrMaterial>(m).unwrap().clone();
    let log = run(&mut world);
    assert!(log.counts.is_empty() && log.approximations.is_empty());
    assert_eq!(world.get::<PbrMaterial>(m).unwrap(), &before);
}
