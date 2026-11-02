//! `lower-forzatech-materials` on hand-built materials: what the `environment` rule makes of the three maps, their
//! tilings, an alpha test, a texture no archive holds and a family it has no rule for.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

mod lowering;

use primase_core::pass::PassLog;
use primase_core::pbr::{AlphaMode, PbrMaterial, UvTransform, Wrap};
use primase_core::{
    ColorSpace, DrawLayer, DrawOrder, Entity, Mesh, Name, NormalGreen, NormalZ, Primitive, Semantic, Stream,
    TexFormat, TexRole, Texture, Topology, World,
};
use primase_forzatech::components::{
    EmissionByBaseColour, ForzaMaterial, InstanceLevel, MaterialLevel, MaterialTexture, MaterialTextures,
    Parameter, ShaderLevel, Value,
};

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

fn float(name: &str, hash: u32, v: f32) -> Parameter {
    parameter(name, hash, Value::Float(v))
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
fn environment_maps_tilings_and_packing() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [200, 100, 50, 255]);
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 255, 255]);
    // rough 0.2, metal 1, occlusion 0.5, porosity 0
    let rma = texture(&mut world, "rma", ColorSpace::Linear, [51, 255, 128, 0]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "environment",
            "Standard",
            vec![
                tex("DiffAlphaTex_A", 1),
                tex("NormHeightTex_A", 2),
                tex("RoughMetAoPorTex_A", 3),
                parameter("BaseColorAlpha_A_UVTiling", 10, Value::Vector2([2.0, 3.0])),
                parameter("NormalHeight_A_UVTiling", 11, Value::Vector2([4.0, 4.0])),
                parameter("RoughMetAoPoros_A_UVTiling", 12, Value::Vector2([1.0, 1.0])),
                parameter("Uv0Offset", 13, Value::Vector2([0.5, 0.25])),
                parameter("AlphaTest", 14, Value::Int(0)),
                parameter("SwatchTint", 15, Value::Int(1)),
            ],
        ),
    );
    world.insert(
        e,
        MaterialTextures(vec![
            bound(1, Some(diffuse)),
            bound(2, Some(normal)),
            bound(3, Some(rma)),
        ]),
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    // tiling * (uv + offset)
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
    // a tiling of 1 with an offset is a transform of its own
    let mr = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(mr.transform.unwrap().offset, [0.5, 0.25]);
    // one packed map for both slots
    assert_eq!(pbr.occlusion_texture, pbr.metallic_roughness_texture);
    assert_eq!((pbr.metallic, pbr.roughness), (1.0, 1.0));
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);

    // R occlusion, G roughness, B metallic
    let packed = world.get::<Texture>(mr.texture).unwrap();
    assert_eq!(packed.levels[0], vec![128, 51, 255, 255]);
    assert_eq!(packed.color_space, ColorSpace::Linear);
    assert_eq!(packed.role, TexRole::Mask);

    // the normal map is the game's: X red, green up, Z rebuilt
    let n = world.get::<Texture>(normal).unwrap();
    assert_eq!(n.role, TexRole::Normal);
    let convention = n.normal.unwrap();
    assert_eq!(
        (convention.green, convention.z),
        (NormalGreen::Up, NormalZ::Rebuild)
    );
    assert_eq!(world.get::<Texture>(diffuse).unwrap().role, TexRole::Color);

    // the switch is on and no palette is bound: untinted, counted, with nothing for the baking pass
    assert_eq!(log.approximations["swatch tint palette not found: untinted"], 1);
    assert!(!world.has::<primase_forzatech::components::SwatchTint>(e));
    assert_eq!(log.counts["environment materials lowered"], 1);
}

#[test]
fn srgb_packed_source_is_decoded_as_the_shader_sees_it() {
    let mut world = World::new();
    // a BC1 `_SRGB` roughness-metal-occlusion map: the shader reads the decoded values
    let rma = texture(&mut world, "rma", ColorSpace::Srgb, [188, 0, 255, 255]);
    let e = world.spawn();
    world.insert(
        e,
        material("environment", "Standard", vec![tex("RoughMetAoPorTex_A", 3)]),
    );
    world.insert(e, MaterialTextures(vec![bound(3, Some(rma))]));
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    let packed = world
        .get::<Texture>(pbr.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    // sRGB 188 is linear 0.5 (127.5, rounded to 128)
    assert_eq!(packed.levels[0], vec![255, 128, 0, 255]);
}

#[test]
fn absent_textures_are_counted_not_guessed() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(
        e,
        material(
            "environment",
            "AlphaTest",
            vec![
                tex("DiffAlphaTex_A", 1),
                // an empty path: no texture bound
                tex("NormHeightTex_A", 0),
                parameter("AlphaTest", 14, Value::Int(1)),
            ],
        ),
    );
    // hash 1 is in no archive
    world.insert(e, MaterialTextures(vec![bound(1, None)]));
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert!(pbr.base_color_texture.is_none());
    assert!(pbr.normal_texture.is_none());
    assert!(pbr.metallic_roughness_texture.is_none());
    assert_eq!((pbr.metallic, pbr.roughness), (0.0, 1.0));
    assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
    assert_eq!(log.approximations["base colour texture in no archive: white"], 1);
    assert_eq!(log.approximations["no normal map"], 1);
    assert_eq!(log.approximations["no RoughMetAoPor map: dielectric, rough"], 1);
}

