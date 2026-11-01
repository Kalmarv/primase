//! The ground families' rules on hand-built materials: `environment_plot` (the `environment` rule over a plot's
//! parameters, with its run-time terms named), the terrain families' planar mapping, and the pass that writes the
//! planar coordinates per placement.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

use primase_core::layered::{Channel, Composition, Source};
use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{PbrMaterial, UvTransform};
use primase_core::{
    ColorSpace, Component, Entity, Instance, LayeredMaterial, Mesh, Name, NormalGreen, NormalZ, Primitive,
    Semantic, Stream, TexFormat, TexRole, Texture, Topology, Transform, World,
};
use primase_forzatech::components::{
    ForzaMaterial, InstanceLevel, MaterialLevel, MaterialTexture, MaterialTextures, Parameter, PlanarUv,
    ShaderLevel, Value,
};
use primase_forzatech::passes::{LowerForzaMaterials, LowerForzaPlanarUv};
use std::collections::BTreeMap;

fn texture(world: &mut World, name: &str, space: ColorSpace, texel: [u8; 4]) -> Entity {
    let e = world.spawn();
    world.insert(e, Name(name.to_owned()));
    world.insert(
        e,
        Texture {
            width: 1,
            height: 1,
            format: TexFormat::Rgba8Unorm,
            levels: vec![texel.to_vec()],
            color_space: space,
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

fn material(family: &str, technique: &str, parameters: Vec<Parameter>) -> ForzaMaterial {
    ForzaMaterial {
        instance: InstanceLevel {
            name: "instance".to_owned(),
            atst: [0, 0],
            xtra: Vec::new(),
            parent: String::new(),
            parameters,
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
            technique: technique.to_owned(),
            third: String::new(),
            user_data: Vec::new(),
            parameters: Vec::new(),
            trailer: None,
        }),
        shader: Some(ShaderLevel {
            entry: String::new(),
            source: String::new(),
            family: family.to_owned(),
            declared: Vec::new(),
            constants: Vec::new(),
            textures: Vec::new(),
            samplers: Vec::new(),
            constants_size: 0,
            defaults: Vec::new(),
            trailer: None,
        }),
    }
}

fn bound(path_hash: u32, texture: Option<Entity>) -> MaterialTexture {
    MaterialTexture { path_hash, texture }
}

fn tex(name: &str, hash: u32) -> Parameter {
    parameter(
        name,
        hash,
        Value::Texture {
            path: String::new(),
            path_hash: hash,
        },
    )
}

#[test]
fn environment_plot_is_the_environment_rule_with_its_own_run_time_terms_named() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [200, 100, 50, 255]);
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 255, 255]);
    // rough 0.2, metal 1, occlusion 0.5, porosity 0
    let rma = texture(&mut world, "rma", ColorSpace::Linear, [51, 255, 128, 0]);
    let macro_ao = texture(&mut world, "macro", ColorSpace::Linear, [255, 255, 255, 255]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "environment_plot",
            "PlotWithRCSM",
            vec![
                tex("DiffAlphaTex_A", 1),
                tex("NormHeightTex_A", 2),
                tex("RoughMetAoPorTex_A", 3),
                tex("AoWetBlendContextTex", 4),
                parameter("BaseColorAlpha_A_UVTiling", 10, Value::Vector2([2.0, 3.0])),
                parameter("NormalHeight_A_UVTiling", 11, Value::Vector2([4.0, 4.0])),
                parameter("RoughMetAoPoros_A_UVTiling", 12, Value::Vector2([1.0, 1.0])),
                parameter("Uv0Offset", 13, Value::Vector2([0.5, 0.25])),
                parameter("Modulate", 14, Value::Int(1)),
                parameter("TerrainWetness", 15, Value::Int(1)),
                parameter("Winter", 16, Value::Int(0)),
            ],
        ),
    );
    world.insert(
        e,
        MaterialTextures(vec![
            bound(1, Some(diffuse)),
            bound(2, Some(normal)),
            bound(3, Some(rma)),
            bound(4, Some(macro_ao)),
        ]),
    );
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    let base = pbr.base_color_texture.unwrap();
    assert_eq!(base.texture, diffuse);
    assert_eq!(
        base.transform,
        Some(UvTransform {
            offset: [1.0, 0.75],
            scale: [2.0, 3.0],
            rotation: 0.0
        })
    );
    assert_eq!(pbr.normal_texture.unwrap().transform.unwrap().scale, [4.0, 4.0]);
    assert_eq!(pbr.occlusion_texture, pbr.metallic_roughness_texture);
    let packed = world
        .get::<Texture>(pbr.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!(packed.levels[0], vec![128, 51, 255, 255]);
    let convention = world.get::<Texture>(normal).unwrap().normal.unwrap();
    assert_eq!(
        (convention.green, convention.z),
        (NormalGreen::Up, NormalZ::Rebuild)
    );

    // the plot's own terms, and only those that the material switches on
    assert_eq!(log.approximations["modulate texture left out"], 1);
    assert_eq!(
        log.approximations["terrain snow and wetness (run-time) left out: dry, snow-free"],
        1
    );
    assert_eq!(
        log.approximations["macro occlusion (second texture coordinate set) left out"],
        1
    );
    assert!(!log.approximations.contains_key("winter state left out"));
    assert!(!log.approximations.contains_key("swatch tint left out"));
    assert_eq!(log.counts["environment_plot materials lowered"], 1);
}

