//! State reading retains alternatives; only an explicitly run pass selects one.
mod common;
mod inputs;
use fromsoft_formats::{Game, Keys, dcx::Decoder};
use primase_core::{
    Resource, World,
    pass::{Pass, PassManager, PassRegistry, Pipeline, RunReport},
    state::StateAssignment,
};
use primase_fromsoft::{
    game::ParamKey,
    sekiro::{
        self,
        passes::state::ResolveFromSoftState,
        source::{DrawParam, ResolvedState, StateInputs, StateReadScope},
    },
};
use std::collections::BTreeMap;

#[test]
fn the_common_state_scope_keeps_every_candidate_before_the_pass_selects()
-> Result<(), Box<dyn std::error::Error>> {
    let (Some(root), Some(keys), Some(dll)) = (
        inputs::install(inputs::Game::Sekiro),
        inputs::keys(inputs::Game::Sekiro),
        inputs::oodle(inputs::Tool::Oodle6),
    ) else {
        return Ok(());
    };
    let install = common::install(
        root,
        Game::Sekiro,
        Keys::from_folder(keys, Game::Sekiro, Game::Sekiro.param_key_sha256())?,
    );
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    let archive = install.open()?;
    let expected = fromsoft_formats::area::draw_params(&archive, Game::Sekiro, "m11_01_00_00");
    let mut world = World::new();
    sekiro::read_game_area(
        &install,
        "m11_01_00_00",
        &decoder,
        &sekiro::Options {
            state_inputs: StateReadScope {
                weather: false,
                common: true,
            },
            draw_param: sekiro::DrawParams::File("m11_01_0000".into()),
            region: Some(sekiro::Region {
                rect: [1_000_000., 1_000_000., 1_000_001., 1_000_001.],
                margin: 0.,
            }),
            ..Default::default()
        },
        &mut world,
    )?;
    let source = world.resource::<StateInputs>().expect("requested raw scope");
    assert_eq!(source.draw_params.keys().cloned().collect::<Vec<_>>(), expected);
    assert!(source.common.is_some());
    assert!(world.resource::<StateAssignment>().is_none());
    assert!(world.resource::<ResolvedState>().is_none());
    assert_eq!(world.resource::<DrawParam>().unwrap().file, "m11_01_0000");
    eprintln!(
        "Sekiro m11_01 state candidates: {} GPARAMs, {} decoded bytes, {} retained DrawParam JSON bytes, {} common-script bytes",
        source.draw_params.len(),
        source.draw_param_bytes,
        source.draw_param_json_bytes,
        source.common.as_ref().unwrap().script_bytes
    );
    // Eager retention is intentional for this small data family. This budget guards that decision.
    assert!(source.draw_param_bytes < 8 * 1024 * 1024);
    assert!(source.draw_param_json_bytes < 8 * 1024 * 1024);
    let before = source.to_json()?;
    let input = sekiro::world_state::FlagSet {
        name: "synthetic map override".into(),
        source: "script semantics, not a captured flag set".into(),
        flags: BTreeMap::from([
            (8302, true),
            (8305, false),
            (8306, false),
            (9800, true),
            (9801, true),
            (9802, false),
            (9803, false),
        ]),
    };
    let pass = ResolveFromSoftState::new(None, Some(input))?;
    let options = pass.options();
    let mut registry = PassRegistry::new();
    sekiro::passes::register(&mut registry);
    let pipeline = Pipeline {
        passes: vec![Box::new(pass)],
    };
    let pipeline = registry.parse(&pipeline.to_string())?;
    assert_eq!(pipeline.passes[0].options(), options);
    let manager = PassManager::new(registry);
    let mut report = RunReport::default();
    manager.run(&mut world, &pipeline, &mut report)?;
    assert_eq!(world.resource::<DrawParam>().unwrap().file, "m11_01_0020");
    assert_eq!(world.resource::<StateInputs>().unwrap().to_json()?, before);
    let state = world
        .resource::<ResolvedState>()
        .unwrap()
        .world_state
        .as_ref()
        .unwrap();
    assert_eq!(state.selection.envmap, Some(4));
    assert_eq!(state.selection.gparam_sub_ids.get("m11_01"), Some(&20));
    assert_eq!(
        report.steps[0].log.details["common event state"],
        [primase_core::json::to_value(state)?]
    );
    // An empty plan is different from no sky read: preserve the original named refusal.
    world.resource_mut::<StateInputs>().unwrap().sky_values = Some(Vec::new());
    let error = manager
        .run(&mut world, &pipeline, &mut RunReport::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("enabled dome membership selects 0 sky values")
    );
    world.resource_mut::<StateInputs>().unwrap().sky_values = None;
    world.remove_resource::<DrawParam>();
    let error = manager
        .run(&mut world, &pipeline, &mut RunReport::default())
        .unwrap_err();
    assert!(error.to_string().contains("base draw-param file is unresolved"));
    // A program that did not request GPARAMs still resolves its flag state.
    world.resource_mut::<StateInputs>().unwrap().draw_param_requested = false;
    manager.run(&mut world, &pipeline, &mut RunReport::default())?;
    assert!(world.resource::<DrawParam>().is_none());
    Ok(())
}
