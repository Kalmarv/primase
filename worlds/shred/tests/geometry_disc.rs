//! Structural census against the game's own tables and AABBs, not a second decoder as semantic oracle.
use primase_core::{Instance, Mesh, frame::SourceFrame};
use primase_shred::read::{level, pak::Pak};
use primase_testdata::{Game, Query, expect_found, install};
use std::path::{Path, PathBuf};

fn archives(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = e.expect("directory entry").path();
        if path.is_dir() {
            archives(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pak")) {
            out.push(path);
        }
    }
}

#[test]
fn every_quartet_and_mesh_including_mixed_embedded() {
    let Some(root) = install(Game::Shred) else {
        return;
    };
    let mut paths = Vec::new();
    archives(&root.join("PS3_GAME/USRDIR"), &mut paths);
    paths.sort();
    expect_found(Query::Install(Game::Shred, None), "PAKs", paths.len());
    let mut totals = [0; 6];
    let mut with_geometry = 0;
    for path in paths {
        let pak = Pak::open(&path).unwrap();
        // Identity here is a stated census frame only: the caller does not assert these are physical metres.
        let (world, r) = level::read_level(
            &[(path.file_name().unwrap().to_str().unwrap(), &pak)],
            SourceFrame::WORLD,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        if r.quartets > 0 {
            with_geometry += 1;
        }
        for (a, b) in totals.iter_mut().zip([
            r.quartets,
            r.meshes,
            r.rigid,
            r.refused,
            r.submeshes,
            r.placements,
        ]) {
            *a += b;
        }
        assert!(
            world.verify_components().is_empty(),
            "{}: {:?}",
            path.display(),
            world.verify_components()
        );
        assert_eq!(world.column::<Mesh>().map_or(0, |c| c.len()), r.rigid);
        assert_eq!(world.column::<Instance>().map_or(0, |c| c.len()), r.placements);
    }
    eprintln!(
        "SHRED geometry census: {with_geometry} PAKs; [quartets, meshes, rigid, refused, submeshes, placements] = {totals:?}; every rigid position checked against source AABB (0.05 source units)"
    );
    assert_eq!(with_geometry, 324);
    assert_eq!(totals, [657, 21_309, 17_897, 3_412, 35_114, 168_887]);
}
