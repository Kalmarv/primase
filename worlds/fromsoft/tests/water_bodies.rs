//! The water surfaces of Elden Ring's Stormveil (`m10_00_00_00`, the castle's slice that holds its lake) and Sekiro's
//! Ashina Castle courtyard (`m11_01`), measured from their own forward programs ([`primase_fromsoft::water_derive::measure`])
//! as `resolve-fromsoft-shaders` binds them: every material whose program reads the frame's scene colour and depth is
//! run, and what it draws is printed. The lake `m10_00_692` (`M[PlacidLake]`) measures as its program computes from its
//! material binary's constants: its absorption (`float3_0` plus `float3_2`: 0.215, 0.115 and 0.09 per metre), its
//! Fresnel weight (F0 0.04), its shore fade and its two normal maps, each within 1%. Sekiro's courtyard water
//! (`M[Interaction]_Tessellation_Water`, `g_BlendMode` 0) is drawn into the G-buffer and reads nothing behind it.
//!
//! A fixture program of known optics (`tests/fixtures/water_body.hlsl`) measures as it states, and `derive-fromsoft-water`
//! gives its material the body it measures, without an install.
//!
//! The install test needs `installs.elden`, `keys.elden` and `tools.oodle6`.
mod common;
mod inputs;

use fromsoft_formats::{Game, Keys, dcx::Decoder};
use iridia_interface::Stage;
use primase_core::World;
use primase_core::shader::{Shader, ShaderBinding};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::materials::Material;
use primase_fromsoft::sekiro;
use primase_fromsoft::sekiro::source::Region;
use primase_fromsoft::water_derive::{Measured, measure};
use primase_fromsoft::water_probe::WaterProbe;
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Each bound material whose program reads the scene behind it, by its name: what it measured, or why not.
fn measured(world: &World) -> BTreeMap<String, (String, std::result::Result<Measured, String>)> {
    let mut out = BTreeMap::new();
    for (e, material) in world.iter::<Material>() {
        let Some(binding) = world.get::<ShaderBinding>(e) else {
            continue;
        };
        let Some(stage) = binding.stage(Stage::Pixel) else {
            continue;
        };
        let Some(shader) = world.get::<Shader>(stage.shader) else {
            continue;
        };
        let Some((vertex, vertex_entity)) = binding
            .stage(Stage::Vertex)
            .and_then(|v| world.get::<Shader>(v.shader).map(|s| (s, v.shader)))
        else {
            out.insert(
                material.mtd.clone(),
                (material.shader.clone(), Err("no bound vertex program".to_owned())),
            );
            continue;
        };
        let probe = match WaterProbe::new(shader, stage, vertex, vertex_entity) {
            Ok(p) => p,
            Err(why) => {
                out.insert(material.mtd.clone(), (material.shader.clone(), Err(why)));
                continue;
            }
        };
        if !probe.reads_behind() {
            continue;
        }
        let result = measure(&probe).map_err(|n| format!("{} ({})", n.reason, n.detail));
        out.insert(material.mtd.clone(), (material.shader.clone(), result));
    }
    out
}

fn print(area: &str, found: &BTreeMap<String, (String, std::result::Result<Measured, String>)>) {
    for (name, (shader, m)) in found {
        match m {
            Ok(m) => println!(
                "{area} {name} {shader}: from {} m: extinction {:?} scatter {:?} far {:?} f0 {:.4} (schlick error {:.4}) shore {:.3} m opacity {:.3} waves {:?} not carried {:?}",
                m.distance,
                m.extinction,
                m.scatter,
                m.far_scatter,
                m.f0,
                m.schlick_error,
                m.shore_fade,
                m.deep_opacity,
                m.waves
                    .iter()
                    .map(|w| (
                        w.register,
                        w.transform.scale,
                        w.transform.offset,
                        w.velocity,
                        w.strength
                    ))
                    .collect::<Vec<_>>(),
                m.not_carried
            ),
            Err(why) => println!("{area} {name} {shader}: not measured: {why}"),
        }
    }
}

