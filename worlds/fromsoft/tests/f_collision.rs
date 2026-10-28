//! The `f` collision binders' extern meshes, read with the area (`Options::f_collision`) and lowered by
//! `lower-fromsoft-collision{f-meshes=on}`, against the `h` collision they sit beside.
//!
//! Per area of the sample: the reader pairs each `h` file with the `f` file of its model; the lowered `f` soup holds
//! every `f` triangle of the files lowered and the `game` soup every `h` triangle (the same world frame); and every
//! vertex of the `f` soup is measured against the nearest `game` triangle (exact up to `CELL` metres, found with a grid).
//! Controls: the same vertices with y and z swapped, and measured against the `game` triangles of another file of the
//! area. A frame mismatch would put the vertices far from the surface, as the controls do.
//!
//! Sekiro's sample is Ashina Castle (`m11_01_00_00`) and Hirata Estate (`m20_00_00_00`); Elden Ring's, the legacy
//! dungeons Stormveil Castle (`m10_00_00_00`) and Raya Lucaria (`m14_00_00_00`). Where an area's `h` files are
//! placeholders (Elden Ring's catacombs, caves and the like store one triangle) there is nothing to measure against, and
//! the area is not in the sample.
//!
//! Runs where the install, its keys and the Oodle library are configured; otherwise it does nothing.
#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod common;
mod inputs;

use fromsoft_formats::{Game, Install, Keys, dcx::Decoder};
use primase_core::pass::{Pass, PassLog};
use primase_core::{Collision, World};
use primase_fromsoft::game::ParamKey;
use primase_fromsoft::sekiro::{
    self,
    collision::{FMeshes, LowerFromSoftCollision},
    settings::HitFilters,
    source::{CollisionSource, MapCollision},
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A rectangle no map reaches: the collision files are read whole whatever the region.
const NOWHERE: sekiro::Region = sekiro::Region {
    rect: [1.0e6, 1.0e6, 1.0e6 + 1.0, 1.0e6 + 1.0],
    margin: 0.0,
};
const CELL: f64 = 4.0;

type V3 = [f64; 3];
type Tri = [V3; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn along(a: V3, d: V3, t: f64) -> V3 {
    [a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t]
}

/// The squared distance from `p` to the triangle `abc`: the closest point by the Voronoi regions of its vertices,
/// edges and face.
fn distance_squared(p: V3, [a, b, c]: Tri) -> f64 {
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    let (vc, vb, va) = (d1 * d4 - d3 * d2, d5 * d2 - d1 * d6, d3 * d6 - d5 * d4);
    let q = if d1 <= 0. && d2 <= 0. {
        a
    } else if d3 >= 0. && d4 <= d3 {
        b
    } else if vc <= 0. && d1 >= 0. && d3 <= 0. {
        along(a, ab, d1 / (d1 - d3))
    } else if d6 >= 0. && d5 <= d6 {
        c
    } else if vb <= 0. && d2 >= 0. && d6 <= 0. {
        along(a, ac, d2 / (d2 - d6))
    } else if va <= 0. && d4 - d3 >= 0. && d5 - d6 >= 0. {
        along(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6)))
    } else {
        let denominator = 1. / (va + vb + vc);
        along(along(a, ab, vb * denominator), ac, vc * denominator)
    };
    let d = sub(p, q);
    dot(d, d)
}

/// Triangles in cells of `CELL` metres, so the nearest one within `CELL` of a point is among the 27 cells around it.
struct Grid {
    cells: HashMap<[i64; 3], Vec<usize>>,
    /// Triangles too large to cell: tested for every point.
    large: Vec<usize>,
    triangles: Vec<Tri>,
}

fn cell(p: V3) -> [i64; 3] {
    p.map(|x| (x / CELL).floor() as i64)
}

impl Grid {
    fn new(triangles: Vec<Tri>) -> Self {
        let mut cells: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
        let mut large = Vec::new();
        for (i, t) in triangles.iter().enumerate() {
            let lo = cell([0, 1, 2].map(|k| t.iter().map(|v| v[k]).fold(f64::MAX, f64::min)));
            let hi = cell([0, 1, 2].map(|k| t.iter().map(|v| v[k]).fold(f64::MIN, f64::max)));
            if (0..3).map(|k| (hi[k] - lo[k] + 1) as u64).product::<u64>() > 512 {
                large.push(i);
                continue;
            }
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        cells.entry([x, y, z]).or_default().push(i);
                    }
                }
            }
        }
        Grid {
            cells,
            large,
            triangles,
        }
    }

    /// The distance to the nearest triangle if it is at most `CELL`.
    fn nearest(&self, p: V3) -> Option<f64> {
        let c = cell(p);
        let mut best = f64::MAX;
        for &i in &self.large {
            best = best.min(distance_squared(p, self.triangles[i]));
        }
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &i in self
                        .cells
                        .get(&[c[0] + dx, c[1] + dy, c[2] + dz])
                        .into_iter()
                        .flatten()
                    {
                        best = best.min(distance_squared(p, self.triangles[i]));
                    }
                }
            }
        }
        (best <= CELL * CELL).then(|| best.sqrt())
    }
}

