//! `bake-forzatech-swatch-tint` on hand-built prototypes: the palette is read at (vertex green, placement byte), the
//! triangles the vertex red tints are split from the rest, and placements with one tint share one material.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    ColorSpace, Entity, Instance, Lods, MaterialVariants, Mesh, Name, Primitive, Semantic, Stream, TexFormat,
    TexRole, Texture, Topology, World,
};
use primase_forzatech::components::{
    ForzaMaterial, InstanceLevel, MaterialLevel, MaterialTexture, MaterialTextures, Parameter, Placement,
    PlacementOrigin, ShaderLevel, SwatchTint, Value,
};
use primase_forzatech::passes::{BakeForzaSwatchTint, LowerForzaMaterials};
use primase_foundations::color::{linear_to_srgb8, srgb8_to_linear};
use std::collections::BTreeSet;

/// A 2 x 2 linear palette: columns by green, rows by byte. Red, green / blue, yellow.
fn palette(world: &mut World) -> Entity {
    let e = world.spawn();
    world.insert(e, Name("palette".to_owned()));
    world.insert(
        e,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 255, 255, 0, 255,
            ]],
            color_space: ColorSpace::Linear,
            role: TexRole::Other,
            normal: None,
        },
    );
    e
}

fn parameter(name: &str, hash: u32, value: Value) -> Parameter {
    Parameter {
        hash,
        flag_hash: None,
        name: Some(name.to_owned()),
        value,
    }
}

/// An `environment` material with the swatch tint on and `palette` bound.
fn swatch_material(world: &mut World, palette: Entity) -> Entity {
    let e = world.spawn();
    world.insert(
        e,
        ForzaMaterial {
            instance: InstanceLevel {
                name: "instance".to_owned(),
                atst: [0, 0],
                xtra: Vec::new(),
                parent: String::new(),
                parameters: vec![
                    parameter("SwatchTint", 1, Value::Int(1)),
                    parameter(
                        "SwatchTintTexture",
                        2,
                        Value::Texture {
                            path: String::new(),
                            path_hash: 7,
                        },
                    ),
                ],
                trailer: None,
            },
            material: Some(MaterialLevel {
                entry: String::new(),
                source: String::new(),
                name: "material".to_owned(),
                atst: [0, 0],
                blst: [0; 5],
                xtra: None,
                shader: String::new(),
                technique: String::new(),
                third: String::new(),
                user_data: Vec::new(),
                parameters: Vec::new(),
                trailer: None,
            }),
            shader: Some(ShaderLevel {
                entry: String::new(),
                source: String::new(),
                family: "environment".to_owned(),
                declared: Vec::new(),
                constants: Vec::new(),
                textures: Vec::new(),
                samplers: Vec::new(),
                constants_size: 0,
                defaults: Vec::new(),
                trailer: None,
            }),
        },
    );
    world.insert(
        e,
        MaterialTextures(vec![MaterialTexture {
            path_hash: 7,
            texture: Some(palette),
        }]),
    );
    e
}

/// A prototype of `triangles` (one vertex colour each: red, green) drawn with `material`.
fn prototype(world: &mut World, material: Entity, triangles: &[[f32; 2]]) -> Entity {
    let positions: Vec<[f32; 3]> = (0..triangles.len())
        .flat_map(|t| {
            let x = t as f32;
            [[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]]
        })
        .collect();
    let colours: Vec<[f32; 4]> = triangles
        .iter()
        .flat_map(|c| [[c[0], c[1], 0.0, 1.0]; 3])
        .collect();
    let indices: Vec<u32> = (0..u32::try_from(positions.len()).unwrap()).collect();
    let e = world.spawn();
    world.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: [
                (Semantic::Position, Stream::F32x3(positions)),
                (Semantic::ShaderInput(0), Stream::F32x4(colours)),
            ]
            .into_iter()
            .collect(),
            indices: Some(indices),
            material: Some(material),
        }]),
    );
    e
}

fn place(world: &mut World, prototype: Entity, origin: PlacementOrigin) -> Entity {
    let e = world.spawn();
    world.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    world.insert(
        e,
        Placement {
            origin,
            season_exclusion: 0,
            material_variant: 0,
        },
    );
    e
}

