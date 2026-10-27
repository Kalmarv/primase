//! The forge listing (kinds and names from the tables alone) and the placing of a bare Mesh or LODSelector with its
//! Materials, on Riders Republic's WhiteRoom forges (`installs.riders`). The oracle for the placed mesh is
//! RRAssetStudio's read of it (`oracles.rrassetstudio.riders/meshes`, optional): its vertex and triangle counts. The
//! listing's rows are the hand-checked rows of `installs.riders/DataPC_MAD_WhiteRoom_patch_01.forge` (tests/forge.rs).
use anvil_formats::archive::{Archive, At, Error as ArchiveError};
use anvil_formats::forge::Forge;
use anvil_formats::listing::{KINDS, Kind};
use primase_anvil::components::{AnvilLods, AnvilMesh};
use primase_anvil::read::placements::{Error, InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::{Instance, World};
use primase_testdata::{Game, OracleSet, install, install_file, oracle};
use serde_json::Value;

/// The patch's listing is its tables' rows, in order, each with the kind its type id names: the hand-checked rows 0
/// (a type no reader reads), 1 (LODSelector), 2 (Mesh) and 5 (TextureMap), and every row's id, name and size equal to
/// the tables'.
#[test]
fn a_forge_lists_its_entries_by_kind_and_name() {
    let Some(path) = install_file(Game::Riders, "DataPC_MAD_WhiteRoom_patch_01.forge") else {
        return;
    };
    let f = Forge::open(&path).unwrap();
    let listed: Vec<_> = f.listing().collect();
    assert_eq!(listed.len(), 337);
    for l in &listed {
        assert_eq!(l.id, f.index[l.entry].id);
        assert_eq!(l.size, f.index[l.entry].size);
        assert_eq!(l.name, f.names[l.entry].name);
        assert_eq!(l.type_id, f.names[l.entry].type_id);
    }
    let row = |k: usize| (listed[k].name, listed[k].kind, listed[k].id, listed[k].size);
    assert_eq!(row(0), ("GlobalMetaFile", None, 16, 7281));
    assert_eq!(
        row(1),
        (
            "MAD_PRP_DARKROOM_Stand",
            Some(Kind::LodSelector),
            2_008_284_084_114,
            205
        )
    );
    assert_eq!(
        row(2),
        (
            "MAD_PRP_DARKROOM_Stand_LOD0",
            Some(Kind::Mesh),
            2_008_284_084_116,
            14_047
        )
    );
    assert_eq!(
        row(5),
        (
            "MAD_TEX_TIL_Metal_Painted_A_DiffuseMap_PC_Scarlett_Prospero",
            Some(Kind::TextureMap),
            1_797_335_016_519,
            43_301
        )
    );
    // every kind this forge holds is a kind of the table; a type with no kind is listed with its type id as stored
    let held: std::collections::BTreeSet<Kind> = listed.iter().filter_map(|l| l.kind).collect();
    assert!(held.iter().all(|k| KINDS.contains(k)));
    assert!(held.contains(&Kind::Mesh) && held.contains(&Kind::Material));
    assert!(
        listed
            .iter()
            .filter(|l| l.kind.is_none())
            .all(|l| Kind::of(l.type_id).is_none())
    );
}

/// An archive's listing runs forge by forge in the order given.
#[test]
fn an_archive_lists_its_forges_in_order() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let paths = [
        "DataPC_MAD_WhiteRoom_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ]
    .map(|f| dir.join(f));
    let archive = Archive::open(&paths).unwrap();
    let counts = [337, 388];
    for (forge, n) in counts.into_iter().enumerate() {
        assert_eq!(archive.listing().filter(|(f, _)| *f == forge).count(), n);
    }
    let order: Vec<usize> = archive.listing().map(|(f, _)| f).collect();
    assert!(order.windows(2).all(|w| w[0] <= w[1]));
}

/// The LODSelector `MAD_PRP_DARKROOM_Stand` places once at the origin with its mesh level and the Mesh alone places its
/// one level, each Material read; a TextureMap isn't an object and is refused by type. RRAssetStudio's counts of the
/// mesh equal the placed buffers'.
#[test]
fn a_bare_object_is_placed_with_its_materials() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let paths = [
        "DataPC_MAD_WhiteRoom_patch_01.forge",
        "DataPC_MAD_WhiteRoom.forge",
    ]
    .map(|f| dir.join(f));
    let mut archive = Archive::open(&paths).unwrap();
    let mut w = World::new();
    let mut placed = Placed::default();
    let at = |entry| At { forge: 0, entry };
    let mut placer =
        Placer::new(&mut archive, &mut w, SourceFrame::ENU, InstancedLevels::Every).with_materials();
    let selector = placer
        .place_object(at(1), &mut placed)
        .unwrap()
        .expect("a placement");
    let mesh = placer
        .place_object(at(2), &mut placed)
        .unwrap()
        .expect("a placement");
    let refused = placer.place_object(at(5), &mut placed);
    assert!(
        matches!(refused, Err(Error::Type { found, .. }) if found == Kind::TextureMap.type_id()),
        "{refused:?}"
    );
    let past = placer.place_object(
        At {
            forge: 0,
            entry: 100_000,
        },
        &mut placed,
    );
    assert!(matches!(past, Err(Error::Type { found: 0, .. })), "{past:?}");
    let none = placer.place_object(At { forge: 9, entry: 0 }, &mut placed);
    assert!(
        matches!(none, Err(Error::Archive(ArchiveError::NoForge(9)))),
        "{none:?}"
    );
    assert_eq!(placed.placements, [selector, mesh]);
    let prototype = |p| w.get::<Instance>(p).expect("placed").prototype;
    // the selector's one mesh level is the prototype's mesh and carries no further levels; the mesh alone likewise
    let (a, b) = (prototype(selector), prototype(mesh));
    assert!(w.has::<AnvilMesh>(a) && !w.has::<AnvilLods>(a));
    assert!(w.has::<AnvilMesh>(b) && !w.has::<AnvilLods>(b));
    let m = w.get::<AnvilMesh>(b).unwrap();
    assert_eq!(m.index_id, 2_008_284_084_116);
    assert!(!m.materials.is_empty());
    assert_eq!(m.material_entities.len(), m.materials.len());
    assert!(m.material_entities.iter().any(Option::is_some));
    // the oracle's counts
    let Some(oracle) = oracle(OracleSet::RrAssetStudio, Game::Riders, "meshes") else {
        return;
    };
    let text = std::fs::read_to_string(oracle.join("summary.jsonl")).expect("summary.jsonl");
    let row: Value = text
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("a row"))
        .find(|v| v["forge"] == "DataPC_MAD_WhiteRoom_patch_01.forge" && v["ordinal"] == 2)
        .expect("the oracle's row of the mesh");
    assert_eq!(row["vertex_count"].as_u64(), Some(m.positions.len() as u64));
    assert_eq!(row["triangle_count"].as_u64(), Some(m.triangles.len() as u64));
}
