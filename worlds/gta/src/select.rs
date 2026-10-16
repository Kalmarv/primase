//! `gta-select-region`: which placements a region export draws, by the rules the earlier skate exporter used, so a
//! world read with a reach around the region ([`crate::region`]) comes down to what that export placed. Every rule is
//! a choice of that export (not something the game does), counted in the pass log:
//!
//! - an entity is taken when its pivot lies in the region, or, for a real piece of the world (HD or ORPHANHD with no
//!   LOD children), when its archetype's box placed by the entity's transform reaches into it (a box wider than the
//!   reach is judged by its pivot);
//! - SLOD2 and coarser stand-ins are dropped (the level's own far view does that job);
//! - a LOD stand-in with no children linked but a child LOD distance is dropped (the game never shows it near);
//! - a LOD stand-in some of whose children are placed is dropped (the children replace it); one without placed
//!   children is kept ("LOD fallback") unless a real placement's pivot lies inside its box (2 m height slack); a
//!   drawable named like a far-view mesh (`_lod`, `_slod2`) counts as a stand-in there whatever its level;
//! - shadow proxy archetypes (flag 2048), reflection proxy entities (flag words of that export's list), time archetypes
//!   not shown at the chosen hour, placements smaller than a minimum size and placements above a maximum priority are
//!   dropped;
//! - interior instances (no geometry of their own) are dropped; the entities placed in an interior are taken when the
//!   instance's pivot lies in the region, else by the rules above, except their shadow and reflection helper meshes.
//!   Which interiors that export placed at all came from its own interior table; that table is not a pass here, so
//!   interiors are counted, not matched.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, Instance, Name, Transform, World};

use crate::components::{GtaArchetype, GtaEntity};
use crate::region::GTA;

/// How far outside the region a box may reach and still be judged by its box, not its pivot (metres).
const REACH: f32 = 600.0;
/// Reflection proxy entities: whole flag words that export skipped.
const REFLECTION_PROXY_FLAGS: [u32; 6] = [135790592, 135790593, 672661504, 536870912, 35127296, 39321602];

/// The region rules, with the region in GTA's ground coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectRegion {
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// The hour time archetypes are judged at (that export: 12).
    pub hour: u32,
    /// Placements with a higher priority level are dropped (3: none).
    pub max_priority: i32,
    /// Placements whose bounding radius (the archetype's, times the larger of the entity's scales) is below this are
    /// dropped (that export: 0.5 m).
    pub min_size: f32,
}

impl SelectRegion {
    pub fn from_options(o: &Options) -> Result<SelectRegion, String> {
        let num = |k: &str, d: f32| -> Result<f32, String> {
            o.get(k).map_or(Ok(d), |v| {
                v.parse().map_err(|_| format!("{k}: not a number: {v}"))
            })
        };
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1", "hour", "max_priority", "min_size"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        Ok(SelectRegion {
            min: [num("x0", 0.0)?, num("y0", 0.0)?],
            max: [num("x1", 0.0)?, num("y1", 0.0)?],
            hour: num("hour", 12.0)? as u32,
            max_priority: num("max_priority", 3.0)? as i32,
            min_size: num("min_size", 0.0)?,
        })
    }

    fn inside(&self, p: [f32; 3]) -> bool {
        p[0] >= self.min[0] && p[0] < self.max[0] && p[1] >= self.min[1] && p[1] < self.max[1]
    }

    /// Whether the archetype's box, placed by the transform (GTA's frame), reaches into the region.
    fn touches(&self, a: &GtaArchetype, t: &Transform) -> bool {
        let p = t.cols[3];
        if self.inside(p) {
            return true;
        }
        let (c, h) = box_centre_half(a);
        let centre = point(t, c);
        let half = half_extent(t, h);
        if !(half[0] <= REACH && half[1] <= REACH) {
            return false;
        }
        centre[0] + half[0] > self.min[0]
            && centre[0] - half[0] < self.max[0]
            && centre[1] + half[1] > self.min[1]
            && centre[1] - half[1] < self.max[1]
    }
}

fn box_centre_half(a: &GtaArchetype) -> ([f32; 3], [f32; 3]) {
    (
        std::array::from_fn(|i| (a.bb_min[i] + a.bb_max[i]) * 0.5),
        std::array::from_fn(|i| (a.bb_max[i] - a.bb_min[i]) * 0.5),
    )
}

fn point(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| t.cols[3][i] + t.cols[0][i] * p[0] + t.cols[1][i] * p[1] + t.cols[2][i] * p[2])
}

fn half_extent(t: &Transform, h: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| {
        (t.cols[0][i] * h[0]).abs() + (t.cols[1][i] * h[1]).abs() + (t.cols[2][i] * h[2]).abs()
    })
}

/// A drawable named like a far-view mesh (`..._lod`, `..._slod`, `..._slod2`): some maps place such meshes as HD.
fn named_stand_in(name: &str) -> bool {
    let n = name.trim_end_matches(|c: char| c.is_ascii_digit());
    n.ends_with("_lod") || n.ends_with("_slod")
}

/// A placement the rules look at: its entity data, archetype, transform in GTA's frame.
struct Candidate {
    entity: Entity,
    name: String,
    ge: GtaEntity,
    arch: GtaArchetype,
    t: Transform,
}