fn rule(byte: u8) -> PlacementOrigin {
    PlacementOrigin::Rule {
        file: String::new(),
        category: String::new(),
        block: String::new(),
        record: 0,
        route: String::new(),
        gameplay_tag: String::new(),
        variation: [byte, 0, 0, 0],
        frame: 0,
    }
}

fn hand(tint: [f32; 4]) -> PlacementOrigin {
    PlacementOrigin::Hand {
        instance: 0,
        flags: 0,
        parent: None,
        band_rows: [None; 6],
        gameplay_tag: None,
        routes: Vec::new(),
        tint,
        lod_group: None,
    }
}

fn base(world: &World, e: Entity) -> [f32; 4] {
    world.get::<PbrMaterial>(e).unwrap().base_color
}

#[test]
fn the_palette_is_read_at_the_vertex_green_and_the_placements_byte() {
    let mut world = World::new();
    let pal = palette(&mut world);
    let material = swatch_material(&mut world, pal);
    // a tinted triangle of column 1, an untinted one, a tinted one of column 0
    let a = prototype(&mut world, material, &[[0.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
    // the same material, one tinted triangle of column 1 alone
    let b = prototype(&mut world, material, &[[0.0, 1.0]]);
    let a0 = place(&mut world, a, rule(0));
    let a1 = place(&mut world, a, rule(255));
    // a tint that is not a whole byte draws untinted; one that is reads the palette like a rule placement
    let a2 = place(&mut world, a, hand([0.3; 4]));
    let a3 = place(&mut world, a, hand([1.0; 4]));
    let a4 = place(&mut world, a, hand([2.0; 4]));
    let b0 = place(&mut world, b, rule(0));

    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
    assert!(world.has::<SwatchTint>(material));
    let mut log = PassLog::default();
    BakeForzaSwatchTint.run(&mut world, &mut log).unwrap();

    // the hand-placed instance draws untinted; the tinted ones have a variant each
    let variant = |p: Entity| world.get::<Instance>(p).unwrap().variant;
    // (model variant, byte) in order, the untinted first
    // untinted, byte 0, row 0.3, row 1.0 (byte 255 and a hand tint of one are the same row)
    assert_eq!(
        [variant(a0), variant(a1), variant(a2), variant(a3), variant(a4)],
        [1, 3, 2, 3, 0]
    );
    assert_eq!(variant(b0), 0);

    // untinted, then the tinted groups by green: three one-triangle primitives
    let mesh = world.get::<Mesh>(a).unwrap();
    assert_eq!(mesh.primitives().len(), 3);
    assert!(
        mesh.primitives()
            .iter()
            .all(|p| p.indices.as_ref().unwrap().len() == 3 && p.material == Some(material))
    );
    let maps = &world.get::<MaterialVariants>(a).unwrap().0;
    assert_eq!(maps.len(), 3);
    assert!(maps[0].is_empty());
    // column 0 (green 0): byte 0 reads red, byte 255 blue; the hand placement (id 0) draws the material itself
    // (row 0.3 reads the palette's first row, as byte 0 does: one material between them)
    assert_eq!(base(&world, maps[1][&1]), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(maps[1][&2], maps[1][&1]);
    assert_eq!(base(&world, maps[1][&3]), [0.0, 0.0, 1.0, 1.0]);
    assert!(!maps[1].contains_key(&0));
    // column 1 (green 255): byte 0 reads green, byte 255 yellow
    assert_eq!(base(&world, maps[2][&1]), [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(base(&world, maps[2][&3]), [1.0, 1.0, 0.0, 1.0]);

    // one primitive of one group stays whole; its green-0 tint is the first prototype's, shared
    let mesh = world.get::<Mesh>(b).unwrap();
    assert_eq!(mesh.primitives().len(), 1);
    let shared = world.get::<MaterialVariants>(b).unwrap().0[0][&0];
    assert_eq!(shared, maps[2][&1]);

    // four tinted copies in all, and no swatch record or vertex colour left
    assert_eq!(
        log.counts["swatch-tinted material copies made, one per material and tint"],
        4
    );
    assert_eq!(world.iter::<SwatchTint>().count(), 0);
    for (_, mesh) in world.iter::<Mesh>() {
        assert!(
            mesh.primitives()
                .iter()
                .all(|p| !p.streams.contains_key(&Semantic::ShaderInput(0)))
        );
    }
    // the untouched material is still white, and the copies are the only other lowered materials
    assert_eq!(base(&world, material), [1.0; 4]);
    let lowered: BTreeSet<Entity> = world.iter::<PbrMaterial>().map(|(e, _)| e).collect();
    assert_eq!(lowered.len(), 5);
    assert_eq!(
        log.omitted["hand-placed instances of swatch-tinted prototypes whose tint is outside 0 to 1: untinted"],
        1
    );
}

#[test]
fn a_base_colour_texture_is_baked_with_the_tint_by_its_alpha() {
    let mut world = World::new();
    let pal = palette(&mut world);
    let material = swatch_material(&mut world, pal);
    // an sRGB base colour of two texels: opaque, and alpha 0 (not tinted)
    let diffuse = world.spawn();
    world.insert(diffuse, Name("diffuse".to_owned()));
    world.insert(
        diffuse,
        Texture {
            width: 2,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![vec![200, 100, 50, 255, 200, 100, 50, 0]],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    world.insert(
        material,
        MaterialTextures(vec![
            MaterialTexture {
                path_hash: 7,
                texture: Some(pal),
            },
            MaterialTexture {
                path_hash: 8,
                texture: Some(diffuse),
            },
        ]),
    );
    let mut m = world.get::<ForzaMaterial>(material).unwrap().clone();
    m.instance.parameters.push(parameter(
        "DiffAlphaTex_A",
        3,
        Value::Texture {
            path: String::new(),
            path_hash: 8,
        },
    ));
    world.insert(material, m);
    // one tinted triangle of column 1 (green 255)
    let a = prototype(&mut world, material, &[[0.0, 1.0]]);
    let a0 = place(&mut world, a, rule(0));
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
    let mut log = PassLog::default();
    BakeForzaSwatchTint.run(&mut world, &mut log).unwrap();

    assert_eq!(world.get::<Instance>(a0).unwrap().variant, 0);
    let tinted = world.get::<MaterialVariants>(a).unwrap().0[0][&0];
    // the palette's column 1, row 0 is green; the factor is (1 - a) + a tint per channel, in linear light
    let baked = world
        .get::<PbrMaterial>(tinted)
        .unwrap()
        .base_color_texture
        .unwrap()
        .texture;
    assert_ne!(baked, diffuse);
    let t = world.get::<Texture>(baked).unwrap();
    assert_eq!(t.color_space, ColorSpace::Srgb);
    let expect = |c: u8, tint: f64| linear_to_srgb8(srgb8_to_linear(c) * tint);
    assert_eq!(
        t.levels[0],
        vec![
            expect(200, 0.0),
            expect(100, 1.0),
            expect(50, 0.0),
            255,
            // alpha 0: untinted
            200,
            100,
            50,
            0,
        ]
    );
    assert_eq!(base(&world, tinted), [1.0; 4]);
    assert_eq!(
        log.counts["swatch-tinted base colour textures baked, one per texture and tint"],
        1
    );
    // the source texture is untouched
    assert_eq!(world.get::<Texture>(diffuse).unwrap().levels[0][0], 200);
}

#[test]
fn a_prototype_with_levels_of_detail_is_left_untinted_and_counted() {
    let mut world = World::new();
    let pal = palette(&mut world);
    let material = swatch_material(&mut world, pal);
    let a = prototype(&mut world, material, &[[0.0, 0.0]]);
    let form = prototype(&mut world, material, &[[0.0, 0.0]]);
    world.insert(
        a,
        Lods {
            detail: 0,
            forms: vec![primase_core::LodForm { detail: 1, form }],
            default: None,
        },
    );
    place(&mut world, a, rule(0));
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
    let mut log = PassLog::default();
    BakeForzaSwatchTint.run(&mut world, &mut log).unwrap();
    assert!(!world.has::<MaterialVariants>(a));
    assert_eq!(world.get::<Mesh>(a).unwrap().primitives().len(), 1);
    assert_eq!(
        log.omitted["swatch-tinted prototypes with levels of detail: untinted (select-lod first)"],
        1
    );
}