#[test]
fn a_family_without_a_rule_is_left_and_counted() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, material("terr_cube", "Standard", Vec::new()));
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    assert!(!world.has::<PbrMaterial>(e));
    assert_eq!(
        log.omitted["materials of shader family terr_cube left unlowered"],
        1
    );
}

#[test]
fn a_material_of_no_family_is_left_only_when_something_draws_it() {
    let mut world = World::new();
    let drawn = world.spawn();
    let undrawn = world.spawn();
    for e in [drawn, undrawn] {
        let mut m = material("", "Standard", Vec::new());
        m.shader = None;
        world.insert(e, m);
    }
    let mesh = world.spawn();
    world.insert(
        mesh,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: [(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]
            .into_iter()
            .collect(),
            indices: Some(vec![0, 1, 2]),
            material: Some(drawn),
        }]),
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    assert_eq!(
        log.omitted["materials of shader family no shader family left unlowered"],
        1
    );
    assert_eq!(
        log.omitted["triangles of shader family no shader family left unlowered"],
        1
    );
    assert_eq!(log.counts["materials of no shader family that nothing draws"], 1);
}

fn foliage(extra: Vec<Parameter>, atst: u8) -> ForzaMaterial {
    let mut parameters = vec![
        tex("DiffuseTexture", 1),
        tex("NormalTexture", 2),
        tex("RTAOTexture", 3),
        float("RoughnessScale", 20, 0.5),
        float("AlphaStepMin", 21, 0.25),
        float("AlphaStepMax", 22, 0.75),
        parameter("DoubleSided", 23, Value::Int(1)),
    ];
    parameters.extend(extra);
    let mut m = material("uber_foliage", "BushVertDeform", parameters);
    m.material.as_mut().unwrap().atst = [atst, 0];
    m
}

#[test]
fn foliage_bush_roughness_normal_occlusion_and_alpha_step() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [10, 200, 10, 255]);
    // normal in rg, roughness 0.8 in alpha
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 0, 204]);
    // the occlusion is in blue
    let ao = texture(&mut world, "ao", ColorSpace::Linear, [0, 0, 100, 255]);
    let e = world.spawn();
    world.insert(
        e,
        foliage(
            vec![
                parameter("Bush", 30, Value::Int(1)),
                parameter("Tint", 31, Value::Int(1)),
            ],
            1,
        ),
    );
    world.insert(
        e,
        MaterialTextures(vec![
            bound(1, Some(diffuse)),
            bound(2, Some(normal)),
            bound(3, Some(ao)),
        ]),
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.base_color_texture.unwrap().texture, diffuse);
    assert!(pbr.double_sided);
    // the alpha step range 0.25..0.75 at the middle reference
    assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
    assert_eq!((pbr.metallic, pbr.roughness), (0.0, 1.0));
    // roughness 0.8 * 0.5 = 0.4 in G, no metal
    let rough = world
        .get::<Texture>(pbr.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!(rough.levels[0], vec![255, 102, 0, 255]);
    assert_eq!(rough.color_space, ColorSpace::Linear);
    // the occlusion map is its own texture: R is the source's blue
    let occlusion = world
        .get::<Texture>(pbr.occlusion_texture.unwrap().texture)
        .unwrap();
    assert_eq!(occlusion.levels[0], vec![100, 100, 100, 255]);
    // a bush takes its normal from the same map
    assert_eq!(pbr.normal_texture.unwrap().texture, normal);
    let n = world.get::<Texture>(normal).unwrap();
    assert_eq!(n.role, TexRole::Normal);
    assert_eq!(n.normal.unwrap().green, NormalGreen::Up);

    assert_eq!(log.counts["uber_foliage materials lowered"], 1);
    assert_eq!(log.approximations["tint (colour lookup) left out"], 1);
    assert_eq!(
        log.approximations["alpha-test reference taken as one half of the step range"],
        1
    );
    assert!(!log.approximations.contains_key("canopy normal left out"));
}