#[test]
fn the_water_programs_measure_as_their_materials_state() -> Result<()> {
    let (Some(elden_root), Some(elden_keys), Some(dll)) = (
        inputs::install(inputs::Game::Elden),
        inputs::keys(inputs::Game::Elden),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        elden_root,
        Game::EldenRing,
        Keys::from_folder(elden_keys, Game::EldenRing, Game::EldenRing.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let mut world = World::default();
    sekiro::read_game_area(
        &install,
        "m10_00_00_00",
        &decoder,
        &sekiro::Options {
            region: Some(Region {
                rect: [-162., 5., 108., 250.],
                margin: 0.,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}")?;
    let found = measured(&world);
    print("m10_00_00_00", &found);
    let (_, lake) = found
        .get("m10_00_692")
        .ok_or("the lake m10_00_692 is not bound forward")?;
    let lake = lake
        .as_ref()
        .map_err(|e| format!("the lake is not measured: {e}"))?;
    // what M[PlacidLake]_FwdSdw computes from m10_00_692's constants: the extinction float3_0 + float3_2, the Fresnel
    // weight 0.96 - 0.96 (1 - N.V)^5 (F0 0.04), no colour of its own where deep (color_0's fifth value is 0, as the
    // captured draws bind it), the shore fade 1 / float_3, normal map A (t1, tiling 4) at full strength and B (t0, tiling
    // 2 times float2_5) at float_9; and its body blended in with the distance from the camera, which the body does not
    // carry
    let close = |got: f32, want: f32, what: &str| {
        assert!(
            (got - want).abs() <= 0.01 * want.abs().max(0.01),
            "{what}: {got}, the program's {want} ({lake:?})"
        );
    };
    for (k, stated) in [0.215f32, 0.115, 0.09].into_iter().enumerate() {
        close(lake.extinction[k], stated, "extinction");
    }
    close(lake.f0, 0.04, "F0");
    for k in 0..3 {
        close(lake.scatter[k], 0., "scatter");
    }
    close(lake.shore_fade, 1. / 3., "shore fade");
    close(lake.deep_opacity, 1., "deep opacity");
    let wave = |register: u32| {
        lake.waves
            .iter()
            .find(|w| w.register == register)
            .unwrap_or_else(|| panic!("no wave t{register}: {:?}", lake.waves))
    };
    close(wave(1).strength, 1., "map A's strength");
    close(wave(1).transform.scale[0], 4., "map A's tiling");
    close(wave(0).strength, 0.2, "map B's strength");
    close(wave(0).transform.scale[0], 24., "map B's tiling");
    assert!(
        lake.not_carried
            .iter()
            .any(|r| r == primase_fromsoft::water_derive::DISTANCE_BODY),
        "{:?}",
        lake.not_carried
    );
    // the lake's program reads the light set's Vec_0.w, which no Elden Ring capture states: the body says so
    assert!(
        lake.stated
            .contains_key(primase_fromsoft::layer_bake::resolve::SUN_SPREAD_POLICY),
        "{:?}",
        lake.stated
    );
    // Sekiro: the Ashina courtyard slice every Sekiro real-data gate reads, with its tessellated water
    let Some(sekiro_root) = inputs::install(inputs::Game::Sekiro) else {
        return Ok(());
    };
    let Some(sekiro_install) = common::sekiro(&sekiro_root)? else {
        return Ok(());
    };
    let mut world = World::default();
    sekiro::read_game_area(
        &sekiro_install,
        "m11_01",
        &decoder,
        &sekiro::Options {
            region: Some(Region {
                rect: [-170., 180., -50., 300.],
                margin: 30.,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    common::run(&mut world, "resolve-fromsoft-shaders{unbound-textures=preview}")?;
    print("m11_01", &measured(&world));
    Ok(())
}

/// The fixture's water (`tests/fixtures/water_body.hlsl`) and its vertex program (`water_body_vs.hlsl`), bound as
/// `resolve-fromsoft-shaders` binds an Elden Ring forward material with its normal map, in `world`.
struct Fixture {
    material: fromsoft_formats::mtd::MaterialDefinition,
    /// The pixel program's binding, its program the entity `program`.
    stage: primase_core::shader::StageBinding,
    program: primase_core::Entity,
    shader: Shader,
    vertex: Shader,
    /// The entity the binding binds the normal map to.
    wave: primase_core::Entity,
}

fn fixture(world: &mut World) -> Result<Fixture> {
    use fromsoft_formats::mtd::{MaterialDefinition, Parameter, Texture, Value};
    use iridia_interface::ShaderFormat;
    use primase_fromsoft::layer_bake::resolve::{Unbound, resolve};
    use primase_fromsoft::materials::{Binding, LayeredMaterial};
    // SHA-256 0a741dcb48bc262d8da6261d61721420e7a54864f372aa23dcd29edff98223a4 (fxc 10.0.19041 ps_5_0)
    const PROGRAM: &[u8] = include_bytes!("fixtures/water_body.dxbc");
    // SHA-256 962d0eb804d88b51efaf94742fcbbfb3ddfb6d22fbcbd6d991470138fd31e6da (fxc 10.0.19041 vs_5_0)
    const VERTEX: &[u8] = include_bytes!("fixtures/water_body_vs.dxbc");
    let kind = "Texture2D_0_NormalMap";
    let definition = MaterialDefinition {
        shader: "Fixture.spx".into(),
        description: String::new(),
        parameters: vec![Parameter {
            name: "g_BlendMode".into(),
            value: Value::Int(vec![2]),
        }],
        textures: vec![Texture {
            kind: kind.into(),
            uv_number: 1,
            shader_data_index: 0,
            path: "wave_n.tif".into(),
            extra: vec![],
            version: 5,
        }],
        block_lengths: vec![],
        padding: vec![],
    };
    let material = LayeredMaterial {
        mtd: "fixture_water".into(),
        definition: definition.clone(),
        bindings: BTreeMap::from([(
            kind.to_owned(),
            Binding {
                texture: "wave_n".into(),
                scale: [1.; 2],
                uv_number: 1,
                address: [None; 2],
                available: true,
                coarse_of: None,
            },
        )]),
    };
    let executor = iridia_exec::ProgramShader::from_ir(primase_core::shader::lift_dxbc(PROGRAM)?)?;
    let program = world.spawn();
    let wave = world.spawn();
    let resolution = resolve(
        &executor,
        program,
        &material.view(),
        Game::EldenRing,
        None,
        Unbound::Preview,
        &mut |_, _| Ok(Some(wave)),
    )?;
    Ok(Fixture {
        material: definition,
        stage: resolution.stage,
        program,
        // the pixel program's interface as the lifter reads it, as the reader stores it (resolve-fromsoft-shaders): the
        // binding's resource ids are its
        shader: Shader {
            format: ShaderFormat::Dxbc,
            bytecode: PROGRAM.to_vec(),
            interface: iridia_exec::PixelShader::interface(&executor).clone(),
        },
        vertex: Shader {
            format: ShaderFormat::Dxbc,
            bytecode: VERTEX.to_vec(),
            interface: iridia_dxbc::reflection(VERTEX)?,
        },
        wave,
    })
}

/// The fixture's water measures as its program states: each constant given back.
#[test]
fn a_water_program_of_known_optics_measures_as_it_states() -> Result<()> {
    let mut world = World::default();
    let Fixture {
        stage,
        shader,
        vertex,
        ..
    } = fixture(&mut world)?;
    let probe = WaterProbe::new(&shader, &stage, &vertex, world.spawn())?;
    assert!(probe.reads_behind());
    let m = measure(&probe).map_err(|n| format!("{} ({})", n.reason, n.detail))?;
    let near = |got: f32, want: f32, what: &str| {
        assert!(
            (got - want).abs() <= 1e-3 * want.abs().max(1.),
            "{what}: {got}, stated {want}"
        );
    };
    for (k, sigma) in [0.3, 0.2, 0.1].into_iter().enumerate() {
        near(m.extinction[k], sigma, "extinction");
    }
    for (k, body) in [0.05, 0.1, 0.15].into_iter().enumerate() {
        near(m.scatter[k], body, "scatter");
    }
    assert_eq!(m.far_scatter, None);
    near(m.f0, 0.02, "f0");
    assert!(m.schlick_error < 1e-3, "{}", m.schlick_error);
    near(m.shore_fade, 0.5, "shore fade");
    near(m.deep_opacity, 1., "deep opacity");
    let [w] = &m.waves[..] else {
        panic!("waves {:?}", m.waves);
    };
    near(w.transform.scale[0], 3., "tiling u");
    near(w.transform.scale[1], 3., "tiling v");
    near(w.velocity[0], 0.01, "scroll u");
    near(w.velocity[1], -0.02, "scroll v");
    near(w.strength, 0.5, "strength");
    assert!(m.not_carried.is_empty(), "{:?}", m.not_carried);
    Ok(())
}

/// `derive-fromsoft-water` gives the fixture's material its class water and the body its program measures (its ior from
/// the measured F0 of 0.02, its wave the bound normal map at the program's tiling), and counts a material whose
/// definition states water and whose program does not read the scene behind it, which keeps its class without a body.
#[test]
fn the_water_pass_gives_a_water_program_its_measured_body() -> Result<()> {
    use fromsoft_formats::mtd::{Parameter, Value};
    use primase_core::WaterBody;
    use primase_core::shader::{FrameTexture, StageBinding, TextureSource};
    use primase_fromsoft::materials::Definition;
    let mut world = World::default();
    let f = fixture(&mut world)?;
    // the normal map the binding binds: one flat texel (the probe answers material reads itself)
    world.insert(
        f.wave,
        primase_core::Texture {
            width: 1,
            height: 1,
            format: primase_core::TexFormat::Rgba8Unorm,
            levels: vec![vec![128, 128, 255, 255]],
            color_space: primase_core::ColorSpace::Linear,
            role: primase_core::TexRole::Mask,
            normal: None,
        },
    );
    world.insert(f.program, f.shader);
    let vertex = world.spawn();
    world.insert(vertex, f.vertex);
    let bound = |stage: StageBinding| ShaderBinding {
        stages: vec![stage, StageBinding::new(Stage::Vertex, vertex)],
        ..ShaderBinding::default()
    };
    let source = |class: &str, stated: bool| {
        let mut definition = f.material.clone();
        if stated {
            definition.parameters.push(Parameter {
                name: primase_fromsoft::materials::STATED_WATER.into(),
                value: Value::Bool(true),
            });
        }
        Material {
            mtd: format!("fixture_{class}"),
            definition: Some(Definition::Mtd(definition)),
            gx: vec![],
            shader: "Fixture.spx".into(),
            class: class.into(),
            skip: None,
            cutoff: 0.,
            bindings: BTreeMap::new(),
            layered: None,
        }
    };
    let water = world.spawn();
    world.insert(water, source("alpha", false));
    world.insert(water, bound(f.stage.clone()));
    // the same program bound without the frame's scene colour: it reads nothing behind its surface
    let mut blind = f.stage.clone();
    blind
        .textures
        .retain(|t| !matches!(t.source, TextureSource::Frame(FrameTexture::SceneColour)));
    let stated = world.spawn();
    world.insert(stated, source("water", true));
    world.insert(stated, bound(blind));
    // with the pass checks on in every build: what the pass leaves is verified (the bindings' textures among it)
    let manager = common::manager()
        .with_pass_checks(true)
        .with_conservation(primase_core::ledger::Conservation::Enforce);
    let mut report = primase_core::pass::RunReport::default();
    manager.run(
        &mut world,
        &manager.registry.parse("derive-fromsoft-water")?,
        &mut report,
    )?;
    let near = |got: f32, want: f32, what: &str| {
        assert!(
            (got - want).abs() <= 1e-3 * want.abs().max(1.),
            "{what}: {got}, stated {want}"
        );
    };
    assert_eq!(
        world.get::<Material>(water).map(|m| m.class.as_str()),
        Some("water")
    );
    let body = world
        .get::<WaterBody>(water)
        .ok_or("the fixture's material has no body")?;
    for (k, sigma) in [0.3, 0.2, 0.1].into_iter().enumerate() {
        near(body.extinction[k], sigma, "extinction");
    }
    for (k, scatter) in [0.05, 0.1, 0.15].into_iter().enumerate() {
        near(body.scatter[k], scatter, "scatter");
    }
    near(body.ior, WaterBody::ior_of_f0(0.02), "ior");
    near(body.shore_fade, 0.5, "shore fade");
    let [wave] = &body.waves[..] else {
        panic!("waves {:?}", body.waves);
    };
    assert_eq!(wave.normal.texture, f.wave);
    let transform = wave.normal.transform.ok_or("the wave has no transform")?;
    near(transform.scale[0], 3., "tiling u");
    near(wave.velocity[1], -0.02, "scroll v");
    near(wave.strength, 0.5, "strength");
    assert!(world.get::<WaterBody>(stated).is_none());
    assert_eq!(
        world.get::<Material>(stated).map(|m| m.class.as_str()),
        Some("water")
    );
    let log = &report.steps[0].log;
    let not_behind = format!(
        "materials whose program reads the scene behind it, given no water body: {}",
        primase_fromsoft::sekiro::passes::water::NOT_BEHIND
    );
    assert_eq!(log.counts.get(&not_behind), Some(&1), "{:?}", log.counts);
    // the runs without a sun stated the light's direction: the body's count says so
    let no_sun = format!(
        "water program run with {}",
        primase_fromsoft::layer_bake::NO_DIRECTIONAL_LIGHT_POLICY
    );
    assert!(
        log.approximations.contains_key(&no_sun),
        "{:?}",
        log.approximations
    );
    Ok(())
}
