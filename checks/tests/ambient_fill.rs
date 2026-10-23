//! The promoted ambient fill reproduces SkateBIRD's own (`program/src/ambient.rs` `add_fill` at SkateBIRD 1072922,
//! copied below unchanged but for its name, as the oracle): `lower-unity-ambient{probe=stored-gradient}` then
//! `fill-ambient-lights` with the program's choices (a 15 m grid anchored at the level's Unity minimum corner, its
//! collider bounds, every cell meeting them, clearance a quarter spacing, reach 1.5 spacings) make the same lights, bit
//! for bit: positions, colour, intensity, range, shadows, and the same number left out near surfaces. Only their names
//! differ (the lattice index, not the program's Unity-ordered one).

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use primase_core::pass::{PassLog, PassManager, PassRegistry, Pipeline, RunReport};
use primase_core::{
    Instance, Light, LightKind, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World,
};
use primase_lighting::{FillAmbientLights, FillCells, FillGrid};
use primase_unity::components::{UnityAmbient, UnityRenderSettings};
use primase_unity::lower_ambient::{LowerUnityAmbient, UnityAmbientProbe};

/// SkateBIRD's `add_fill` and its surface index, as the program has them.
mod program {
    use std::collections::BTreeMap;

    use primase_core::pass::PassLog;
    use primase_core::{Instance, Light, LightKind, Mesh, Name, Topology, Transform, World};
    use primase_unity::components::UnityRenderSettings;

    fn linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    pub fn add_fill(w: &mut World, bounds: ([f64; 3], [f64; 3]), spacing: f64) -> PassLog {
        let mut log = PassLog::default();
        let Some(a) = w
            .resource::<UnityRenderSettings>()
            .and_then(|s| s.ambient.clone())
        else {
            log.count("levels without an ambient: no fill", 1);
            return log;
        };
        let colours: Vec<[f32; 4]> = match a.mode {
            1 => vec![a.sky, a.equator, a.ground],
            3 => vec![a.sky],
            m => {
                log.approximate(
                    &format!("ambient mode {m} (skybox or custom) approximated by its stored sky, equator and ground"),
                    1,
                );
                vec![a.sky, a.equator, a.ground]
            }
        };
        log.approximate(
            "the ambient's colours reduced to their mean for the fill lights",
            1,
        );
        let mut mean = [0.0f32; 3];
        for c in &colours {
            for (m, v) in mean.iter_mut().zip(c) {
                *m += linear(*v) * a.intensity / colours.len() as f32;
            }
        }
        let level = mean.iter().copied().fold(0.0f32, f32::max);
        if level <= 0.0 {
            log.count("levels with a black ambient: no fill", 1);
            return log;
        }
        let colour = mean.map(|c| c / level);
        let reach = spacing / 2.0;
        let intensity = level * (reach * reach) as f32;
        let clearance = spacing / 4.0;
        let surfaces = Surfaces::of(w, clearance);
        let (lo, hi) = bounds;
        let steps = |i: usize| (((hi[i] - lo[i]) / spacing).ceil() as usize).max(1);
        for ix in 0..steps(0) {
            for iy in 0..steps(1) {
                for iz in 0..steps(2) {
                    let u = [
                        lo[0] + (ix as f64 + 0.5) * spacing,
                        lo[1] + (iy as f64 + 0.5) * spacing,
                        lo[2] + (iz as f64 + 0.5) * spacing,
                    ];
                    if surfaces.within([u[0], u[1], -u[2]], clearance) {
                        log.count("ambient fill lights left out (near a drawn surface)", 1);
                        continue;
                    }
                    let e = w.spawn();
                    w.insert(e, Name(format!("ambient fill {ix}.{iy}.{iz}")));
                    w.insert(
                        e,
                        Transform {
                            cols: [
                                [1.0, 0.0, 0.0],
                                [0.0, 1.0, 0.0],
                                [0.0, 0.0, 1.0],
                                [u[0] as f32, u[1] as f32, -u[2] as f32],
                            ],
                        },
                    );
                    w.insert(
                        e,
                        Light {
                            kind: LightKind::Point,
                            color: colour,
                            intensity,
                            range: Some((spacing * 1.5) as f32),
                            cast_shadows: Some(false),
                        },
                    );
                    log.count("ambient fill lights", 1);
                }
            }
        }
        log.approximate("ambient light as shadowless point lights on a grid", 1);
        log
    }