#[test]
fn foliage_tree_has_no_normal_and_an_impostor_is_one_horizon_frame() {
    let mut world = World::new();
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 255, 255]);
    let tree = world.spawn();
    world.insert(tree, foliage(vec![parameter("Tree", 30, Value::Int(1))], 0));
    world.insert(tree, MaterialTextures(vec![bound(2, Some(normal))]));
    let billboard = world.spawn();
    world.insert(
        billboard,
        foliage(vec![parameter("Billboard", 32, Value::Int(1))], 1),
    );
    let atlas = texture(&mut world, "atlas", ColorSpace::Srgb, [90, 120, 60, 255]);
    let impostor = world.spawn();
    world.insert(
        impostor,
        foliage(
            vec![
                parameter("Impostor", 33, Value::Int(1)),
                parameter("FramesXY_Colour", 34, Value::Int(4)),
            ],
            1,
        ),
    );
    world.insert(impostor, MaterialTextures(vec![bound(1, Some(atlas))]));
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(tree).unwrap();
    assert!(pbr.normal_texture.is_none());
    assert!(pbr.metallic_roughness_texture.is_some());
    // not alpha tested: opaque, no occlusion map bound
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
    assert_eq!(world.get::<Texture>(normal).unwrap().role, TexRole::Other);
    assert_eq!(log.approximations["canopy normal left out"], 1);
    // the tree and the billboard card bind no occlusion map
    assert_eq!(log.approximations["no occlusion map"], 2);

    // a billboard is a flat card: lowered rough, with the vertex normal, and double-sided
    let card = world.get::<PbrMaterial>(billboard).unwrap();
    assert!(card.double_sided);
    assert!(card.normal_texture.is_none());
    assert_eq!(
        log.approximations["billboard writes no roughness or normal: rough, the vertex normal"],
        1
    );
    // an impostor is an atlas of views chosen by the view direction: drawn as the atlas's corner frame (`u` and `v`
    // 1, a view from the horizon) of 4 x 4, alpha-tested, on a double-sided card, the other frames' maps left out
    let card = world.get::<PbrMaterial>(impostor).unwrap();
    let base = card.base_color_texture.unwrap();
    assert_eq!(
        base.transform,
        Some(UvTransform {
            offset: [0.75, 0.75],
            scale: [0.25, 0.25],
            rotation: 0.0
        })
    );
    assert_eq!(base.wrap, [Wrap::Clamp; 2]);
    assert!(card.double_sided);
    assert!(matches!(card.alpha_mode, AlphaMode::Mask { .. }));
    assert!(card.occlusion_texture.is_none() && card.normal_texture.is_none());
    assert_eq!(
        log.approximations["impostor drawn as one fixed horizon frame (no view selection, blend or depth march)"],
        1
    );
    assert_eq!(
        log.approximations["impostor normal and roughness frames left out: rough, the vertex normal"],
        1
    );
    assert_eq!(log.approximations["impostor occlusion frames left out"], 1);
    assert!(!log.omitted.keys().any(|k| k.contains("uber_foliage")));
}

#[test]
fn branch_maps_tilings_roughness_and_occlusion() {
    let mut world = World::new();
    // roughness 0.6 in the diffuse alpha
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [90, 60, 30, 153]);
    // normal in rg, occlusion 0.5 in alpha
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 0, 128]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "uber_branch",
            "Branch3D",
            vec![
                tex("DiffuseA", 1),
                tex("NormalA", 2),
                parameter("DiffuseAUVTiling", 10, Value::Vector2([2.0, 4.0])),
                parameter("NormalAUVTiling", 11, Value::Vector2([3.0, 3.0])),
                float("NormalAIntensity", 12, 0.5),
                parameter("NormalAlpha_AO", 13, Value::Int(1)),
                parameter("MegaSkirt", 14, Value::Int(1)),
            ],
        ),
    );
    world.insert(
        e,
        MaterialTextures(vec![bound(1, Some(diffuse)), bound(2, Some(normal))]),
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(
        pbr.base_color_texture.unwrap().transform.unwrap().scale,
        [2.0, 4.0]
    );
    let rough = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(rough.transform.unwrap().scale, [2.0, 4.0]);
    assert_eq!(
        world.get::<Texture>(rough.texture).unwrap().levels[0],
        vec![255, 153, 0, 255]
    );
    assert_eq!(pbr.normal_texture.unwrap().transform.unwrap().scale, [3.0, 3.0]);
    assert_eq!(pbr.normal_scale, 0.5);
    let occlusion = pbr.occlusion_texture.unwrap();
    assert_eq!(occlusion.transform.unwrap().scale, [3.0, 3.0]);
    assert_eq!(
        world.get::<Texture>(occlusion.texture).unwrap().levels[0],
        vec![128, 128, 128, 255]
    );
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
    assert_eq!(log.approximations["mega-skirt terrain blend left out"], 1);
    assert_eq!(log.counts["uber_branch materials lowered"], 1);
}

