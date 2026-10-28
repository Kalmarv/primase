//! Players whose files are in UnityFS bundles, read through `Player` like a loose data folder: Big Walk's `data.unity3d`
//! (the player's own files in one bundle, `installs.bigwalk/Big Walk_Data`), Sledding Game's Addressables bundles beside
//! its loose files (`installs.sledding/Sledding Game_Data/StreamingAssets/aa/StandaloneWindows64`) and one level of
//! PEAK's AssetBundles with the loose files of its player (`installs.peak`), each read with `tools.unity_tpk`.
//!
//! A bundled file is named like a loose one and its objects are read from the bundle by range. The scene list of Big
//! Walk's `globalgamemanagers` lists the scenes of its `levelN` files, one of its levels reads into the world, every
//! bundled Texture2D of Sledding Game reads with its pixels from its bundle's stream nodes, and one PEAK level's scene,
//! meshes and textures read from its bundle.

#![allow(clippy::unwrap_used)]

mod inputs;

use std::path::PathBuf;

use inputs::{Game, Tool, install_file, tool};
use primase_core::World;
use primase_unity::level::read_level;
use unity_formats::TypeTreePackage;
use unity_formats::listing::list;
use unity_formats::mesh::UnityMeshData;
use unity_formats::player::{BundleSpec, NodeSelection, Player};
use unity_formats::scene::Scene;
use unity_formats::texture::UnityTexture;

fn package() -> Option<TypeTreePackage> {
    let tpk = tool(Tool::UnityTpk)?;
    Some(TypeTreePackage::parse(&std::fs::read(tpk).unwrap()).unwrap())
}

/// Big Walk: the bundle is the player. Its twelve serialized files are named like the folder's would be, the build
/// scenes are `level0` to `level3`, and the smallest scene reads into the world.
#[test]
fn a_player_in_one_bundle_reads_like_a_folder() {
    let (Some(data), Some(package)) = (install_file(Game::Bigwalk, "Big Walk_Data"), package()) else {
        return;
    };
    let bundle = data.join("data.unity3d");
    let player = Player::open_with_bundles(
        &data,
        package,
        &[BundleSpec {
            path: &bundle,
            nodes: NodeSelection::All,
        }],
    )
    .unwrap();
    let names: Vec<&str> = player.files.keys().map(String::as_str).collect();
    eprintln!("Big Walk: {} files: {names:?}", names.len());
    // the bundle's twelve files and the loose `Resources/unity default resources` beside it
    assert_eq!(player.files.len(), 13, "{names:?}");
    assert_eq!(player.files.values().filter(|f| f.bundle().is_some()).count(), 12);
    for want in [
        "globalgamemanagers",
        "level0",
        "level1",
        "level2",
        "level3",
        "resources.assets",
    ] {
        assert!(player.files.contains_key(want), "{want} in {names:?}");
    }
    let scenes = player.scenes().unwrap();
    assert_eq!(
        scenes.iter().map(|s| s.file.as_str()).collect::<Vec<_>>(),
        ["level0", "level1", "level2", "level3"]
    );
    // the objects of a bundled file decode from the bundle: the platform file's first scene is named
    assert!(scenes.iter().all(|s| !s.name.is_empty()), "{scenes:?}");
    let mut w = World::new();
    let report = read_level(&player, "level1", &mut w).unwrap();
    eprintln!(
        "Big Walk level1: {} renderers, {} materials, {} textures",
        report.renderers, report.materials, report.textures
    );
}