    struct Surfaces {
        cell: f64,
        triangles: Vec<[[f64; 3]; 3]>,
        buckets: BTreeMap<[i64; 3], Vec<usize>>,
    }

    impl Surfaces {
        fn of(w: &World, cell: f64) -> Surfaces {
            let mut triangles = Vec::new();
            for (e, i) in w.iter::<Instance>() {
                let (Some(t), Some(mesh)) = (w.get::<Transform>(e), w.get::<Mesh>(i.prototype)) else {
                    continue;
                };
                let place = |p: [f32; 3]| -> [f64; 3] {
                    std::array::from_fn(|r| {
                        (0..3)
                            .map(|c| f64::from(t.cols[c][r]) * f64::from(p[c]))
                            .sum::<f64>()
                            + f64::from(t.cols[3][r])
                    })
                };
                for p in mesh
                    .primitives()
                    .iter()
                    .filter(|p| p.topology == Topology::Triangles)
                {
                    let Some(pos) = p.positions() else { continue };
                    let corners: Vec<usize> = match &p.indices {
                        Some(ix) => ix.iter().map(|&k| k as usize).collect(),
                        None => (0..pos.len()).collect(),
                    };
                    for c in corners.as_chunks::<3>().0 {
                        if c.iter().all(|&k| k < pos.len()) {
                            triangles.push([place(pos[c[0]]), place(pos[c[1]]), place(pos[c[2]])]);
                        }
                    }
                }
            }
            let mut buckets: BTreeMap<[i64; 3], Vec<usize>> = BTreeMap::new();
            let key = |v: f64| (v / cell).floor() as i64;
            for (n, tri) in triangles.iter().enumerate() {
                let lo: [i64; 3] =
                    std::array::from_fn(|a| key(tri.iter().map(|p| p[a]).fold(f64::INFINITY, f64::min)));
                let hi: [i64; 3] =
                    std::array::from_fn(|a| key(tri.iter().map(|p| p[a]).fold(f64::NEG_INFINITY, f64::max)));
                for x in lo[0]..=hi[0] {
                    for y in lo[1]..=hi[1] {
                        for z in lo[2]..=hi[2] {
                            buckets.entry([x, y, z]).or_default().push(n);
                        }
                    }
                }
            }
            Surfaces {
                cell,
                triangles,
                buckets,
            }
        }

        fn within(&self, p: [f64; 3], d: f64) -> bool {
            let k: [i64; 3] = std::array::from_fn(|a| (p[a] / self.cell).floor() as i64);
            for x in k[0] - 1..=k[0] + 1 {
                for y in k[1] - 1..=k[1] + 1 {
                    for z in k[2] - 1..=k[2] + 1 {
                        for &n in self.buckets.get(&[x, y, z]).into_iter().flatten() {
                            if primase_meshops::point_triangle_distance_squared(p, self.triangles[n]) < d * d
                            {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        }
    }
}

/// The program's choices (`settings.rs`: `AMBIENT_FILL`; `ambient.rs`: clearance and reach).
const SPACING: f64 = 15.0;

/// A level: SkateBIRD's office ambient (trilight, as its scan reads it) in `mode`, and surfaces that keep some lattice
/// points out: a floor across half the level near the lower row, a wall, and a rotated slab.
fn level(mode: i64) -> World {
    let mut w = World::new();
    w.insert_resource(UnityRenderSettings {
        sun: None,
        pipeline: None,
        ambient: Some(UnityAmbient {
            mode,
            sky: [0.5647, 0.5972, 0.6795, 1.0],
            equator: [0.7835, 0.7835, 0.7835, 1.0],
            ground: [0.5457, 0.5457, 0.5457, 1.0],
            intensity: 1.0,
            probe: None,
        }),
        ..Default::default()
    });
    let mut place = |corners: Vec<[f32; 3]>, indices: Option<Vec<u32>>, cols: [[f32; 3]; 4]| {
        let prototype = w.spawn();
        w.insert(
            prototype,
            Mesh::new(vec![Primitive {
                topology: Topology::Triangles,
                streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(corners))]),
                indices,
                material: None,
            }]),
        );
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype,
                variant: 0,
            },
        );
        w.insert(e, Transform { cols });
    };
    let at = |t: [f32; 3]| [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], t];
    // scene frame (x, y, -z of Unity's)
    place(
        vec![
            [-40.0, 0.0, 20.0],
            [5.0, 0.0, 20.0],
            [5.0, 0.0, -55.0],
            [-40.0, 0.0, -55.0],
        ],
        Some(vec![0, 1, 2, 0, 2, 3]),
        at([0.0, 3.1, 0.0]),
    );
    place(
        vec![
            [0.0, 0.0, 0.0],
            [0.0, 25.0, 0.0],
            [0.0, 25.0, -70.0],
            [0.0, 0.0, 0.0],
            [0.0, 25.0, -70.0],
            [0.0, 0.0, -70.0],
        ],
        None,
        at([20.5, -2.0, 18.0]),
    );
    let (s, c) = 0.6f32.sin_cos();
    place(
        vec![
            [-6.0, 0.0, -6.0],
            [6.0, 0.0, -6.0],
            [6.0, 0.0, 6.0],
            [-6.0, 0.0, 6.0],
        ],
        Some(vec![0, 1, 2, 0, 2, 3]),
        [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0], [30.1, 19.0, -25.3]],
    );
    w
}

