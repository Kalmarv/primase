//! What a part owns for its summaries, against the whole map: the closure of its roots (a light or a placement reached
//! through a root counts; root -> placement -> prototype), not what it only loaded as context, and a dependency two
//! parts both reach counted once.

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
use primase_core::{
    ColorSpace, Component, Entity, Instance, Mesh, Name, Primitive, Semantic, Stream, TexFormat, TexRole,
    Texture, Topology, Transform, World,
};
use primase_reskate::components::{SkateBaseFactor, SkateLight, SkateMaterial, SkateScene};
use primase_reskate::passes::{
    BudgetReskateTextures, DecideReskateFloor, LightCalibration, PrepareReskateFloor, RaiseOptions,
    SelectReskateLights, SummarizeReskateFloor, SummarizeReskateLights, SummarizeReskateTextures,
};

/// A root that reaches other entities (as a group or a rig would).
#[derive(Clone, Debug, PartialEq)]
struct Holds(Vec<Entity>);
impl Component for Holds {
    const NAME: &'static str = "test.holds";
    fn references(&self, out: &mut Vec<Entity>) {
        out.extend(&self.0);
    }
}

/// Loads parts of a prepared world, as a reader loads them: the asked entities' closure, under the same ids.
struct Prepared(World);
impl Reader for Prepared {
    fn read(&mut self) -> Result<(World, Report)> {
        Ok((self.0.clone(), Vec::new()))
    }
    fn load(&mut self, world: &World, roots: &BTreeSet<Entity>) -> Result<(World, Report)> {
        Ok((world.subset(&world.closure(roots.iter().copied())), Vec::new()))
    }
    fn emits(&self) -> &'static [&'static str] {
        &["core", "skate", "pbr", "test"]
    }
    fn prototype_box(&self, _world: &World, _prototype: Entity) -> Option<[[f64; 3]; 2]> {
        None
    }
}

/// The parts given, in order, as often as asked.
struct Given(Vec<Part>, usize);
impl PartSource for Given {
    fn plan(&mut self, _world: &World, _boxes: &dyn Fn(Entity) -> Option<[[f64; 3]; 2]>) -> Result<()> {
        Ok(())
    }
    fn next_part(&mut self) -> Result<Option<Part>> {
        self.1 += 1;
        Ok(self.0.get(self.1 - 1).cloned())
    }
    fn restart(&mut self) -> Result<()> {
        self.1 = 0;
        Ok(())
    }
}

fn part(roots: &[Entity], context: &[Entity]) -> Part {
    Part {
        roots: roots.iter().copied().collect(),
        context: context.iter().copied().collect(),
        whole: false,
        area: None,
    }
}

/// What the parts wrote, by entity: an entity two parts both write must be written the same.
type Wrote = BTreeMap<Entity, String>;