/// A lowered soup's triangles.
fn triangles(c: &Collision) -> Vec<Tri> {
    c.triangles
        .iter()
        .map(|t| t.map(|i| c.positions[i as usize].map(f64::from)))
        .collect()
}

/// Fractions of `points` whose nearest triangle of `grid` is within each bound (metres).
fn within(grid: &Grid, points: &[V3], bounds: &[f64]) -> Vec<f64> {
    let mut counts = vec![0usize; bounds.len()];
    for &p in points {
        if let Some(d) = grid.nearest(p) {
            for (count, &bound) in counts.iter_mut().zip(bounds) {
                *count += usize::from(d <= bound);
            }
        }
    }
    counts.iter().map(|&n| n as f64 / points.len() as f64).collect()
}

/// What the `f` binder of each sampled area holds, counted straight from its tagfiles (every member's bodies and
/// `hkGeometry` array lengths, not through the reader): its files, and the files whose `hknpPhysicsSystemData` has no
/// body at all (0 bodies, 0 vertices, 0 triangles), named by the `h` file of their model.
const F_BINDERS: [(&str, usize, &[&str]); 4] = [
    (
        "m11_01_00_00",
        192,
        &[
            "h11_01_00_00_004900.hkx",
            "h11_01_00_00_005100.hkx",
            "h11_01_00_00_012200.hkx",
            "h11_01_00_00_015800.hkx",
            "h11_01_00_00_015900.hkx",
            "h11_01_00_00_016500.hkx",
            "h11_01_00_00_016600.hkx",
        ],
    ),
    (
        "m20_00_00_00",
        140,
        &["h20_00_00_00_011500.hkx", "h20_00_00_00_011600.hkx"],
    ),
    (
        "m10_00_00_00",
        98,
        &[
            "h10_00_00_00_000700.hkx",
            "h10_00_00_00_000800.hkx",
            "h10_00_00_00_001301.hkx",
            "h10_00_00_00_001400.hkx",
            "h10_00_00_00_001500.hkx",
            "h10_00_00_00_003000.hkx",
            "h10_00_00_00_003300.hkx",
            "h10_00_00_00_003700.hkx",
            "h10_00_00_00_004600.hkx",
            "h10_00_00_00_004700.hkx",
            "h10_00_00_00_004800.hkx",
            "h10_00_00_00_004900.hkx",
            "h10_00_00_00_005700.hkx",
            "h10_00_00_00_005900.hkx",
            "h10_00_00_00_006000.hkx",
            "h10_00_00_00_006100.hkx",
            "h10_00_00_00_006500.hkx",
            "h10_00_00_00_006600.hkx",
            "h10_00_00_00_006900.hkx",
            "h10_00_00_00_007000.hkx",
            "h10_00_00_00_007100.hkx",
            "h10_00_00_00_007400.hkx",
            "h10_00_00_00_007500.hkx",
            "h10_00_00_00_007800.hkx",
            "h10_00_00_00_008000.hkx",
            "h10_00_00_00_008100.hkx",
            "h10_00_00_00_008200.hkx",
            "h10_00_00_00_008300.hkx",
            "h10_00_00_00_008900.hkx",
            "h10_00_00_00_009000.hkx",
            "h10_00_00_00_009100.hkx",
            "h10_00_00_00_009500.hkx",
            "h10_00_00_00_010100.hkx",
            "h10_00_00_00_010200.hkx",
            "h10_00_00_00_010301.hkx",
            "h10_00_00_00_010700.hkx",
            "h10_00_00_00_010800.hkx",
            "h10_00_00_00_011100.hkx",
            "h10_00_00_00_011500.hkx",
            "h10_00_00_00_011700.hkx",
            "h10_00_00_00_012400.hkx",
            "h10_00_00_00_014300.hkx",
            "h10_00_00_00_015800.hkx",
        ],
    ),
    (
        "m14_00_00_00",
        71,
        &[
            "h14_00_00_00_000800.hkx",
            "h14_00_00_00_001600.hkx",
            "h14_00_00_00_002400.hkx",
            "h14_00_00_00_002700.hkx",
            "h14_00_00_00_004000.hkx",
            "h14_00_00_00_004100.hkx",
            "h14_00_00_00_004300.hkx",
            "h14_00_00_00_004700.hkx",
            "h14_00_00_00_004800.hkx",
            "h14_00_00_00_005700.hkx",
            "h14_00_00_00_005800.hkx",
            "h14_00_00_00_007300.hkx",
            "h14_00_00_00_009200.hkx",
            "h14_00_00_00_010000.hkx",
        ],
    ),
];

