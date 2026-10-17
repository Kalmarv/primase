//! The tile byte budget: a cell whose file is over the budget is split into quadtree subcells until every file fits;
//! cells within it keep their bytes and names.

use primase_core::{Entity, Instance, Mesh, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_web::{Options, TileSet, ViewerItem, write};
use std::collections::{BTreeMap, BTreeSet};

/// An `n` x `n` grid of quads over `size` metres, its vertices nudged by `seed` so no two prototypes are alike.
fn grid(n: usize, size: f32, seed: f32) -> Mesh {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, z) = (i as f32 / n as f32 * size, j as f32 / n as f32 * size);
            positions.push([x, seed + (i * j) as f32 * 1e-3, -z]);
        }
    }
    let mut indices = Vec::new();
    let v = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    for j in 0..n {
        for i in 0..n {
            indices.extend([
                v(i, j),
                v(i + 1, j),
                v(i + 1, j + 1),
                v(i, j),
                v(i + 1, j + 1),
                v(i, j + 1),
            ]);
        }
    }
    Mesh::new(vec![Primitive {
        topology: Topology::Triangles,
        streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(positions))]),
        indices: Some(indices),
        material: None,
    }])
}

/// A placement of `proto` at east `x`, north `north`.
fn place(w: &mut World, proto: Entity, x: f32, north: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [x, 0., -north];
    w.insert(e, t);
    e
}

fn options(budget: Option<u64>) -> Options {
    Options {
        compress: false,
        gpu_instancing: false,
        tile_byte_budget: budget,
        ..Options::default()
    }
}

fn sums(set: &TileSet) -> [u64; 4] {
    let t = &set.index.tiles;
    [
        t.iter().map(|t| t.triangles).sum(),
        t.iter().map(|t| t.placements).sum(),
        t.iter().map(|t| t.instanced_placements).sum(),
        t.iter().map(|t| t.viewer_items).sum(),
    ]
}

/// Cell (0, 0) holds 16 distinct prototypes on a 4 x 4 grid and four viewer records; cell (1, 0) one small one.
fn city() -> (World, Vec<ViewerItem>) {
    let mut w = World::new();
    for k in 0..16 {
        let proto = w.spawn();
        w.insert(proto, grid(12, 10., k as f32));
        place(
            &mut w,
            proto,
            25. + 50. * (k % 4) as f32,
            25. + 50. * (k / 4) as f32,
        );
    }
    let small = w.spawn();
    w.insert(small, grid(1, 1., 99.));
    place(&mut w, small, 250., 50.);
    let items = [[10., 10.], [190., 10.], [10., 190.], [190., 190.]]
        .iter()
        .enumerate()
        .map(|(i, [x, n])| ViewerItem {
            name: format!("marker {i}"),
            kind: "marker".into(),
            anchor: [*x, 0., -*n],
            data: serde_json::json!({}),
        })
        .collect();
    (w, items)
}