#[test]
fn projection_lod_reads_its_atlas_cell_and_signed_roughness_metalness() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [200, 200, 200, 255]);
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 0, 255]);
    // a signed texel with roughness 64/127 (about 0.5) and metalness 1 (127/127)
    let lit = texture(&mut world, "lit", ColorSpace::Srgb, [255, 200, 0, 255]);
    let rm = texture(&mut world, "rm", ColorSpace::Linear, [0, 0, 0, 0]);
    world.get_mut::<Texture>(rm).unwrap().levels = vec![vec![64, 127, 0, 127]];
    world.get_mut::<Texture>(rm).unwrap().format = TexFormat::Rgba8Snorm;
    let e = world.spawn();
    world.insert(
        e,
        material(
            "projection_lod",
            "StandardAlphaTest",
            vec![
                tex("DiffuseATexture", 1),
                tex("RoughnessMetalnessATexture", 2),
                tex("NormalATexture", 3),
                tex("EmissiveTexture", 4),
                parameter("UVOutMinMax", 10, Value::Vector([0.25, 0.5, 0.5, 1.0])),
                parameter("RoughnessMetal", 11, Value::Int(1)),
                parameter("Emissive", 12, Value::Int(1)),
                float("EmissiveTextureScale", 13, 2.5),
            ],
        ),
    );
    world
        .get_mut::<ForzaMaterial>(e)
        .unwrap()
        .material
        .as_mut()
        .unwrap()
        .atst = [1, 0];
    world.insert(
        e,
        MaterialTextures(vec![
            bound(1, Some(diffuse)),
            bound(2, Some(rm)),
            bound(3, Some(normal)),
            bound(4, Some(lit)),
        ]),
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    let cell = UvTransform {
        offset: [0.25, 0.5],
        scale: [0.25, 0.5],
        rotation: 0.0,
    };
    assert_eq!(pbr.base_color_texture.unwrap().transform, Some(cell));
    assert_eq!(pbr.normal_texture.unwrap().transform, Some(cell));
    let mr = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(mr.transform, Some(cell));
    assert_eq!((pbr.metallic, pbr.roughness), (1.0, 1.0));
    // R 1, G roughness, B metallic: the signed values back, 64/127 and 1
    let packed = world.get::<Texture>(mr.texture).unwrap();
    assert_eq!(packed.levels[0], vec![255, 129, 255, 255]);
    assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
    // the emission map at the same cell, the scale 2.5 as the factor (the part above 1 in the strength)
    assert_eq!(pbr.emissive_texture.unwrap().transform, Some(cell));
    assert_eq!(pbr.emissive, [1.0; 3]);
    assert_eq!(pbr.emissive_strength, 2.5);
    assert_eq!(world.get::<Texture>(lit).unwrap().role, TexRole::Color);
    assert_eq!(
        log.approximations["projection_lod emission without the distance encoding and the sky preset's scale (run-time); the instance variation is applied per placement"],
        1
    );
    assert_eq!(log.counts["projection_lod materials lowered"], 1);
}

/// A material whose emission map is black emits nothing, and says so as a count rather than an approximation.
#[test]
fn projection_lod_with_a_black_emission_map_emits_nothing() {
    let mut world = World::new();
    let black = texture(&mut world, "black", ColorSpace::Srgb, [0, 0, 0, 255]);
    let e = world.spawn();
    world.insert(
        e,
        material(
            "projection_lod",
            "Standard",
            vec![
                tex("EmissiveTexture", 4),
                parameter("Emissive", 12, Value::Int(1)),
                float("EmissiveTextureScale", 13, 1.0),
            ],
        ),
    );
    world.insert(e, MaterialTextures(vec![bound(4, Some(black))]));
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert!(pbr.emissive_texture.is_none());
    assert_eq!(pbr.emissive_strength, PbrMaterial::default().emissive_strength);
    assert_eq!(
        log.counts["projection_lod emissive materials whose emission texture is black: no emission"],
        1
    );
    assert!(!log.approximations.keys().any(|k| k.contains("emission")));
}

/// `uberlod`: the diffuse map as the base colour, the emission map times `EmissiveScale` as the emission, and a material
/// with a normal map left unlowered by name.
#[test]
fn uberlod_is_its_diffuse_and_emission_maps() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [200, 200, 200, 255]);
    let lit = texture(&mut world, "lit", ColorSpace::Srgb, [255, 200, 0, 255]);
    let make = |world: &mut World, normal_map: i32| {
        let e = world.spawn();
        world.insert(
            e,
            material(
                "uberlod",
                "Default",
                vec![
                    tex("DiffuseATexture", 1),
                    tex("EmissiveTexture", 4),
                    parameter("Emissive", 12, Value::Int(1)),
                    parameter("NormalMap", 14, Value::Int(normal_map)),
                    float("EmissiveScale", 13, 3.0),
                    float("Roughness", 15, 0.8),
                ],
            ),
        );
        world.insert(
            e,
            MaterialTextures(vec![bound(1, Some(diffuse)), bound(4, Some(lit))]),
        );
        e
    };
    let plain = make(&mut world, 0);
    let normal = make(&mut world, 1);
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    let pbr = world.get::<PbrMaterial>(plain).unwrap();
    assert_eq!(pbr.base_color_texture.unwrap().texture, diffuse);
    assert_eq!(pbr.emissive_texture.unwrap().texture, lit);
    assert_eq!((pbr.emissive, pbr.emissive_strength), ([1.0; 3], 3.0));
    assert_eq!(pbr.roughness, 0.8);
    assert!(!world.has::<PbrMaterial>(normal));
    assert_eq!(log.counts["uberlod materials lowered"], 1);
    assert_eq!(
        log.omitted["materials of shader family uberlod with a normal map left unlowered"],
        1
    );
}

