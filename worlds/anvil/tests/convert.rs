//! The Anvil section reader ([`AnvilReader`]) on Riders Republic's forges (`installs.riders`): the cells a rectangle
//! or a list selects, the objects they list (each cell's `GridCellDataBlock` count, as hand-counted in the notes:
//! docs/placement.md §5 for WhiteRoom, docs/skate-plan.md §0 for the CITYSCHOOL campus), the marks of where each was
//! listed, the reader's namespaces and the load step. `select-anvil-listed` is tested on a world laid out by hand.
use std::collections::BTreeSet;

use primase_anvil::components::{
    AnvilBounds, AnvilCollisionFilter, AnvilListed, AnvilMesh, AnvilTerrain, ListedBy,
};
use primase_anvil::passes::{Coarse, SelectAnvilListed};
use primase_anvil::read::convert::{
    AnvilOptions, AnvilReader, CellSelection, Edition, ForgeFile, GridSelection, Levels, TerrainSelection,
};
use primase_anvil::read::placements::{InstancedCollision, InstancedLevels};
use primase_core::convert::{Reader, check_emits};
use primase_core::frame::SourceFrame;
use primase_core::pass::{Pass, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Entity, Instance, Name, Transform, World};
use primase_testdata::{Game, install};

fn whiteroom(dir: &std::path::Path) -> AnvilOptions {
    AnvilOptions {
        forges: vec![
            ForgeFile {
                path: dir.join("DataPC_MAD_WhiteRoom_patch_01.forge"),
                edition: Edition::Patch,
            },
            ForgeFile {
                path: dir.join("DataPC_MAD_WhiteRoom.forge"),
                edition: Edition::Base,
            },
        ],
        frame: SourceFrame::ENU,
        grids: vec![GridSelection {
            grid: "MAD_WhiteRoom".into(),
            cells: CellSelection::Numbered(vec![20, 27, 28, 35, 36]),
        }],
        instanced_levels: InstancedLevels::Every,
        instanced_collision: InstancedCollision::PerInstance,
        terrain: None,
    }
}

fn report<'a>(r: &'a [(String, String)], label: &str) -> &'a str {
    r.iter()
        .find(|(l, _)| l == label)
        .map(|(_, t)| t.as_str())
        .expect(label)
}

/// WhiteRoom's cells 20, 27, 28, 35 and 36 list 9, 5, 4, 8 and 24 objects (the block counts of
/// docs/placement.md §5); AvatarCoordinator is listed by cells 20, 28 and 36; the placements carry their marks, every
/// mark's owner is an id the listing reached and the world holds only the reader's namespaces.
#[test]
fn a_numbered_selection_reads_the_cells_the_notes_count() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut reader = AnvilReader::open(&whiteroom(&dir)).unwrap();
    let (world, lines) = reader.read().unwrap();
    assert_eq!(
        report(&lines, "anvil cells"),
        "MAD_WhiteRoom cell 20 (level 0): 9 objects; MAD_WhiteRoom cell 27 (level 0): 5 objects; \
         MAD_WhiteRoom cell 28 (level 0): 4 objects; MAD_WhiteRoom cell 35 (level 0): 8 objects; \
         MAD_WhiteRoom cell 36 (level 0): 24 objects"
    );
    check_emits(&world, reader.emits(), "read").unwrap();
    let marked: Vec<(Entity, &AnvilListed)> = world.iter::<AnvilListed>().collect();
    assert!(!marked.is_empty());
    // every placement is marked, and every mark says level 0 (a Numbered selection of level-0 cells)
    for (e, _) in world.iter::<Instance>() {
        assert!(world.has::<AnvilListed>(e), "{e:?} placed without its mark");
    }
    assert!(marked.iter().all(|(_, l)| l.reached_from == 0));
    let avatar: BTreeSet<u32> = marked
        .iter()
        .filter(|(e, _)| world.get::<Name>(*e).is_some_and(|n| n.0 == "AvatarCoordinator"))
        .flat_map(|(_, l)| l.cells.iter().map(|c| c.cell))
        .collect();
    assert!(
        avatar.is_empty() || avatar == BTreeSet::from([20, 28, 36]),
        "{avatar:?}"
    );
    // the world holds no Materials until a part is loaded
    assert!(
        world
            .iter::<AnvilMesh>()
            .all(|(_, m)| m.material_entities.is_empty())
    );
    // loading every root reads the Materials, once each, linked from the ranges
    let roots: BTreeSet<Entity> = world.entities().collect();
    let (part, _) = reader.load(&world, &roots).unwrap();
    check_emits(&part, reader.emits(), "load").unwrap();
    let linked = part
        .iter::<AnvilMesh>()
        .filter(|(_, m)| m.material_entities.iter().any(Option::is_some))
        .count();
    assert!(linked > 0);
    assert!(
        part.iter::<AnvilMesh>()
            .all(|(_, m)| m.material_entities.len() == m.materials.len())
    );
}