#[test]
fn cells_over_the_budget_split_and_keep_every_count() {
    let (w, items) = city();
    let with = |budget| {
        write(
            &w,
            &Options {
                viewer_items: items.clone(),
                ..options(budget)
            },
        )
        .unwrap()
    };
    let whole = with(None);
    assert_eq!(whole.index.tiles.len(), 2);
    let big = whole
        .index
        .tiles
        .iter()
        .find(|t| (t.col, t.row) == (0, 0))
        .unwrap();
    let budget = big.bytes / 3;
    let split = with(Some(budget));
    assert!(split.over_budget.is_empty(), "{:?}", split.over_budget);
    assert!(split.index.tiles.len() > 2);
    for t in &split.index.tiles {
        assert!(t.bytes <= budget, "{} is {} bytes", t.file, t.bytes);
        assert_eq!(split.files[&t.file].len() as u64, t.bytes);
    }
    assert_eq!(
        sums(&split),
        sums(&whole),
        "triangles, placements, instanced placements, viewer records"
    );
    // the small cell is untouched: same name, same bytes
    let small = whole
        .index
        .tiles
        .iter()
        .find(|t| (t.col, t.row) == (1, 0))
        .unwrap();
    assert!(small.bytes <= budget);
    assert_eq!(split.files.get(&small.file), whole.files.get(&small.file));
    assert!(
        split
            .index
            .tiles
            .iter()
            .any(|t| t.file == small.file && t.cell.is_none())
    );
    // the big one's subcells: within the cell, unique, named for themselves
    let mut seen = BTreeSet::new();
    for t in split.index.tiles.iter().filter(|t| (t.col, t.row) == (0, 0)) {
        let c = t.cell.expect("a subcell");
        assert!(c.level >= 1 && c.x < 1 << c.level && c.y < 1 << c.level);
        assert!(seen.insert((c.level, c.x, c.y)));
        assert!(
            t.file
                .starts_with(&format!("tile_0_0_{}-{}-{}.", c.level, c.x, c.y)),
            "{}",
            t.file
        );
        // its content lies in its own square of the cell (the placements' pivots, so their meshes start there)
        let side = 200. / f64::from(1u32 << c.level);
        let (x0, n0) = (f64::from(c.x) * side, f64::from(c.y) * side);
        assert!(t.min[0] >= x0 - 1e-9 && t.min[0] < x0 + side, "{t:?}");
        assert!(-t.max[2] >= n0 - 1e-9 - 10. && -t.max[2] < n0 + side, "{t:?}");
    }
    // the index: format 2.0, `cell` only on subcells, and write_to accepts the set
    let json = serde_json::to_value(&split.index).unwrap();
    assert_eq!(json["format_version"], "2.0");
    for t in json["tiles"].as_array().unwrap() {
        let whole_cell = (t["col"].as_i64(), t["row"].as_i64()) == (Some(1), Some(0));
        assert_eq!(t.get("cell").is_none(), whole_cell, "{t}");
        if !whole_cell {
            assert!(t["cell"]["level"].is_u64() && t["cell"]["x"].is_u64() && t["cell"]["y"].is_u64());
        }
    }
    let dir = std::env::temp_dir().join("primase-web-budget-split");
    let _ = std::fs::remove_dir_all(&dir);
    split.write_to(&dir).unwrap();
    // deterministic
    assert_eq!(with(Some(budget)).files, split.files);
}

#[test]
fn one_placement_over_the_budget_is_written_and_reported() {
    let mut w = World::new();
    let proto = w.spawn();
    w.insert(proto, grid(40, 10., 0.));
    place(&mut w, proto, 50., 50.);
    let set = write(&w, &options(Some(1024))).unwrap();
    assert_eq!(set.index.tiles.len(), 1);
    let t = &set.index.tiles[0];
    assert!(t.bytes > 1024 && t.cell.is_none());
    assert_eq!(set.over_budget.len(), 1);
    assert_eq!(set.over_budget[0].file, t.file);
    assert_eq!(set.over_budget[0].bytes, t.bytes);
    assert!(set.over_budget[0].reason.contains("one placement"));
}

#[test]
fn triangles_filed_by_centre_split_with_their_cell() {
    let mut w = World::new();
    let proto = w.spawn();
    // one mesh over the whole cell, filed by triangle centre
    w.insert(proto, grid(32, 199., 0.));
    let e = place(&mut w, proto, 0.5, 0.5);
    let opts = |budget| Options {
        split_by_tile: BTreeSet::from([e]),
        ..options(budget)
    };
    let whole = write(&w, &opts(None)).unwrap();
    assert_eq!(whole.index.tiles.len(), 1);
    let budget = whole.index.tiles[0].bytes / 5;
    let split = write(&w, &opts(Some(budget))).unwrap();
    assert!(split.over_budget.is_empty(), "{:?}", split.over_budget);
    assert!(split.index.tiles.len() >= 5);
    for t in &split.index.tiles {
        assert!(t.bytes <= budget);
        assert_eq!(t.placements, 1, "a piece of the one placement");
    }
    assert_eq!(
        split.index.tiles.iter().map(|t| t.triangles).sum::<u64>(),
        whole.index.tiles[0].triangles
    );
}

#[test]
fn lights_follow_their_subcell() {
    use primase_core::{Light, LightKind};
    let (mut w, items) = city();
    for (x, n) in [(20., 20.), (180., 180.), (100., 120.)] {
        let e = w.spawn();
        w.insert(
            e,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: 5.0,
                range: None,
                cast_shadows: None,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 3., -n];
        w.insert(e, t);
    }
    let with = |budget| {
        write(
            &w,
            &Options {
                viewer_items: items.clone(),
                ..options(budget)
            },
        )
        .unwrap()
    };
    let whole = with(None);
    let budget = whole.index.tiles.iter().map(|t| t.bytes).max().unwrap() / 3;
    let split = with(Some(budget));
    assert!(split.index.tiles.iter().any(|t| t.cell.is_some()));
    let lights = |s: &TileSet| s.index.tiles.iter().map(|t| t.lights).sum::<u64>();
    assert_eq!(lights(&whole), 3);
    assert_eq!(lights(&split), 3, "every light survives the split");
}