const BOUNDS: [f64; 4] = [0.05, 0.25, 1.0, 2.0];

/// What one area measured.
struct Area {
    f_files: usize,
    h_triangles: usize,
    f_triangles: usize,
    /// Fractions of `f` vertices within `BOUNDS` of a `game` triangle, then the two controls'.
    f_vs_h: Vec<f64>,
    swapped: Vec<f64>,
    /// The same fractions for as many points as there are vertices, uniform in the box of the vertices: the chance rate.
    chance: Vec<f64>,
    other_file: Vec<f64>,
}

fn measure(game: Game, install: &Install, decoder: &Decoder<'_>, map: &str) -> Result<Area> {
    let mut world = World::default();
    let options = sekiro::Options {
        region: Some(NOWHERE),
        f_collision: true,
        ..Default::default()
    };
    let report = sekiro::read_game_area(install, map, decoder, &options, &mut world)?;
    assert!(report.collision.f_binder, "{map}: an f binder");
    let (_, binder_files, empty) = F_BINDERS.iter().find(|(m, ..)| m == &map).unwrap();
    // every file of the binder is read with a record or left unpaired, and the files without a body are the named ones
    assert_eq!(
        report.collision.f_files + report.collision.unpaired_f_files,
        *binder_files,
        "{map}: f files read and unpaired"
    );
    assert_eq!(
        report.collision.empty_f_files, *empty,
        "{map}: the f files without a body"
    );
    let (filters, f_files, h_total, f_total, first_vertices, other_file, other_grid) = {
        let records: Vec<&MapCollision> = world.iter::<MapCollision>().map(|(_, r)| r).collect();
        let paired: Vec<&&MapCollision> = records.iter().filter(|r| !r.f_triangles.is_empty()).collect();
        // the f files read are the records with triangles and the named files without a body (Ashina Castle: 191 read,
        // 184 with triangles, 7 without)
        assert_eq!(
            report.collision.f_files,
            paired.len() + report.collision.empty_f_files.len(),
            "{map}: f files read, with and without triangles"
        );
        let f_total = records.iter().map(|r| r.f_triangles.len()).sum::<usize>();
        assert_eq!(report.collision.f_triangles, f_total);
        let filters: BTreeSet<u8> = records
            .iter()
            .flat_map(|r| r.parts.iter().map(|p| p.hit_filter))
            .collect();
        // another file of the area: the h triangles of the largest record other than the first paired f file's
        let first = paired[0];
        let other = records
            .iter()
            .filter(|r| r.file != first.file && r.triangles.len() > 1)
            .max_by_key(|r| r.triangles.len())
            .unwrap();
        let other_grid = Grid::new(
            other
                .triangles
                .iter()
                .map(|t| t.map(|v| v.map(f64::from)))
                .collect(),
        );
        let first_vertices: Vec<V3> = first
            .f_triangles
            .iter()
            .flatten()
            .map(|v| v.map(f64::from))
            .collect();
        (
            filters,
            paired.len(),
            records.iter().map(|r| r.triangles.len()).sum::<usize>(),
            f_total,
            first_vertices,
            other.file.clone(),
            other_grid,
        )
    };
    let pass = LowerFromSoftCollision {
        hit_filters: HitFilters::new(filters).unwrap(),
        water: BTreeSet::new(),
        leave_out: BTreeSet::new(),
        reach: None,
        f_meshes: FMeshes::On,
    };
    let mut log = PassLog::default();
    pass.run(&mut world, &mut log).unwrap();
    let mut by_group = BTreeMap::<String, Vec<Tri>>::new();
    for (e, c) in world.iter::<Collision>() {
        let group = world.get::<CollisionSource>(e).unwrap().group.clone();
        by_group.entry(group).or_default().extend(triangles(c));
    }
    assert_eq!(
        by_group.keys().map(String::as_str).collect::<Vec<_>>(),
        ["f", "game"],
        "{map}: groups"
    );
    assert_eq!(by_group["f"].len(), f_total, "{map}: every f triangle is lowered");
    assert_eq!(
        by_group["game"].len(),
        h_total,
        "{map}: every h triangle is lowered"
    );
    let grid = Grid::new(by_group["game"].clone());
    let vertices: Vec<V3> = by_group["f"].iter().flatten().copied().collect();
    let swapped: Vec<V3> = vertices.iter().map(|v| [v[0], v[2], v[1]]).collect();
    let chance = {
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for v in &vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        // a fixed linear congruential sequence, so the control is the same on every run
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut unit = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let points: Vec<V3> = (0..vertices.len())
            .map(|_| [0, 1, 2].map(|k| lo[k] + unit() * (hi[k] - lo[k])))
            .collect();
        within(&grid, &points, &BOUNDS)
    };
    let area = Area {
        f_files,
        h_triangles: h_total,
        f_triangles: f_total,
        f_vs_h: within(&grid, &vertices, &BOUNDS),
        swapped: within(&grid, &swapped, &BOUNDS),
        chance,
        other_file: within(&other_grid, &first_vertices, &BOUNDS),
    };
    println!(
        "{} {map}: {} f files, {} h triangles, {} f triangles; f vertices within {BOUNDS:?} m of an h triangle {:.3?}; y and z swapped {:.3?}; uniform points in the box of the vertices {:.3?}; one file's f vertices against {}'s h {:.3?}",
        game.name(),
        area.f_files,
        area.h_triangles,
        area.f_triangles,
        area.f_vs_h,
        area.swapped,
        area.chance,
        other_file,
        area.other_file
    );
    Ok(area)
}

