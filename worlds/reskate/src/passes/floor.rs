//! `prepare-reskate-floor{min_height=H,stub_below=S[,floor=F]}`: the map's floor, chosen by the program, and the move
//! that puts it at `min_height`.
//!
//! skate. needs the ground above a height; a map built from a source that lies lower is lifted. The pass measures
//! every placed part (each placement's prototype primitives, through the placement's transform; without any, the
//! lowest point of each world collision triangle): its floor is the 1st percentile of the parts' lowest points, or
//! `floor` when the program pins it. The lift is `min_height - floor` (none when the floor is already at or above
//! it); the cutoff is `floor - stub_below`. Then it applies both to the world itself, so the writer writes what it
//! gets:
//! - a part whose highest point lies below the cutoff is left out of its placement (`skate.parts_off`), and a
//!   placement left with none is removed; a world collision triangle wholly below the cutoff is left out (a hull soup
//!   whole when its highest point is), and a soup left with none is removed; every part, placement and triangle
//!   counted;
//! - every coordinate moves up by the lift: placements, world collision, lights, markers (a marker without a height
//!   finds the lifted ground), grind curves and water surfaces.
//!
//! The plan is recorded (`skate.floor_plan`): floor, lift, cutoff and what was left out. Without this pass nothing is
//! lifted or left out. The measurement uses the map writer's frame and arithmetic, so a map lifted here matches one
//! the writer used to lift itself.

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Entity, Instance, Mesh, Transform, World};

use crate::components::{
    CollisionMode, SkateCollision, SkateFloorPlan, SkateGrindCurve, SkateLight, SkateMarker, SkatePartsOff,
    SkatePrototype, SkateRock, SkateScene, SkateWater,
};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct PrepareReskateFloor {
    min_height: f32,
    stub_below: f32,
    floor: Option<f32>,
}

impl PrepareReskateFloor {
    /// The floor lifted to `min_height` (finite metres); geometry wholly more than `stub_below` (finite, 0 or more)
    /// below the floor left out.
    pub fn new(min_height: f32, stub_below: f32) -> Result<PrepareReskateFloor, String> {
        if !min_height.is_finite() {
            return Err(format!("min_height {min_height} is not finite metres"));
        }
        if !(stub_below.is_finite() && stub_below >= 0.0) {
            return Err(format!(
                "stub_below {stub_below} is not finite metres of 0 or more"
            ));
        }
        Ok(PrepareReskateFloor {
            min_height,
            stub_below,
            floor: None,
        })
    }

    /// The floor pinned at `floor` (finite metres) instead of measured.
    pub fn with_pinned_floor(mut self, floor: f32) -> Result<PrepareReskateFloor, String> {
        if !floor.is_finite() {
            return Err(format!("floor {floor} is not finite metres"));
        }
        self.floor = Some(floor);
        Ok(self)
    }

    pub fn min_height(&self) -> f32 {
        self.min_height
    }

    pub fn stub_below(&self) -> f32 {
        self.stub_below
    }

    pub fn pinned_floor(&self) -> Option<f32> {
        self.floor
    }

    /// From its options (`min_height` and `stub_below` required, `floor` optional), through the same constructors.
    pub fn from_options(o: &Options) -> Result<PrepareReskateFloor, String> {
        let number = |k: &str| -> Result<Option<f32>, String> {
            o.get(k)
                .map(|v| {
                    v.parse::<f32>()
                        .map_err(|_| format!("{k}={v} is not a number of metres"))
                })
                .transpose()
        };
        if let Some(k) = o
            .keys()
            .find(|k| !matches!(k.as_str(), "min_height" | "stub_below" | "floor"))
        {
            return Err(format!("unknown option {k}"));
        }
        let (Some(min_height), Some(stub_below)) = (number("min_height")?, number("stub_below")?) else {
            return Err(
                "min_height and stub_below are required: the floor's height and the cutoff below it".into(),
            );
        };
        let pass = PrepareReskateFloor::new(min_height, stub_below)?;
        match number("floor")? {
            Some(f) => pass.with_pinned_floor(f),
            None => Ok(pass),
        }
    }
}

/// [`PrepareReskateFloor::from_options`] as a pass factory.
pub fn prepare_reskate_floor(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(PrepareReskateFloor::from_options(o)?))
}

