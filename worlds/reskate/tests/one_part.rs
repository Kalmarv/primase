//! A ReSkate map is written whole: the passes that work on the whole map (floor, region, spawn, marker snaps, lights,
//! water, pause map) refuse a part its source does not state is the whole (a part of a world planned in two cells
//! with a halo, with or without context; two context-free parts of a source with no plan), and run on a part stated
//! whole (the whole world, a one-group plan) and on a world outside the part protocol; and every preparation pass, run
//! by the pass manager as a program runs it, keeps to its declarations.

#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use primase_core::convert::{
    Part, PartScope, PartSource, PartitionGroup, PartitionPlan, PlannedParts, WholeWorld,
};
use primase_core::pass::{Pass, PassLog, PassManager, PassRegistry, RunReport};
use primase_core::pbr::PbrMaterial;
use primase_core::{
    Collision, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, Surfaces, Topology, Transform,
    World,
};
use primase_reskate::passes::{
    ClipReskateRegion, OceanMaterials, PauseExtent, PauseSource, PlaceReskateSpawn, PrepareReskateFloor,
    PrepareReskatePauseMap, PrepareReskateWater, RaiseOptions, SelectReskateLights, SnapReskateMarkers,
    WaterExtent,
};

/// Flat concrete over east 0..512, north 0..256 at height 0; one prototype placed in each 256 m cell (east 10 and
/// east 300, north 10).
fn unraised() -> (World, [Entity; 2]) {
    let mut w = World::new();
    let mut surfaces = Surfaces::default();
    let concrete = surfaces.index("concrete").unwrap();
    w.insert_resource(surfaces);
    let ground = w.spawn();
    let mut c = Collision::default();
    c.extend(
        &[
            [0.0, 0.0, 0.0],
            [512.0, 0.0, 0.0],
            [0.0, 0.0, -256.0],
            [512.0, 0.0, -256.0],
        ],
        &[[0, 1, 2], [2, 1, 3]],
        concrete,
        |p| p,
    );
    w.insert(ground, c);
    let material = w.spawn();
    w.insert(material, Name("stone".into()));
    w.insert(material, PbrMaterial::default());
    let proto = w.spawn();
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    let place = |w: &mut World, east: f32| {
        let e = w.spawn();
        w.insert(
            e,
            Instance {
                prototype: proto,
                variant: 0,
            },
        );
        let mut t = Transform::IDENTITY;
        t.cols[3] = [east, 0.0, -10.0];
        w.insert(e, t);
        e
    };
    let placed = [place(&mut w, 10.0), place(&mut w, 300.0)];
    (w, placed)
}

