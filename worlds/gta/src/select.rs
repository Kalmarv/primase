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
//! - shadow proxy archetypes (flag 2048), reflection proxy entities (flag words of that export's list), placements
//!   smaller than a minimum size and placements above a maximum priority are dropped (time archetypes are a world
//!   state, `gta.hour`, resolved before this pass);
//! - interior instances (no geometry of their own) are dropped; the entities placed in an interior are taken when the
//!   instance's pivot lies in the region, else by the rules above, except their shadow and reflection helper meshes.
//!   Which interiors that export placed at all came from its own interior table; that table is not a pass here, so
//!   interiors are counted, not matched.
//!
//! It also states how far what it kept reaches on the ground (`gta.region_extent`, [`RegionExtent`]): the union of
//! the kept placements' archetype boxes, each placed by its transform, with a box wider than the reach counted by its
//! pivot (as the rules judge it). The seabed a kept placement draws reaches past the region; `gta-select-water`
//! (`clip=kept`) reads it.
use std::collections::{BTreeMap, BTreeSet};

use primase_core::pass::{Options, Pass, PassLog};
use primase_core::{Component, Entity, Instance, JsonError, Name, Resource, Transform, World};

use crate::components::{GtaArchetype, GtaEntity};
use crate::rect::GroundRect;
use crate::region::GTA;

/// How far outside the region a box may reach and still be judged by its box, not its pivot (metres).
const REACH: f32 = 600.0;
/// Reflection proxy entities: whole flag words that export skipped.
const REFLECTION_PROXY_FLAGS: [u32; 6] = [135790592, 135790593, 672661504, 536870912, 35127296, 39321602];

/// `gta.region_extent`: the ground extent `[x0, y0, x1, y1]` (GTA coordinates) of the placements `gta-select-region`
/// kept (see the module documentation); `None` when it kept none.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct RegionExtent(pub Option<[f32; 4]>);

impl Resource for RegionExtent {
    const NAME: &'static str = "gta.region_extent";
    fn to_json(&self) -> Result<Option<serde_json::Value>, JsonError> {
        primase_core::json::to_value(self).map(Some)
    }
}

/// A placement's optionality, GTA's `priorityLevel` (option values 0-3): required, then optional at high, medium and
/// low density settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PriorityLevel {
    Required = 0,
    OptionalHigh = 1,
    OptionalMedium = 2,
    OptionalLow = 3,
}

impl PriorityLevel {
    /// Option text `v` (`0`-`3`).
    pub fn parse(v: &str) -> Result<PriorityLevel, String> {
        match v {
            "0" => Ok(PriorityLevel::Required),
            "1" => Ok(PriorityLevel::OptionalHigh),
            "2" => Ok(PriorityLevel::OptionalMedium),
            "3" => Ok(PriorityLevel::OptionalLow),
            _ => Err(format!("max_priority: {v} is not 0-3")),
        }
    }

    /// The option text.
    pub fn number(self) -> i32 {
        self as i32
    }
}

/// The region rules (see the module documentation): a placement is kept when its pivot or its box reaches into the
/// region and the rules keep it. Made with [`SelectRegion::new`] (every size and priority kept), its rules set by the
/// checked setters; from options by the same calls (`x0`..`y1` required).
#[derive(Clone, Debug, PartialEq)]
pub struct SelectRegion {
    region: GroundRect,
    max_priority: PriorityLevel,
    min_size: f32,
}

impl SelectRegion {
    /// The rules over `region`, keeping every priority level and size.
    pub fn new(region: GroundRect) -> SelectRegion {
        SelectRegion {
            region,
            max_priority: PriorityLevel::OptionalLow,
            min_size: 0.0,
        }
    }

    /// Placements of a priority level above `level` are dropped.
    pub fn max_priority(mut self, level: PriorityLevel) -> SelectRegion {
        self.max_priority = level;
        self
    }