#[test]
fn terrain_is_mapped_by_position_and_its_world_normal_becomes_a_planar_tangent_one() {
    let mut world = World::new();
    // colour: x = 2 * 191/255 - 1 = 0.498, z = 2 * 64/255 - 1 = -0.498, occlusion 128/255
    let colour = texture(&mut world, "colour", ColorSpace::Linear, [191, 64, 0, 128]);
    let albedo = texture(&mut world, "albedo", ColorSpace::Srgb, [200, 100, 50, 255]);
    let gloss = texture(&mut world, "gloss", ColorSpace::Linear, [186, 0, 0, 255]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "terr_mega",
            "Default",
            vec![
                tex("TerrainColourTexture", 1),
                tex("TerrainNormalTexture", 2),
                tex("TerrainGlossTexture", 3),
                parameter("BlendMapScale", 10, Value::Vector([0.001, -0.002, 0.0, 0.0])),
                parameter("BlendMapOffset", 11, Value::Vector([0.25, 0.75, 0.0, 0.0])),
                parameter("Snow", 12, Value::Int(0)),
                parameter("Noise", 13, Value::Int(1)),
            ],
        ),
    );
    world.insert(
        e,
        MaterialTextures(vec![
            bound(1, Some(colour)),
            bound(2, Some(albedo)),
            bound(3, Some(gloss)),
        ]),
    );
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(world.get::<PlanarUv>(e).unwrap().texcoord, 3);
    let base = pbr.base_color_texture.unwrap();
    assert_eq!(base.texture, albedo);
    assert_eq!(base.texcoord, 3);
    assert_eq!(
        base.transform,
        Some(UvTransform {
            offset: [0.25, 0.75],
            scale: [0.001, -0.002],
            rotation: 0.0
        })
    );
    // the occlusion is the colour map's alpha squared: (128/255)^2 = 0.252
    let occlusion = world
        .get::<Texture>(pbr.occlusion_texture.unwrap().texture)
        .unwrap();
    assert_eq!(occlusion.levels[0], vec![64, 64, 64, 255]);
    // the roughness is the gloss map's red over 0.93, no metal: 186/255 / 0.93 = 0.784
    let mr = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(mr.texcoord, 3);
    assert_eq!(
        world.get::<Texture>(mr.texture).unwrap().levels[0],
        vec![255, 200, 0, 255]
    );
    assert_eq!((pbr.metallic, pbr.roughness), (0.0, 1.0));
    // the normal: red x, green -z (encoded 1 - g), blue the rebuilt y
    let n = pbr.normal_texture.unwrap();
    assert_eq!(n.texcoord, 3);
    let normal = world.get::<Texture>(n.texture).unwrap();
    assert_eq!(normal.role, TexRole::Normal);
    assert_eq!(normal.levels[0][..2], [191, 191]);
    assert_eq!(normal.levels[0][2], 218);
    let convention = normal.normal.unwrap();
    assert_eq!(
        (convention.green, convention.z),
        (NormalGreen::Up, NormalZ::Rebuild)
    );

    assert_eq!(log.approximations["noise blend left out"], 1);
    assert!(!log.approximations.contains_key("snow left out"));
    assert_eq!(log.counts["terr_mega materials lowered"], 1);
}