/// [`unraised`], raised.
fn world() -> (World, [Entity; 2]) {
    let (mut w, placed) = unraised();
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("parts", primase_reskate::passes::LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    (w, placed)
}

/// The two cells, each with a 100 m halo, the west cell owning everything but the east placement.
fn plan(w: &World, [west, east]: [Entity; 2]) -> PartitionPlan {
    let cell = |key: &str, roots: BTreeSet<Entity>, x0: f64| PartitionGroup {
        key: key.into(),
        roots,
        context: BTreeSet::new(),
        area: Some([x0 - 100.0, -100.0, x0 + 356.0, 356.0]),
    };
    let mut west_roots: BTreeSet<Entity> = w.entities().collect();
    west_roots.remove(&east);
    assert!(west_roots.contains(&west));
    PartitionPlan {
        by: "two cells".into(),
        groups: vec![
            cell("cell 0,0", west_roots, 0.0),
            cell("cell 1,0", BTreeSet::from([east]), 256.0),
        ],
    }
}

fn parts(source: &mut dyn PartSource, w: &World) -> Vec<Part> {
    source
        .plan(w, &|_| Some([[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]))
        .unwrap();
    std::iter::from_fn(|| source.next_part().unwrap()).collect()
}

fn passes() -> Vec<Box<dyn Pass>> {
    vec![
        Box::new(PrepareReskateFloor::new(100.0, 250.0).unwrap()),
        Box::new(ClipReskateRegion::new([0.0, 0.0, 512.0, 256.0], 30.0).unwrap()),
        Box::new(PlaceReskateSpawn::centre()),
        Box::new(SnapReskateMarkers::for_default_encoding()),
        Box::new(SelectReskateLights::new(8, 200.0).unwrap()),
        Box::new(PrepareReskateWater::new(WaterExtent::All, OceanMaterials::All).unwrap()),
        Box::new(
            PrepareReskatePauseMap::new(PauseSource::Geometry {
                height: 720,
                extent: PauseExtent::Content,
                drawing: primase_reskate::passes::PauseDrawing {
                    leave_out: primase_reskate::passes::PauseLeaveOut::Nothing,
                    colour: primase_reskate::passes::PauseColour::Mean,
                },
            })
            .unwrap(),
        ),
    ]
}

/// Each pass on a fresh world given `scope` (none: outside the part protocol; and the plan, when `with_plan`): its
/// error, if it refused.
fn run_each(scope: Option<&Part>, with_plan: bool) -> Vec<(&'static str, Result<(), String>)> {
    passes()
        .into_iter()
        .map(|pass| {
            let (mut w, placed) = world();
            // the spawn the snap takes, as place-reskate-spawn makes it before the part is scoped
            if pass.name() == "snap-reskate-markers" {
                PlaceReskateSpawn::centre()
                    .run(&mut w, &mut PassLog::default())
                    .unwrap();
            }
            if with_plan {
                let p = plan(&w, placed);
                w.insert_resource(p);
            }
            if let Some(scope) = scope {
                w.insert_resource(PartScope {
                    roots: scope.roots.clone(),
                    shared: scope.shared.clone(),
                    context: scope.context.clone(),
                    whole: scope.whole,
                    area: scope.area,
                    first_made: None,
                    placed: Default::default(),
                });
            }
            (pass.name(), pass.run(&mut w, &mut PassLog::default()))
        })
        .collect()
}

#[test]
fn a_part_not_stated_whole_is_refused_and_the_whole_map_is_taken() {
    let refused = |scope: &Part, with_plan: bool| {
        for (name, r) in run_each(Some(scope), with_plan) {
            // the region clip is local to each entity, and so is water but for its content extent (which a part takes
            // merged over every part): a part is prepared as the whole is (tests/parts.rs)
            if name == "clip-reskate-region" || name == "prepare-reskate-water" {
                assert!(r.is_ok(), "{r:?}");
                continue;
            }
            let e = r.unwrap_err();
            assert!(
                e.starts_with(name) && e.contains("does not state is the whole"),
                "{e}"
            );
        }
    };
    // the planned parts: the west cell has the east placement as context (its box reaches the halo); the east
    // cell's halo does not reach the west placement, but it reaches the ground soup the west cell owns (east 0..512),
    // which it loads as context; neither is the whole
    let (mut w, placed) = world();
    let ground: Vec<Entity> = w.iter::<Collision>().map(|(e, _)| e).collect();
    assert_eq!(ground.len(), 1);
    let p = plan(&w, placed);
    w.insert_resource(p);
    let planned = parts(&mut PlannedParts::default(), &w);
    assert_eq!(planned.len(), 2);
    assert_eq!(planned[0].context, BTreeSet::from([placed[1]]));
    assert_eq!(planned[1].context, BTreeSet::from([ground[0]]));
    assert!(!planned[0].whole && !planned[1].whole);
    refused(&planned[0], false);
    refused(&planned[1], true);
    // a source of its own giving two context-free parts with no plan in the world: refused by what it states
    let all: BTreeSet<Entity> = world().0.entities().collect();
    refused(
        &Part {
            roots: all.iter().copied().filter(|&e| e != placed[1]).collect(),
            shared: Default::default(),
            context: BTreeSet::new(),
            whole: false,
            area: None,
        },
        false,
    );
    // the whole world, one part; a one-group plan without context; a world outside the part protocol: each runs
    let (mut w, _) = world();
    let whole = parts(&mut WholeWorld::default(), &w);
    assert_eq!(whole.len(), 1);
    assert!(whole[0].whole && whole[0].context.is_empty());
    w.insert_resource(PartitionPlan {
        by: "one cell".into(),
        groups: vec![PartitionGroup {
            key: "cell 0,0".into(),
            roots: w.entities().collect(),
            context: BTreeSet::new(),
            area: None,
        }],
    });
    let one = parts(&mut PlannedParts::default(), &w);
    assert_eq!(one.len(), 1);
    assert!(one[0].whole);
    for scope in [Some(&whole[0]), Some(&one[0]), None] {
        for (name, r) in run_each(scope, false) {
            assert!(r.is_ok(), "{name}: {r:?}");
        }
    }
}

/// Every ReSkate preparation pass, parsed from a program's text and run by the pass manager on the whole world from
/// raise-skate to the blend budget: each one's declarations hold (a name or resource it adds, changes or removes is
/// one it declares), as a conversion's program runs them.
#[test]
fn the_preparation_passes_run_under_the_pass_manager() {
    let (mut w, _) = unraised();
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let program = "raise-skate{name=parts,light_scale=standard}, lower-layered-reskate{overlays=all}, lower-tint-reskate, \
                   lower-vertex-alpha-reskate, prepare-reskate-floor{min_height=100,stub_below=250}, \
                   clip-reskate-region{x0=0,y0=0,x1=512,y1=256,apron=30}, prepare-reskate-grinds, \
                   place-reskate-spawn{at=centre}, snap-reskate-markers, select-reskate-lights{per_cell=8,cell=200}, \
                   prepare-reskate-water{extent=all}, prepare-reskate-pause-map{from=geometry,height=64,extent=content,leave-out=nothing,colour=mean}, \
                   prepare-reskate-textures, budget-reskate-textures{max=64}, prepare-reskate-uv-repeats{decals=omit,blended=cutout}, prepare-reskate-overlays{step=0.004}, \
                   prepare-reskate-blend-order{grid=200}, prepare-reskate-collision{surfaces=coalesce,weld=mm},                    summarize-reskate-map, plan-reskate-blend-budget{objects=pool}";
    let pipeline = registry.parse(program).unwrap();
    let n = pipeline.passes.len();
    let mut report = RunReport::default();
    PassManager::new(registry)
        .with_conservation(primase_core::pass::Conservation::Enforce)
        .run(&mut w, &pipeline, &mut report)
        .unwrap();
    assert_eq!(report.steps.len(), n);
    // every record the program leaves is one the writer takes
    let prepared = primase_reskate::target().check_prepared(&w);
    assert!(
        prepared.unsupported_resources.is_empty(),
        "{:?}",
        prepared.unsupported_resources
    );
}

/// Where the spawn goes is the program's: place-reskate-spawn without a place is refused; at=centre, or east and north,
/// are taken and printed back.
#[test]
fn the_spawn_place_is_stated() {
    let mut registry = PassRegistry::new();
    primase_reskate::register(&mut registry);
    let e = registry.parse("place-reskate-spawn").unwrap_err().to_string();
    assert!(e.contains("one of at or east is required"), "{e}");
    for program in [
        "place-reskate-spawn{at=centre}",
        "place-reskate-spawn{east=10,north=-4}",
    ] {
        let p = registry.parse(program).unwrap();
        let printed = primase_core::pass::pass_text(p.passes[0].as_ref());
        assert_eq!(
            registry.parse(&printed).unwrap().passes[0].options(),
            p.passes[0].options(),
            "{printed}"
        );
    }
}
