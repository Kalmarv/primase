//! A part's area (`Part::area`, a partition plan's group: what it owns and its halo) against the whole map: what a part
//! loads as context for its operations that read the ground around their own content. A long object owned by one part
//! reaches into the next (loaded by its authored box), the world's collision soup owned by one part reaches every area
//! (loaded by its points' box), and the spawn placed at the map's centre is placed by the part whose area holds the
//! ground its snap reads, not the part holding the map's smallest root: each part's markers land where the whole map
//! lands them.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    Finished, PartitionGroup, PartitionPlan, PlannedParts, Reader, Report, Result, Summarized, Writer,
    convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::summary::SummaryRegistry;
use primase_core::{
    Collision, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology, Transform,
    World,
};
use primase_reskate::components::{MarkerKind, Other, SkateMarker, Snap, SnapMode};
use primase_reskate::passes::{
    PlaceReskateSpawn, RaiseOptions, SnapReskateMarkers, SummarizeReskateContent, SummarizeReskateInstances,
    SummarizeReskateSpawn, SummarizeReskateWater,
};

/// A box mesh's corners, `[x0, y, z0]` to `[x1, y, z1]`, as two triangles.
fn slab(w: &mut World, name: &str, x1: f32, z1: f32, y: f32, surface: u16) -> Entity {
    let pos = vec![[0.0, y, 0.0], [x1, y, 0.0], [0.0, y, z1], [x1, y, z1]];
    let tris = vec![[0u32, 1, 2], [2, 1, 3]];
    let e = w.spawn();
    w.insert(e, Name(name.into()));
    w.insert(
        e,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(Semantic::Position, Stream::F32x3(pos.clone()))]),
            indices: Some(tris.iter().flatten().copied().collect()),
            material: None,
        }]),
    );
    let mut c = Collision::default();
    c.extend(&pos, &tris, surface, |p| p);
    w.insert(e, c);
    e
}

