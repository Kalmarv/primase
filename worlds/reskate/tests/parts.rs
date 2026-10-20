//! A ReSkate map converted in parts (`convert_parts_summarized`): what the whole-map passes decide over every part is
//! what they decide on the whole map, whatever order the parts arrive in. The floor: measured on each part's own
//! placements, merged, decided once, applied to each part. The light cap: each part's lights capped per cell, merged
//! into the cap over every light, kept by each part. The texture budget: each part's textures weighed (exact sums),
//! merged into the whole map's weights, the heaviest kept in each part. The region clip: local to each entity, so
//! each part is clipped as the whole is. Water's content extent: the content measured on each part, merged into the
//! whole map's box, each part's surfaces clipped to it.

#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use primase_core::convert::{
    Finished, Part, PartSource, Reader, Report, Result, Summarized, Writer, convert_parts_summarized,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, Pipeline, Target};
use primase_core::pbr::{PbrMaterial, TextureRef};
use primase_core::summary::SummaryRegistry;
use primase_core::{ColorSpace, Light, LightKind, TexFormat, TexRole, Texture};
use primase_core::{Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Topology, Transform, World};
use primase_reskate::components::{
    SkateBaseFactor, SkateFloorPlan, SkateLight, SkateMaterial, SkatePartsOff,
};
use primase_reskate::passes::{
    BudgetReskateTextures, DecideReskateFloor, LightCalibration, PrepareReskateFloor, RaiseOptions,
    SelectReskateLights, SummarizeReskateFloor, SummarizeReskateLights, SummarizeReskateTextures,
};

