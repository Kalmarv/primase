//! The ten levels' `vfx_level_load_<level>` scripts of the decrypted scripts archive: every call has literal
//! arguments and is read, each script sets fog exactly once, and `ShredReader` puts MIKE's parameters in its world.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod inputs;

use primase_core::{convert::Reader, frame::SourceFrame};
use primase_shred::{components::LevelParams, read::convert::ShredReader};
use inputs::{Game, OracleSet, install, oracle};
use rme_formats::{
    level_params::{ScriptInputs, ScriptValue, read_level_params},
    pak::Pak,
};

const LEVELS: [&str; 10] = [
    "charlie", "delta", "echo", "foxtrot", "india", "juliet", "mike", "november", "oscar", "papa",
];

fn numbers(args: &[ScriptValue]) -> Vec<f64> {
    args.iter()
        .map(|a| match a {
            ScriptValue::Number(n) => *n,
            other => panic!("{other:?} is not a number"),
        })
        .collect()
}

#[test]
fn every_level_script_reads_whole_and_sets_fog_once() {
    let Some(path) = oracle(OracleSet::Corpus, Game::Shred, "scripts.pak") else {
        return;
    };
    let scripts = Pak::open(path).unwrap();
    let lua = scripts
        .entries()
        .iter()
        .filter(|e| e.name.starts_with(b"script\\") && e.name.ends_with(b".lua"))
        .count();
    assert_eq!(lua, 1278);
    for level in LEVELS {
        let (params, report) = read_level_params(&ScriptInputs {
            scripts: &scripts,
            level,
        })
        .unwrap();
        let params = params.unwrap_or_else(|| panic!("{level}: {:?}", report));
        assert!(report.unread.is_empty(), "{level}: {:?}", report.unread);
        assert_eq!(report.read, params.calls.len());
        assert!(!params.calls.is_empty());
        for function in ["change_fog", "set_back_light", "enable_fog"] {
            assert_eq!(params.calls_to(function).count(), 1, "{level} {function}");
        }
        assert_eq!(
            params.calls_to("change_fog").next().unwrap().args.len(),
            9,
            "{level}"
        );
    }
    // the literals of two scripts, as the scripts write them
    let read = |level| {
        read_level_params(&ScriptInputs {
            scripts: &scripts,
            level,
        })
        .unwrap()
        .0
        .unwrap()
    };
    let fog =
        |p: &rme_formats::level_params::LevelParams| numbers(&p.calls_to("change_fog").next().unwrap().args);
    assert_eq!(
        fog(&read("charlie")),
        [
            400.0, 700.0, -10.0, 400.0, 0.9960784, 0.6156863, 0.5058824, 0.4156863, 1.0
        ]
    );
    assert_eq!(
        fog(&read("mike")),
        [
            30.0,
            600.0,
            -650.0,
            150.0,
            0.003921569,
            0.2784314,
            0.4117647,
            0.5215687,
            0.0
        ]
    );
    assert_eq!(
        numbers(&read("charlie").calls_to("set_back_light").next().unwrap().args),
        [0.2431373, 0.4705882, 0.5921569, 0.1882353]
    );
}

#[test]
fn the_reader_gives_mikes_world_its_parameters() {
    let (Some(root), Some(path)) = (
        install(Game::Shred),
        oracle(OracleSet::Corpus, Game::Shred, "scripts.pak"),
    ) else {
        return;
    };
    let scripts = Pak::open(path).unwrap();
    let pak = Pak::open(root.join("PS3_GAME/USRDIR/levels/mike.pak")).unwrap();
    let mut reader = ShredReader::new(vec![("MIKE", &pak)], SourceFrame::WORLD).with_scripts(ScriptInputs {
        scripts: &scripts,
        level: "mike",
    });
    let (world, lines) = reader.read().unwrap();
    let params = world.resource::<LevelParams>().unwrap();
    assert_eq!(params.script, "script\\vfx_level_load_mike.lua");
    assert_eq!(reader.params.read, params.calls.len());
    assert!(lines.iter().any(|(k, _)| k == "shred level params"));
    assert!(world.verify_resources().is_empty());
}