struct Capture(Rc<RefCell<Wrote>>, fn(&World) -> Wrote);
impl Writer for Capture {
    fn target(&self) -> Target {
        primase_reskate::target()
    }
    fn write(&mut self, part: &World) -> Result<()> {
        let mut got = self.0.borrow_mut();
        for (e, s) in (self.1)(part) {
            if let Some(before) = got.insert(e, s.clone()) {
                assert_eq!(before, s, "{e:?} written differently by two parts");
            }
        }
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<Finished> {
        Ok(Finished::default())
    }
}

fn convert(
    world: World,
    parts: Vec<Part>,
    summarize: Vec<Box<dyn Pass>>,
    decide: Vec<Box<dyn Pass>>,
    passes: Vec<Box<dyn Pass>>,
    seen: fn(&World) -> Wrote,
) -> Wrote {
    let mut registry = SummaryRegistry::new();
    primase_reskate::register_summaries(&mut registry);
    let got = Rc::new(RefCell::new(Wrote::new()));
    convert_parts_summarized(
        &mut Prepared(world),
        &mut Given(parts, 0),
        Box::new(Capture(got.clone(), seen)),
        &PassManager::new(PassRegistry::new()),
        &Pipeline { passes: Vec::new() },
        &Summarized {
            registry: &registry,
            summarize: &Pipeline { passes: summarize },
            decide: &Pipeline { passes: decide },
        },
        &Pipeline { passes },
    )
    .unwrap();
    got.take()
}

fn light(w: &mut World, x: f32, energy: f32) -> Entity {
    let e = w.spawn();
    w.insert(
        e,
        SkateLight {
            spot: false,
            position: [x, 0.0, -10.0],
            direction: [0.0, -1.0, 0.0],
            tangent: [1.0, 0.0, 0.0],
            color: [1.0; 3],
            energy,
            range: 10.0,
            radius: 0.1,
            spot_size: 0.0,
            spot_blend: 0.0,
            time_of_day: 127,
            cast_shadow: false,
            priority: None,
            group: String::new(),
            unknown_flags: 0,
        },
    );
    e
}

fn lights(w: &World) -> Wrote {
    w.iter::<SkateLight>()
        .map(|(e, l)| (e, format!("{}", l.energy)))
        .collect()
}

/// The keys of the lights the merged summary a part was handed keeps (the cap decided over every part).
fn kept(w: &World) -> Wrote {
    w.resource::<primase_reskate::components::SkateLightsSummary>()
        .map(|s| {
            s.kept()
                .into_iter()
                .map(|k| (Entity::from_index(k as u32), "kept".to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Lights in one 200 m cell, capped at 2: A's root reaches L1, L2 and L4; B's root reaches L4 too (a shared
/// dependency), B owns L3 and has L1 as context only. The whole map keeps L4 and L1; so does the cap decided over the
/// parts (a part counting only its direct roots would keep L3 alone; one counting L4 twice would keep L4 alone).
#[test]
fn lights_reached_through_roots_are_owned_once_and_context_is_not() {
    let mut w = World::new();
    w.insert_resource(SkateScene {
        name: "own".into(),
        ..Default::default()
    });
    let l1 = light(&mut w, 10.0, 5.0);
    let l2 = light(&mut w, 20.0, 3.0);
    let l3 = light(&mut w, 30.0, 4.0);
    let l4 = light(&mut w, 40.0, 6.0);
    let h1 = w.spawn();
    w.insert(h1, Holds(vec![l1, l2, l4]));
    let h2 = w.spawn();
    w.insert(h2, Holds(vec![l4]));
    let cap = || SelectReskateLights::new(2, 200.0).unwrap();
    let mut whole = w.clone();
    cap().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected: Wrote = lights(&whole)
        .into_keys()
        .map(|e| (e, "kept".to_owned()))
        .collect();
    assert_eq!(expected.keys().copied().collect::<Vec<_>>(), [l1, l4]);
    let got = convert(
        w,
        vec![part(&[h1], &[]), part(&[h2, l3], &[l1])],
        vec![Box::new(SummarizeReskateLights::new(cap()))],
        Vec::new(),
        Vec::new(),
        kept,
    );
    assert_eq!(got, expected);
}

/// A slab prototype `width` metres wide drawn with a texture of its own (grey `grey`).
fn slab(w: &mut World, width: f32, grey: u8) -> Entity {
    let tex = w.spawn();
    w.insert(tex, Name(format!("t{grey}")));
    w.insert(
        tex,
        Texture {
            width: 2,
            height: 2,
            format: TexFormat::Rgba8Unorm,
            levels: vec![[grey, grey, grey, 255].repeat(4)],
            color_space: ColorSpace::Srgb,
            role: TexRole::Color,
            normal: None,
        },
    );
    let material = w.spawn();
    w.insert(material, Name(format!("m{grey}")));
    w.insert(
        material,
        PbrMaterial {
            base_color_texture: Some(TextureRef::new(tex)),
            ..PbrMaterial::default()
        },
    );
    let proto = w.spawn();
    w.insert(proto, Name(format!("slab{grey}")));
    w.insert(
        proto,
        Mesh::new(vec![Primitive {
            topology: Topology::Triangles,
            streams: BTreeMap::from([(
                Semantic::Position,
                Stream::F32x3(vec![[0.0, 0.0, 0.0], [width, 1.0, 0.0], [0.0, 0.0, -width]]),
            )]),
            indices: None,
            material: Some(material),
        }]),
    );
    proto
}

fn place(w: &mut World, proto: Entity, x: f32, height: f32) -> Entity {
    let p = w.spawn();
    w.insert(
        p,
        Instance {
            prototype: proto,
            variant: 0,
        },
    );
    let mut t = Transform::IDENTITY;
    t.cols[3] = [x, height, -1.0];
    w.insert(p, t);
    p
}

/// Each placement's height and each material's base factor, as written.
fn floor_and_textures(w: &World) -> Wrote {
    let mut out: Wrote = w
        .iter::<Instance>()
        .map(|(e, _)| (e, format!("height {}", w.get::<Transform>(e).unwrap().cols[3][1])))
        .collect();
    out.extend(w.iter::<SkateMaterial>().map(|(e, m)| {
        (
            e,
            format!(
                "base {:?} factor {:?}",
                m.base.is_some(),
                w.get::<SkateBaseFactor>(e).map(|f| f.0)
            ),
        )
    }));
    out
}

/// A sunk, wide placement reached only through a root (root -> placement -> prototype) sets the floor and the
/// heaviest texture; a part that counted only its direct roots would miss it.
#[test]
fn a_placement_reached_through_a_root_is_measured_and_weighed() {
    let mut w = World::new();
    let wide = slab(&mut w, 40.0, 30);
    let small = slab(&mut w, 2.0, 120);
    let other = slab(&mut w, 3.0, 220);
    let sunk = place(&mut w, wide, 0.0, -100.0);
    let mut placed = vec![place(&mut w, small, 10.0, 0.0), place(&mut w, small, 20.0, 1.0)];
    placed.push(place(&mut w, other, 30.0, 2.0));
    primase_reskate::passes::raise_skate(
        &mut w,
        &RaiseOptions::new("own", LightCalibration::Standard).unwrap(),
    )
    .unwrap();
    let holder = w.spawn();
    w.insert(holder, Holds(vec![sunk]));
    let floor = || PrepareReskateFloor::new(0.0, 500.0).unwrap();
    let budget = || BudgetReskateTextures::new(2).unwrap();
    let mut whole = w.clone();
    floor().run(&mut whole, &mut PassLog::default()).unwrap();
    budget().run(&mut whole, &mut PassLog::default()).unwrap();
    let expected = floor_and_textures(&whole);
    // the floor is the sunk slab's (-100 m, lifted 100 m), and its texture is the one kept
    assert_eq!(expected[&sunk], "height 0");
    let got = convert(
        w,
        vec![part(&[holder], &[]), part(&placed, &[])],
        vec![
            Box::new(SummarizeReskateFloor),
            Box::new(SummarizeReskateTextures::new(budget())),
        ],
        vec![Box::new(DecideReskateFloor::new(floor()))],
        vec![Box::new(floor()), Box::new(budget())],
        floor_and_textures,
    );
    assert_eq!(got, expected);
}

/// Makes lights in each part: from every seed light it holds (energy under 5), a spark nine times as bright, placed
/// from its seed (PassLog::place_from); and an undeclared temporary of energy 100, never placed.
#[derive(Debug)]
struct Spark;
impl Pass for Spark {
    fn name(&self) -> &'static str {
        "spark"
    }
    fn provides(&self) -> &[&'static str] {
        &["skate.light"]
    }
    fn run(&self, w: &mut World, log: &mut PassLog) -> std::result::Result<(), String> {
        let seeds: Vec<(Entity, SkateLight)> = w
            .iter::<SkateLight>()
            .filter(|(_, l)| l.energy < 5.0)
            .map(|(e, l)| (e, l.clone()))
            .collect();
        for (seed, l) in seeds {
            let spark = w.spawn();
            w.insert(
                spark,
                SkateLight {
                    energy: l.energy * 9.0,
                    position: [l.position[0] + 1.0, l.position[1], l.position[2]],
                    ..l.clone()
                },
            );
            log.place_from(spark, seed);
            let temporary = w.spawn();
            w.insert(temporary, SkateLight { energy: 100.0, ..l });
        }
        Ok(())
    }
}

/// Lights made by a pass are owned as conversion writes them: a spark placed from an owned seed is the part's, one
/// placed from a seed it only loaded as context is the seed owner's (not counted twice), an undeclared temporary is
/// nobody's. With the pass in both traversals and the light cap (2 per cell) decided over the parts, the parts keep and
/// write what the whole map (WholeWorld: one part, the whole) keeps and writes: the two sparks, 18 and 9. Counting the
/// temporaries (energy 100) would keep them and write nothing; counting the context copies would fill the cap with two
/// 18s.
#[test]
fn made_lights_are_owned_as_conversion_writes_them() {
    let mut w = World::new();
    w.insert_resource(SkateScene {
        name: "own".into(),
        ..Default::default()
    });
    let s1 = light(&mut w, 10.0, 1.0);
    let s2 = light(&mut w, 30.0, 2.0);
    let cap = || SelectReskateLights::new(2, 200.0).unwrap();
    let energies = |w: &World| -> Wrote {
        w.iter::<SkateLight>()
            .map(|(_, l)| (Entity::from_index(l.energy as u32), format!("{}", l.energy)))
            .collect()
    };
    let run = |parts: Vec<Part>| {
        convert(
            w.clone(),
            parts,
            vec![Box::new(Spark), Box::new(SummarizeReskateLights::new(cap()))],
            Vec::new(),
            vec![Box::new(Spark), Box::new(cap())],
            energies,
        )
    };
    let whole = run(vec![Part {
        roots: BTreeSet::from([s1, s2]),
        context: BTreeSet::new(),
        whole: true,
        area: None,
    }]);
    assert_eq!(whole.values().cloned().collect::<Vec<_>>(), ["9", "18"]);
    // each part holds the other's seed as context
    let parts = run(vec![part(&[s1], &[s2]), part(&[s2], &[s1])]);
    assert_eq!(parts, whole);
}