/// The `environment` emission: the base colour map times the emission map baked into one map, the tint times
/// `GIEmissiveScale` the factor; left out when the base colour map is tiled, kept on its own coordinate set (`EmissiveAltUV`)
/// for the rebasing pass, and as a count when it is black.
#[test]
fn environment_emission_is_the_base_colour_times_the_emission_map() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [255, 255, 255, 255]);
    let lit = texture(&mut world, "lit", ColorSpace::Srgb, [255, 128, 0, 255]);
    let black = texture(&mut world, "black", ColorSpace::Srgb, [0, 0, 0, 255]);
    let make = |world: &mut World, extra: Vec<Parameter>, emission: Entity| {
        let e = world.spawn();
        let mut parameters = vec![
            tex("DiffAlphaTex_A", 1),
            tex("EmissiveTexture", 4),
            parameter("Emissive", 20, Value::Int(1)),
            parameter("EmissiveTint", 21, Value::Colour([1.0, 0.5, 2.0, 1.0])),
            float("GIEmissiveScale", 22, 1.5),
            parameter("Uv0Offset", 13, Value::Vector2([0.5, 0.25])),
        ];
        parameters.extend(extra);
        world.insert(e, material("environment", "Standard", parameters));
        world.insert(
            e,
            MaterialTextures(vec![bound(1, Some(diffuse)), bound(4, Some(emission))]),
        );
        e
    };
    // an untiled base colour map is read at the offset like the emission map (the tiling parameter is 1)
    let plain = make(
        &mut world,
        vec![parameter(
            "BaseColorAlpha_A_UVTiling",
            10,
            Value::Vector2([1.0, 1.0]),
        )],
        lit,
    );
    let tiled = make(
        &mut world,
        vec![parameter(
            "BaseColorAlpha_A_UVTiling",
            10,
            Value::Vector2([2.0, 2.0]),
        )],
        lit,
    );
    let alt = make(
        &mut world,
        vec![parameter("EmissiveAltUV", 23, Value::Int(1))],
        lit,
    );
    let dark = make(&mut world, Vec::new(), black);
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(plain).unwrap();
    // the tint times 1.5: (1.5, 0.75, 3): the strength is the largest channel, the factor the rest
    assert_eq!(pbr.emissive_strength, 3.0);
    assert_eq!(pbr.emissive, [0.5, 0.25, 1.0]);
    let slot = pbr.emissive_texture.unwrap();
    assert_eq!(slot.transform, pbr.base_color_texture.unwrap().transform);
    let baked = world.get::<Texture>(slot.texture).unwrap();
    assert_eq!(baked.color_space, ColorSpace::Srgb);
    for (got, want) in baked.levels[0].iter().zip([255u8, 128, 0, 255]) {
        assert!(got.abs_diff(want) <= 1, "{:?}", baked.levels[0]);
    }
    for e in [tiled, dark] {
        assert!(world.get::<PbrMaterial>(e).unwrap().emissive_texture.is_none());
    }
    // the emission map on TEXCOORD4 is kept on it, marked for rebase-texcoord-set and the product of the two maps
    let kept = world.get::<PbrMaterial>(alt).unwrap();
    let slot = kept.emissive_texture.unwrap();
    assert_eq!((slot.texcoord, slot.texture), (4, lit));
    assert_eq!((kept.emissive_strength, kept.emissive), (3.0, [0.5, 0.25, 1.0]));
    assert_eq!(kept.base_color_texture.unwrap().texcoord, 0);
    assert!(world.has::<EmissionByBaseColour>(alt));
    assert_eq!(
        log.counts["EmissiveAltUV emission maps kept on TEXCOORD4 for rebase-texcoord-set"],
        1
    );
    assert_eq!(
        log.approximations["emission left out: the base colour map is tiled, so it and the emission map are on different coordinates"],
        1
    );
    assert_eq!(
        log.counts["environment emissive materials whose emission texture is black: no emission"],
        1
    );
    assert_eq!(log.counts["environment atlases baked as emission maps"], 1);
}

