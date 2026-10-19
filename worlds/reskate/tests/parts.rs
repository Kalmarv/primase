//! A ReSkate map converted in parts (`convert_parts_summarized`): what the whole-map passes decide over every part is
//! what they decide on the whole map, whatever order the parts arrive in. The floor: measured on each part's own
//! placements, merged, decided once, applied to each part.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    Finished, Part, PartSource, Reader, Report, Result, Summarized, Writer, convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::pbr::PbrMaterial;
use primase_core::summary::SummaryRegistry;
use primase_core::{Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_reskate::components::{SkateFloorPlan, SkatePartsOff};
use primase_reskate::passes::{
    DecideReskateFloor, LightCalibration, PrepareReskateFloor, RaiseOptions, SummarizeReskateFloor,
};

/// A raised map of 150 placements of a one-metre-high slab, at heights from -20 m to 54.5 m in a scattered order, and
/// the first sunk to -200 m (below the floor's cutoff: left out).
fn map() -> World {
    let mut w = World::new();
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(material, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(proto, Name("slab".into()));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [2.0, 1.0, 0.0], [0.0, 0.0, -2.0]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    for i in 0..150u32 {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        let height = if i == 0 {
            -200.0
        } else {
            (i * 37 % 150) as f32 * 0.5 - 20.0
        };
        t.cols[3] = [i as f32 * 3.0, height, -1.0];
        w.insert(p, t);
    }
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("parts", LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    w
}

/// Loads parts of a prepared world, as a reader loads them: each root's closure, under the same ids.
struct Prepared(World);
impl Reader for Prepared {
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
        None
    }
}

/// The placements in `parts` groups (placement `i` in group `i % parts`), in the order `order` gives the groups.
struct Groups {
    parts: usize,
    order: Vec<usize>,
    groups: Vec<BTreeSet<Entity>>,
    next: usize,
}
impl PartSource for Groups {
    fn plan(&mut self, world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.groups = vec![BTreeSet::new(); self.parts];
        for (i, (e, _)) in world.iter::<Instance>().enumerate() {
            self.groups[i % self.parts].insert(e);
        }
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        let Some(&g) = self.order.get(self.next) else {
            return Ok(None);
        };
        self.next += 1;
        Ok(Some(Part {
            roots: self.groups[g].clone(),
            context: BTreeSet::new(),
            whole: self.parts == 1,
        }))
    }
    fn restart(&mut self) -> Result<()> {
        self.next = 0;
        Ok(())
    }
}

/// Each placement written: its height and the parts left out of it; and the floor plan each part recorded.
type Placements = BTreeMap<Entity, (u32, Option<Vec<bool>>)>;

fn placements(w: &World) -> Placements {
    w.iter::<Instance>()
        .map(|(e, _)| {
            (
                e,
                (
                    w.get::<Transform>(e).unwrap().cols[3][1].to_bits(),
                    w.get::<SkatePartsOff>(e).map(|o| o.0.clone()),
                ),
            )
        })
        .collect()
}

fn plan_of(w: &World) -> (Option<u32>, u32, Option<u32>) {
    let p = w.resource::<SkateFloorPlan>().unwrap();
    (
        p.floor.map(f32::to_bits),
        p.lift.to_bits(),
        p.drop_below.map(f32::to_bits),
    )
}

struct Capture(Rc<RefCell<(Placements, BTreeSet<(Option<u32>, u32, Option<u32>)>)>>);
impl Writer for Capture {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let mut c = self.0.borrow_mut();
        c.0.extend(placements(part));
        c.1.insert(plan_of(part));
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn floor() -> PrepareReskateFloor {
    PrepareReskateFloor::new(0.0, 5.0).unwrap()
}

/// The map converted in `parts` parts arriving in `order`, with `apply` as the floor pass of the second traversal.
fn in_parts(
    parts: usize,
    order: &[usize],
    apply: PrepareReskateFloor,
) -> Result<(Placements, BTreeSet<(Option<u32>, u32, Option<u32>)>)> {
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let got = Rc::new(RefCell::new((Placements::new(), BTreeSet::new())));
    let pipeline = |p: Box<dyn Pass>| Pipeline { passes: vec![p] };
    convert_parts_summarized(
        &mut Prepared(map()),
        &mut Groups {
            parts,
            order: order.to_vec(),
            groups: Vec::new(),
            next: 0,
        },
        Box::new(Capture(got.clone())),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Summarized {
            registry: &registry,
            summarize: &pipeline(Box::new(SummarizeReskateFloor)),
            decide: &pipeline(Box::new(DecideReskateFloor::new(floor()))),
        },
        &pipeline(Box::new(apply)),
    )?;
    Ok(got.take())
}

#[test]
fn the_floor_over_parts_is_the_whole_maps_in_any_order() {
    let mut whole = map();
    floor().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected = (placements(&whole), BTreeSet::from([plan_of(&whole)]));
    // the floor is the second lowest of 150 slab bottoms (the 1st percentile, -19.5 m): the map moves up by 19.5 m and
    // the sunk slab, wholly below the cutoff 5 m under the floor, goes
    let (floor_at, lift, _) = expected.1.first().copied().unwrap();
    assert_eq!(
        (f32::from_bits(floor_at.unwrap()), f32::from_bits(lift)),
        (-19.5, 19.5)
    );
    assert_eq!(expected.0.len(), 149, "the placement below the cutoff left out");
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        assert_eq!(in_parts(3, &order, floor()).unwrap(), expected, "{order:?}");
    }
    // one part, the whole: the same
    assert_eq!(in_parts(1, &[0], floor()).unwrap(), expected);
}

#[test]
fn a_part_takes_only_the_decision_made_for_its_settings() {
    let e = in_parts(3, &[0, 1, 2], PrepareReskateFloor::new(1.0, 5.0).unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("the floor was decided for"), "{e}");
    // a part with no decision is refused, naming the summary passes
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<Instance>().map(|(e, _)| e).take(5).collect();
    w.insert_resource(primase_core::convert::PartScope {
        roots,
        context: BTreeSet::new(),
        whole: false,
    });
    let e = floor().run(&mut w, &mut PassLog::default()).unwrap_err();
    assert!(e.contains("summarize-reskate-floor"), "{e}");
}
