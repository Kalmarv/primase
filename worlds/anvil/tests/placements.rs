//! Real Entities placed into a world through an archive of Riders Republic's forges (searched patch first: a stated
//! order), then their meshes and levels lowered under the pass manager with its checks on. Instance counts and scales
//! as the Riders Republic placement notes give them (docs/placement.md §14).
use primase_anvil::components::{AnvilLods, AnvilMaterialOverrides, AnvilMesh, AnvilMeshShape};
use primase_anvil::passes::{LowerAnvilLods, LowerAnvilMesh, MeshNormals, UvV, UvWords};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Instance, Lods, Mesh, Transform, World};
use primase_testdata::real_data;

/// The cube root of a transform's determinant: its uniform scale where it is one.
fn scale(t: &Transform) -> f64 {
    t.determinant().cbrt()
}

#[test]
fn fixtures_place_and_lower() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(&mut archive, SourceFrame::WORLD, InstancedLevels::Slot);
    // an instanced aspen, 10 instances, slots MMMI-: under Slot, level 0 at each
    placer
        .place(&mut w, At { forge: 1, entry: 2 }, &mut placed)
        .expect("place");
    assert_eq!(placed.placements.len(), 10);
    let prototype = w.get::<Instance>(placed.placements[0]).expect("placed").prototype;
    assert!(
        placed
            .placements
            .iter()
            .all(|&p| w.get::<Instance>(p).map(|i| i.prototype) == Some(prototype))
    );
    // level 0's mesh, its slot holding the matrices; levels 1 and 2 (slots holding none) not placed
    assert!(w.has::<AnvilMesh>(prototype) && !w.has::<AnvilLods>(prototype));
    assert_eq!((placed.impostor_levels, placed.levels_unplaced), (1, 2));
    let scales: Vec<f64> = placed.placements[..3]
        .iter()
        .map(|&p| scale(w.get::<Transform>(p).expect("transform")))
        .collect();
    for (found, noted) in scales.iter().zip([1.13, 1.03, 1.15]) {
        assert!((found - noted).abs() < 0.01, "{scales:?}");
    }
    // the aspen's RigidBody: an instanced Entity's collision isn't placed
    assert_eq!((placed.collisions.len(), placed.instanced_collision), (0, 1));
    // plastic cups: a RigidBody whose shape is a MeshShape, placed once at the Entity's transform
    let before = placed.placements.len();
    placer
        .place(
            &mut w,
            At {
                forge: 1,
                entry: 47_242,
            },
            &mut placed,
        )
        .expect("place");
    let cups = placed.placements[before];
    let collision = *placed.collisions.last().expect("collision");
    let shape = w.get::<Instance>(collision).expect("placed").prototype;
    assert!(w.has::<AnvilMeshShape>(shape));
    assert_eq!(w.get::<Transform>(collision), w.get::<Transform>(cups));
    // a plain decal, scale 2.2497, one placement
    let before = placed.placements.len();
    placer
        .place(
            &mut w,
            At {
                forge: 1,
                entry: 50_890,
            },
            &mut placed,
        )
        .expect("place");
    assert_eq!(placed.placements.len(), before + 1);
    let decal = scale(w.get::<Transform>(placed.placements[before]).expect("transform"));
    assert!((decal - 2.2497).abs() < 1e-3, "{decal}");
    // a flag, not instanced, slots MMM--, one material-info entry per slot
    let before = placed.placements.len();
    placer
        .place(
            &mut w,
            At {
                forge: 1,
                entry: 354_769,
            },
            &mut placed,
        )
        .expect("place");
    assert_eq!(placed.placements.len(), before + 1);
    let flag = placed.placements[before];
    let whole = w.get::<Instance>(flag).expect("placed").prototype;
    let lods = w.get::<AnvilLods>(whole).expect("levels").clone();
    assert_eq!(
        (lods.detail, lods.forms.iter().map(|f| f.0).collect::<Vec<_>>()),
        (0, vec![1, 2])
    );
    let overrides = w.get::<AnvilMaterialOverrides>(flag).expect("overrides");
    assert_eq!(
        overrides
            .levels
            .iter()
            .map(|(l, p)| (*l, p.len()))
            .collect::<Vec<_>>(),
        vec![(0, 1), (1, 1), (2, 1)]
    );
    // the meshes lowered, then the levels
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let pipeline = Pipeline {
        passes: vec![
            Box::new(LowerAnvilMesh {
                uv_words: UvWords::Signed,
                uv_divisor: 2048.0,
                uv_v: UvV::AsStored,
                normals: MeshNormals::Packed127,
            }),
            Box::new(LowerAnvilLods),
        ],
    };
    let mut report = RunReport::default();
    manager.run(&mut w, &pipeline, &mut report).expect("lower");
    assert!(w.has::<Mesh>(prototype) && !w.has::<AnvilMesh>(prototype));
    assert!(w.has::<Mesh>(whole) && !w.has::<AnvilLods>(whole));
    let lods = w.get::<Lods>(whole).expect("core levels");
    assert_eq!((lods.detail, lods.forms.len(), lods.default), (0, 2, None));

    // under Selector, the aspen's every mesh level at each instance
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(&mut archive, SourceFrame::WORLD, InstancedLevels::Selector);
    placer
        .place(&mut w, At { forge: 1, entry: 2 }, &mut placed)
        .expect("place");
    assert_eq!((placed.placements.len(), placed.levels_unplaced), (10, 0));
    let prototype = w.get::<Instance>(placed.placements[0]).expect("placed").prototype;
    let lods = w.get::<AnvilLods>(prototype).expect("levels");
    assert_eq!(lods.forms.iter().map(|f| f.0).collect::<Vec<_>>(), vec![1, 2]);
}

/// The entry named `name` in the archive's forge `forge`.
fn named(archive: &Archive, forge: usize, name: &str) -> At {
    let names = &archive.forge(forge).expect("forge").names;
    let entry = names.iter().position(|n| n.name == name).expect(name);
    At { forge, entry }
}

#[test]
fn groups_place_their_own_visuals_and_report_members() {
    let Some(dir) = real_data("PRIMASE_ANVIL_DIR") else {
        return;
    };
    // a USNAT group of instanced signs: Sign_NoEntry x 2 and Sign_Parkingtime_B x 1, no members (docs/world.md §7)
    let order = ["DataPC_MAD_USNAT.forge", "DataPC.forge"];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let at = named(&archive, 0, "MAD_GRP_CITYSCHOOL_Parking_Campus_A_016_Short");
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(&mut archive, SourceFrame::WORLD, InstancedLevels::Selector);
    placer.place(&mut w, at, &mut placed).expect("place");
    assert_eq!((placed.placements.len(), placed.members.len()), (3, 0));
    // its RigidBody's shape, a ReferenceListShape: counted, not read
    assert_eq!(placed.shapes_unread.get(&0x2D67_5BA2), Some(&1));
    // WhiteRoom's light group: no components, three member Entities listed by no cell (docs/placement.md §5)
    let mut archive = Archive::open(&[dir.join("DataPC_MAD_WhiteRoom.forge")]).expect("archive");
    let at = named(&archive, 0, "Light_Spot_CharaGroup");
    let mut placed = Placed::default();
    let mut placer = Placer::new(&mut archive, SourceFrame::WORLD, InstancedLevels::Selector);
    placer.place(&mut w, at, &mut placed).expect("place");
    assert_eq!((placed.placements.len(), placed.members.len()), (0, 3));
}
