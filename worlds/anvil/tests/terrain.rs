//! Real terrain (Riders Republic build 25394987, DataPC_MAD_USNAT): the level-8 nodes under the CITYSCHOOL campus
//! read and lowered to ground under the pass manager with its checks on, in the frame the Riders Republic notes place
//! the quadtree in (docs/terrain.md §3.3: ShortRange cell 9380, x -3584..-3456, y 1152..1280).
use primase_anvil::components::AnvilTerrain;
use primase_anvil::components::TerrainTile;
use primase_anvil::passes::{
    CellPoint, GroundColour, GroundNormals, LowerAnvilTerrain, Pieces, Steep, SurfaceBy,
};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::grid::{self, StoredGridCellDataBlock, StoredGridPartition, WORLD};
use primase_anvil::read::placements::{InstancedLevels, Placed, Placer};
use primase_anvil::read::terrain::{TerrainRead, read_terrain};
use primase_anvil::read::terrain_material::TERRAIN_MATERIAL_BANK;
use primase_anvil::read::tnl::TERRAIN_NODE_LAYER;
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{ColorSpace, Mesh, Transform, World};
use primase_testdata::{Game, install};

#[test]
fn the_campus_ground_reads_and_lowers() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let mut archive = Archive::open(&[dir.join("DataPC_MAD_USNAT.forge")]).expect("archive");
    let names = &archive.forge(0).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.type_id == TERRAIN_NODE_LAYER)
        .expect("a TerrainNodeLayer");
    let rect = [-3584.0, 1152.0, -3456.0, 1280.0];
    let bank = At {
        forge: 0,
        entry: names
            .iter()
            .position(|n| n.type_id == TERRAIN_MATERIAL_BANK)
            .expect("a TerrainMaterialBank"),
    };
    let mut w = World::new();
    let ground = read_terrain(
        &mut w,
        &mut archive,
        At { forge: 0, entry },
        TerrainRead {
            origin: [-8192.0, -8192.0],
            side: 16_384.0,
            level: 8,
            rect,
            frame: SourceFrame::WORLD,
            bank: Some(bank),
            noise: None,
        },
    )
    .expect("terrain");
    // the nodes whose 64 m squares meet the rectangle (edges included): rows 145..=148, columns 71..=74
    let t = w.get::<AnvilTerrain>(ground).expect("tiles");
    let mut nodes: Vec<(u32, u32)> = t.tiles.iter().map(|t| (t.row, t.column)).collect();
    nodes.sort_unstable();
    let want: Vec<(u32, u32)> = (145..=148).flat_map(|r| (71..=74).map(move |c| (r, c))).collect();
    assert_eq!(nodes, want);
    // the sample at (-3500, 1200), 9,384 and 18,784 half-metres from the corner: node (row 146, column 73), sample
    // (i, j) = (96, 40), stored from TerrainTile::CORE_AT
    let tile = t
        .tiles
        .iter()
        .find(|t| (t.row, t.column) == (146, 73))
        .expect("node");
    let z = 5000.0 * f64::from(tile.heights[(96 + TerrainTile::CORE_AT) * 132 + 40 + TerrainTile::CORE_AT])
        / f64::from(1u32 << 20);
    let pass = LowerAnvilTerrain {
        rect,
        step: 1,
        range: 5000.0,
        offset: 0.0,
        sample: CellPoint::Corner,
        colour: ColorSpace::Srgb,
        collision: Some(SurfaceBy::CollisionMaterial),
        surface: "rr-ground-unnamed".to_owned(),
        ground: GroundColour::Tile,
        pieces: Pieces::Whole,
        normals: GroundNormals::None,
        steep: Steep::None,
    };
    let manager = PassManager::new(PassRegistry::new()).with_pass_checks(true);
    let mut report = RunReport::default();
    manager
        .run(
            &mut w,
            &Pipeline {
                passes: vec![Box::new(pass)],
            },
            &mut report,
        )
        .expect("lower");
    // 128 m at 0.5 m, both edges: 257 x 257 samples, local to the ground's placement (its least corner rounded down
    // to 64 m), which puts them back where they were to the bit
    let mesh = w.get::<Mesh>(ground).expect("ground");
    let placed = w
        .iter::<primase_core::Instance>()
        .find(|(_, i)| i.prototype == ground)
        .map(|(e, _)| e)
        .expect("placed");
    let origin = w.get::<Transform>(placed).expect("transform").cols[3];
    assert!(origin.iter().all(|c| c % 64.0 == 0.0), "{origin:?}");
    let p: Vec<[f32; 3]> = mesh.primitives()[0]
        .positions()
        .expect("positions")
        .iter()
        .map(|q| std::array::from_fn(|k| q[k] + origin[k]))
        .collect();
    assert_eq!(p.len(), 257 * 257);
    assert_eq!(
        mesh.primitives()[0].indices.as_ref().map(Vec::len),
        Some(256 * 256 * 6)
    );
    // (-3500, 1200) is sample (168, 96) of the rectangle
    let at = p[96 * 257 + 168];
    assert_eq!([at[0], at[1]], [-3500.0, 1200.0]);
    assert!((f64::from(at[2]) - z).abs() < 1e-3, "{at:?} against {z}");
    // the notes' window around it spans 2308.8 to 2357.2 m
    assert!(p.iter().all(|q| (2300.0..2360.0).contains(&q[2])));
}