    /// Placements whose bounding radius (the archetype's, times the larger of the entity's scales) is below `metres`
    /// are dropped: an error unless it is finite and not negative.
    pub fn min_size(mut self, metres: f32) -> Result<SelectRegion, String> {
        if !(metres.is_finite() && metres >= 0.0) {
            return Err(format!("min_size {metres}: finite and not negative"));
        }
        self.min_size = metres;
        Ok(self)
    }

    pub fn region(&self) -> GroundRect {
        self.region
    }

    pub fn from_options(o: &Options) -> Result<SelectRegion, String> {
        for k in o.keys() {
            if !["x0", "y0", "x1", "y1", "max_priority", "min_size"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let region = GroundRect::from_options(o)?
            .ok_or("x0, y0, x1 and y1 are required: the region is the caller's choice")?;
        let mut pass = SelectRegion::new(region);
        if let Some(v) = o.get("max_priority") {
            pass = pass.max_priority(PriorityLevel::parse(v)?);
        }
        if let Some(v) = o.get("min_size") {
            let metres = v.parse().map_err(|_| format!("min_size: not a number: {v}"))?;
            pass = pass.min_size(metres)?;
        }
        Ok(pass)
    }

    fn inside(&self, p: [f32; 3]) -> bool {
        self.region.contains(p[0], p[1])
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
        let [x0, y0, x1, y1] = self.region.get();
        centre[0] + half[0] > x0
            && centre[0] - half[0] < x1
            && centre[1] + half[1] > y0
            && centre[1] - half[1] < y1
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
        let mut o = Options::new();
        self.region.insert_options(&mut o);
        o.insert("max_priority".into(), self.max_priority.number().to_string());
        o.insert("min_size".into(), self.min_size.to_string());
        o
    }
    fn requires(&self) -> &[&'static str] {
        &[GtaEntity::NAME, GtaArchetype::NAME]
    }
    /// The placements the rules leave out.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn provides(&self) -> &[&'static str] {
        &[RegionExtent::NAME]
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
                } else if c.arch.bs_radius
                    * c.t.cols[0..3]
                        .iter()
                        .map(|col| col.iter().map(|v| v * v).sum::<f32>().sqrt())
                        .fold(0f32, f32::max)
                    < self.min_size
                {
                    reason = Some("smaller than the minimum size");
                } else if ge.priority_level > self.max_priority.number() {
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
        // how far what is kept reaches: each box placed, a box wider than the reach by its pivot
        let mut extent: Option<[f32; 4]> = None;
        for c in kept.iter().filter(|c| !drop.contains(&c.entity)) {
            let (cc, h) = box_centre_half(&c.arch);
            let (centre, half) = (point(&c.t, cc), half_extent(&c.t, h));
            let (lo, hi) = if half[0] <= REACH && half[1] <= REACH {
                (
                    [centre[0] - half[0], centre[1] - half[1]],
                    [centre[0] + half[0], centre[1] + half[1]],
                )
            } else {
                let p = c.t.cols[3];
                ([p[0], p[1]], [p[0], p[1]])
            };
            let r = extent.get_or_insert([lo[0], lo[1], hi[0], hi[1]]);
            *r = [r[0].min(lo[0]), r[1].min(lo[1]), r[2].max(hi[0]), r[3].max(hi[1])];
        }
        w.insert_resource(RegionExtent(extent));
        for e in &drop {
            w.despawn(*e);
        }
        log.count("kept", (all.len() - drop.len()) as u64);
        Ok(())
    }
}

/// `gta-select-stand-ins{levels,x0,y0,x1,y1}`: a far view of the map from GTA's own LOD stand-ins. Only the placements
/// whose LOD level (`rage__eLodType`: 1 LOD, 2 SLOD1, 3 SLOD2, 4 SLOD3, 6 SLOD4) is one of `levels` (`2;3;4`, SLOD1
/// to SLOD3, unless given) are kept,
/// given a region only those whose pivot lies in it; every other placement is despawned (the real geometry, the
/// other levels, interiors and what is placed in them), counted per level kept. The levels nest (a SLOD2 stands in
/// for its SLOD1 children, which stand in for their LODs), so levels kept together overlap where both are placed: a
/// pipeline picks the levels it draws.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectStandIns {
    pub levels: BTreeSet<i32>,
    /// `[x0, y0, x1, y1]`, GTA ground coordinates; `None`: the whole map read.
    pub region: Option<[f32; 4]>,
}