/// The height of local point `v` placed by `t`, as the map writer computes it in its frame (x, -z, y): the placement's
/// origin height plus its axes' height components times the point's writer coordinates, summed in that order.
fn height(t: &Transform, v: [f32; 3]) -> f32 {
    let c = t.cols;
    // writer frame: the point (x, -z, y), the axes ex = w(c0), ey = -w(c2), ez = w(c1); their heights c0.y, -c2.y, c1.y
    c[3][1] + c[0][1] * v[0] + (-c[2][1]) * (-v[2]) + c[1][1] * v[1]
}

/// Each part's height span `[lowest, highest]` (a part with no positions spans nothing).
fn spans(mesh: &Mesh, t: &Transform) -> Vec<[f32; 2]> {
    mesh.primitives()
        .iter()
        .map(|p| {
            p.positions()
                .unwrap_or(&[])
                .iter()
                .fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], &v| {
                    let z = height(t, v);
                    [lo.min(z), hi.max(z)]
                })
        })
        .collect()
}

/// The floor: `pinned`, else the 1st percentile of `lows`; `None` without either.
fn select_floor(lows: &mut [f32], pinned: Option<f32>) -> Option<f32> {
    if pinned.is_some() {
        return pinned;
    }
    if lows.is_empty() {
        return None;
    }
    lows.sort_by(f32::total_cmp);
    Some(lows[lows.len() / 100])
}

/// The world's collision soups (no placed prototype's), each with the transform that puts it in scene space.
fn world_soups(w: &World) -> Vec<(Entity, Transform)> {
    let owned: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
    w.iter::<Collision>()
        .map(|(e, _)| e)
        .filter(|e| !owned.contains(e) && !w.has::<SkatePrototype>(*e))
        .map(|e| {
            let t = if w.has::<SkateCollision>(e) {
                Transform::IDENTITY
            } else {
                w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY)
            };
            (e, t)
        })
        .collect()
}

impl Pass for PrepareReskateFloor {
    fn name(&self) -> &'static str {
        "prepare-reskate-floor"
    }
    fn options(&self) -> Options {
        let mut o = Options::from([
            ("min_height".to_owned(), self.min_height.to_string()),
            ("stub_below".to_owned(), self.stub_below.to_string()),
        ]);
        if let Some(f) = self.floor {
            o.insert("floor".into(), f.to_string());
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.parts_off", "skate.floor_plan"]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            "skate.scene",
            "core.transform",
            "core.collision",
            "skate.rock",
            "skate.light",
            "skate.marker",
            "skate.grind_curve",
            "skate.water",
        ]
    }
    /// World collision wholly below the cutoff, and placements left with no part.
    fn removes(&self) -> &[&'static str] {
        &["core.collision", "skate.rock"]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        super::require_one_part(w, self.name())?;
        if w.resource::<SkateFloorPlan>().is_some() {
            return Err(
                "prepare-reskate-floor: the world's floor is already prepared (skate.floor_plan)".into(),
            );
        }
        // the placements the map draws and their parts' height spans
        let placed: Vec<(Entity, Vec<[f32; 2]>)> = w
            .iter::<Instance>()
            .filter(|(_, i)| w.has::<SkatePrototype>(i.prototype))
            .map(|(e, i)| {
                let t = w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
                (
                    e,
                    w.get::<Mesh>(i.prototype).map_or_else(Vec::new, |m| spans(m, &t)),
                )
            })
            .collect();
        let mut lows: Vec<f32> = placed
            .iter()
            .flat_map(|(_, s)| s.iter().map(|b| b[0]))
            .filter(|v| v.is_finite())
            .collect();
        let soups = world_soups(w);
        if lows.is_empty() {
            for (e, t) in &soups {
                let c = w.get::<Collision>(*e).expect("listed");
                for tri in &c.triangles {
                    lows.push(
                        tri.iter()
                            .map(|&i| t.apply(c.positions[i as usize])[1])
                            .fold(f32::INFINITY, f32::min),
                    );
                }
            }
        }
        // the scene's own floor statement (min_height) is the program's to apply: this pass applied its choice
        if let Some(scene) = w.resource_mut::<SkateScene>() {
            scene.min_height = None;
        }
        let Some(floor) = select_floor(&mut lows, self.floor) else {
            log.count("maps with nothing to measure a floor on: nothing moved", 1);
            w.insert_resource(SkateFloorPlan {
                floor: None,
                lift: 0.0,
                drop_below: None,
                placements_removed: 0,
                parts_removed: 0,
            });
            return Ok(());
        };
        let lift = (self.min_height - floor).max(0.);
        let drop_below = floor - self.stub_below;
        // parts wholly below the cutoff left out; a placement left with none removed
        let (mut parts_removed, mut placements_removed) = (0u64, 0u64);
        for (e, spans) in placed {
            let off: Vec<bool> = spans.iter().map(|b| b[1] < drop_below).collect();
            let n = off.iter().filter(|&&o| o).count();
            if n == 0 {
                continue;
            }
            parts_removed += n as u64;
            if n == off.len() {
                w.despawn(e);
                placements_removed += 1;
            } else {
                w.insert(e, SkatePartsOff(off));
            }
        }
        // world collision wholly below the cutoff left out (a hull whole)
        let mut triangles_removed = 0u64;
        for (e, t) in &soups {
            let c = w.get::<Collision>(*e).expect("listed");
            let y = |i: u32| t.apply(c.positions[i as usize])[1];
            let hull = w
                .get::<SkateCollision>(*e)
                .is_some_and(|sc| sc.0.mode == Some(CollisionMode::Hull));
            let keep: Vec<bool> = if hull {
                let top = (0..c.positions.len() as u32).map(y).fold(f32::MIN, f32::max);
                vec![top >= drop_below; c.triangles.len()]
            } else {
                c.triangles
                    .iter()
                    .map(|tri| y(tri[0]).max(y(tri[1]).max(y(tri[2]))) >= drop_below)
                    .collect()
            };
            let cut = keep.iter().filter(|k| !**k).count();
            if cut == 0 {
                continue;
            }
            triangles_removed += cut as u64;
            let mut c = c.clone();
            let rock = w.get::<SkateRock>(*e).cloned();
            let mut k = keep.iter();
            c.surfaces.retain(|_| *k.next().expect("one per triangle"));
            let mut k = keep.iter();
            c.triangles.retain(|_| *k.next().expect("one per triangle"));
            if c.triangles.is_empty() {
                w.remove::<Collision>(*e);
                w.remove::<SkateRock>(*e);
                log.count("world collision soups wholly below the cutoff, removed", 1);
                continue;
            }
            if let Some(mut rock) = rock {
                let mut k = keep.iter();
                rock.0.retain(|_| *k.next().expect("one per triangle"));
                w.insert(*e, rock);
            }
            w.insert(*e, c);
        }
        if lift != 0.0 {
            self.lift(w, &soups, lift);
        }
        log.count("parts below the cutoff, left out", parts_removed);
        log.count(
            "placements with every part below the cutoff, removed",
            placements_removed,
        );
        log.count(
            "world collision triangles below the cutoff, left out",
            triangles_removed,
        );
        w.insert_resource(SkateFloorPlan {
            floor: Some(floor),
            lift,
            drop_below: Some(drop_below),
            placements_removed,
            parts_removed,
        });
        Ok(())
    }
}

