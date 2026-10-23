//! `snap-reskate-markers`: the markers that ask for a snap (`street`, `ground`, `open`, or a marker with no height,
//! which snaps to the street) put where the map's collision says, before the writer.
//!
//! The map writer used to snap them while it assembled the map's collision. This pass gives the same answer
//! without writing anything: for each marker it assembles only the 256 m collision cells the marker's search reaches,
//! as the writer files them (the world's collision soups cleaned, the placed prototypes' collision placed by the
//! writer's own rules: [`crate::map::placement_fate`], the hull soups and the water surfaces), and runs the writer's
//! own snap kernels on them. A marker whose snap finds ground gets that position and height and no snap; its default
//! lift for a snapped point (a spawn's 0.3 m, a trainer spot's 0.5 m) is written out, so the writer adds the same.
//! One whose snap finds nothing keeps its position and its snap becomes `none` (counted), as the writer treated it.
//!
//! The collision depends on settings the writer applies too (`limit`, `offset`, instancing: which placements past
//! the limit are dropped whole); they default to the writer's defaults, and the writer refuses a map snapped with
//! settings other than its own (`skate.marker_snaps` records them).
//!
//! A street snap keeps off the water surfaces as they are when it runs: run it before a pass that cuts them
//! (`prepare-reskate-water`) to snap against the uncut water, as the writer did.
//!
//! A map converted in parts snaps each part's markers against what it loaded, with the map's own inputs merged over
//! every part: which prototypes are instanced (`summarize-reskate-instances`), the water surfaces
//! (`summarize-reskate-water`) and, without a stated region, the map's content extent (`summarize-reskate-content`).
//! The part must hold all the ground its own markers' searches read: its source states the area its context covers
//! (`core.part_scope`'s area), and a marker whose search square reaches beyond it is refused, as is a part stating no
//! area or missing a merged input.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use primase_core::World;
use primase_core::convert::PartScope;
use primase_core::pass::{Given, OptionSpec, Options, Pass, PassLog, PassSpec};
use primase_meshops::vector::Vec3;

use super::Owned;
use crate::assembly::{EncodingOptions, SoupWorld, bounds, hulls, writer_placements};
use crate::collision::{PropSoup, Query, Snapped, add_prop, search_square, snap_queries};
use crate::components::{
    MarkerKind, SkateContentBounds, SkateInstanceCounts, SkateMarker, SkateMarkerSnaps, SkateWater,
    SkateWaterSurfaces, Snap, SnapMode as DialectSnap,
};
use crate::map::{Fate, PlacementRules, apron, placed_points, placement_fate};
use crate::model::CollisionMode;
use crate::scene::{Scene, SnapMode, WaterQuad, build_geometry};

/// See the module documentation.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapReskateMarkers {
    basis: SkateMarkerSnaps,
}

impl SnapReskateMarkers {
    /// With the writer's default encoding ([`EncodingOptions::default`]).
    pub fn for_default_encoding() -> SnapReskateMarkers {
        let o = EncodingOptions::default();
        SnapReskateMarkers {
            basis: SkateMarkerSnaps {
                limit: o.limit,
                offset: o.offset.map(|v| v as f32),
                min_instances: o.min_instances,
                min_instance_tris: o.min_instance_tris,
                max_render_edge: o.max_render_edge,
            },
        }
    }

    /// For the encoding the map will be written with (its limit, offset and instancing), through the setters' checks.
    pub fn for_encoding(e: &EncodingOptions) -> Result<SnapReskateMarkers, String> {
        SnapReskateMarkers::for_default_encoding()
            .with_limit(e.limit)?
            .with_offset(e.offset.map(|v| v as f32))?
            .with_instancing(e.min_instances, e.min_instance_tris, e.max_render_edge)
    }

