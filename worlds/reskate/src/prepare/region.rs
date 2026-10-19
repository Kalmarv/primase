//! `clip-reskate-region{x0=,y0=,x1=,y1=}`: the map's region, chosen by the program, and what falls outside it.
//!
//! The region is a ground-frame rectangle `[east0, north0, east1, north1)`, half-open. The pass records it as the
//! scene's (`skate.scene` `region`: the map's extent), marks each placement whose position lies outside it as clipped
//! (`skate.placement` `clip`: the writer leaves a clipped placement out), and cuts the world's collision soups (the
//! `core.collision` no placed prototype owns) at it: a triangle whose centroid lies outside is left out, with its rock
//! flag (a hull soup is kept or left out whole, by the mean of its vertices, as Studio takes a hull whole), and a soup
//! left with none is removed; a `skate.collision` entry is marked cut (`region_cut`), so a scene
//! written from the world says so. A placement that already states its clip, or a soup entry that states
//! `region_cut = false`, keeps it. Every placement clipped and every triangle cut is counted. The writer obeys the
//! flags and decides nothing from the region itself; without this pass nothing is clipped.

use std::collections::BTreeSet;

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Collision, Entity, Instance, Transform, World};

use crate::dialect::{CollisionMode, SkateCollision, SkatePlacement, SkatePrototype, SkateRock, SkateScene};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipReskateRegion {
    region: [f32; 4],
}

impl ClipReskateRegion {
    /// The region `[east0, north0, east1, north1)`, metres: finite, the far bounds above the near ones.
    pub fn new(region: [f32; 4]) -> Result<ClipReskateRegion, String> {
        let [x0, y0, x1, y1] = region;
        if !region.iter().all(|v| v.is_finite()) {
            return Err(format!(
                "the region [{x0}, {y0}, {x1}, {y1}) is not finite metres"
            ));
        }
        if !(x1 > x0 && y1 > y0) {
            return Err(format!("the region [{x0}, {y0}, {x1}, {y1}) is empty"));
        }
        Ok(ClipReskateRegion { region })
    }

    pub fn region(&self) -> [f32; 4] {
        self.region
    }

    /// From its options (all four bounds required), through [`ClipReskateRegion::new`].
    pub fn from_options(o: &Options) -> Result<ClipReskateRegion, String> {
        let mut b = [None; 4];
        for (k, v) in o {
            let i = match k.as_str() {
                "x0" => 0,
                "y0" => 1,
                "x1" => 2,
                "y1" => 3,
                _ => return Err(format!("unknown option {k}")),
            };
            b[i] = Some(
                v.parse::<f32>()
                    .map_err(|_| format!("{k}={v} is not a number of metres"))?,
            );
        }
        let [Some(x0), Some(y0), Some(x1), Some(y1)] = b else {
            return Err("x0, y0, x1 and y1 are required: the region's ground-frame bounds, metres".into());
        };
        ClipReskateRegion::new([x0, y0, x1, y1])
    }
}

/// [`ClipReskateRegion::from_options`] as a pass factory.
pub fn clip_reskate_region(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(ClipReskateRegion::from_options(o)?))
}