/// Sledding Game: the Addressables bundles hold files that point into each other (`archive:/CAB-…/CAB-…`), the loose
/// files are beside them. Every bundle read whole gives 50 serialized files with their stored trees, and an object of
/// one decodes.
#[test]
fn addressables_bundles_beside_the_loose_files() {
    let (Some(data), Some(package)) = (install_file(Game::Sledding, "Sledding Game_Data"), package()) else {
        return;
    };
    let dir = data.join("StreamingAssets/aa/StandaloneWindows64");
    let mut bundles: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "bundle"))
        .collect();
    bundles.sort();
    assert_eq!(bundles.len(), 50);
    let specs: Vec<BundleSpec> = bundles
        .iter()
        .map(|path| BundleSpec {
            path,
            nodes: NodeSelection::All,
        })
        .collect();
    let player = Player::open_with_bundles(&data, package, &specs).unwrap();
    let bundled = player.files.values().filter(|f| f.bundle().is_some()).count();
    assert_eq!(bundled, 50);
    assert_eq!(
        player.files.len() - bundled,
        9,
        "the loose files: seven in the data folder, two builtin ones in Resources"
    );
    assert_eq!(player.bundles.len(), 50);
    // a stored tree decodes the object it belongs to; the bundles' textures read their streamed pixels from the stream
    // nodes of their own bundles
    let mut read = 0;
    for (name, f) in &player.files {
        if f.bundle().is_none() {
            continue;
        }
        for o in &f.objects {
            let r = unity_formats::player::ObjRef {
                file: name.clone(),
                path_id: o.path_id,
            };
            if player.class_name(&r) == Some("Texture2D") {
                let t = UnityTexture::read(&player, &r).unwrap();
                assert!(!t.data.is_empty(), "{name}/{}", o.path_id);
                read += 1;
            }
        }
    }
    eprintln!("Sledding Game: {read} bundled textures read");
    assert!(read > 0);
}

/// PEAK: one level of `levels192-207.assetbundle` with the files its pointers name (its shared assets and other levels'
/// in the bundle), not the bundle's other levels, beside the loose files of the player. The scene's hierarchy and
/// renderers read from the bundle, and meshes and textures of its shared assets read with their data. (`read_level`
/// itself stops on this level at an AnimatorController of PEAK's, which this reader does not read: not a bundle matter.)
#[test]
fn one_level_of_peaks_bundle_reads() {
    let (Some(data), Some(package)) = (install_file(Game::Peak, "PEAK_Data"), package()) else {
        return;
    };
    let bundle = data.join("StreamingAssets/maps/levels192-207.assetbundle");
    let level = "BuildPlayer-Level_207";
    let player = Player::open_with_bundles(
        &data,
        package,
        &[BundleSpec {
            path: &bundle,
            nodes: NodeSelection::Closure(&[level]),
        }],
    )
    .unwrap();
    let bundled: Vec<&str> = player
        .files
        .iter()
        .filter(|(_, f)| f.bundle().is_some())
        .map(|(n, _)| n.as_str())
        .collect();
    eprintln!("PEAK {level}: {bundled:?}");
    assert!(bundled.contains(&"buildplayer-level_207"));
    assert!(bundled.contains(&"buildplayer-level_207.sharedassets"));
    assert!(
        bundled.len() < 8,
        "a level and what it points at, not the bundle: {bundled:?}"
    );
    let scene = Scene::read(&player, "buildplayer-level_207").unwrap();
    let (renderers, skipped) = scene.renderers(&player).unwrap();
    eprintln!("PEAK {level}: {} renderers, skipped {skipped:?}", renderers.len());
    assert!(!renderers.is_empty());
    let meshes = list(&player, Some(&["Mesh"])).unwrap();
    assert!(!meshes.entries.is_empty());
    for e in meshes.entries.iter().take(5) {
        let m = UnityMeshData::read(&player, &e.object).unwrap();
        assert!(m.vertex_count > 0, "{}", m.name);
    }
    let textures = list(&player, Some(&["Texture2D"])).unwrap();
    assert!(!textures.entries.is_empty());
    for e in textures.entries.iter().take(5) {
        let t = UnityTexture::read(&player, &e.object).unwrap();
        assert!(!t.data.is_empty(), "{}", t.name);
    }
    eprintln!(
        "PEAK {level}: {} meshes and {} textures in its files",
        meshes.entries.len(),
        textures.entries.len()
    );
}
