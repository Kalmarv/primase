//! `lower-forzatech-materials` on hand-built materials: what the `environment` rule makes of the three maps, their
//! tilings, an alpha test, a texture no archive holds and a family it has no rule for.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test over data it builds itself"
)]

use primase_core::pass::{Pass, PassLog};
use primase_core::pbr::{AlphaMode, PbrMaterial, UvTransform};
use primase_core::{ColorSpace, Entity, Name, NormalGreen, NormalZ, TexFormat, TexRole, Texture, World};
use primase_forzatech::components::{
    ForzaMaterial, InstanceLevel, MaterialLevel, MaterialTexture, MaterialTextures, Parameter, ShaderLevel,
    Value,
};
use primase_forzatech::passes::LowerForzaMaterials;

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
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

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

    assert_eq!(log.approximations["swatch tint left out"], 1);
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
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
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
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
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
    world.insert(e, material("glass", "Standard", Vec::new()));
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();
    assert!(!world.has::<PbrMaterial>(e));
    assert_eq!(log.omitted["materials of shader family glass left unlowered"], 1);
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
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

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
fn foliage_tree_has_no_normal_and_a_billboard_is_left() {
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
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

    let pbr = world.get::<PbrMaterial>(tree).unwrap();
    assert!(pbr.normal_texture.is_none());
    assert!(pbr.metallic_roughness_texture.is_some());
    // not alpha tested: opaque, no occlusion map bound
    assert_eq!(pbr.alpha_mode, AlphaMode::Opaque);
    assert_eq!(world.get::<Texture>(normal).unwrap().role, TexRole::Other);
    assert_eq!(log.approximations["canopy normal left out"], 1);
    assert_eq!(log.approximations["no occlusion map"], 1);

    assert!(!world.has::<PbrMaterial>(billboard));
    assert_eq!(
        log.omitted["materials of shader family uber_foliage billboard or impostor left unlowered"],
        1
    );
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
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

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
                parameter("UVOutMinMax", 10, Value::Vector([0.25, 0.5, 0.5, 1.0])),
                parameter("RoughnessMetal", 11, Value::Int(1)),
                parameter("Emissive", 12, Value::Int(1)),
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
        ]),
    );
    let mut log = PassLog::default();
    LowerForzaMaterials.run(&mut world, &mut log).unwrap();

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
    assert_eq!(log.approximations["emission left out"], 1);
    assert_eq!(log.counts["projection_lod materials lowered"], 1);
}