    /// The map's half extent the writer is given (finite, above 20 m).
    pub fn with_limit(mut self, limit: f32) -> Result<SnapReskateMarkers, String> {
        if !(limit.is_finite() && limit > 20.0) {
            return Err(format!("limit {limit} is not finite metres above 20"));
        }
        self.basis.limit = limit;
        Ok(self)
    }

    /// The map offset the writer is given (finite metres, east and north).
    pub fn with_offset(mut self, offset: [f32; 2]) -> Result<SnapReskateMarkers, String> {
        if !offset.iter().all(|v| v.is_finite()) {
            return Err(format!("offset {offset:?} is not finite metres"));
        }
        self.basis.offset = offset;
        Ok(self)
    }

    /// The writer's instancing thresholds (counts of 0 or more) and longest render edge (`None`: the scene's).
    pub fn with_instancing(
        mut self,
        min_instances: i64,
        min_instance_tris: i64,
        max_render_edge: Option<f32>,
    ) -> Result<SnapReskateMarkers, String> {
        if min_instances < 0 || min_instance_tris < 0 {
            return Err("instancing thresholds are counts of 0 or more".into());
        }
        if max_render_edge.is_some_and(|e| !(e.is_finite() && e >= 0.0)) {
            return Err(format!(
                "max_render_edge {max_render_edge:?} is not finite metres of 0 or more"
            ));
        }
        self.basis.min_instances = min_instances;
        self.basis.min_instance_tris = min_instance_tris;
        self.basis.max_render_edge = max_render_edge;
        Ok(self)
    }

    pub fn basis(&self) -> &SkateMarkerSnaps {
        &self.basis
    }

