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

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use primase_core::World;
use primase_core::pass::{Options, Pass, PassLog};
use primase_meshops::vector::Vec3;

use crate::build::{EncodingOptions, SoupWorld, bounds, hulls, writer_placements};
use crate::collision::{PropSoup, Query, Snapped, add_prop, snap_queries};
use crate::dialect::{MarkerKind, SkateMarker, SkateMarkerSnaps, Snap, SnapMode as DialectSnap};
use crate::map::{Fate, PlacementRules, apron, placed_points, placement_fate};
use crate::model::CollisionMode;
use crate::scene::{MeshCounters, Scene, SnapMode, build_geometry};

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

    /// From its options (all optional, the writer's defaults otherwise), through the same setters.
    pub fn from_options(o: &Options) -> Result<SnapReskateMarkers, String> {
        let number = |k: &str| -> Result<Option<f64>, String> {
            o.get(k)
                .map(|v| v.parse::<f64>().map_err(|_| format!("{k}={v} is not a number")))
                .transpose()
        };
        let known = [
            "limit",
            "offset_x",
            "offset_y",
            "min_instances",
            "min_instance_tris",
            "max_render_edge",
        ];
        if let Some(k) = o.keys().find(|k| !known.contains(&k.as_str())) {
            return Err(format!("unknown option {k}"));
        }
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
            o.get(k).map_or(Ok(d), |v| {
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

/// [`SnapReskateMarkers::from_options`] as a pass factory.
pub fn snap_reskate_markers(o: &Options) -> Result<Box<dyn Pass>, String> {
    Ok(Box::new(SnapReskateMarkers::from_options(o)?))
}

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
        super::require_one_part(w, self.name())?;
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
            let (placed, ground) = self.snap(&mut s, &queries)?;
            let entities: Vec<primase_core::Entity> = w.iter::<SkateMarker>().map(|(e, _)| e).collect();
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
    /// Each query's placed position and ground (NaN: nothing found), as the writer's collision answers it.
    fn snap(&self, s: &mut Scene, queries: &[Query]) -> Result<Snapped, String> {
        let b = &self.basis;
        // the placements as the writer takes them, its region and the region's apron
        let placements = writer_placements(s, 200.0);
        let region = s.region.unwrap_or_else(|| bounds(s, &placements));
        let edge = apron(region);
        let rules = PlacementRules {
            min_instances: b.min_instances,
            min_instance_tris: b.min_instance_tris,
            max_render_edge: b.max_render_edge.unwrap_or(s.max_render_edge),
            limit: b.limit,
            region,
        };
        let off = Vec3::new(b.offset[0], b.offset[1], 0.0);
        // the prototypes' geometry and collision, as the writer builds them
        let mut counters = MeshCounters::default();
        let mut meshes = HashMap::new();
        let placed_protos: std::collections::BTreeSet<u32> = placements.iter().map(|p| p.proto).collect();
        for p in &s.protos {
            if placed_protos.contains(&p.key)
                && let Some(m) = build_geometry(s, p, &mut counters).map_err(|e| e.to_string())?
            {
                meshes.insert(p.key, m);
            }
        }
        let mut count: HashMap<u32, i64> = HashMap::new();
        for p in &placements {
            *count.entry(p.proto).or_default() += 1;
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