#[test]
fn the_f_meshes_lie_in_the_frame_of_the_h_collision_beside_them() -> Result<()> {
    let Some(dll) = inputs::oodle(inputs::Tool::Oodle6) else {
        return Ok(());
    };
    let dll = common::oodle(dll)?;
    let decoder = Decoder {
        oodle: Some(&dll),
        max_output: 1 << 30,
    };
    for (game, data, maps, near) in [
        (
            Game::Sekiro,
            inputs::Game::Sekiro,
            &["m11_01_00_00", "m20_00_00_00"][..],
            // within 1 m: Sekiro's f meshes follow the h surface
            0.90,
        ),
        (
            Game::EldenRing,
            inputs::Game::Elden,
            &["m10_00_00_00", "m14_00_00_00"][..],
            // within 2 m: Elden Ring's sit further from the h surface
            0.40,
        ),
    ] {
        let (Some(root), Some(keys)) = (inputs::install(data), inputs::keys(data)) else {
            continue;
        };
        let keys = Keys::from_folder(keys, game, game.param_key_sha256())?;
        let install = common::install(root, game, keys);
        let bound = if game == Game::Sekiro { 2 } else { 3 };
        for map in maps {
            let area = measure(game, &install, &decoder, map)?;
            assert!(
                area.f_vs_h[bound] >= near,
                "{map}: {:.3} of the f vertices within {} m of an h triangle",
                area.f_vs_h[bound],
                BOUNDS[bound]
            );
            // the chance rate is the fraction of points uniform in the box of the f vertices that lie within the bound of
            // an h triangle, which depends on how much of the box the h collision fills (Stormveil's dense dungeon: 0.062
            // within 2 m, Raya Lucaria's 0.091). The vertices with y and z swapped are no better than chance (measured
            // 0.062 and 0.071 there, 0.013 and 0 within 1 m in Sekiro), and the f vertices as stored are many times
            // better than it (0.793 against 0.062 in Stormveil, 0.467 against 0.091 in Raya Lucaria, 0.986 and 0.989
            // against 0.025 and 0.007 in Sekiro): a wrong frame would look like the controls.
            assert!(
                area.swapped[bound] <= 1.5 * area.chance[bound],
                "{map}: y and z swapped {:.3?} against chance {:.3?}",
                area.swapped,
                area.chance
            );
            assert!(
                area.f_vs_h[bound] >= 4.0 * area.chance[bound],
                "{map}: f {:.3?} against chance {:.3?}",
                area.f_vs_h,
                area.chance
            );
            assert!(
                area.other_file[bound] <= 0.25,
                "{map}: another file's h: {:.3?}",
                area.other_file
            );
        }
    }
    Ok(())
}