/// A point light bit for bit: position, colour, intensity, range, shadows.
type LightBits = ([u32; 3], [u32; 3], u32, Option<u32>, Option<bool>);

/// Every point light: its position and light, bit for bit, sorted.
fn lights(w: &World) -> Vec<LightBits> {
    let mut out: Vec<_> = w
        .iter::<Light>()
        .map(|(e, l)| {
            assert_eq!(l.kind, LightKind::Point);
            (
                w.get::<Transform>(e).unwrap().cols[3].map(f32::to_bits),
                l.color.map(f32::to_bits),
                l.intensity.to_bits(),
                l.range.map(f32::to_bits),
                l.cast_shadows,
            )
        })
        .collect();
    out.sort();
    out
}

fn promoted(w: &mut World, probe: UnityAmbientProbe, lo: [f64; 3], hi: [f64; 3]) -> PassLog {
    // Unity's (x, y, z) is the scene's (x, y, -z): the anchor is the Unity minimum corner, the bounds the same box
    let grid = FillGrid::new(
        [lo[0], lo[1], -lo[2]],
        SPACING,
        [[lo[0], lo[1], -hi[2]], [hi[0], hi[1], -lo[2]]],
        FillCells::Meeting,
        SPACING / 4.0,
        SPACING * 1.5,
    )
    .unwrap();
    let mut report = RunReport::default();
    PassManager::new(PassRegistry::new())
        .run(
            w,
            &Pipeline {
                passes: vec![
                    Box::new(LowerUnityAmbient { probe }),
                    Box::new(FillAmbientLights::new(grid)),
                ],
            },
            &mut report,
        )
        .unwrap();
    report.steps.remove(1).log
}

#[test]
fn the_promoted_fill_makes_the_programs_lights() {
    // Unity bounds no whole number of spacings across, so the lattice overhangs them and z's flip matters
    let (lo, hi) = ([-37.3, -2.1, -18.4], [41.9, 23.7, 52.6]);
    for (mode, probe) in [
        (1, UnityAmbientProbe::Omit),
        (1, UnityAmbientProbe::StoredGradient),
        (0, UnityAmbientProbe::StoredGradient),
    ] {
        let mut theirs = level(mode);
        let their_log = program::add_fill(&mut theirs, (lo, hi), SPACING);
        let mut ours = level(mode);
        let our_log = promoted(&mut ours, probe, lo, hi);
        let (a, b) = (lights(&theirs), lights(&ours));
        assert!(a.len() > 10, "mode {mode}: {} lights", a.len());
        assert_eq!(a, b, "mode {mode}");
        let left_out = their_log.counts["ambient fill lights left out (near a drawn surface)"];
        assert!(left_out > 0);
        assert_eq!(
            our_log.counts["ambient fill lights left out (within the clearance of a surface)"],
            left_out
        );
        assert_eq!(
            our_log.counts["ambient fill lights"],
            their_log.counts["ambient fill lights"]
        );
        assert_eq!(
            6 * 2 * 5,
            our_log.counts["ambient fill lights"] + left_out,
            "79.2 x 25.8 x 71 m: 6 x 2 x 5 cells"
        );
        assert!(
            ours.iter::<Light>()
                .all(|(e, _)| ours.get::<Name>(e).unwrap().0.starts_with("ambient fill "))
        );
    }
}