/// The road decal built of a diffuse (roughness in its alpha), a mask, a metalness map and a normal map.
fn road_decal(
    world: &mut World,
    family: &str,
    parameters: Vec<Parameter>,
    maps: [Option<Entity>; 4],
) -> Entity {
    let e = world.spawn();
    let mut parameters = parameters;
    parameters.extend([
        tex("DecalDiffuseATexture", 1),
        tex("DecalMaskATexture", 2),
        tex("Mask_MetalnessATexture", 3),
        tex("DecalNormalA", 4),
        parameter("RoadUVMultiplierA", 20, Value::Vector([1.0, 1.0, 1.0, 1.0])),
    ]);
    world.insert(e, material(family, "Decal", parameters));
    world.insert(
        e,
        MaterialTextures(maps.iter().zip(1..).map(|(m, h)| bound(h, *m)).collect()),
    );
    e
}

#[test]
fn road_decal_bakes_mask_tint_and_roughness_metalness_and_draws_as_a_decal() {
    let mut world = World::new();
    // linear 0.5 grey with roughness 0.5 in the alpha, sRGB
    let diffuse = texture(&mut world, "paint", ColorSpace::Srgb, [188, 188, 188, 128]);
    let mask = texture(&mut world, "mask", ColorSpace::Linear, [204, 0, 0, 255]);
    let metal = texture(&mut world, "metal", ColorSpace::Linear, [255, 0, 0, 255]);
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 255, 255]);
    let e = road_decal(
        &mut world,
        "uber_roaddecal",
        vec![
            parameter("AlphaMask", 10, Value::Int(1)),
            parameter("Metalness", 11, Value::Int(1)),
            parameter("PerObjectTint0OnOff", 12, Value::Int(1)),
            parameter("ObjectTintColor", 13, Value::Colour([0.5, 0.5, 0.5, 1.0])),
            parameter("Modulate", 14, Value::Int(1)),
            parameter("WorldSpaceAlphaOnOff", 15, Value::Int(1)),
        ],
        [Some(diffuse), Some(mask), Some(metal), Some(normal)],
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
    // the alpha is the mask's red; the colour is overlay(a, 0.5^(1/2.2)) back in sRGB
    let colour = world
        .get::<Texture>(pbr.base_color_texture.unwrap().texture)
        .unwrap();
    assert_eq!(colour.color_space, ColorSpace::Srgb);
    let linear = ((188.0f64 / 255.0 + 0.055) / 1.055).powf(2.4);
    let tint = 0.5f64.powf(1.0 / 2.2);
    let overlaid = if linear > 0.5 {
        1.0 - 2.0 * (1.0 - linear) * (1.0 - tint)
    } else {
        2.0 * linear * tint
    };
    let expected = i32::from(primase_foundations::color::linear_to_srgb8(overlaid));
    assert!(
        (i32::from(colour.levels[0][0]) - expected).abs() <= 1,
        "{:?}",
        colour.levels[0]
    );
    assert_eq!(colour.levels[0][3], 204);
    // G the diffuse alpha, B the metalness map's red
    let packed = world
        .get::<Texture>(pbr.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!(packed.levels[0], vec![255, 128, 255, 255]);
    assert_eq!((pbr.metallic, pbr.roughness), (1.0, 1.0));
    assert_eq!(
        world
            .get::<Texture>(pbr.normal_texture.unwrap().texture)
            .unwrap()
            .role,
        TexRole::Normal
    );
    assert_eq!(world.get::<DrawOrder>(e).unwrap().layer, DrawLayer::Decal);
    assert_eq!(log.approximations["modulate (vertex colour's red) left out"], 1);
    assert_eq!(log.approximations["world-space alpha mask left out"], 1);
    assert_eq!(log.counts["uber_roaddecal materials lowered"], 1);
}

#[test]
fn road_decal_without_an_alpha_mask_is_opaque_and_keeps_its_diffuse() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "paint", ColorSpace::Srgb, [200, 100, 50, 64]);
    let mask = texture(&mut world, "mask", ColorSpace::Linear, [0, 0, 0, 255]);
    let e = road_decal(
        &mut world,
        "uber_roaddecal",
        vec![
            parameter("AlphaMask", 10, Value::Int(0)),
            parameter("Enabled", 11, Value::Int(0)),
        ],
        [Some(diffuse), Some(mask), None, None],
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
    assert_eq!(pbr.base_color_texture.unwrap().texture, diffuse);
    // roughness from the diffuse alpha, no metal
    let packed = world
        .get::<Texture>(pbr.metallic_roughness_texture.unwrap().texture)
        .unwrap();
    assert_eq!(packed.levels[0][1], 64);
    assert_eq!(pbr.metallic, 0.0);
    assert_eq!(world.get::<DrawOrder>(e).unwrap().layer, DrawLayer::Decal);
    assert_eq!(log.approximations["no normal map"], 1);
    assert_eq!(
        log.approximations["decal switched off (Enabled) drawn: no pixel shader reads it"],
        1
    );
}