impl PrepareReskateFloor {
    /// Every coordinate of the map moved up by `lift`.
    fn lift(&self, w: &mut World, soups: &[(Entity, Transform)], lift: f32) {
        let placements: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        for e in placements {
            let mut t = w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY);
            t.cols[3][1] += lift;
            w.insert(e, t);
        }
        for (e, _) in soups {
            if !w.has::<Collision>(*e) {
                continue;
            }
            // a soup placed by a transform of its own moves by it; one in scene space moves its points
            match w.get::<Transform>(*e).copied() {
                Some(mut t) if !w.has::<SkateCollision>(*e) => {
                    t.cols[3][1] += lift;
                    w.insert(*e, t);
                }
                _ => {
                    let mut c = w.get::<Collision>(*e).expect("checked").clone();
                    for p in &mut c.positions {
                        p[1] += lift;
                    }
                    w.insert(*e, c);
                }
            }
        }
        if let Some(mut lights) = w.column_mut::<SkateLight>() {
            for (_, l) in lights.iter_mut() {
                l.position[1] += lift;
            }
        }
        if let Some(mut markers) = w.column_mut::<SkateMarker>() {
            for (_, m) in markers.iter_mut() {
                if let Some((_, Some(h))) = m.position.as_mut() {
                    *h += lift;
                }
                if let Some(t) = m.m.as_mut() {
                    t[10] += lift;
                }
            }
        }
        if let Some(mut curves) = w.column_mut::<SkateGrindCurve>() {
            for (_, c) in curves.iter_mut() {
                for p in &mut c.points {
                    p[1] += lift;
                }
            }
        }
        if let Some(mut water) = w.column_mut::<SkateWater>() {
            for (_, s) in water.iter_mut() {
                if let Some(h) = s.height.as_mut() {
                    *h += f64::from(lift);
                }
            }
        }
    }
}