/// A reader with no forge, no grid or no World is refused by name.
#[test]
fn a_selection_that_cannot_be_read_is_refused() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut o = whiteroom(&dir);
    o.grids.clear();
    assert!(
        AnvilReader::open(&o)
            .unwrap_err()
            .to_string()
            .contains("no grids")
    );
    let mut o = whiteroom(&dir);
    o.forges.clear();
    assert!(
        AnvilReader::open(&o)
            .unwrap_err()
            .to_string()
            .contains("no forges")
    );
    let mut o = whiteroom(&dir);
    o.grids[0].grid = "NoSuchGrid".into();
    let mut reader = AnvilReader::open(&o).unwrap();
    assert!(
        reader
            .read()
            .unwrap_err()
            .to_string()
            .contains("no GridPartition named NoSuchGrid")
    );
}

/// The CITYSCHOOL campus square (cell 9380 of ShortRange and of TerrainWorldSmall, 128 m a side at x -3584, y 1152):
/// ShortRange's level-0 cell lists 244 objects and TerrainWorldSmall's 452 (docs/skate-plan.md §0). Reading the other
/// levels adds the coarser cells that overlap it, whose objects are marked with the finest level that reaches them; the
/// terrain under the square is read. The cityschool read as a whole is slow, so one test covers it.
#[test]
fn a_rectangle_selects_its_cells_at_the_levels_stated() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let rect = [-3584.0, 1152.0, -3456.0, 1280.0];
    let usnat = |levels: Levels, terrain: Option<TerrainSelection>| AnvilOptions {
        forges: [
            ("DataPC_MAD_USNAT_patch_01.forge", Edition::Patch),
            ("DataPC_MAD_USNAT.forge", Edition::Base),
            ("DataPC_patch_01.forge", Edition::Patch),
            ("DataPC.forge", Edition::Base),
        ]
        .map(|(f, edition)| ForgeFile {
            path: dir.join(f),
            edition,
        })
        .to_vec(),
        frame: SourceFrame::ENU,
        grids: ["ShortRange", "GRN_Template_TerrainWorldSmall"]
            .map(|g| GridSelection {
                grid: g.into(),
                cells: CellSelection::Over { rect, levels },
            })
            .to_vec(),
        instanced_levels: InstancedLevels::Every,
        instanced_collision: InstancedCollision::PerInstance,
        terrain,
    };
    let mut reader = AnvilReader::open(&usnat(Levels::Finest, None)).unwrap();
    let (fine, lines) = reader.read().unwrap();
    assert_eq!(
        report(&lines, "anvil cells"),
        "ShortRange cell 9380 (level 0): 244 objects; GRN_Template_TerrainWorldSmall cell 9380 (level 0): 452 objects"
    );
    assert!(fine.iter::<AnvilListed>().all(|(_, l)| l.reached_from == 0));
    let terrain = TerrainSelection {
        origin: [-8192.0, -8192.0],
        side: 16_384.0,
        level: 8,
        rect: [-3648.0, 1088.0, -3392.0, 1344.0],
        layer: None,
        bank: Some("TMB_MAD".into()),
        noise: Some("TerrainSplattingNoise_Set".into()),
    };
    let mut reader = AnvilReader::open(&usnat(Levels::AllButCoarsest, Some(terrain))).unwrap();
    let (all, lines) = reader.read().unwrap();
    assert!(report(&lines, "anvil cells").contains("(level 1)"));
    let coarse: Vec<&AnvilListed> = all
        .iter::<AnvilListed>()
        .map(|(_, l)| l)
        .filter(|l| l.reached_from > 0)
        .collect();
    assert!(!coarse.is_empty());
    assert!(all.iter::<AnvilListed>().count() > fine.iter::<AnvilListed>().count());
    // the terrain under the rectangle: the level-8 nodes of 64 m whose squares meet it, edges included (the 256 m
    // square's edges lie on node edges, so 4 nodes a side and one more each side: 6 x 6; tests/terrain.rs counts the
    // same way for the campus's 128 m square: 4 x 4)
    let tiles = all.iter::<AnvilTerrain>().next().expect("terrain").1.tiles.len();
    assert_eq!(tiles, 36);
    // a coarse owner's marks name only cells of a level above 0 unless a group's member reaches it from level 0
    for l in &coarse {
        assert!(l.cells.iter().all(|c| c.level > 0));
    }
}

/// A world laid out by hand: three owners listed only at a coarser level and one a level-0 cell reaches, run through
/// `select-anvil-listed`.
fn marked(
    w: &mut World,
    owner: u64,
    level: u32,
    at: [f32; 3],
    bounds: Option<([f32; 3], [f32; 3])>,
    collision: bool,
) -> Entity {
    let e = w.spawn();
    w.insert(e, Name(format!("owner {owner}")));
    let prototype = w.spawn();
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = SourceFrame::ENU.point(at);
    w.insert(e, t);
    w.insert(
        e,
        AnvilListed {
            owner,
            frame: SourceFrame::ENU,
            cells: vec![ListedBy {
                grid: "G".into(),
                cell: 1,
                level,
            }],
            reached_from: level,
        },
    );
    if let Some((lo, hi)) = bounds {
        let (a, b) = (SourceFrame::ENU.point(lo), SourceFrame::ENU.point(hi));
        w.insert(
            e,
            AnvilBounds {
                min: std::array::from_fn(|k| a[k].min(b[k])),
                max: std::array::from_fn(|k| a[k].max(b[k])),
            },
        );
    }
    if collision {
        w.insert(
            e,
            AnvilCollisionFilter {
                query: 0,
                layers: [0, 0],
                ground: false,
            },
        );
    }
    e
}