#[test]
fn retro_road_decal_height_alpha_is_the_mask_over_its_range() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "paint", ColorSpace::Srgb, [255, 255, 255, 255]);
    // mask 26 / 255 = 0.102: sat(0.102 / 0.3) = 0.34
    let mask = texture(&mut world, "mask", ColorSpace::Linear, [26, 0, 0, 255]);
    let e = road_decal(
        &mut world,
        "uber_roaddecalretro",
        vec![
            parameter("HeightAlpha", 10, Value::Int(1)),
            parameter("MaxHeightAlpha", 11, Value::Float(0.3)),
            parameter("PerObjectTint0OnOff", 12, Value::Int(0)),
        ],
        [Some(diffuse), Some(mask), None, None],
    );
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    let pbr = world.get::<PbrMaterial>(e).unwrap();
    assert_eq!(pbr.alpha_mode, AlphaMode::Mask { cutoff: 0.5 });
    let colour = world
        .get::<Texture>(pbr.base_color_texture.unwrap().texture)
        .unwrap();
    // 26 / 255 / 0.3 = 0.3398 -> 87 (the float32 range is 0.3 less a hair)
    assert!(
        (i32::from(colour.levels[0][3]) - 87).abs() <= 1,
        "{:?}",
        colour.levels[0]
    );
    assert_eq!(&colour.levels[0][..3], &[255, 255, 255]);
    assert_eq!(
        log.approximations["retro-reflection and the distance fade of the alpha (scene constants) left out"],
        1
    );
    assert_eq!(log.counts["uber_roaddecalretro materials lowered"], 1);
}

#[test]
fn deferred_decal_is_blended_in_the_decal_layer_and_projected_ones_are_left() {
    let mut world = World::new();
    let diffuse = texture(&mut world, "diffuse", ColorSpace::Srgb, [200, 100, 50, 255]);
    let normal = texture(&mut world, "normal", ColorSpace::Linear, [128, 128, 255, 255]);
    // roughness 0.2, occlusion 0.5
    let rmao = texture(&mut world, "rmao", ColorSpace::Linear, [51, 255, 128, 0]);
    let build = |world: &mut World, intensity: f32, nao: i32, projected: i32| {
        let e = world.spawn();
        world.insert(
            e,
            material(
                "environmentdeferreddecal",
                "Standard",
                vec![
                    tex("DiffAlphaTex", 1),
                    tex("NormalTex", 2),
                    tex("RoughMetAOTex", 3),
                    parameter("DiffAlphaUVTiling", 10, Value::Vector2([0.5, 0.5])),
                    parameter("Uv0Offset", 11, Value::Vector2([0.5, 0.0])),
                    parameter("DiffRoughIntensity", 12, Value::Float(0.75)),
                    parameter("NormalAOIntensity", 13, Value::Float(intensity)),
                    parameter("NormalAO", 14, Value::Int(nao)),
                    parameter("Projected", 15, Value::Int(projected)),
                    parameter("SwatchTint", 16, Value::Int(1)),
                ],
            ),
        );
        world.insert(
            e,
            MaterialTextures(vec![
                bound(1, Some(diffuse)),
                bound(2, Some(normal)),
                bound(3, Some(rmao)),
            ]),
        );
        e
    };
    let with_normal = build(&mut world, 1.0, 1, 0);
    let without_normal = build(&mut world, 0.0, 1, 0);
    let plain = build(&mut world, 1.0, 0, 0);
    let projected = build(&mut world, 1.0, 1, 1);
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);

    let pbr = world.get::<PbrMaterial>(with_normal).unwrap();
    assert_eq!(pbr.alpha_mode, AlphaMode::Blend);
    assert_eq!(pbr.base_color[3], 0.75);
    assert_eq!(
        pbr.base_color_texture.unwrap().transform,
        Some(UvTransform {
            offset: [0.25, 0.0],
            scale: [0.5, 0.5],
            rotation: 0.0
        })
    );
    // R occlusion (blue), G roughness (red), no metal; one map in both slots
    let slot = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(pbr.occlusion_texture, Some(slot));
    assert_eq!(
        world.get::<Texture>(slot.texture).unwrap().levels[0],
        vec![128, 51, 0, 255]
    );
    assert!(pbr.normal_texture.is_some());
    assert_eq!(
        world.get::<DrawOrder>(with_normal).unwrap().layer,
        DrawLayer::Decal
    );
    assert!(!world.get::<DrawOrder>(with_normal).unwrap().depth_write);

    // a zero normal weight leaves the normal out, not the occlusion
    let pbr = world.get::<PbrMaterial>(without_normal).unwrap();
    assert!(pbr.normal_texture.is_none() && pbr.occlusion_texture.is_some());
    // no NAO: roughness only
    let pbr = world.get::<PbrMaterial>(plain).unwrap();
    assert!(pbr.normal_texture.is_none() && pbr.occlusion_texture.is_none());
    let slot = pbr.metallic_roughness_texture.unwrap();
    assert_eq!(
        world.get::<Texture>(slot.texture).unwrap().levels[0],
        vec![255, 51, 0, 255]
    );

    assert!(!world.has::<PbrMaterial>(projected));
    assert_eq!(
        log.omitted["materials of shader family environmentdeferreddecal projected left unlowered"],
        1
    );
    assert_eq!(log.approximations["swatch tint left out"], 3);
    assert_eq!(log.counts["environmentdeferreddecal materials lowered"], 3);
}

