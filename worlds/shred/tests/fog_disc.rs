//! The ten levels' own `change_fog` calls through `lower-shred-fog`: each level gets linear fog whose distances and
//! colour are the script's literals.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use primase_core::pass::{PassManager, PassRegistry, RunReport};
use primase_core::{Atmosphere, FogFalloff, World};
use primase_shred::read::{
    level_params::{ScriptInputs, read_level_params},
    pak::Pak,
};
use primase_testdata::{Game, OracleSet, oracle};

const LEVELS: [&str; 10] = [
    "charlie", "delta", "echo", "foxtrot", "india", "juliet", "mike", "november", "oscar", "papa",
];

#[test]
fn every_level_gets_its_scripts_linear_fog() {
    let Some(path) = oracle(OracleSet::Corpus, Game::Shred, "scripts.pak") else {
        return;
    };
    let scripts = Pak::open(path).unwrap();
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    let pipeline = registry.parse("lower-shred-fog{metres-per-unit=1}").unwrap();
    for level in LEVELS {
        let (params, _) = read_level_params(&ScriptInputs {
            scripts: &scripts,
            level,
        })
        .unwrap();
        let params = params.unwrap();
        let args: Vec<f64> = params
            .calls_to("change_fog")
            .next()
            .unwrap()
            .args
            .iter()
            .map(|a| match a {
                primase_shred::components::ScriptValue::Number(n) => *n,
                other => panic!("{other:?}"),
            })
            .collect();
        let mut w = World::new();
        w.insert_resource(params);
        let mut report = RunReport::default();
        PassManager::new(registry_clone())
            .with_pass_checks(true)
            .run(&mut w, &pipeline, &mut report)
            .unwrap_or_else(|e| panic!("{level}: {e}"));
        let fog = w.resource::<Atmosphere>().unwrap().fog.clone().unwrap();
        assert_eq!(
            fog.falloff,
            FogFalloff::Linear {
                start: args[0] as f32,
                end: args[1] as f32
            },
            "{level}"
        );
        assert_eq!(
            fog.colour,
            [args[4] as f32, args[5] as f32, args[6] as f32],
            "{level}"
        );
    }
}

fn registry_clone() -> PassRegistry {
    let mut registry = PassRegistry::new();
    primase_shred::register(&mut registry);
    registry
}
