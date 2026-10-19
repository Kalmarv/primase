//! `snap-reskate-markers` over parts (`convert_parts_summarized` with a partition plan's parts): each part snaps its
//! own markers against what it loaded, with the map's instance counts and water merged over every part, and lands them
//! where the whole map does, whatever order the parts arrive in. A part whose area does not hold a marker's search is
//! refused.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    Finished, PartScope, PartitionGroup, PartitionPlan, PlannedParts, Reader, Report, Result, Summarized,
    Writer, convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::summary::SummaryRegistry;
use primase_core::{
    Collision, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology, Transform,
    World,
};
use primase_reskate::components::{
    MarkerKind, Other, SkateInstanceCounts, SkateMarker, SkateWater, SkateWaterSurfaces, Snap, SnapMode,
};
use primase_reskate::passes::{
    RaiseOptions, SnapReskateMarkers, SummarizeReskateContent, SummarizeReskateInstances,
    SummarizeReskateWater,
};

/// The deck's local box: 60 m by 60 m, 3 m up.
const DECK_BOX: [[f64; 3]; 2] = [[0.0, 3.0, -60.0], [60.0, 3.0, 0.0]];

/// Where the three decks stand (east).
const DECKS: [f32; 3] = [100.0, 1500.0, 4700.0];