impl SelectStandIns {
    pub fn from_options(o: &Options) -> Result<SelectStandIns, String> {
        for k in o.keys() {
            if !["levels", "x0", "y0", "x1", "y1"].contains(&k.as_str()) {
                return Err(format!("unknown option {k}"));
            }
        }
        let levels: BTreeSet<i32> = o
            .get("levels")
            .map_or("2;3;4", String::as_str)
            .split(';')
            .map(|v| match v.trim().parse() {
                Ok(l @ (1..=4 | 6)) => Ok(l),
                _ => Err(format!("levels: {v} is no stand-in level (1-4, 6)")),
            })
            .collect::<Result<_, _>>()?;
        let num = |k: &str| -> Result<Option<f32>, String> {
            o.get(k)
                .map(|v| v.parse().map_err(|_| format!("{k}: not a number: {v}")))
                .transpose()
        };
        let region = match [num("x0")?, num("y0")?, num("x1")?, num("y1")?] {
            [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
            [None, None, None, None] => None,
            _ => return Err("x0, y0, x1 and y1 go together".into()),
        };
        Ok(SelectStandIns { levels, region })
    }
}

impl Pass for SelectStandIns {
    fn name(&self) -> &'static str {
        "gta-select-stand-ins"
    }
    fn options(&self) -> Options {
        let mut o = Options::new();
        o.insert(
            "levels".to_string(),
            self.levels
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join(";"),
        );
        if let Some([x0, y0, x1, y1]) = self.region {
            for (k, v) in [("x0", x0), ("y0", y0), ("x1", x1), ("y1", y1)] {
                o.insert(k.to_string(), v.to_string());
            }
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &[GtaEntity::NAME]
    }
    /// Every placement but the stand-ins kept.
    fn despawns_entities(&self) -> bool {
        true
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        let mut drop = Vec::new();
        for (e, _) in w.iter::<Instance>() {
            let Some(ge) = w.get::<GtaEntity>(e) else {
                continue;
            };
            let reason = if ge.interior.is_some() {
                Some("interior or placed in one")
            } else if !self.levels.contains(&ge.lod_level) {
                Some("not a stand-in of the levels kept")
            } else if let Some([x0, y0, x1, y1]) = self.region {
                let p = w
                    .get::<Transform>(e)
                    .ok_or("a placement without a transform")?
                    .cols[3];
                // core (x, y, z) is GTA (x, -z, y)
                let (gx, gy) = (p[0], -p[2]);
                (!(gx >= x0 && gx < x1 && gy >= y0 && gy < y1)).then_some("outside the region")
            } else {
                None
            };
            match reason {
                Some(r) => {
                    log.count(&format!("dropped: {r}"), 1);
                    drop.push(e);
                }
                None => log.count(&format!("kept: level {}", ge.lod_level), 1),
            }
        }
        for e in drop {
            w.despawn(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod stand_in_tests {
    use super::*;

    fn placed(w: &mut World, lod_level: i32, interior: Option<u32>, x: f32) -> Entity {
        let proto = w.spawn();
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        w.insert(
            e,
            Transform {
                cols: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [x, 0.0, -5.0]],
            },
        );
        w.insert(
            e,
            GtaEntity {
                ymap: "m.ymap".into(),
                index: 0,
                interior,
                lod_parent: None,
                archetype: 0,
                flags: 0,
                guid: 0,
                parent_index: -1,
                lod_dist: 100.0,
                child_lod_dist: 0.0,
                lod_level,
                num_children: 0,
                priority_level: 0,
                ambient_occlusion_multiplier: 255,
                artificial_ambient_occlusion: 255,
                tint_value: 0,
            },
        );
        e
    }

    /// Of an HD, a LOD, a SLOD1, a SLOD2, a SLOD3 and a SLOD2 in an interior, `levels=2;3;4` keeps the three stand-ins
    /// outside interiors; a region (x below 10, GTA y 5 is core z -5) keeps those whose pivot lies in it.
    #[test]
    fn only_the_stand_ins_of_the_levels_kept() {
        let mut w = World::new();
        let hd = placed(&mut w, 0, None, 1.0);
        let lod = placed(&mut w, 1, None, 1.0);
        let s1 = placed(&mut w, 2, None, 1.0);
        let s2 = placed(&mut w, 3, None, 20.0);
        let s3 = placed(&mut w, 4, None, 1.0);
        let inner = placed(&mut w, 3, Some(4), 1.0);
        let mut o = Options::new();
        o.insert("levels".into(), "2;3;4".into());
        let pass = SelectStandIns::from_options(&o).unwrap();
        let mut log = PassLog::default();
        pass.run(&mut w, &mut log).unwrap();
        let alive: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        assert_eq!(alive, vec![s1, s2, s3]);
        assert!(![hd, lod, inner].iter().any(|e| alive.contains(e)));
        assert_eq!(log.counts.get("kept: level 3"), Some(&1));

        for (k, v) in [("x0", "0"), ("y0", "0"), ("x1", "10"), ("y1", "10")] {
            o.insert(k.into(), v.into());
        }
        let mut log = PassLog::default();
        SelectStandIns::from_options(&o)
            .unwrap()
            .run(&mut w, &mut log)
            .unwrap();
        let alive: Vec<Entity> = w.iter::<Instance>().map(|(e, _)| e).collect();
        assert_eq!(alive, vec![s1, s3]);
        assert_eq!(log.counts.get("dropped: outside the region"), Some(&1));
    }

    /// The region's extent is the kept boxes placed; a box wider than the reach counts by its pivot. Two HD leaves with
    /// their pivots in the region (GTA (1, 5) and (2, 5)): a 20 x 10 box from the pivot, and a 1400 m wide one.
    #[test]
    fn the_region_extent_is_the_kept_boxes_with_a_wide_box_by_its_pivot() {
        let mut w = World::new();
        for (x, bb_min, bb_max) in [
            (1.0, [0.0, 0.0, 0.0], [20.0, 10.0, 1.0]),
            (2.0, [-700.0, -700.0, 0.0], [700.0, 700.0, 1.0]),
        ] {
            let e = placed(&mut w, 0, None, x);
            let proto = w.get::<Instance>(e).unwrap().prototype;
            w.insert(
                proto,
                crate::components::GtaArchetype {
                    ytyp: String::new(),
                    name: 1,
                    kind: "base",
                    flags: 0,
                    special_attribute: 0,
                    lod_dist: 100.0,
                    hd_texture_dist: 0.0,
                    bb_min,
                    bb_max,
                    bs_centre: [0.0; 3],
                    bs_radius: 12.0,
                    texture_dictionary: 0,
                    drawable_dictionary: 0,
                    physics_dictionary: 0,
                    clip_dictionary: 0,
                    asset_type: 2,
                    asset_name: 1,
                    time_flags: None,
                },
            );
        }
        SelectRegion::new(GroundRect::new([0.0, 0.0, 10.0, 10.0]).unwrap())
            .run(&mut w, &mut PassLog::default())
            .unwrap();
        assert_eq!(w.iter::<Instance>().count(), 2);
        assert_eq!(
            w.resource::<RegionExtent>(),
            Some(&RegionExtent(Some([1.0, 5.0, 21.0, 15.0])))
        );
    }

    /// Levels are stand-in levels; with none given, SLOD1 to SLOD3.
    #[test]
    fn levels_are_stand_in_levels() {
        let mut o = Options::new();
        o.insert("levels".into(), "0;2".into());
        assert!(SelectStandIns::from_options(&o).is_err());
        assert_eq!(
            SelectStandIns::from_options(&Options::new()).unwrap().levels,
            BTreeSet::from([2, 3, 4])
        );
    }
}