impl Pass for SelectRegion {
    fn name(&self) -> &'static str {
        "gta-select-region"
    }
    fn options(&self) -> Options {
        [
            ("x0", self.min[0].to_string()),
            ("y0", self.min[1].to_string()),
            ("x1", self.max[0].to_string()),
            ("y1", self.max[1].to_string()),
            ("hour", self.hour.to_string()),
            ("max_priority", self.max_priority.to_string()),
            ("min_size", self.min_size.to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }
    fn requires(&self) -> &[&'static str] {
        &[GtaEntity::NAME, GtaArchetype::NAME]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut all = Vec::new();
        for (e, inst) in w.iter::<Instance>() {
            let (Some(ge), Some(t)) = (w.get::<GtaEntity>(e), w.get::<Transform>(e)) else {
                continue;
            };
            let Some(arch) = w.get::<GtaArchetype>(inst.prototype) else {
                continue;
            };
            let name = w
                .get::<Name>(inst.prototype)
                .map_or(String::new(), |n| n.0.to_lowercase());
            all.push(Candidate {
                entity: e,
                name,
                ge: ge.clone(),
                arch: arch.clone(),
                t: GTA.transform_to_source(t),
            });
        }
        // LOD stand-ins with at least one child placed
        let placed: BTreeSet<(String, u32)> = all
            .iter()
            .filter(|c| c.ge.interior.is_none() || c.ge.interior == Some(c.ge.index))
            .map(|c| (c.ge.ymap.clone(), c.ge.index))
            .collect();
        let live: BTreeSet<(String, u32)> = all
            .iter()
            .filter_map(|c| c.ge.lod_parent.clone())
            .filter(|p| placed.contains(p))
            .collect();
        // interior instances, by (map, index): whether their pivot lies in the region
        let interiors: BTreeMap<(String, u32), bool> = all
            .iter()
            .filter(|c| c.arch.kind == "interior" && c.ge.interior == Some(c.ge.index))
            .map(|c| ((c.ge.ymap.clone(), c.ge.index), self.inside(c.t.cols[3])))
            .collect();
        let mut drop = BTreeSet::new();
        let mut kept: Vec<&Candidate> = Vec::new();
        for c in &all {
            let ge = &c.ge;
            let mut reason: Option<&str> = None;
            let in_interior = ge
                .interior
                .is_some_and(|i| i != ge.index || c.arch.kind != "interior");
            let pivot_in = self.inside(c.t.cols[3]);
            let hd = ge.lod_level == 0 || ge.lod_level == 5;
            let leaf = ge.num_children == 0;
            if c.arch.kind == "interior" {
                reason = Some("interior instance (no geometry of its own)");
            } else if in_interior {
                let whole = ge
                    .interior
                    .and_then(|i| interiors.get(&(ge.ymap.clone(), i)))
                    .copied()
                    .unwrap_or(false);
                if !whole && !self.touches(&c.arch, &c.t) {
                    reason = Some("outside the region");
                } else if ge.flags & 0x0A80_0000 != 0 || ge.flags & 0x2100_0000 == 0x2000_0000 {
                    reason = Some("interior helper mesh");
                } else if ge.num_children > 0 {
                    reason = Some("interior LOD parent");
                }
            } else if matches!(ge.lod_level, 3 | 4 | 6) {
                reason = Some("SLOD2+ stand-in");
            } else if leaf && !hd && ge.child_lod_dist > 0.0 {
                reason = Some("far-view stand-in without linked children");
            } else if !pivot_in && (!leaf || !hd || !self.touches(&c.arch, &c.t)) {
                reason = Some("outside the region");
            } else if !leaf && live.contains(&(ge.ymap.clone(), ge.index)) {
                reason = Some("LOD stand-in replaced by its placed children");
            }
            if reason.is_none() {
                if c.arch.flags & 2048 != 0 {
                    reason = Some("shadow proxy archetype");
                } else if REFLECTION_PROXY_FLAGS.contains(&ge.flags) && !in_interior {
                    reason = Some("reflection proxy entity");
                } else if c
                    .arch
                    .time_flags
                    .is_some_and(|f| (f >> (self.hour % 24)) & 1 == 0)
                {
                    reason = Some("time archetype not shown at the hour");
                } else if c.arch.bs_radius
                    * c.t.cols[0..3]
                        .iter()
                        .map(|col| col.iter().map(|v| v * v).sum::<f32>().sqrt())
                        .fold(0f32, f32::max)
                    < self.min_size
                {
                    reason = Some("smaller than the minimum size");
                } else if ge.priority_level > self.max_priority {
                    reason = Some("priority above the maximum");
                }
            }
            match reason {
                Some(r) => {
                    log.count(&format!("dropped: {r}"), 1);
                    drop.insert(c.entity);
                }
                None => kept.push(c),
            }
        }
        // a kept LOD stand-in (or one named like a far-view mesh) over a kept real placement's pivot goes
        let stand_in =
            |c: &Candidate| (c.ge.lod_level != 0 && c.ge.lod_level != 5) || named_stand_in(&c.name);
        for c in &kept {
            if !stand_in(c) || c.ge.interior.is_some() {
                continue;
            }
            let (cc, h) = box_centre_half(&c.arch);
            let centre = point(&c.t, cc);
            let mut half = half_extent(&c.t, h);
            if !(half[0] <= REACH && half[1] <= REACH) {
                continue;
            }
            half[2] += 2.0;
            let over = kept
                .iter()
                .any(|o| !stand_in(o) && (0..3).all(|i| (o.t.cols[3][i] - centre[i]).abs() <= half[i]));
            if over {
                log.count("dropped: stand-in over placed real geometry", 1);
                drop.insert(c.entity);
            }
        }
        for e in &drop {
            w.despawn(*e);
        }
        log.count("kept", (all.len() - drop.len()) as u64);
        Ok(())
    }
}