/// Flat concrete at height 0 over east -500..5000, north -500..500; three placements of a 288-triangle deck 3 m up
/// (instanced: placed three times), the last one's reach crossing the map's 4780 m limit (an instanced placement there
/// is clipped, its collision dropped); over each deck a marker snapping to the ground from everything from 10 m; one
/// water surface.
fn map() -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    for i in 0..22 {
        for j in 0..4 {
            let (x, z) = (-500.0 + 250.0 * i as f32, 500.0 - 250.0 * j as f32);
            c.extend(
                &[
                    [x, 0.0, z],
                    [x + 250.0, 0.0, z],
                    [x, 0.0, z - 250.0],
                    [x + 250.0, 0.0, z - 250.0],
                ],
                &[[0, 1, 2], [2, 1, 3]],
                concrete,
                |p| p,
            );
        }
    }
    w.insert(ground, c);
    let (mut pos, mut tris) = (Vec::new(), Vec::new());
    for i in 0..12u32 {
        for j in 0..12u32 {
            let (x, z) = (5.0 * i as f32, -5.0 * j as f32);
            let n = pos.len() as u32;
            pos.extend([
                [x, 3.0, z],
                [x + 5.0, 3.0, z],
                [x, 3.0, z - 5.0],
                [x + 5.0, 3.0, z - 5.0],
            ]);
            tris.extend([[n, n + 1, n + 2], [n + 2, n + 1, n + 3]]);
        }
    }
    let deck = w.spawn();
    w.insert(deck, Name("deck".into()));
    w.insert(
        deck,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(pos.clone()))]),
            indices: Some(tris.iter().flatten().copied().collect()),
            material: None,
        }]),
    );
    let mut dc = Collision::default();
    dc.extend(&pos, &tris, concrete, |p| p);
    w.insert(deck, dc);
    for x in DECKS {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: deck,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [x, 0.0, 0.0];
        w.insert(p, t);
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("snap_parts", primase_reskate::passes::LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    for x in DECKS {
        let m = w.spawn();
        w.insert(
            m,
            SkateMarker {
                kind: MarkerKind::Travel,
                position: Some(([x + 20.0, -30.0], Some(10.0))),
                yaw: None,
                m: None,
                blueprint: None,
                snap: Some(Snap::Object {
                    mode: SnapMode::Ground,
                    above: None,
                    from: Some("all".into()),
                    min_up: None,
                    rings: None,
                    directions: None,
                    max_above_ground: None,
                    headroom: None,
                    other: Other::new(),
                }),
                lift: None,
                shelter: None,
                trainer: None,
                other: Other::new(),
            },
        );
    }
    let water = w.spawn();
    w.insert(
        water,
        SkateWater {
            corners: vec![[1200.0, -5.0], [1280.0, -5.0], [1280.0, 5.0], [1200.0, 5.0]],
            height: Some(0.0),
            other: Default::default(),
        },
    );
    w
}

/// The entity's ground east: a placement's or marker's, a water surface's first corner; the ground soup's west edge.
fn east_of(w: &World, e: Entity) -> Option<f32> {
    if let Some(t) = w.get::<Transform>(e)
        && w.has::<Instance>(e)
    {
        return Some(t.cols[3][0]);
    }
    if let Some(m) = w.get::<SkateMarker>(e) {
        return m.position.map(|p| p.0[0]);
    }
    if let Some(s) = w.get::<SkateWater>(e) {
        return Some(s.corners[0][0]);
    }
    w.has::<primase_reskate::components::SkateSoup>(e)
        .then_some(-500.0)
}

/// Three parts by east (west of 1000 m, to 3000 m, beyond), each area its own span and `halo` around it, in `order`.
fn plan(w: &World, halo: f64, order: &[usize]) -> PartitionPlan {
    let spans = [(-500.0, 1000.0), (1000.0, 3000.0), (3000.0, 5000.0)];
    let mut groups: Vec<PartitionGroup> = spans
        .iter()
        .map(|&(x0, x1)| PartitionGroup {
            key: format!("east {x0}"),
            roots: w
                .entities()
                .filter(|&e| east_of(w, e).is_some_and(|x| f64::from(x) >= x0 && f64::from(x) < x1))
                .collect(),
            context: BTreeSet::new(),
            area: Some([x0 - halo, -500.0 - halo, x1 + halo, 500.0 + halo]),
        })
        .collect();
    groups = order.iter().map(|&i| groups[i].clone()).collect();
    PartitionPlan {
        by: "east".into(),
        groups,
    }
}

/// Reads the map with its plan; loads each part's closure under the same ids; the deck's authored box.
struct Planned(World);
impl Reader for Planned {
    fn read(&mut self) -> Result<(World, Report)> {
        Ok((self.0.clone(), Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "skate", "pbr"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        Some(DECK_BOX)
    }
}

/// Each marker written: its position as bits, its snap and lift.
type Markers = BTreeMap<Entity, (Option<([u32; 2], Option<u32>)>, Option<Snap>, Option<u64>)>;

fn markers(w: &World, keep: impl Fn(Entity) -> bool) -> Markers {
    w.iter::<SkateMarker>()
        .filter(|&(e, _)| keep(e))
        .map(|(e, m)| {
            (
                e,
                (
                    m.position
                        .map(|(p, h)| (p.map(f32::to_bits), h.map(f32::to_bits))),
                    m.snap.clone(),
                    m.lift.map(f64::to_bits),
                ),
            )
        })
        .collect()
}

/// What the parts wrote: their own markers, and the merged instance counts and water each part was handed.
#[derive(Default)]
struct Got {
    markers: Markers,
    counts: Vec<SkateInstanceCounts>,
    water: Vec<SkateWaterSurfaces>,
}

struct Capture(Rc<RefCell<Got>>);
impl Writer for Capture {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let mut g = self.0.borrow_mut();
        let own = part
            .resource::<PartScope>()
            .map(|s| part.closure(s.roots.iter().copied()));
        g.markers
            .extend(markers(part, |e| own.as_ref().is_none_or(|o| o.contains(&e))));
        g.counts
            .push(part.resource::<SkateInstanceCounts>().unwrap().clone());
        g.water
            .push(part.resource::<SkateWaterSurfaces>().unwrap().clone());
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn in_parts(halo: f64, order: &[usize]) -> Result<Got> {
    let mut w = map();
    let p = plan(&w, halo, order);
    w.insert_resource(p);
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let got = Rc::new(RefCell::new(Got::default()));
    let summarize: Vec<Box<dyn Pass>> = vec![
        Box::new(SummarizeReskateInstances),
        Box::new(SummarizeReskateWater),
        Box::new(SummarizeReskateContent),
    ];
    convert_parts_summarized(
        &mut Planned(w),
        &mut PlannedParts::default(),
        Box::new(Capture(got.clone())),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Summarized {
            registry: &registry,
            summarize: &Pipeline { passes: summarize },
            decide: &Pipeline { passes: Vec::new() },
        },
        &Pipeline {
            passes: vec![Box::new(SnapReskateMarkers::for_default_encoding())],
        },
    )?;
    Ok(got.take())
}

/// The markers' heights as the whole map snaps them.
fn heights(m: &Markers) -> Vec<Option<f32>> {
    m.values()
        .map(|(p, _, _)| p.and_then(|(_, h)| h).map(f32::from_bits))
        .collect()
}

#[test]
fn markers_snapped_over_parts_land_where_the_whole_map_lands_them() {
    // the whole map: the first two decks collide (3 m), the third, instanced across the limit, does not (the ground)
    let mut whole = map();
    SnapReskateMarkers::for_default_encoding()
        .run(&mut whole, &mut PassLog::default())
        .unwrap();
    let expected = markers(&whole, |_| true);
    assert_eq!(heights(&expected), [Some(3.0), Some(3.0), Some(0.0)]);
    // the control: the third deck counted alone (as its part holds it) is not instanced, and collides
    let mut alone = map();
    let others: Vec<Entity> = alone
        .iter::<Instance>()
        .map(|(e, _)| e)
        .filter(|&e| alone.get::<Transform>(e).unwrap().cols[3][0] < 4000.0)
        .collect();
    for e in others {
        alone.despawn(e);
    }
    SnapReskateMarkers::for_default_encoding()
        .run(&mut alone, &mut PassLog::default())
        .unwrap();
    assert_eq!(heights(&markers(&alone, |_| true))[2], Some(3.0));
    // the summaries over the whole map, to compare the merged ones with
    let mut summed = map();
    SummarizeReskateInstances
        .run(&mut summed, &mut PassLog::default())
        .unwrap();
    SummarizeReskateWater
        .run(&mut summed, &mut PassLog::default())
        .unwrap();
    let counts = summed.resource::<SkateInstanceCounts>().unwrap().clone();
    assert_eq!(
        counts
            .by_prototype
            .values()
            .map(BTreeSet::len)
            .collect::<Vec<_>>(),
        [3]
    );
    let water = summed.resource::<SkateWaterSurfaces>().unwrap().clone();
    assert_eq!(water.surfaces.len(), 1);
    for order in [[0, 1, 2], [2, 0, 1], [1, 2, 0]] {
        let got = in_parts(400.0, &order).unwrap();
        assert_eq!(got.markers, expected, "{order:?}");
        assert_eq!(got.counts, vec![counts.clone(); 3], "{order:?}");
        assert_eq!(got.water, vec![water.clone(); 3], "{order:?}");
    }
}

#[test]
fn a_part_whose_area_does_not_hold_a_markers_search_is_refused() {
    // a 100 m halo: the east part's area ends at 5100 m; the third marker's search reads cells to 5120 m
    let e = in_parts(100.0, &[0, 1, 2]).err().unwrap().to_string();
    assert!(
        e.contains("search reads the ground over") && e.contains("beyond the part's area"),
        "{e}"
    );
    // a part stating no area
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<SkateMarker>().map(|(e, _)| e).collect();
    for p in [
        Box::new(SummarizeReskateInstances) as Box<dyn Pass>,
        Box::new(SummarizeReskateWater),
        Box::new(SummarizeReskateContent),
    ] {
        p.run(&mut w, &mut PassLog::default()).unwrap();
    }
    w.insert_resource(PartScope {
        roots: roots.clone(),
        context: BTreeSet::new(),
        whole: false,
        area: None,
        first_made: None,
    });
    let e = SnapReskateMarkers::for_default_encoding()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("states no area"), "{e}");
    // a part missing a merged input, by the pass that makes it
    for (missing, pass) in [
        ("skate.instance_counts", "summarize-reskate-instances"),
        ("skate.water_surfaces", "summarize-reskate-water"),
    ] {
        let mut w = map();
        if missing != "skate.instance_counts" {
            SummarizeReskateInstances
                .run(&mut w, &mut PassLog::default())
                .unwrap();
        }
        w.insert_resource(PartScope {
            roots: roots.clone(),
            context: BTreeSet::new(),
            whole: false,
            area: Some([-1e4, -1e4, 1e4, 1e4]),
            first_made: None,
        });
        let e = SnapReskateMarkers::for_default_encoding()
            .run(&mut w, &mut PassLog::default())
            .unwrap_err();
        assert!(e.contains(pass), "{missing}: {e}");
    }
}
