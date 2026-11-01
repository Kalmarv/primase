//! Source-owned replacement descriptions on installed Unity draws. Counts here are source bindings, not a
//! consumer's executable-stage refusals or GPU pipeline count. The paired lab proof checks the latter separately.
#![allow(clippy::unwrap_used)]

mod inputs;

use std::collections::BTreeSet;

use inputs::{Game, Tool, install_file, tool};
use iridia_interface::Stage;
use primase_core::pass::{Options, PassLog};
use primase_core::shader::{FallbackCoordinates, InputFallbackValue, InputRole, Shader, ShaderBinding};
use primase_core::{Name, World};
use primase_unity::components::{UnityMaterial, UnityRenderSettings};
use primase_unity::level::{LevelOptions, read_level_with};
use primase_unity::program::read_programs;
use primase_unity::resolve_shaders::RESOLVE_UNITY_SHADERS;
use unity_formats::TypeTreePackage;
use unity_formats::player::{Player, RenderPipeline};

fn installed(
    data: Option<std::path::PathBuf>,
    level: &str,
    quality: Option<&str>,
    material: &str,
    program: &str,
    expected: [usize; 3],
) {
    let (Some(data), Some(tpk)) = (data, tool(Tool::UnityTpk)) else {
        return;
    };
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let mut world = World::new();
    read_level_with(
        &player,
        level,
        &LevelOptions {
            quality: quality.map(str::to_owned),
        },
        &mut world,
    )
    .unwrap();
    let pipeline = world
        .resource::<UnityRenderSettings>()
        .and_then(|s| s.pipeline.clone())
        .unwrap_or(RenderPipeline::BuiltIn);
    let names: BTreeSet<_> = world
        .iter::<UnityMaterial>()
        .filter_map(|(_, m)| m.shader.clone())
        .collect();
    read_programs(
        &player,
        &mut world,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
        &pipeline,
    )
    .unwrap();
    let pass = RESOLVE_UNITY_SHADERS
        .instance(&Options::from([
            ("tier".into(), "highest".into()),
            ("keywords".into(), "scene".into()),
        ]))
        .unwrap();
    let mut log = PassLog::default();
    pass.run(&mut world, &mut log).unwrap();
    let mut totals = [0; 3];
    for (_, binding) in world.iter::<ShaderBinding>() {
        let mut described = false;
        for stage in binding.stages.iter().filter(|s| s.stage == Stage::Pixel) {
            for input in &stage.inputs {
                if let InputRole::PreviousOr { fallback, .. } = &input.role {
                    fallback.check().unwrap();
                    described = true;
                    totals[1] += 1;
                    totals[2] += usize::from(fallback.coordinate_copies.is_some());
                }
            }
        }
        totals[0] += usize::from(described);
    }
    let (entity, _) = world
        .iter::<UnityMaterial>()
        .find(|(e, _)| world.get::<Name>(*e).is_some_and(|n| n.0 == material))
        .expect("named real material");
    let pixel = world
        .get::<ShaderBinding>(entity)
        .unwrap()
        .stages
        .iter()
        .find(|s| s.stage == Stage::Pixel)
        .unwrap();
    assert_eq!(world.get::<Name>(pixel.shader).unwrap().0, program);
    let shader = world.get::<Shader>(pixel.shader).unwrap();
    let uv = pixel
        .inputs
        .iter()
        .find(|i| i.label.name == "TEXCOORD" && i.label.index == 0)
        .unwrap();
    let InputRole::PreviousOr { fallback, .. } = &uv.role else {
        panic!("{material}: {uv:?}")
    };
    assert_eq!(
        fallback.value,
        InputFallbackValue::MaterialCoordinates(FallbackCoordinates {
            slot: primase_core::pbr::TextureSlot::BaseColor,
            absent_texture_set: 0,
            absent_stream: [0; 2],
        })
    );
    assert!(shader.interface.inputs.iter().any(|i| i.label == uv.label));
    assert!(fallback.coordinate_copies.is_none());
    if material == "M_SaltRock" {
        for (name, value) in [
            ("NORMAL", InputFallbackValue::Normal),
            (
                "COLOR",
                InputFallbackValue::MaterialVertexColour {
                    absent: [1.0f32.to_bits(); 4],
                },
            ),
        ] {
            let input = pixel
                .inputs
                .iter()
                .find(|i| i.label.name == name && i.label.index == 0)
                .unwrap();
            let InputRole::PreviousOr { fallback, .. } = &input.role else {
                panic!("{input:?}")
            };
            assert_eq!(fallback.value, value);
            assert!(fallback.coordinate_copies.is_none());
        }
        let copied = pixel
            .inputs
            .iter()
            .find(|i| i.label.name == "TEXCOORD" && i.label.index == 3)
            .unwrap();
        let InputRole::PreviousOr { fallback, .. } = &copied.role else {
            panic!("{copied:?}")
        };
        assert_eq!(fallback.value, InputFallbackValue::Literal([0; 4]));
        let copies = fallback.coordinate_copies.as_ref().unwrap();
        assert_eq!(
            copies.coordinates,
            FallbackCoordinates {
                slot: primase_core::pbr::TextureSlot::BaseColor,
                absent_texture_set: 0,
                absent_stream: [0; 2],
            }
        );
        assert_eq!(copies.lanes, [Some(0), Some(1), None, None]);
    }
    println!(
        "{}: bindings/inputs/UV-copy inputs {totals:?}; {material}/{program}: {:?}",
        data.file_name().unwrap().to_string_lossy(),
        pixel.inputs
    );
    assert_eq!(
        totals, expected,
        "all source bindings given replacement descriptions"
    );
}

#[test]
fn skatebirds_real_draw_states_its_replacement_inputs() {
    installed(
        install_file(Game::Skatebird, "SkateBIRD_Data"),
        "level1",
        Some("Fantastic"),
        "Painted Brick Smoother",
        "Standard (Specular setup)-Pixel",
        [198, 211, 0],
    );
}

#[test]
fn sleddings_real_draw_states_its_replacement_inputs() {
    installed(
        install_file(Game::Sledding, "Sledding Game_Data"),
        "level1",
        Some("PC High"),
        "Wood Lighter",
        "Universal Render Pipeline/Lit-Pixel",
        [194, 211, 29],
    );
}

#[test]
fn peaks_real_draw_states_its_replacement_inputs() {
    installed(
        install_file(Game::Peak, "PEAK_Data"),
        "level4",
        None,
        "M_SaltRock",
        "W/Peak_Rock-Pixel",
        [695, 2022, 664],
    );
}