fn run(w: &mut World, text: &str) -> RunReport {
    let mut registry = PassRegistry::new();
    primase_anvil::register(&mut registry);
    let manager = PassManager::new(registry);
    let pipeline: Pipeline = manager.registry.parse(text).expect("the pass text");
    let mut report = RunReport::default();
    manager.run(w, &pipeline, &mut report).expect("the pass runs");
    report
}

/// Square (0, 0)..(100, 100) is the section. A level-0 owner stays whatever its box; a coarse owner whose box overlaps
/// it stays and one whose box doesn't goes; the instances of a coarse group are kept by their positions; a coarse
/// owner's collision follows its visual; `drop` takes every coarse one and `keep` none. The marks are removed.
#[test]
fn coarse_owners_are_kept_where_they_reach_the_rectangle() {
    let build = |w: &mut World| {
        let fine = marked(
            w,
            1,
            0,
            [500.0, 500.0, 0.0],
            Some(([490.0; 3], [510.0; 3])),
            false,
        );
        let near = marked(
            w,
            2,
            1,
            [150.0, 50.0, 0.0],
            Some(([90.0, 40.0, 0.0], [160.0, 60.0, 5.0])),
            false,
        );
        let near_collision = marked(w, 2, 1, [150.0, 50.0, 0.0], None, true);
        let far = marked(
            w,
            3,
            1,
            [900.0, 900.0, 0.0],
            Some(([890.0, 890.0, 0.0], [910.0, 910.0, 5.0])),
            false,
        );
        let far_collision = marked(w, 3, 1, [900.0, 900.0, 0.0], None, true);
        // a group of two instances sharing one big box: one stands in the square, one far off
        let big = Some(([0.0, 0.0, 0.0], [1000.0, 1000.0, 10.0]));
        let in_square = marked(w, 4, 1, [50.0, 50.0, 0.0], big, false);
        let out_square = marked(w, 4, 1, [800.0, 800.0, 0.0], big, false);
        [
            fine,
            near,
            near_collision,
            far,
            far_collision,
            in_square,
            out_square,
        ]
    };
    let mut w = World::new();
    let [
        fine,
        near,
        near_collision,
        far,
        far_collision,
        in_square,
        out_square,
    ] = build(&mut w);
    let report = run(&mut w, "select-anvil-listed{coarse=overlap,rect=0,0,100,100}");
    let alive = |e: Entity| w.is_alive(e);
    assert!([fine, near, near_collision, in_square].into_iter().all(alive));
    assert!(![far, far_collision, out_square].into_iter().any(alive));
    assert_eq!(w.iter::<AnvilListed>().count(), 0);
    assert!(
        report.steps[0]
            .log
            .approximations
            .keys()
            .any(|a| a.contains("bounding boxes"))
    );
    let mut w = World::new();
    let [fine, near, _, far, _, in_square, _] = build(&mut w);
    run(&mut w, "select-anvil-listed{coarse=drop}");
    assert!(w.is_alive(fine));
    assert!(![near, far, in_square].into_iter().any(|e| w.is_alive(e)));
    let mut w = World::new();
    let all = build(&mut w);
    run(&mut w, "select-anvil-listed{coarse=keep}");
    assert!(all.into_iter().all(|e| w.is_alive(e)));
    assert_eq!(w.iter::<AnvilListed>().count(), 0);
}

/// The options are refused unless stated: `coarse` is required, `rect` goes with `overlap` only and is a rectangle.
#[test]
fn the_pass_options_are_checked() {
    let mut registry = PassRegistry::new();
    primase_anvil::register(&mut registry);
    for (text, why) in [
        ("select-anvil-listed", "coarse is required"),
        ("select-anvil-listed{coarse=overlap}", "needs rect"),
        ("select-anvil-listed{coarse=keep,rect=0,0,1,1}", "overlap only"),
        ("select-anvil-listed{coarse=overlap,rect=5,0,1,1}", "x0 < x1"),
        ("select-anvil-listed{coarse=most}", "keep, drop or overlap"),
    ] {
        let e = registry
            .parse(text)
            .err()
            .unwrap_or_else(|| panic!("{text} was accepted"))
            .to_string();
        assert!(e.contains(why), "{text}: {e}");
    }
    let p = SelectAnvilListed {
        coarse: Coarse::Overlap([0.0, 1.0, 2.0, 3.0]),
    };
    assert_eq!(p.options().get("rect").map(String::as_str), Some("0,1,2,3"));
}