/// A raised map of 150 placements of a one-metre-high slab, at heights from -20 m to 54.5 m in a scattered order, and
/// the first sunk to -200 m (below the floor's cutoff: left out); and 30 point lights across two 200 m cells, their
/// energies repeating (so a cap meets ties of priority).
fn map() -> World {
    let mut w = World::new();
    // six slabs, each its own material and texture, the k-th k + 1 times as wide (as high: the floor sees them alike)
    let protos: Vec<Entity> = (0..6u8)
        .map(|k| {
            let tex = w.spawn();
            w.insert(tex, Name(format!("stone_{k}")));
            w.insert(
                tex,
                Texture {
                    width: 2,
                    height: 2,
                    format: TexFormat::Rgba8Unorm,
                    levels: vec![[40 * k, 200, 100, 255].repeat(4)],
                    color_space: ColorSpace::Srgb,
                    role: TexRole::Color,
                    normal: None,
                },
            );
            let material = w.spawn();
            w.insert(material, Name(format!("stone_{k}")));
            w.insert(
                material,
                PbrMaterial {
                    base_color_texture: Some(TextureRef::new(tex)),
                    ..PbrMaterial::default()
                },
            );
            let width = 2.0 * f32::from(k + 1);
            let proto = w.spawn();
            w.insert(proto, Name(format!("slab_{k}")));
            w.insert(
                proto,
                Mesh::new(vec![Primitive {
                    topology: Topology::Triangles,
                    streams: BTreeMap::from([(
                        Semantic::Position,
                        Stream::F32x3(vec![[0.0, 0.0, 0.0], [width, 1.0, 0.0], [0.0, 0.0, -2.0]]),
                    )]),
                    indices: None,
                    material: Some(material),
                }]),
            );
            proto
        })
        .collect();
    for i in 0..150u32 {
        let p = w.spawn();
        w.insert(
            p,
            Instance {
                prototype: protos[(i * 7 % 6) as usize],
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
    for i in 0..30u32 {
        let l = w.spawn();
        w.insert(
            l,
            Light {
                kind: LightKind::Point,
                color: [1.0; 3],
                intensity: (i % 7 + 1) as f32 * 100.0,
                range: Some(10.0),
                cast_shadows: None,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [(i * 13) as f32, 3.0, (i % 5) as f32 * -10.0];
        w.insert(l, t);
    }
    // a world collision soup: two triangles, one each side of east 200 m, of the one surface
    w.insert_resource(primase_core::Surfaces(vec![primase_core::Surface::new(
        "concrete",
    )]));
    let soup = w.spawn();
    w.insert(
        soup,
        primase_core::Collision {
            positions: vec![
                [100.0, 0.0, -1.0],
                [102.0, 0.0, -1.0],
                [101.0, 0.0, -3.0],
                [300.0, 0.0, -1.0],
                [302.0, 0.0, -1.0],
                [301.0, 0.0, -3.0],
            ],
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            surfaces: vec![0, 0],
        },
    );
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("parts", LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    // water: one inside the content, one across its east edge, one far beyond it
    for x in [100.0, 440.0, 1000.0] {
        let e = w.spawn();
        w.insert(
            e,
            primase_reskate::components::SkateWater {
                corners: vec![[x, -5.0], [x + 80.0, -5.0], [x + 80.0, 5.0], [x, 5.0]],
                height: Some(0.0),
                other: Default::default(),
            },
        );
    }
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

/// The placements and lights in `parts` groups (the `i`th in group `i % parts`), in the order `order` gives the groups.
struct Groups {
    parts: usize,
    order: Vec<usize>,
    groups: Vec<BTreeSet<Entity>>,
    next: usize,
}
impl PartSource for Groups {
    fn plan(&mut self, world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        self.groups = vec![BTreeSet::new(); self.parts];
        let roots = world
            .iter::<Instance>()
            .map(|(e, _)| e)
            .chain(world.iter::<SkateLight>().map(|(e, _)| e))
            .chain(
                world
                    .iter::<primase_reskate::components::SkateWater>()
                    .map(|(e, _)| e),
            )
            .chain(
                world
                    .iter::<primase_core::Collision>()
                    .map(|(e, _)| e)
                    .filter(|&e| !world.iter::<Instance>().any(|(_, i)| i.prototype == e)),
            );
        for (i, e) in roots.enumerate() {
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
            shared: Default::default(),
            context: BTreeSet::new(),
            whole: self.parts == 1,
            area: None,
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

fn plan_of(w: &World) -> Option<Plan> {
    w.resource::<SkateFloorPlan>().map(|p| {
        (
            p.floor.map(f32::to_bits),
            p.lift.to_bits(),
            p.drop_below.map(f32::to_bits),
        )
    })
}

fn lights(w: &World) -> BTreeSet<Entity> {
    w.iter::<SkateLight>().map(|(e, _)| e).collect()
}

/// A floor plan's floor, lift and cutoff, as bits.
type Plan = (Option<u32>, u32, Option<u32>);

/// Each material written: its base texture and its base colour factor, as bits.
type Materials = BTreeMap<Entity, (Option<Entity>, Option<[u32; 4]>)>;

fn materials(w: &World) -> Materials {
    w.iter::<SkateMaterial>()
        .map(|(e, m)| {
            (
                e,
                (m.base, w.get::<SkateBaseFactor>(e).map(|f| f.0.map(f32::to_bits))),
            )
        })
        .collect()
}

/// Every placement written, every part's floor plan, every light and material written.
type Captured = (Placements, BTreeSet<Option<Plan>>, BTreeSet<Entity>, Materials);

struct Capture(Rc<RefCell<Captured>>);
impl Writer for Capture {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let mut c = self.0.borrow_mut();
        c.0.extend(placements(part));
        c.1.insert(plan_of(part));
        c.2.extend(lights(part));
        // a material several parts draw is written the same by each
        for (m, state) in materials(part) {
            if let Some(before) = c.3.insert(m, state) {
                assert_eq!(before, state, "material {m:?} written differently by two parts");
            }
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn floor() -> PrepareReskateFloor {
    PrepareReskateFloor::new(0.0, 5.0).unwrap()
}

/// A program in parts: the summary passes, the decision passes and the passes each part is written with.
type Program = [Vec<Box<dyn Pass>>; 3];

/// The floor's program, `apply` the floor pass of the second traversal.
fn floor_program(apply: PrepareReskateFloor) -> Program {
    [
        vec![Box::new(SummarizeReskateFloor)],
        vec![Box::new(DecideReskateFloor::new(floor()))],
        vec![Box::new(apply)],
    ]
}

fn cap() -> SelectReskateLights {
    SelectReskateLights::new(4, 200.0).unwrap()
}

/// The light cap's program, `apply` the cap of the second traversal.
fn lights_program(apply: SelectReskateLights) -> Program {
    [
        vec![Box::new(SummarizeReskateLights::new(cap()))],
        Vec::new(),
        vec![Box::new(apply)],
    ]
}

fn budget() -> BudgetReskateTextures {
    BudgetReskateTextures::new(3).unwrap()
}

/// The texture budget's program, `apply` the budget of the second traversal.
fn budget_program(apply: BudgetReskateTextures) -> Program {
    [
        vec![Box::new(SummarizeReskateTextures::new(budget()))],
        Vec::new(),
        vec![Box::new(apply)],
    ]
}

/// The map converted in `parts` parts arriving in `order`, with `program`.
fn in_parts(parts: usize, order: &[usize], program: Program) -> Result<Captured> {
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let got = Rc::new(RefCell::new((
        Placements::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        Materials::new(),
    )));
    let [summarize, decide, passes] = program;
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
            summarize: &Pipeline { passes: summarize },
            decide: &Pipeline { passes: decide },
        },
        &Pipeline { passes },
    )?;
    Ok(got.take())
}

#[test]
fn the_floor_over_parts_is_the_whole_maps_in_any_order() {
    let mut whole = map();
    floor().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected = (
        placements(&whole),
        BTreeSet::from([plan_of(&whole)]),
        lights(&whole),
        materials(&whole),
    );
    // the floor is the second lowest of 150 slab bottoms (the 1st percentile, -19.5 m): the map moves up by 19.5 m and
    // the sunk slab, wholly below the cutoff 5 m under the floor, goes
    let (floor_at, lift, _) = expected.1.first().copied().unwrap().unwrap();
    assert_eq!(
        (f32::from_bits(floor_at.unwrap()), f32::from_bits(lift)),
        (-19.5, 19.5)
    );
    assert_eq!(expected.0.len(), 149, "the placement below the cutoff left out");
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        assert_eq!(
            in_parts(3, &order, floor_program(floor())).unwrap(),
            expected,
            "{order:?}"
        );
    }
    // one part, the whole: the same
    assert_eq!(in_parts(1, &[0], floor_program(floor())).unwrap(), expected);
}

#[test]
fn a_part_takes_only_the_decision_made_for_its_settings() {
    let e = in_parts(
        3,
        &[0, 1, 2],
        floor_program(PrepareReskateFloor::new(1.0, 5.0).unwrap()),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("the floor was decided for"), "{e}");
    // a part with no decision is refused, naming the summary passes
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<Instance>().map(|(e, _)| e).take(5).collect();
    w.insert_resource(primase_core::convert::PartScope {
        roots,
        shared: Default::default(),
        context: BTreeSet::new(),
        whole: false,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    let e = floor().run(&mut w, &mut PassLog::default()).unwrap_err();
    assert!(e.contains("summarize-reskate-floor"), "{e}");
}

#[test]
fn the_light_cap_over_parts_is_the_whole_maps_in_any_order() {
    let mut whole = map();
    cap().run(&mut whole, &mut PassLog::default()).unwrap();
    let kept = lights(&whole);
    // 30 lights in two cells, four kept in each
    assert_eq!(kept.len(), 8);
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        assert_eq!(
            in_parts(3, &order, lights_program(cap())).unwrap().2,
            kept,
            "{order:?}"
        );
    }
    assert_eq!(in_parts(1, &[0], lights_program(cap())).unwrap().2, kept);
    // each part capping only its own lights keeps more: the cap is the map's
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<SkateLight>().map(|(e, _)| e).step_by(3).collect();
    w.insert_resource(primase_core::convert::PartScope {
        roots,
        shared: Default::default(),
        context: BTreeSet::new(),
        whole: false,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    let e = cap().run(&mut w, &mut PassLog::default()).unwrap_err();
    assert!(e.contains("summarize-reskate-lights"), "{e}");
    // a summary for another cap is refused
    let e = in_parts(
        3,
        &[0, 1, 2],
        lights_program(SelectReskateLights::new(2, 200.0).unwrap()),
    )
    .unwrap_err()
    .to_string();
    assert!(
        e.contains("the lights were summarized for 4 per 200 m cell"),
        "{e}"
    );
}

#[test]
fn the_texture_budget_over_parts_is_the_whole_maps_in_any_order() {
    let mut whole = map();
    budget().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected = materials(&whole);
    // six textures, a budget of three: the two heaviest (the widest slabs) kept, four drawn as their mean colour
    let replaced = expected
        .values()
        .filter(|(base, f)| base.is_none() && f.is_some())
        .count();
    assert_eq!((expected.len(), replaced), (6, 4));
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        assert_eq!(
            in_parts(3, &order, budget_program(budget())).unwrap().3,
            expected,
            "{order:?}"
        );
    }
    assert_eq!(in_parts(1, &[0], budget_program(budget())).unwrap().3, expected);
    // weights for another budget are refused
    let e = in_parts(
        3,
        &[0, 1, 2],
        budget_program(BudgetReskateTextures::new(2).unwrap()),
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("the textures were weighed for a budget of 3"), "{e}");
}

/// The map's summary merges over its parts: each part's own cull IDs (its collision cells apart) summed, for one
/// grouping and one pool; parts summarized for other settings are refused.
#[test]
fn the_map_summary_sums_over_parts() {
    use primase_reskate::components::{CellsCounted, OverlayGrouping, SkateMapSummary};
    let grouping = OverlayGrouping {
        tile: 200.0,
        min_instances: 4,
        min_instance_tris: 0,
        max_render_edge: 80.0,
        limit: 4000.0,
        offset: [0.0, 0.0],
    };
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let part = |other_cull_ids: usize, cull_ids: usize| {
        let mut w = World::new();
        w.insert_resource(SkateMapSummary {
            grouping,
            cull_ids,
            summed: other_cull_ids,
            shared: BTreeMap::new(),
            water: false,
            cells: CellsCounted::Apart,
            chains: BTreeMap::new(),
        });
        registry.take_summaries(&w)
    };
    let merged = registry
        .merge_order_free(&[part(10, 4096), part(0, 4096), part(32, 4096)])
        .unwrap();
    assert_eq!(merged.resource::<SkateMapSummary>().unwrap().other_cull_ids(), 42);
    let e = registry
        .merge_order_free(&[part(10, 4096), part(1, 2048)])
        .unwrap_err();
    assert!(e.starts_with("skate.map_summary: parts summarized for"), "{e}");
}

/// Each placement's clip and each soup's triangles, as written.
type Clipped = (BTreeMap<Entity, Option<bool>>, BTreeMap<Entity, usize>);

fn clipped(w: &World) -> Clipped {
    (
        w.iter::<primase_reskate::components::SkatePlacement>()
            .map(|(e, p)| (e, p.clip))
            .collect(),
        w.iter::<primase_core::Collision>()
            .filter(|(e, _)| !w.iter::<Instance>().any(|(_, i)| i.prototype == *e))
            .map(|(e, c)| (e, c.triangles.len()))
            .collect(),
    )
}

#[test]
fn the_region_clip_over_parts_is_the_whole_maps() {
    use primase_core::convert::convert_parts;
    use primase_reskate::passes::ClipReskateRegion;
    let clip = || ClipReskateRegion::new([0.0, -10.0, 200.0, 10.0], 30.0).unwrap();
    let mut whole = map();
    clip().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected = clipped(&whole);
    let inside = expected.0.values().filter(|c| **c == Some(false)).count();
    assert!(
        inside > 0 && inside < expected.0.len(),
        "some placements inside, some out"
    );
    assert_eq!(
        expected.1.values().copied().collect::<Vec<_>>(),
        [1],
        "the soup cut to its inside triangle"
    );
    struct Got(Rc<RefCell<Clipped>>);
    impl Writer for Got {
        fn target(&self) -> Target {
            primase_reskate::target()
        }
        fn write(&mut self, part: &World) -> Result<()> {
            let (p, s) = clipped(part);
            let mut g = self.0.borrow_mut();
            g.0.extend(p);
            g.1.extend(s);
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<Finished> {
            Ok(Finished::default())
        }
    }
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        let got = Rc::new(RefCell::new((BTreeMap::new(), BTreeMap::new())));
        convert_parts(
            &mut Prepared(map()),
            &mut Groups {
                parts: 3,
                order: order.clone(),
                groups: Vec::new(),
                next: 0,
            },
            Box::new(Got(got.clone())),
            &PassManager::new(PassRegistry::new()),
            &Pipeline { passes: Vec::new() },
            &Pipeline {
                passes: vec![Box::new(clip())],
            },
        )
        .unwrap();
        assert_eq!(got.take(), expected, "{order:?}");
    }
}

/// Each water surface written: its corners, as bits.
type Waters = BTreeMap<Entity, Vec<[u32; 2]>>;

fn waters(w: &World) -> Waters {
    w.iter::<primase_reskate::components::SkateWater>()
        .map(|(e, s)| (e, s.corners.iter().map(|c| c.map(f32::to_bits)).collect()))
        .collect()
}

#[test]
fn water_clipped_to_the_content_over_parts_is_the_whole_maps() {
    use primase_reskate::passes::{
        OceanMaterials, PrepareReskateWater, SummarizeReskateContent, WaterExtent,
    };
    let water =
        || PrepareReskateWater::new(WaterExtent::Content { margin: 5.0 }, OceanMaterials::Stated).unwrap();
    let mut whole = map();
    water().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected = waters(&whole);
    // the far one goes, the one across the edge is cut, the inside one stays whole
    assert_eq!(expected.len(), 2);
    struct Got(Rc<RefCell<Waters>>);
    impl Writer for Got {
        fn target(&self) -> Target {
            primase_reskate::target()
        }
        fn write(&mut self, part: &World) -> Result<()> {
            self.0.borrow_mut().extend(waters(part));
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<Finished> {
            Ok(Finished::default())
        }
    }
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        let got = Rc::new(RefCell::new(Waters::new()));
        convert_parts_summarized(
            &mut Prepared(map()),
            &mut Groups {
                parts: 3,
                order: order.clone(),
                groups: Vec::new(),
                next: 0,
            },
            Box::new(Got(got.clone())),
            &PassManager::new(PassRegistry::new()),
            &Pipeline { passes: Vec::new() },
            &Summarized {
                registry: &registry,
                summarize: &Pipeline {
                    passes: vec![Box::new(SummarizeReskateContent)],
                },
                decide: &Pipeline { passes: Vec::new() },
            },
            &Pipeline {
                passes: vec![Box::new(water())],
            },
        )
        .unwrap();
        assert_eq!(got.take(), expected, "{order:?}");
    }
    // a part without the merged content is refused
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<Instance>().map(|(e, _)| e).take(5).collect();
    w.insert_resource(primase_core::convert::PartScope {
        roots,
        shared: Default::default(),
        context: BTreeSet::new(),
        whole: false,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    let e = water().run(&mut w, &mut PassLog::default()).unwrap_err();
    assert!(e.contains("summarize-reskate-content"), "{e}");
}

/// Every spawn written, by its ground position (as bits); the parts that wrote one.
fn spawns(w: &World) -> Vec<[u32; 2]> {
    w.iter::<primase_reskate::components::SkateMarker>()
        .filter(|(_, m)| m.kind == primase_reskate::components::MarkerKind::Spawn)
        .map(|(_, m)| m.position.unwrap().0.map(f32::to_bits))
        .collect()
}

#[test]
fn the_spawn_over_parts_is_placed_once_where_the_whole_map_places_it() {
    use primase_reskate::passes::{PlaceReskateSpawn, SummarizeReskateSpawn};
    let mut whole = map();
    PlaceReskateSpawn::centre()
        .run(&mut whole, &mut PassLog::default())
        .unwrap();
    let expected = spawns(&whole);
    assert_eq!(expected.len(), 1);
    struct Got(Rc<RefCell<Vec<Vec<[u32; 2]>>>>);
    impl Writer for Got {
        fn target(&self) -> Target {
            primase_reskate::target()
        }
        fn write(&mut self, part: &World) -> Result<()> {
            self.0.borrow_mut().push(spawns(part));
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<Finished> {
            Ok(Finished::default())
        }
    }
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        let got = Rc::new(RefCell::new(Vec::new()));
        convert_parts_summarized(
            &mut Prepared(map()),
            &mut Groups {
                parts: 3,
                order: order.clone(),
                groups: Vec::new(),
                next: 0,
            },
            Box::new(Got(got.clone())),
            &PassManager::new(PassRegistry::new()),
            &Pipeline { passes: Vec::new() },
            &Summarized {
                registry: &registry,
                summarize: &Pipeline {
                    passes: vec![Box::new(SummarizeReskateSpawn)],
                },
                decide: &Pipeline { passes: Vec::new() },
            },
            &Pipeline {
                passes: vec![Box::new(PlaceReskateSpawn::centre())],
            },
        )
        .unwrap();
        let by_part = got.take();
        // one part placed it, the same one whatever the order, where the whole map does
        assert_eq!(by_part.iter().filter(|s| !s.is_empty()).count(), 1, "{order:?}");
        assert_eq!(by_part.concat(), expected, "{order:?}");
    }
    // a part without the summary is refused
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<Instance>().map(|(e, _)| e).take(5).collect();
    w.insert_resource(primase_core::convert::PartScope {
        roots,
        shared: Default::default(),
        context: BTreeSet::new(),
        whole: false,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    let e = PlaceReskateSpawn::centre()
        .run(&mut w, &mut PassLog::default())
        .unwrap_err();
    assert!(e.contains("summarize-reskate-spawn"), "{e}");
}

#[test]
fn the_pause_picture_over_parts_is_drawn_over_the_whole_maps_content() {
    use primase_reskate::components::SkateContentBounds;
    use primase_reskate::passes::{
        PauseColour, PauseDrawing, PauseExtent, PauseLeaveOut, PauseSource, PrepareReskatePauseMap,
        SkatePausePicture, SummarizeReskateContent,
    };
    let drawing = PauseDrawing {
        leave_out: PauseLeaveOut::Nothing,
        colour: PauseColour::Mean,
    };
    let pause = || {
        PrepareReskatePauseMap::new(PauseSource::Geometry {
            height: 720,
            extent: PauseExtent::Content,
            drawing: drawing.clone(),
        })
        .unwrap()
    };
    // the whole map leaves the extent to the writer, which takes its region: the content's extent
    let mut whole = map();
    pause().run(&mut whole, &mut PassLog::default()).unwrap();
    assert_eq!(
        whole.resource::<SkatePausePicture>(),
        Some(&SkatePausePicture::Geometry {
            height: 720,
            extent: None,
            drawing: drawing.clone()
        })
    );
    let mut measured = map();
    SummarizeReskateContent
        .run(&mut measured, &mut PassLog::default())
        .unwrap();
    let extent = measured.resource::<SkateContentBounds>().unwrap().extent();
    // every part draws over that extent, stated
    struct Got(Rc<RefCell<Vec<SkatePausePicture>>>);
    impl Writer for Got {
        fn target(&self) -> Target {
            primase_reskate::target()
        }
        fn write(&mut self, part: &World) -> Result<()> {
            self.0
                .borrow_mut()
                .push(part.resource::<SkatePausePicture>().unwrap().clone());
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<Finished> {
            Ok(Finished::default())
        }
    }
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    for order in [vec![0, 1, 2], vec![2, 0, 1], vec![1, 2, 0]] {
        let got = Rc::new(RefCell::new(Vec::new()));
        convert_parts_summarized(
            &mut Prepared(map()),
            &mut Groups {
                parts: 3,
                order: order.clone(),
                groups: Vec::new(),
                next: 0,
            },
            Box::new(Got(got.clone())),
            &PassManager::new(PassRegistry::new()),
            &Pipeline { passes: Vec::new() },
            &Summarized {
                registry: &registry,
                summarize: &Pipeline {
                    passes: vec![Box::new(SummarizeReskateContent)],
                },
                decide: &Pipeline { passes: Vec::new() },
            },
            &Pipeline {
                passes: vec![Box::new(pause())],
            },
        )
        .unwrap();
        let stated = SkatePausePicture::Geometry {
            height: 720,
            extent: Some(extent.map(|v| v as f32)),
            drawing: drawing.clone(),
        };
        assert_eq!(got.take(), vec![stated; 3], "{order:?}");
    }
    // the extent as stated is the writer's region exactly (whole metres)
    assert!(extent.iter().all(|&v| f64::from(v as f32) == v));
    // a part without the merged content is refused
    let mut w = map();
    let roots: BTreeSet<Entity> = w.iter::<Instance>().map(|(e, _)| e).take(5).collect();
    w.insert_resource(primase_core::convert::PartScope {
        roots,
        shared: Default::default(),
        context: BTreeSet::new(),
        whole: false,
        area: None,
        first_made: None,
        placed: Default::default(),
    });
    let e = pause().run(&mut w, &mut PassLog::default()).unwrap_err();
    assert!(e.contains("summarize-reskate-content"), "{e}");
}