/// The ground at (x, y), bilinear between the 0.5 m samples of `t`'s level-8 tiles (each node's from TerrainTile::CORE_AT), at
/// range 5000 and offset 0.
fn ground_at(t: &AnvilTerrain, x: f64, y: f64) -> Option<f64> {
    let (gx, gy) = ((x + 8192.0) / 0.5, (y + 8192.0) / 0.5);
    let (fx, fy) = (gx.floor(), gy.floor());
    let sample = |cx: f64, cy: f64| -> Option<f64> {
        let (cx, cy) = (cx as u32, cy as u32);
        let tile = t
            .tiles
            .iter()
            .find(|tl| (tl.row, tl.column) == (cy / 128, cx / 128))?;
        let v = tile.heights
            [((cy % 128) as usize + TerrainTile::CORE_AT) * 132 + (cx % 128) as usize + TerrainTile::CORE_AT];
        Some(5000.0 * f64::from(v) / f64::from(1u32 << 20))
    };
    let (a, b) = (sample(fx, fy)?, sample(fx + 1.0, fy)?);
    let (c, d) = (sample(fx, fy + 1.0)?, sample(fx + 1.0, fy + 1.0)?);
    let (u, v) = (gx - fx, gy - fy);
    Some(a * (1.0 - u) * (1.0 - v) + b * u * (1.0 - v) + c * (1.0 - u) * v + d * u * v)
}

/// Everything ShortRange cell 9380 lists, placed (every DataLayer: event layouts too), against the ground: measured
/// 284 placements, median +1.16 m, p10 -0.08, p25 +0.08, minimum -1.79, 154 within 1.5 m; the upper quarter
/// (+7.1 m and up) is the notes' event objects standing 8 m above the plaza.
#[test]
fn the_campus_placements_stand_on_the_ground() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let order = [
        "DataPC_MAD_USNAT_patch_01.forge",
        "DataPC_MAD_USNAT.forge",
        "DataPC_patch_01.forge",
        "DataPC.forge",
    ];
    let mut archive = Archive::open(&order.map(|f| dir.join(f))).expect("archive");
    let of = |archive: &Archive, forge: usize, kind: u32| {
        let names = &archive.forge(forge).expect("forge").names;
        let entry = names.iter().position(|n| n.type_id == kind).expect("an entry");
        At { forge, entry }
    };
    // the patch's World: its GridLayouts' u16 run is 4 long
    let world = archive.resource(of(&archive, 0, WORLD)).expect("world");
    let short = grid::grid_descriptions(&world.body, 4)
        .expect("grids")
        .swap_remove(1);
    let p_at = archive
        .find(short.partition.expect("partition").id)
        .expect("partition");
    let p = StoredGridPartition::parse(&archive.resource(p_at).expect("partition").body).expect("partition");
    let cell = short.layout.cell(p.levels, 9380).expect("cell");
    assert_eq!((cell.min, cell.size), ([-3584, 1152], 128));
    let b_at = archive.find(p.cells[9380]).expect("block");
    let block = StoredGridCellDataBlock::parse(&archive.resource(b_at).expect("block").body).expect("block");
    let listed: Vec<At> = block.objects.iter().filter_map(|o| archive.find(o.id)).collect();
    let mut w = World::new();
    let mut placed = Placed::default();
    let mut placer = Placer::new(
        &mut archive,
        &mut w,
        SourceFrame::WORLD,
        InstancedLevels::Selector,
    );
    for at in listed {
        let _ = placer.place(at, &mut placed);
    }
    let layer = of(&archive, 1, TERRAIN_NODE_LAYER);
    let rect = [-3648.0, 1088.0, -3392.0, 1344.0];
    let read = TerrainRead {
        origin: [-8192.0, -8192.0],
        side: 16_384.0,
        level: 8,
        rect,
        frame: SourceFrame::WORLD,
        bank: None,
        noise: None,
    };
    let g = read_terrain(&mut w, &mut archive, layer, read).expect("terrain");
    let t = w.get::<AnvilTerrain>(g).expect("tiles");
    let mut res: Vec<f64> = placed
        .placements
        .iter()
        .filter_map(|&e| {
            let [x, y, z] = w.get::<Transform>(e)?.cols[3].map(f64::from);
            Some(z - ground_at(t, x, y)?)
        })
        .collect();
    res.sort_by(f64::total_cmp);
    let n = res.len();
    let p25 = res[(n - 1) / 4];
    let within = res.iter().filter(|r| r.abs() <= 1.5).count();
    assert!(n > 200, "{n} placements over the ground");
    assert!(
        res[0] > -2.0 && p25.abs() < 0.25 && 2 * within > n,
        "lowest {}, p25 {p25}, {within} of {n} within 1.5 m",
        res[0]
    );
}