    /// From its options (each the writer's default when not given), through the same setters.
    fn from_given(g: &Given) -> Result<SnapReskateMarkers, String> {
        let number = |k: &str| -> Result<Option<f64>, String> {
            g.optional(k)
                .map(|v| v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let mut pass = SnapReskateMarkers::for_default_encoding();
        if let Some(l) = number("limit")? {
            pass = pass.with_limit(l as f32)?;
        }
        let b = pass.basis.clone();
        let offset = [
            number("offset_x")?.map_or(b.offset[0], |v| v as f32),
            number("offset_y")?.map_or(b.offset[1], |v| v as f32),
        ];
        pass = pass.with_offset(offset)?;
        let count = |k: &str, d: i64| -> Result<i64, String> {
            g.optional(k).map_or(Ok(d), |v| {
                v.parse::<i64>().map_err(|_| format!("{k}={v} is not a count"))
            })
        };
        let edge = number("max_render_edge")?.map(|v| v as f32).or(b.max_render_edge);
        pass.with_instancing(
            count("min_instances", b.min_instances)?,
            count("min_instance_tris", b.min_instance_tris)?,
            edge,
        )
    }
}

const SNAP_OPTIONS: [OptionSpec; 6] = {
    use super::overlays::{LIMIT, MAX_RENDER_EDGE, MIN_INSTANCE_TRIS, MIN_INSTANCES, OFFSET_X, OFFSET_Y};
    [
        LIMIT,
        OFFSET_X,
        OFFSET_Y,
        MIN_INSTANCES,
        MIN_INSTANCE_TRIS,
        MAX_RENDER_EDGE,
    ]
};

/// `snap-reskate-markers`'s declaration ([`primase_core::pass::PassRegistry::add`]): the writer settings the collision
/// depends on, each the writer's unless stated (the render tile is not one of them).
pub const SNAP_RESKATE_MARKERS: PassSpec = PassSpec {
    name: "snap-reskate-markers",
    options: &SNAP_OPTIONS,
    make: |g| Ok(Box::new(SnapReskateMarkers::from_given(g)?)),
};

impl Pass for SnapReskateMarkers {
    fn name(&self) -> &'static str {
        "snap-reskate-markers"
    }
    fn options(&self) -> Options {
        let b = &self.basis;
        let mut o = Options::from([
            ("limit".to_owned(), b.limit.to_string()),
            ("offset_x".to_owned(), b.offset[0].to_string()),
            ("offset_y".to_owned(), b.offset[1].to_string()),
            ("min_instances".to_owned(), b.min_instances.to_string()),
            ("min_instance_tris".to_owned(), b.min_instance_tris.to_string()),
        ]);
        if let Some(e) = b.max_render_edge {
            o.insert("max_render_edge".into(), e.to_string());
        }
        o
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.marker_snaps"]
    }
    fn modifies(&self) -> &[&'static str] {
        &["skate.marker"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> Result<(), String> {
        if w.resource::<SkateMarkerSnaps>().is_some() {
            return Err("snap-reskate-markers: the markers are already snapped (skate.marker_snaps)".into());
        }
        let mut s = Scene::from_world(w, Path::new("")).map_err(|e| e.to_string())?;
        let queries: Vec<Query> = s
            .markers
            .iter()
            .map(|m| Query {
                x: m.x,
                y: m.y,
                z: m.z,
                snap: m.snap.clone(),
            })
            .collect();
        let b = &self.basis;
        if queries.iter().any(|q| q.snap.mode != SnapMode::None) {
            let entities: Vec<primase_core::Entity> = w.iter::<SkateMarker>().map(|(e, _)| e).collect();
            if let Some(scope) = w.resource::<PartScope>().filter(|s| !s.whole) {
                Self::check_part(w, scope, &s, &queries, &entities)?;
            }
            // the map's own inputs, merged over every part when they are
            if let Some(ws) = w.resource::<SkateWaterSurfaces>() {
                s.water = ws
                    .surfaces
                    .values()
                    .map(|(c, h)| WaterQuad::of(c.iter().map(|c| (c[0], c[1])).collect(), *h as f32))
                    .collect();
            }
            let counts = w.resource::<SkateInstanceCounts>().cloned();
            let content = w.resource::<SkateContentBounds>().copied();
            let apron_width = crate::assembly::apron_of(w, &s).map_err(|e| e.to_string())?;
            let (placed, ground) =
                self.snap(&mut s, &queries, apron_width, counts.as_ref(), content.as_ref())?;
            for (i, e) in entities.into_iter().enumerate() {
                if queries[i].snap.mode == SnapMode::None {
                    continue;
                }
                let m = w.get_mut::<SkateMarker>(e).expect("listed");
                if ground[i].is_nan() {
                    // as the writer took one: where it is, a spawn without its snapped lift; the snap stated as
                    // none, since a marker without a height and without a snap asks for a street snap
                    m.snap = Some(Snap::Mode(DialectSnap::None));
                    if m.kind == MarkerKind::Spawn {
                        m.lift = None;
                    }
                    if m.position.is_none() {
                        m.position = Some(([queries[i].x, -queries[i].y], None));
                    }
                    log.count("markers whose snap found nothing: kept where they are", 1);
                    continue;
                }
                m.snap = None;
                let (x, y) = placed[i];
                m.position = Some(([x, -y], Some(ground[i])));
                if let Some(t) = m.m.as_mut() {
                    t[9] = x;
                    t[11] = -y;
                }
                // a snapped point's default lift, as the writer added it
                match m.kind {
                    MarkerKind::Spawn => m.lift = Some(m.lift.unwrap_or(0.3)),
                    MarkerKind::Trainer => m.lift = Some(m.lift.unwrap_or(0.5)),
                    _ => {}
                }
                log.count("markers snapped", 1);
            }
        }
        w.insert_resource(b.clone());
        Ok(())
    }
}

impl SnapReskateMarkers {
    /// A part of a map in several snaps its own markers only with the map's merged inputs and the ground they read.
    fn check_part(
        w: &World,
        scope: &PartScope,
        s: &Scene,
        queries: &[Query],
        entities: &[primase_core::Entity],
    ) -> Result<(), String> {
        let missing = |what: &str, pass: &str| {
            let why = format!("a map in parts snaps its markers with {what} over every part ({pass})");
            match super::require_whole_map(w, "snap-reskate-markers") {
                Err(e) => format!("{e}: {why}"),
                Ok(()) => why,
            }
        };
        if w.resource::<SkateInstanceCounts>().is_none() {
            return Err(missing("the placements counted", "summarize-reskate-instances"));
        }
        if w.resource::<SkateWaterSurfaces>().is_none() {
            return Err(missing("the water surfaces", "summarize-reskate-water"));
        }
        if s.region.is_none() && w.resource::<SkateContentBounds>().is_none() {
            return Err(missing(
                "the content's extent (no region is stated)",
                "summarize-reskate-content",
            ));
        }
        let area = scope.area.ok_or(
            "snap-reskate-markers: this part states no area its context covers, and a snap reads the ground around \
             its marker (a part source that plans by area, as a partition plan's)",
        )?;
        let owned = Owned::of(w);
        for (q, &e) in queries.iter().zip(entities) {
            if q.snap.mode == SnapMode::None || !owned.contains(e) {
                continue;
            }
            let sq = search_square(q.x, q.y);
            if sq[0] < area[0] || sq[1] < area[1] || sq[2] > area[2] || sq[3] > area[3] {
                return Err(format!(
                    "snap-reskate-markers: marker {e:?}'s search reads the ground over {sq:?}, beyond the part's area \
                     {area:?}: plan parts whose context reaches 376 m past what they own"
                ));
            }
        }
        Ok(())
    }

    /// Each query's placed position and ground (NaN: nothing found), as the writer's collision answers it; with the
    /// map's instance counts and content extent merged over every part when given.
    fn snap(
        &self,
        s: &mut Scene,
        queries: &[Query],
        apron_width: f32,
        counts: Option<&SkateInstanceCounts>,
        content: Option<&SkateContentBounds>,
    ) -> Result<Snapped, String> {
        let b = &self.basis;
        // the placements as the writer takes them, its region and the region's apron
        let placements = writer_placements(s, 200.0).map_err(|e| e.to_string())?;
        let region = s
            .region
            .unwrap_or_else(|| content.map_or_else(|| bounds(s, &placements), SkateContentBounds::extent));
        let edge = apron(region, apron_width);
        let rules = PlacementRules {
            min_instances: b.min_instances,
            min_instance_tris: b.min_instance_tris,
            max_render_edge: b.max_render_edge.unwrap_or(s.max_render_edge),
            limit: b.limit,
            region,
        };
        let off = Vec3::new(b.offset[0], b.offset[1], 0.0);
        // the prototypes' geometry and collision, as the writer builds them
        let mut meshes = HashMap::new();
        let placed_protos: std::collections::BTreeSet<u32> = placements.iter().map(|p| p.proto).collect();
        for p in &s.protos {
            if placed_protos.contains(&p.key)
                && let Some(m) = build_geometry(p).map_err(|e| e.to_string())?
            {
                meshes.insert(p.key, m);
            }
        }
        let mut count: HashMap<u32, i64> = HashMap::new();
        match counts {
            // the map's, by each prototype's planning id
            Some(c) => {
                for p in &s.protos {
                    let n = c
                        .by_prototype
                        .get(&u64::from(p.entity.index()))
                        .map_or(0, BTreeSet::len);
                    *count.entry(p.key).or_default() += n as i64;
                }
            }
            None => {
                for p in &placements {
                    *count.entry(p.proto).or_default() += 1;
                }
            }
        }
        // each placement's collision by the writer's rules: props per 256 m cell, hulls whole
        let (mut props, mut order) = (BTreeMap::<(i32, i32), PropSoup>::new(), Vec::new());
        let mut extra: Vec<(Vec<f32>, Vec<u32>, bool)> = Vec::new();
        for p in &placements {
            let m = meshes.get(&p.proto);
            let fate = placement_fate(m, p, |h| count.get(&h).copied().unwrap_or(0), &rules, edge, off);
            let clip = match fate {
                Fate::CollisionOnly { collision: Some(c) }
                | Fate::Drawn {
                    collision: Some(c), ..
                } => c,
                _ => continue,
            };
            let m = m.expect("a fate with collision has a mesh");
            let (Some(cp), Some(ci)) = (&m.col_pos, &m.col_idx) else {
                continue;
            };
            match m.col_mode {
                CollisionMode::Hull => extra.push((placed_points(cp, p), ci.clone(), m.col_ground)),
                CollisionMode::Water => {}
                CollisionMode::TriangleMesh => {
                    add_prop(
                        &mut props,
                        &mut order,
                        cp,
                        ci,
                        &m.col_surf,
                        m.col_ground,
                        p.pos,
                        p.ex,
                        p.ey,
                        p.ez,
                        clip,
                    );
                }
            }
        }
        // the world's soups as the writer files them, and its hull soups
        let cut = s.region;
        let world = SoupWorld::new(s, CollisionMode::TriangleMesh, cut, f32::NEG_INFINITY);
        extra.extend(
            hulls(s, cut, f32::NEG_INFINITY)
                .into_iter()
                .map(|h| (h.pos, h.idx, h.ground)),
        );
        Ok(snap_queries(
            &s.surfaces,
            b.limit,
            b.offset,
            &world,
            &props,
            &extra,
            &s.water,
            queries,
        ))
    }
}

/// `summarize-reskate-instances`: the placements this part owns of each prototype (`skate.instance_counts`); merged
/// over the parts, the map's counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummarizeReskateInstances;

/// `summarize-reskate-instances`'s declaration ([`primase_core::pass::PassRegistry::add`]): it takes no options.
pub const SUMMARIZE_RESKATE_INSTANCES: PassSpec = PassSpec {
    name: "summarize-reskate-instances",
    options: &[],
    make: |_| Ok(Box::new(SummarizeReskateInstances)),
};

impl Pass for SummarizeReskateInstances {
    fn name(&self) -> &'static str {
        "summarize-reskate-instances"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.instance_counts"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> Result<(), String> {
        let s = Scene::from_world(w, Path::new("")).map_err(|e| e.to_string())?;
        let owned = Owned::of(w);
        let mut c = SkateInstanceCounts::default();
        for sp in &s.placements {
            if let Some(k) = owned.key(sp.entity) {
                let proto = &s.protos[sp.proto as usize];
                c.by_prototype
                    .entry(u64::from(proto.entity.index()))
                    .or_default()
                    .insert(k);
            }
        }
        w.insert_resource(c);
        Ok(())
    }
}

/// `summarize-reskate-water`: the water surfaces this part owns (`skate.water_surfaces`); merged over the parts, the
/// map's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SummarizeReskateWater;

/// `summarize-reskate-water`'s declaration ([`primase_core::pass::PassRegistry::add`]): it takes no options.
pub const SUMMARIZE_RESKATE_WATER: PassSpec = PassSpec {
    name: "summarize-reskate-water",
    options: &[],
    make: |_| Ok(Box::new(SummarizeReskateWater)),
};

impl Pass for SummarizeReskateWater {
    fn name(&self) -> &'static str {
        "summarize-reskate-water"
    }
    fn requires(&self) -> &[&'static str] {
        &["skate.scene"]
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.water_surfaces"]
    }
    fn run(&self, w: &mut World, _log: &mut PassLog) -> Result<(), String> {
        let owned = Owned::of(w);
        let surfaces = w
            .iter::<SkateWater>()
            .filter_map(|(e, s)| {
                owned
                    .key(e)
                    .map(|k| (k, (s.corners.clone(), s.height.unwrap_or(0.0))))
            })
            .collect();
        w.insert_resource(SkateWaterSurfaces { surfaces });
        Ok(())
    }
}