#[test]
fn planar_coordinates_are_each_placements_own() {
    let mut world = World::new();
    let m = world.spawn();
    world.insert(
        m,
        material(
            "terr_distant",
            "Default",
            vec![
                parameter("BlendMapScale", 10, Value::Vector([1.0, 1.0, 0.0, 0.0])),
                parameter("BlendMapOffset", 11, Value::Vector([0.0; 4])),
            ],
        ),
    );
    let other = world.spawn();
    let prototype = world.spawn();
    world.insert(
        prototype,
        Mesh::new(vec![
            Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [10.0, 0.0, 0.0], [0.0, 5.0, -20.0]]),
                )]),
                indices: None,
                material: Some(m),
            },
            Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(
                    Semantic::Position,
                    Stream::F32x3(vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
                )]),
                indices: None,
                material: Some(other),
            },
        ]),
    );
    let at = |w: &mut World, t: [f32; 3]| {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(
            p,
            Transform {
                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], t],
            },
        );
        p
    };
    let a = at(&mut world, [100.0, 0.0, -200.0]);
    let b = at(&mut world, [-3000.0, 50.0, 7000.0]);
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
    LowerForzaPlanarUv.run(&mut world, &mut log).unwrap();

    let pa = world.get::<Instance>(a).unwrap().prototype;
    let pb = world.get::<Instance>(b).unwrap().prototype;
    assert_eq!(pa, prototype);
    assert_ne!(pb, prototype, "the second placement's own copy");
    assert!(!world.has::<PlanarUv>(m));
    for (proto, t) in [(pa, [100.0, -200.0]), (pb, [-3000.0, 7000.0])] {
        let mesh = world.get::<Mesh>(proto).unwrap();
        let Some(Stream::F32x2(uv)) = mesh.primitives()[0].streams.get(&Semantic::TexCoord(3)).cloned()
        else {
            panic!("coordinates")
        };
        // the game's x, and z = -(the world's z)
        assert_eq!(uv[0], [t[0], -t[1]]);
        assert_eq!(uv[1], [t[0] + 10.0, -t[1]]);
        assert_eq!(uv[2], [t[0], -t[1] + 20.0]);
        // the primitive of another material keeps its own streams
        assert!(!mesh.primitives()[1].streams.contains_key(&Semantic::TexCoord(3)));
    }
    assert_eq!(log.counts["planar-mapped placements"], 2);
    assert_eq!(
        log.counts["planar-mapped prototypes copied for a placement of their own"],
        1
    );
}

fn road(parameters: Vec<Parameter>) -> ForzaMaterial {
    let mut all = vec![
        tex("UberRoadADiffuseTexture", 1),
        tex("UberRoadANormalTexture", 2),
        tex("UberRoadBDiffuseTexture", 3),
        tex("UberRoadBNormalTexture", 4),
        tex("BlendTexture", 5),
        tex("TintTexture", 6),
        parameter("UberRoadAUVSet", 20, Value::Int(-1)),
        parameter("UberRoadAWorldUVScale", 21, Value::Vector2([0.25, 0.5])),
        parameter("UberRoadAUVTiling", 22, Value::Vector2([9.0, 9.0])),
        parameter("UberRoadBUVSet", 23, Value::Int(0)),
        parameter("UberRoadBUVTiling", 24, Value::Vector2([3.0, 6.0])),
        parameter("BlendSmoothness", 25, Value::Float(0.25)),
        parameter("BlendShuffle", 26, Value::Swizzle([2.0, 0.0, 1.0, 3.0])),
        parameter("Winter", 27, Value::Int(1)),
    ];
    all.extend(parameters);
    material("uber_road", "Road", all)
}

fn road_textures(world: &mut World) -> MaterialTextures {
    // a diffuse map holds the roughness in alpha, a normal map the occlusion in blue and a height in alpha
    let da = texture(world, "a diffuse", ColorSpace::Srgb, [10, 20, 30, 51]);
    let na = texture(world, "a normal", ColorSpace::Linear, [128, 128, 64, 255]);
    let db = texture(world, "b diffuse", ColorSpace::Srgb, [40, 50, 60, 102]);
    let nb = texture(world, "b normal", ColorSpace::Linear, [128, 128, 192, 128]);
    let blend = texture(world, "blend", ColorSpace::Linear, [0, 0, 0, 0]);
    let tint = texture(world, "tint", ColorSpace::Linear, [128, 128, 128, 255]);
    MaterialTextures(vec![
        bound(1, Some(da)),
        bound(2, Some(na)),
        bound(3, Some(db)),
        bound(4, Some(nb)),
        bound(5, Some(blend)),
        bound(6, Some(tint)),
    ])
}

