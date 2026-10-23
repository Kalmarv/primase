//! The object listing and the build's scene list against an independent reader (UnityPy 1.25.4's `peek_name` and its
//! decoding of `BuildSettings`): a JSON file `{scenes, objects}` with `scenes` the strings of the BuildSettings object
//! of `globalgamemanagers` (null for a player without one) and `objects` each `file` (relative path), `path_id`,
//! `class` (the class name) and `name` (`peek_name()`: the object's `m_Name` as UnityPy reads it from the first bytes,
//! null for a class without one). Two players: Sledding Game (`installs.sledding/Sledding Game_Data`,
//! `oracles.unitypy.sledding/listing.json`) and SkateBIRD (`installs.skatebird/SkateBIRD_Data`,
//! `oracles.unitypy.skatebird/listing.json`), each read with `tools.unity_tpk`.
//!
//! Every object of every file is listed exactly once, with the class UnityPy names it by, and its name equal to
//! UnityPy's: both read the string first in the object's bytes (a class without an `m_Name` has none in both). The
//! scene list is UnityPy's, each scene in the file `level<index>`.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use primase_core::gate::{GateReport, Tolerance};
use primase_testdata::{Game, OracleSet, Tool, install_file, oracle, tool};
use primase_unity::TypeTreePackage;
use primase_unity::listing::list;
use primase_unity::player::Player;
use serde_json::Value as Json;

/// Class names the oracle's table gives otherwise than the player's own Unity version does: classes Unity renamed or
/// added after UnityPy 1.25.4's class table (ours is the type tree package's for the player's version), by (ours,
/// the oracle's). Counted in the report, and any other difference fails.
const RENAMED: [(&str, &str); 3] = [
    ("PhysicsMaterial", "PhysicMaterial"),
    ("ShaderNameRegistry", "ScriptMapper"),
    ("MultiplayerManager", "Unknown (655991488)"),
];

fn check(data: PathBuf, tpk: PathBuf, oracle: PathBuf) {
    let player = Player::open(
        &data,
        TypeTreePackage::parse(&std::fs::read(&tpk).unwrap()).unwrap(),
    )
    .unwrap();
    let oracle: Json = serde_json::from_slice(&std::fs::read(&oracle).unwrap()).unwrap();
    // the oracle's rows by (file name as the player keys it, path id)
    let mut theirs: BTreeMap<(String, i64), (String, Option<String>)> = BTreeMap::new();
    for o in oracle["objects"].as_array().unwrap() {
        let file = o["file"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .to_lowercase();
        let key = (file, o["path_id"].as_i64().unwrap());
        let row = (
            o["class"].as_str().unwrap().to_owned(),
            o["name"].as_str().map(str::to_owned),
        );
        assert!(
            theirs.insert(key, row).is_none(),
            "the oracle lists an object twice"
        );
    }
    let listing = list(&player, None).unwrap();
    let mut ids = GateReport::new("every object listed once, as the oracle's", Tolerance::Exact);
    let mut classes = GateReport::new("class names", Tolerance::Exact);
    let mut names = GateReport::new("names", Tolerance::Exact);
    let (mut unknown_class, mut renamed) = (0u64, 0u64);
    let mut seen = 0;
    for e in &listing.entries {
        let key = (e.object.file.clone(), e.object.path_id);
        let Some((class, name)) = theirs.get(&key) else {
            ids.fail(format!("{}/{}", key.0, key.1), "not in the oracle");
            continue;
        };
        seen += 1;
        ids.record(format!("{}/{}", key.0, key.1), 0.0);
        let item = format!("{}/{} {class}", key.0, key.1);
        match &e.class {
            Some(c) if c == class => classes.record(&item, 0.0),
            Some(c) if RENAMED.contains(&(c.as_str(), class.as_str())) => {
                renamed += 1;
                classes.record(&item, 0.0);
            }
            Some(c) => classes.fail(&item, format!("ours {c}")),
            // a class the package lacks for this Unity version: nothing to compare, counted
            None => unknown_class += 1,
        }
        if e.class.is_some() {
            if &e.name == name {
                names.record(&item, 0.0);
            } else {
                names.fail(&item, format!("ours {:?} vs {name:?}", e.name));
            }
        }
    }
    if seen != theirs.len() {
        ids.fail(
            "count",
            format!("{seen} listed of {} in the oracle", theirs.len()),
        );
    }
    println!(
        "{ids}; {classes}; {names}; {unknown_class} objects of a class the package lacks, {renamed} of a class the oracle's table names otherwise; unnamed by reason {:?}",
        listing.unnamed
    );
    ids.assert_passed();
    classes.assert_passed();
    names.assert_passed();

    let mut scenes = GateReport::new("the build's scenes", Tolerance::Exact);
    let ours = player.scenes().unwrap();
    let theirs: Vec<&str> = oracle["scenes"]
        .as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap()).collect())
        .unwrap_or_default();
    if ours.len() != theirs.len() {
        scenes.fail("count", format!("{} scenes of {}", ours.len(), theirs.len()));
    }
    for (s, path) in ours.iter().zip(&theirs) {
        let item = format!("scene {}", s.index);
        if s.path == *path
            && s.file == format!("level{}", s.index)
            && path.ends_with(&format!("{}.unity", s.name))
        {
            scenes.record(&item, 0.0);
        } else {
            scenes.fail(&item, format!("{s:?} vs {path}"));
        }
    }
    println!(
        "{scenes}: {:?}",
        ours.iter().map(|s| (&s.file, &s.name)).collect::<Vec<_>>()
    );
    scenes.assert_passed();
}

#[test]
fn sledding_listing_and_scenes_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        install_file(Game::Sledding, "Sledding Game_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Sledding, "listing.json"),
    ) else {
        return;
    };
    check(data, tpk, oracle);
}

#[test]
fn skatebird_listing_and_scenes_match_the_oracle() {
    let (Some(data), Some(tpk), Some(oracle)) = (
        install_file(Game::Skatebird, "SkateBIRD_Data"),
        tool(Tool::UnityTpk),
        oracle(OracleSet::UnityPy, Game::Skatebird, "listing.json"),
    ) else {
        return;
    };
    check(data, tpk, oracle);
}
