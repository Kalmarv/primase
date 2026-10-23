//! Real FloorHeightData (Riders Republic build 25394987): MAD_USNAT's 8192 x 8192 grid and MAD_WhiteRoom's empty one
//! read, the cell order within a block measured as the Riders Republic notes measure it (docs/terrain.md §2), and a
//! square of ground lowered under the pass manager with its checks on.
use primase_anvil::components::AnvilFloorHeight;
use primase_anvil::passes::{CellPoint, LowerAnvilFloor};
use primase_anvil::read::archive::{Archive, At};
use primase_anvil::read::floor::{FLOOR_HEIGHT_DATA, StoredFloorHeight};
use primase_core::frame::SourceFrame;
use primase_core::pass::{PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{Instance, Mesh, World};
use primase_testdata::{Game, install};

/// The FloorHeightData of the forge at `path`, as stored.
fn floor(path: &std::path::Path) -> StoredFloorHeight {
    let mut archive = Archive::open(&[path]).expect("archive");
    let names = &archive.forge(0).expect("forge").names;
    let entry = names
        .iter()
        .position(|n| n.type_id == FLOOR_HEIGHT_DATA)
        .expect("a FloorHeightData");
    let r = archive.resource(At { forge: 0, entry }).expect("resource");
    StoredFloorHeight::parse(&r.body).expect("floor")
}

/// A cell's word, exactly, its index taken from the cell at `(ic, ir)`.
fn word(g: &AnvilFloorHeight, (c, r): (u32, u32), (ic, ir): (u32, u32)) -> f64 {
    let (lo, hi, _) = g.cell(c, r).expect("cell");
    let (_, _, index) = g.cell(ic, ir).expect("cell");
    f64::from(lo) + (f64::from(hi) - f64::from(lo)) * f64::from(index) / 3.0
}

#[test]
fn usnat_grid_reads_and_its_block_borders_are_continuous() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let empty = floor(&dir.join("DataPC_MAD_WhiteRoom.forge"));
    assert!(empty.blocks.is_empty());
    let f = floor(&dir.join("DataPC_MAD_USNAT.forge"));
    let t = &f.trailer;
    assert_eq!(
        (f.cell, t.origin, t.cells, f.blocks.len()),
        (2.0, [-8192.0, -8192.0, -3584.0], [8192, 8192], 4_194_304)
    );
    assert_eq!(t.scale.to_bits(), 0x3D7D_7540, "{}", t.scale);
    // lo <= hi in every block
    assert!(f.blocks.iter().all(|b| b.lo <= b.hi));
    // the mean step between neighbouring cells along x across block borders, with the stored order and with each
    // block's rows and columns swapped (measured here: 14.25 against 86.02; the notes: 14.3 against 86)
    let g = f.component(SourceFrame::WORLD);
    let swap = |c: u32, r: u32| (c - c % 4 + r % 4, r - r % 4 + c % 4);
    let (mut across, mut across_swapped, mut n) = (0.0, 0.0, 0.0);
    for r in (0..8192).step_by(7) {
        for c in (3..8191).step_by(4) {
            across += (word(&g, (c + 1, r), (c + 1, r)) - word(&g, (c, r), (c, r))).abs();
            across_swapped += (word(&g, (c + 1, r), swap(c + 1, r)) - word(&g, (c, r), swap(c, r))).abs();
            n += 1.0;
        }
    }
    let (across, across_swapped) = (across / n, across_swapped / n);
    assert!(
        across * 4.0 < across_swapped,
        "across {across}, swapped {across_swapped}"
    );
}

#[test]
fn a_square_of_ground_lowers() {
    let Some(dir) = install(Game::Riders) else {
        return;
    };
    let g = floor(&dir.join("DataPC_MAD_USNAT.forge")).component(SourceFrame::WORLD);
    // the cell at the world's middle, by hand
    let middle = (4096, 4096);
    let z = f64::from(g.scale) * word(&g, middle, middle) + 925.0;
    let mut w = World::new();
    let grid = w.spawn();
    w.insert(grid, g);
    let pass = LowerAnvilFloor {
        rect: [-64.0, -64.0, 64.0, 64.0],
        step: 1,
        offset: 925.0,
        sample: CellPoint::Corner,
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
    // columns 4064..=4128 and rows likewise: 65 x 65 samples, 64 x 64 squares
    let mesh = w.get::<Mesh>(grid).expect("ground");
    let p = mesh.primitives()[0].positions().expect("positions");
    assert_eq!(p.len(), 65 * 65);
    assert_eq!(
        mesh.primitives()[0].indices.as_ref().map(Vec::len),
        Some(64 * 64 * 6)
    );
    // (0, 0) is column 4096, row 4096: sample 32 of row 32
    let at = p[32 * 65 + 32];
    assert_eq!([at[0], at[1]], [0.0, 0.0]);
    assert!((f64::from(at[2]) - z).abs() < 1e-3, "{at:?} against {z}");
    assert_eq!(
        w.iter::<Instance>().filter(|(_, i)| i.prototype == grid).count(),
        1
    );
}