fn place(w: &mut World, prototype: Entity, east: f32, north: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        Instance {
            prototype,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [east, 0.0, -north];
    w.insert(e, t);
    e
}

/// Flat ground at height 0 over east -100..2700, north -300..300 (one soup, the west part's); an 800 m plank 5 m up from
/// east 900 (the west part's, reaching into the middle part); two rocks at east 100 and 2500 (the map's centre, from
/// its placements, at east 1300: the middle part's); a marker at east 1150 over the plank snapping to the ground from
/// everything from 10 m.
fn map() -> World {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    for i in 0..28 {
        for j in 0..6 {
            let (x, z) = (-100.0 + 100.0 * i as f32, 300.0 - 100.0 * j as f32);
            c.extend(
                &[
                    [x, 0.0, z],
                    [x + 100.0, 0.0, z],
                    [x, 0.0, z - 100.0],
                    [x + 100.0, 0.0, z - 100.0],
                ],
                &[[0, 1, 2], [2, 1, 3]],
                concrete,
                |p| p,
            );
        }
    }
    w.insert(ground, c);
    let plank = slab(&mut w, "plank", 800.0, -4.0, 5.0, concrete);
    place(&mut w, plank, 900.0, 0.0);
    let rock = slab(&mut w, "rock", 2.0, -2.0, 0.5, concrete);
    place(&mut w, rock, 100.0, 0.0);
    place(&mut w, rock, 2500.0, 0.0);
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new(
            "area",
            primase_reskate::passes::LightCalibration::Standard,
            primase_reskate::passes::Unclassified::LeaveOut,
        )
        .unwrap(),
    )
    .unwrap();
    let m = w.spawn();
    w.insert(
        m,
        SkateMarker {
            kind: MarkerKind::Travel,
            position: Some(([1150.0, -2.0], Some(10.0))),
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
    w
}

/// Three parts by east: to 1000 m, to 2000 m, beyond; each area its span and a 700 m halo (a snap reads up to 632 m
/// around its marker). The ground soup is the west part's.
fn plan(w: &World) -> PartitionPlan {
    let east = |e: Entity| -> Option<f32> {
        if let Some(t) = w.get::<Transform>(e)
            && w.has::<Instance>(e)
        {
            return Some(t.cols[3][0]);
        }
        if let Some(m) = w.get::<SkateMarker>(e) {
            return m.position.map(|p| p.0[0]);
        }
        (w.has::<Collision>(e) && !w.has::<Mesh>(e)).then_some(-100.0)
    };
    let halo = 700.0;
    PartitionPlan {
        by: "east".into(),
        groups: [(-200.0, 1000.0), (1000.0, 2000.0), (2000.0, 2800.0)]
            .iter()
            .map(|&(x0, x1)| PartitionGroup {
                key: format!("east {x0}"),
                roots: w
                    .entities()
                    .filter(|&e| east(e).is_some_and(|x| f64::from(x) >= x0 && f64::from(x) < x1))
                    .collect(),
                context: BTreeSet::new(),
                area: Some([x0 - halo, -300.0 - halo, x1 + halo, 300.0 + halo]),
            })
            .collect(),
    }
}

/// Reads the map with its plan; loads each part's closure under the same ids; a prototype's authored box from its mesh.
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
    fn prototype_box(&self, world: &World, prototype: Entity) -> Option<[[f64; 3]; 2]> {
        let mesh = world.get::<Mesh>(prototype)?;
        let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
        for p in mesh.primitives() {
            if let Some(Stream::F32x3(pos)) = p.streams.get(&Semantic::Position) {
                for v in pos {
                    for k in 0..3 {
                        b[0][k] = b[0][k].min(f64::from(v[k]));
                        b[1][k] = b[1][k].max(f64::from(v[k]));
                    }
                }
            }
        }
        Some(b)
    }
}

/// Each marker written: its position as bits, and its snap.
type Markers = BTreeMap<Entity, (Option<([u32; 2], Option<u32>)>, Option<Snap>)>;

fn markers(w: &World) -> Markers {
    w.iter::<SkateMarker>()
        .map(|(e, m)| {
            (
                e,
                (
                    m.position
                        .map(|(p, h)| (p.map(f32::to_bits), h.map(f32::to_bits))),
                    m.snap.clone(),
                ),
            )
        })
        .collect()
}

struct Capture(Rc<RefCell<Markers>>);
impl Writer for Capture {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let mut got = self.0.borrow_mut();
        for (e, m) in markers(part) {
            assert!(got.insert(e, m).is_none(), "{e:?} written by two parts");
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn program() -> Vec<Box<dyn Pass>> {
    vec![
        Box::new(PlaceReskateSpawn::centre()),
        Box::new(SnapReskateMarkers::for_default_encoding()),
    ]
}

#[test]
fn context_by_area_holds_what_each_parts_markers_read() {
    let mut whole = map();
    for p in program() {
        p.run(&mut whole, &mut PassLog::default()).unwrap();
    }
    let expected = markers(&whole);
    // the marker lands on the plank; a spawn is placed at the centre, east 1300
    let height = |m: &Markers, east: f32| {
        m.values()
            .find(|(p, _)| p.is_some_and(|(g, _)| f32::from_bits(g[0]) == east))
            .and_then(|(p, _)| p.and_then(|(_, h)| h).map(f32::from_bits))
    };
    assert_eq!(height(&expected, 1150.0), Some(5.0), "{expected:?}");
    assert_eq!(expected.len(), 2, "{expected:?}");
    let mut w = map();
    let p = plan(&w);
    // the map's smallest root (the ground soup) is the west part's: the spawn's part is another
    assert!(p.groups[0].roots.contains(&w.entities().next().unwrap()));
    w.insert_resource(p);
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let got = Rc::new(RefCell::new(Markers::new()));
    let summarize: Vec<Box<dyn Pass>> = vec![
        Box::new(SummarizeReskateInstances),
        Box::new(SummarizeReskateWater),
        Box::new(SummarizeReskateContent),
        Box::new(SummarizeReskateSpawn),
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
        &Pipeline { passes: program() },
    )
    .unwrap();
    assert_eq!(got.take(), expected);
}