impl Pass for ClipReskateRegion {
    fn name(&self) -> &'static str {
        "clip-reskate-region"
    }
    fn options(&self) -> Options {
        ["x0", "y0", "x1", "y1"]
            .into_iter()
            .zip(self.region)
            .map(|(k, v)| (k.to_owned(), v.to_string()))
            .collect()
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn modifies(&self) -> &[&'static str] {
        &[
            "skate.scene",
            "skate.placement",
            "skate.collision",
            "core.collision",
            "skate.rock",
        ]
    }
    /// A world soup with no triangle left inside.
    fn removes(&self) -> &[&'static str] {
        &["core.collision", "skate.rock"]
    }
    fn legalizes(&self) -> &[&'static str] {
        &[]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        super::require_one_part(w, self.name())?;
        let r = self.region;
        let scene = w
            .resource_mut::<SkateScene>()
            .ok_or("clip-reskate-region: the world is no skate scene (raise-skate first)")?;
        scene.region = Some(r);
        // a placement's ground position: scene (x, y, z) is (east, north) = (x, -z), as the writer reads it
        let inside = |t: &Transform| {
            let (east, north) = (f64::from(t.cols[3][0]), -f64::from(t.cols[3][2]));
            east >= f64::from(r[0])
                && east < f64::from(r[2])
                && north >= f64::from(r[1])
                && north < f64::from(r[3])
        };
        let marks: Vec<(primase_core::Entity, bool)> = w
            .iter::<SkatePlacement>()
            .filter(|(_, p)| p.clip.is_none())
            .filter(|(e, _)| w.has::<Instance>(*e))
            .map(|(e, _)| (e, !w.get::<Transform>(e).is_some_and(inside)))
            .collect();
        for (e, outside) in marks {
            w.get_mut::<SkatePlacement>(e).expect("listed").clip = Some(outside);
            if outside {
                log.count("placements outside the region, clipped", 1);
            } else {
                log.count("placements inside the region", 1);
            }
        }
        // the world's soups: collision no placed prototype owns, in scene space through its entity's transform (a
        // skate.collision soup is in scene space as it is)
        let owned: BTreeSet<Entity> = w.iter::<Instance>().map(|(_, i)| i.prototype).collect();
        let soups: Vec<Entity> = w
            .iter::<Collision>()
            .map(|(e, _)| e)
            .filter(|e| !owned.contains(e) && !w.has::<SkatePrototype>(*e))
            .filter(|e| {
                w.get::<SkateCollision>(*e)
                    .is_none_or(|c| c.0.region_cut != Some(false))
            })
            .collect();
        for e in soups {
            let c = w.get::<Collision>(e).expect("listed");
            let t = if w.has::<SkateCollision>(e) {
                Transform::IDENTITY
            } else {
                w.get::<Transform>(e).copied().unwrap_or(Transform::IDENTITY)
            };
            let inside = |east: f64, north: f64| {
                east >= f64::from(r[0])
                    && east < f64::from(r[2])
                    && north >= f64::from(r[1])
                    && north < f64::from(r[3])
            };
            // a hull is kept or cut whole, by the mean of its vertices
            let hull = w
                .get::<SkateCollision>(e)
                .is_some_and(|sc| sc.0.mode == Some(CollisionMode::Hull));
            let keep: Vec<bool> = if hull {
                let n = c.positions.len().max(1) as f64;
                let (east, north) = c.positions.iter().fold((0f64, 0f64), |(x, y), p| {
                    let p = t.apply(*p);
                    (x + f64::from(p[0]), y + f64::from(-p[2]))
                });
                vec![inside(east / n, north / n); c.triangles.len()]
            } else {
                c.triangles
                    .iter()
                    .map(|tri| {
                        let [a, b, d] = tri.map(|i| t.apply(c.positions[i as usize]));
                        let (cx, cz) = ((a[0] + b[0] + d[0]) / 3.0, (a[2] + b[2] + d[2]) / 3.0);
                        inside(f64::from(cx), -f64::from(cz))
                    })
                    .collect()
            };
            let cut = keep.iter().filter(|k| !**k).count();
            if let Some(sc) = w.get_mut::<SkateCollision>(e) {
                sc.0.region_cut = Some(true);
            }
            if cut == 0 {
                continue;
            }
            log.count("world collision triangles outside the region, cut", cut as u64);
            let mut c = w.get::<Collision>(e).expect("listed").clone();
            let rock = w.get::<SkateRock>(e).cloned();
            let mut k = keep.iter();
            c.surfaces.retain(|_| *k.next().expect("one per triangle"));
            let mut k = keep.iter();
            c.triangles.retain(|_| *k.next().expect("one per triangle"));
            if c.triangles.is_empty() {
                w.remove::<Collision>(e);
                w.remove::<SkateRock>(e);
                log.count("world collision soups wholly outside the region, removed", 1);
                continue;
            }
            if let Some(mut rock) = rock {
                let mut k = keep.iter();
                rock.0.retain(|_| *k.next().expect("one per triangle"));
                w.insert(e, rock);
            }
            w.insert(e, c);
        }
        Ok(())
    }
}