/// A `projection_lod` material whose `NormalATexture` is a 1x1 `Rgba8Snorm` texture of `texel` (signed bytes), with the
/// lowering's log.
fn signed_normal(texel: [u8; 4]) -> (World, Entity, Entity, PassLog) {
    let mut world = World::new();
    let normal = texture(&mut world, "normal", ColorSpace::Linear, texel);
    world.get_mut::<Texture>(normal).unwrap().format = TexFormat::Rgba8Snorm;
    let e = world.spawn();
    world.insert(
        e,
        material("projection_lod", "Standard", vec![tex("NormalATexture", 3)]),
    );
    world.insert(e, MaterialTextures(vec![bound(3, Some(normal))]));
    let mut log = PassLog::default();
    lowering::lower(&mut world, &mut log);
    (world, e, normal, log)
}

/// A normal map in a signed format whose texels are all the neutral normal (`v` about 0.5, so the shader's `2 v - 1` is
/// about 0) is the engine's flat default: the material takes no normal map, counted as such and not as a loss.
#[test]
fn a_flat_signed_normal_map_is_no_normal_map() {
    // 64/127 = 0.504: 2 v - 1 = 0.008, under the flat tilt
    let (world, e, _, log) = signed_normal([64, 64, 127, 127]);
    assert!(world.get::<PbrMaterial>(e).unwrap().normal_texture.is_none());
    assert_eq!(log.counts["flat default normal map: no normal map"], 1);
    // dropping it is an approximation that names the map's largest tilt: the decoded byte 192 is v = 0.5059, 2 v - 1 = 0.012
    let why = "flat default normal map left out: no normal map (largest tilt 0.012, within the 0.02 bound)";
    assert_eq!(log.approximations[why], 1);
    assert!(log.approximations.contains_key(&format!("{why}: triangles")));
    assert!(
        !log.approximations
            .keys()
            .any(|k| k.contains("not lowered") || k.contains("re-encoded")),
        "{:?}",
        log.approximations
    );
}

/// Any other signed normal map is re-encoded: the positive half of the signed value `v` as the unsigned byte glTF reads
/// as `2 b - 1`, the shader's own `2 v - 1`, with the rebuilt `z` in blue.
#[test]
fn a_signed_normal_map_is_re_encoded_as_the_unsigned_map_the_shader_read() {
    // the signed bytes 127 and 32 are v = 1 and 0.252, stored as 255 and 160 (v * 0.5 + 0.5) by the decoder
    let (world, e, source, log) = signed_normal([127, 32, 127, 127]);
    let slot = world
        .get::<PbrMaterial>(e)
        .unwrap()
        .normal_texture
        .expect("a normal map");
    assert_ne!(slot.texture, source);
    let made = world.get::<Texture>(slot.texture).unwrap();
    assert_eq!(made.role, TexRole::Normal);
    assert_eq!(made.color_space, ColorSpace::Linear);
    assert_eq!(made.normal.unwrap().z, NormalZ::Rebuild);
    // x = 2 * 1 - 1 = 1 and y = 2 * 0.2549 - 1 = -0.49: no z is left (0.5 stored); the byte is the signed value back
    assert_eq!(made.levels[0], vec![255, 65, 128, 255]);
    assert_eq!(
        log.counts["signed normal textures re-encoded as glTF normal maps"],
        1
    );
    assert!(!log.counts.contains_key("flat default normal map: no normal map"));
    assert!(
        !log.approximations
            .keys()
            .any(|k| k.contains("normal map not lowered") || k.contains("clamped")),
        "{:?}",
        log.approximations
    );
}

/// A texel below zero cannot be held by glTF's unsigned map (the shader's `2 v - 1` of it is below -1): clamped to
/// zero, counted by texel.
#[test]
fn negative_texels_of_a_signed_normal_map_are_clamped_and_counted() {
    // the signed byte 192 is -64: v = -0.504
    let (world, e, _, log) = signed_normal([192, 64, 127, 127]);
    let slot = world.get::<PbrMaterial>(e).unwrap().normal_texture.unwrap();
    let made = world.get::<Texture>(slot.texture).unwrap();
    assert_eq!(made.levels[0][0], 0);
    assert_eq!(
        log.approximations["signed normal map texels below zero clamped to zero (glTF's unsigned map cannot hold them)"],
        1
    );
}