#[test]
fn a_two_layer_road_is_a_layered_material_over_its_layers_coordinates() {
    let mut world = World::new();
    let textures = road_textures(&mut world);
    let e = world.spawn();
    world.insert(e, road(vec![parameter("Blend2", 30, Value::Int(1))]));
    world.insert(e, textures.clone());
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

    let layered = world.get::<LayeredMaterial>(e).unwrap();
    layered.verify(e, &world).unwrap();
    assert_eq!(layered.layers.len(), 2);
    assert_eq!(layered.weights.composition, Composition::Over);
    // A is mapped by position (TexCoord(3), the world scale), B by TexCoord(0) at its tiling
    let a = layered.layers[0].as_ref().unwrap();
    let b = layered.layers[1].as_ref().unwrap();
    assert_eq!(a.texcoord, Some(3));
    assert_eq!(a.transform.unwrap().scale, [0.25, 0.5]);
    assert_eq!(b.texcoord, Some(0));
    assert_eq!(b.transform.unwrap().scale, [3.0, 6.0]);
    assert_eq!(world.get::<PlanarUv>(e).unwrap().texcoord, 3);

    // the layers' maps: roughness from the diffuse alpha, occlusion from the normal map's blue, no metal
    let layer_b = world.get::<PbrMaterial>(b.material).unwrap();
    let rough = world
        .get::<Texture>(layer_b.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!(rough.levels[0], vec![255, 102, 0, 255]);
    let occlusion = world
        .get::<Texture>(layer_b.occlusion_texture.unwrap().texture)
        .unwrap();
    assert_eq!(occlusion.levels[0], vec![192, 192, 192, 255]);
    assert_eq!(
        world
            .get::<Texture>(layer_b.normal_texture.unwrap().texture)
            .unwrap()
            .role,
        TexRole::Normal
    );

    // B's weight: saturate((w (1 + s) + h - 1) / s), w the blend map's channel BlendShuffle[0] = 2 (blue), h B's normal
    // alpha at B's coordinates
    let channels = layered.weights.channels.as_ref().unwrap();
    let Channel::Source(Source::Mask { bias, .. }) = &channels[0] else {
        panic!("base weight")
    };
    assert_eq!(*bias, 1.0);
    let Channel::Threshold { v, m, k } = &channels[1] else {
        panic!("threshold")
    };
    assert_eq!(*k, [-4.0, 5.0, 4.0, 0.0]);
    let Source::Mask {
        texture,
        channels: pick,
        ..
    } = v
    else {
        panic!("blend mask")
    };
    assert_eq!((texture.texcoord, *pick), (2, [0.0, 0.0, 1.0, 0.0]));
    let Source::Mask {
        texture,
        channels: pick,
        ..
    } = m
    else {
        panic!("height mask")
    };
    assert_eq!((texture.texcoord, *pick), (0, [1.0, 0.0, 0.0, 0.0]));
    assert_eq!(texture.transform.unwrap().scale, [3.0, 6.0]);
    assert_eq!(
        world.get::<Texture>(texture.texture).unwrap().levels[0],
        vec![128, 128, 128, 255]
    );

    assert_eq!(log.approximations["tint overlay left out"], 1);
    assert_eq!(
        log.approximations["layer blend's smoothstep taken as a linear ramp"],
        1
    );
    assert_eq!(log.approximations["frost and snow (run-time) left out: dry"], 1);
    assert_eq!(log.counts["uber_road materials lowered"], 1);
}

#[test]
fn a_one_layer_road_is_a_plain_material_with_its_layers_coordinates_on_its_slots() {
    let mut world = World::new();
    let textures = road_textures(&mut world);
    let e = world.spawn();
    world.insert(e, road(Vec::new()));
    world.insert(e, textures);
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
    assert!(!world.has::<LayeredMaterial>(e));
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    for slot in [
        pbr.base_color_texture,
        pbr.metallic_roughness_texture,
        pbr.occlusion_texture,
        pbr.normal_texture,
    ] {
        let slot = slot.unwrap();
        assert_eq!(slot.texcoord, 3);
        assert_eq!(slot.transform.unwrap().scale, [0.25, 0.5]);
    }
    assert!(world.has::<PlanarUv>(e));
}
